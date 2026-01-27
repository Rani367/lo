//! Cross-platform application paths, replacing Electron's `app.getPath('userData')`
//! and the hardcoded `~/.cache/huggingface`. Uses the `directories` crate.
//!
//! Config (settings.json, history.json, lo-audit.log) lives in the config dir;
//! large downloaded artifacts (llama-server binary, GGUF / ONNX / GGML weights)
//! live in the cache dir.

use directories::{BaseDirs, ProjectDirs};
use std::path::PathBuf;

/// `ProjectDirs::from("com", "lo", "assistant")` — matches the planned bundle id
/// `com.lo.assistant`.
fn project_dirs() -> Option<ProjectDirs> {
    ProjectDirs::from("com", "lo", "assistant")
}

/// Directory for user config (settings.json, history.json, audit log).
///
/// Falls back to `./.lo` when no HOME is resolvable (e.g. a bare CI container).
pub fn config_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.config_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".lo"))
}

/// Directory for large downloaded artifacts (engine binary + model weights).
pub fn cache_dir() -> PathBuf {
    project_dirs()
        .map(|d| d.cache_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from(".lo/cache"))
}

/// Per-OS user home directory (the default filesystem sandbox root).
pub fn home_dir() -> PathBuf {
    BaseDirs::new()
        .map(|d| d.home_dir().to_path_buf())
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Path to the persisted settings file.
pub fn settings_file() -> PathBuf {
    config_dir().join("settings.json")
}

/// Path to the opt-in conversation history file.
pub fn history_file() -> PathBuf {
    config_dir().join("history.json")
}

/// Path to the tool-invocation audit log.
pub fn audit_file() -> PathBuf {
    config_dir().join("lo-audit.log")
}
