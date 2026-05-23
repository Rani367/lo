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
