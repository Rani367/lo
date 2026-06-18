//! Filesystem tools — read/list/search/open/write/move/delete, all sandboxed to
//! the allowed roots. Every path the
//! model supplies goes through [`lo_core::tools::sandbox::resolve_in_roots`],
//! which expands `~`, absolutizes, lexically normalizes, realpath-dereferences
//! symlinks, and verifies the result lives inside an allowed root — so a
//! mistaken or adversarial path can't escape the sandbox.
//!
//! Read-side tools are Safe; write/move/delete are Danger (gated in the registry).

use std::path::Path;

use lo_core::tools::sandbox::{
    self, looks_binary, MAX_LIST, MAX_MATCHES, MAX_READ_BYTES, MAX_SEARCH_DEPTH,
};
use lo_core::LoSettings;

/// Read a text file's contents (size- and binary-guarded).
pub async fn read_file(settings: &LoSettings, path: &str) -> Result<String, String> {
    let abs = resolve(settings, path)?;
    let meta = tokio::fs::metadata(&abs).await.map_err(io_msg)?;
    if meta.is_dir() {
        return Err(format!("{} is a directory — use list_dir.", abs.display()));
    }
    if meta.len() > MAX_READ_BYTES {
        return Err(format!(
            "That file is {} KB; I only read up to {} KB.",
            (meta.len() as f64 / 1024.0).round() as u64,
            MAX_READ_BYTES / 1024
        ));
    }
    let bytes = tokio::fs::read(&abs).await.map_err(io_msg)?;
    if looks_binary(&bytes) {
        return Err("That looks like a binary file, so I won't read it as text.".to_string());
    }
    Ok(String::from_utf8_lossy(&bytes).into_owned())
}

/// List a directory's entries (`d`/`-` prefix), capped at `MAX_LIST`.
pub async fn list_dir(settings: &LoSettings, path: &str) -> Result<String, String> {
    let input = if path.trim().is_empty() { "~" } else { path };
    let abs = resolve(settings, input)?;

    let mut entries: Vec<(bool, String)> = Vec::new();
    let mut read_dir = tokio::fs::read_dir(&abs).await.map_err(io_msg)?;
    while let Some(entry) = read_dir.next_entry().await.map_err(io_msg)? {
        let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
        entries.push((is_dir, entry.file_name().to_string_lossy().into_owned()));
    }

    let total = entries.len();
    let mut lines: Vec<String> = entries
        .into_iter()
        .take(MAX_LIST)
        .map(|(is_dir, name)| format!("{} {name}", if is_dir { "d" } else { "-" }))
        .collect();
    // Sort the rendered lines (after the slice), so directories and files
    // interleave by their `d …` / `- …` rendering.
    lines.sort();

    let more = if total > MAX_LIST {
        format!("\n… and {} more", total - MAX_LIST)
    } else {
        String::new()
    };
    Ok(format!("{}:\n{}{more}", abs.display(), lines.join("\n")))
}

/// Find files whose names contain `query` under `path`, depth-limited.
pub async fn search_files(
    settings: &LoSettings,
    path: &str,
    query: &str,
) -> Result<String, String> {
    let input = if path.trim().is_empty() { "~" } else { path };
    let base = resolve(settings, input)?;
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Err("No search query was given.".to_string());
    }

    let mut matches: Vec<String> = Vec::new();
    walk(&base, 0, &needle, &mut matches).await;

    if matches.is_empty() {
        return Ok(format!(
            "No files matching \"{query}\" under {}.",
            base.display()
        ));
    }
    let mut out = matches.join("\n");
    if matches.len() >= MAX_MATCHES {
        out.push_str("\n… (truncated)");
    }
    Ok(out)
}

/// Recursive directory walk: skip dotfiles + `node_modules`, cap matches+depth.
/// (Boxed future because `async fn` recursion needs an explicitly-sized type.)
fn walk<'a>(
    dir: &'a Path,
    depth: usize,
    needle: &'a str,
    matches: &'a mut Vec<String>,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = ()> + 'a>> {
    Box::pin(async move {
        if matches.len() >= MAX_MATCHES || depth > MAX_SEARCH_DEPTH {
            return;
        }
        let Ok(mut read_dir) = tokio::fs::read_dir(dir).await else {
            return; // unreadable dir — skip
        };
        while let Ok(Some(entry)) = read_dir.next_entry().await {
            if matches.len() >= MAX_MATCHES {
                return;
            }
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') || name == "node_modules" {
                continue;
            }
            let full = entry.path();
            if name.to_lowercase().contains(needle) {
                matches.push(full.to_string_lossy().into_owned());
            }
            let is_dir = entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false);
            if is_dir {
                walk(&full, depth + 1, needle, matches).await;
            }
        }
    })
}

/// Create or overwrite a text file (parent dirs created as needed).
pub async fn write_file(
    settings: &LoSettings,
    path: &str,
    content: &str,
    overwrite: bool,
) -> Result<String, String> {
    let abs = resolve(settings, path)?;
    if !overwrite && abs.exists() {
        return Err(format!(
            "{} already exists. Pass overwrite to replace it.",
            abs.display()
        ));
    }
    if let Some(parent) = abs.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_msg)?;
    }
    tokio::fs::write(&abs, content.as_bytes())
        .await
        .map_err(io_msg)?;
    Ok(format!(
        "Wrote {} bytes to {}.",
        content.len(),
        abs.display()
    ))
}

/// Move or rename a file/folder (both ends validated; parent dirs created).
pub async fn move_path(settings: &LoSettings, from: &str, to: &str) -> Result<String, String> {
    let src = resolve(settings, from)?;
    let dst = resolve(settings, to)?;
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_msg)?;
    }
    tokio::fs::rename(&src, &dst).await.map_err(io_msg)?;
    Ok(format!("Moved {} to {}.", src.display(), dst.display()))
}

/// Copy a file to a new path (both ends validated; parent dirs created). Refuses
/// directories — the model has no recursive-copy need and it avoids surprises.
pub async fn copy_file(settings: &LoSettings, from: &str, to: &str) -> Result<String, String> {
    let src = resolve(settings, from)?;
    let dst = resolve(settings, to)?;
    let meta = tokio::fs::metadata(&src).await.map_err(io_msg)?;
    if meta.is_dir() {
        return Err(format!(
            "{} is a directory; I only copy files.",
            src.display()
        ));
    }
    if let Some(parent) = dst.parent() {
        tokio::fs::create_dir_all(parent).await.map_err(io_msg)?;
    }
    let bytes = tokio::fs::copy(&src, &dst).await.map_err(io_msg)?;
    Ok(format!(
        "Copied {} to {} ({bytes} bytes).",
        src.display(),
        dst.display()
    ))
}

/// Delete a file or folder (recursively for directories).
pub async fn delete_path(settings: &LoSettings, path: &str) -> Result<String, String> {
    let abs = resolve(settings, path)?;
    let meta = tokio::fs::metadata(&abs).await.map_err(io_msg)?;
    if meta.is_dir() {
        tokio::fs::remove_dir_all(&abs).await.map_err(io_msg)?;
    } else {
        tokio::fs::remove_file(&abs).await.map_err(io_msg)?;
    }
    Ok(format!("Deleted {}.", abs.display()))
}

/// Open a file or folder in the OS default handler (Finder/Explorer/editor).
pub async fn open_path(settings: &LoSettings, path: &str) -> Result<String, String> {
    let abs = resolve(settings, path)?;
    // `open` shells out; run it on a blocking thread to avoid stalling the executor.
    let abs_for_task = abs.clone();
    tokio::task::spawn_blocking(move || open::that_detached(&abs_for_task))
        .await
        .map_err(|e| format!("could not open the path: {e}"))?
        .map_err(|e| format!("{e}"))?;
    Ok(format!("Opened {}.", abs.display()))
}

/// Resolve a user path inside the allowed roots, mapping the sandbox error to a
/// plain message (the variants already carry good wording).
fn resolve(settings: &LoSettings, input: &str) -> Result<std::path::PathBuf, String> {
    sandbox::resolve_in_roots(settings, input).map_err(|e| e.to_string())
}

/// Format an I/O error as a plain message for the tool result.
fn io_msg(e: std::io::Error) -> String {
    e.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Settings whose only allowed root is `root` (canonicalized so the sandbox's
    /// realpath check matches it on platforms where the temp dir is symlinked).
    fn settings_with_root(root: &Path) -> LoSettings {
        let canon = std::fs::canonicalize(root).unwrap();
        LoSettings {
            allowed_fs_roots: vec![canon.to_string_lossy().into_owned()],
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn copy_file_duplicates_within_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"hello").unwrap();
        let dst = dir.path().join("nested/b.txt"); // parent dirs are created
        let res = copy_file(&s, src.to_str().unwrap(), dst.to_str().unwrap()).await;
        assert!(res.is_ok(), "{res:?}");
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "hello");
    }

    #[tokio::test]
    async fn copy_file_refuses_directories() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let sub = dir.path().join("sub");
        std::fs::create_dir(&sub).unwrap();
        let dst = dir.path().join("copy");
        let res = copy_file(&s, sub.to_str().unwrap(), dst.to_str().unwrap()).await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn copy_file_rejects_destination_outside_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"x").unwrap();
        // Escape attempt via `..` must be rejected by the sandbox.
        let res = copy_file(&s, src.to_str().unwrap(), "../escape.txt").await;
        assert!(res.is_err());
    }

    #[tokio::test]
    async fn write_then_read_roundtrips() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let p = dir.path().join("note.txt");
        let w = write_file(&s, p.to_str().unwrap(), "hello world", false).await;
        assert!(w.is_ok(), "{w:?}");
        assert_eq!(
            read_file(&s, p.to_str().unwrap()).await.unwrap(),
            "hello world"
        );
    }

    #[tokio::test]
    async fn write_file_refuses_existing_without_overwrite() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let p = dir.path().join("note.txt");
        std::fs::write(&p, b"old").unwrap();
        assert!(write_file(&s, p.to_str().unwrap(), "new", false)
            .await
            .is_err());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "old"); // untouched
                                                                 // With overwrite it replaces.
        assert!(write_file(&s, p.to_str().unwrap(), "new", true)
            .await
            .is_ok());
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "new");
    }

    #[tokio::test]
    async fn write_file_creates_parent_dirs() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let p = dir.path().join("a/b/c.txt");
        assert!(write_file(&s, p.to_str().unwrap(), "x", false)
            .await
            .is_ok());
        assert!(p.exists());
    }

    #[tokio::test]
    async fn read_file_rejects_binary() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let p = dir.path().join("blob.bin");
        std::fs::write(&p, [b'h', b'i', 0, 0xFF, 0xFE]).unwrap(); // NUL byte ⇒ binary
        assert!(read_file(&s, p.to_str().unwrap()).await.is_err());
    }

    #[tokio::test]
    async fn list_dir_marks_dirs_and_files() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        std::fs::write(dir.path().join("file.txt"), b"x").unwrap();
        std::fs::create_dir(dir.path().join("folder")).unwrap();
        let out = list_dir(&s, dir.path().to_str().unwrap()).await.unwrap();
        assert!(out.contains("- file.txt"), "{out}");
        assert!(out.contains("d folder"), "{out}");
    }

    #[tokio::test]
    async fn search_files_matches_by_name_and_skips_dotfiles() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        std::fs::write(dir.path().join("report.md"), b"x").unwrap();
        std::fs::write(dir.path().join(".hidden_report"), b"x").unwrap();
        let out = search_files(&s, dir.path().to_str().unwrap(), "report")
            .await
            .unwrap();
        assert!(out.contains("report.md"), "{out}");
        assert!(!out.contains(".hidden_report"), "{out}");
    }

    #[tokio::test]
    async fn move_path_renames_within_sandbox() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let src = dir.path().join("a.txt");
        std::fs::write(&src, b"data").unwrap();
        let dst = dir.path().join("renamed/b.txt");
        assert!(move_path(&s, src.to_str().unwrap(), dst.to_str().unwrap())
            .await
            .is_ok());
        assert!(!src.exists());
        assert_eq!(std::fs::read_to_string(&dst).unwrap(), "data");
    }

    #[tokio::test]
    async fn delete_path_removes_file_and_dir() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        let f = dir.path().join("gone.txt");
        std::fs::write(&f, b"x").unwrap();
        assert!(delete_path(&s, f.to_str().unwrap()).await.is_ok());
        assert!(!f.exists());
        let sub = dir.path().join("subdir");
        std::fs::create_dir(&sub).unwrap();
        std::fs::write(sub.join("inner.txt"), b"x").unwrap();
        assert!(delete_path(&s, sub.to_str().unwrap()).await.is_ok());
        assert!(!sub.exists());
    }

    #[tokio::test]
    async fn write_move_delete_reject_escapes() {
        let dir = tempfile::tempdir().unwrap();
        let s = settings_with_root(dir.path());
        assert!(write_file(&s, "../escape.txt", "x", true).await.is_err());
        let inside = dir.path().join("a.txt");
        std::fs::write(&inside, b"x").unwrap();
        assert!(move_path(&s, inside.to_str().unwrap(), "../escape.txt")
            .await
            .is_err());
        assert!(delete_path(&s, "../../etc/hosts").await.is_err());
    }
}
