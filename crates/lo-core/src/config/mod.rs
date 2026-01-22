//! Configuration (ported from `src/main/config.ts`). Fully local — there are no
//! API keys to protect. User settings persist as plain JSON in the config dir.
//! The Picovoice access key (optional, for the wake word) is the only secret and
//! it is client-safe.
//!
//! `LoSettings` uses `#[serde(default)]`, so deserializing a partial
//! `settings.json` fills only the present keys and the rest fall back to
//! `Default` — exactly reproducing the TS `{ ...DEFAULT_SETTINGS, ...json }`
//! merge.

pub mod history;
pub mod options;
pub mod paths;
pub mod persona;

use crate::types::{ActivationMode, BackendChoice};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct LoSettings {
    /// Brain model id/path for the active backend (MLX id, GGUF ref, or Ollama tag).
    pub model: String,
    /// Speech-to-text model id/path.
    pub asr_model: String,
