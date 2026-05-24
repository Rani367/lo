//! Filesystem tools — read/list/search/open/write/move/delete, all sandboxed to
//! the allowed roots. Ported from `src/main/tools/files.ts`. Every path the
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
    // The TS sorts the rendered lines (after the slice), so directories and files
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
