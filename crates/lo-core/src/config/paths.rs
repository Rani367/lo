//! Cross-platform application paths, replacing Electron's `app.getPath('userData')`
//! and the hardcoded `~/.cache/huggingface`. Uses the `directories` crate.
//!
//! Config (settings.json, history.json, lo-audit.log) lives in the config dir;
//! large downloaded artifacts (llama-server binary, GGUF / ONNX / GGML weights)
//! live in the cache dir.

use directories::{BaseDirs, ProjectDirs};
