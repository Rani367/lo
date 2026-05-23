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
