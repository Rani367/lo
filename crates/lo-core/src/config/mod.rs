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
    /// kokoro-js model id.
    pub tts_model: String,
    /// Kokoro voice, e.g. `af_heart` (warm American female).
    pub voice: String,
    pub activation_mode: ActivationMode,
    /// How Lo addresses the user.
    pub user_name: String,
    pub voice_enabled: bool,
    pub temperature: f64,
    /// Kokoro speed multiplier; >1 speaks faster, pitch unchanged.
    pub speech_rate: f64,
    /// `auto` (MLX on Apple Silicon, llama.cpp elsewhere) or an explicit engine.
    pub backend: BackendChoice,
    /// Custom OpenAI-compatible base URL (used when `backend == custom`).
    pub llm_url: String,
    /// Optional bearer key for the custom endpoint.
    pub llm_key: String,
    /// When true, dangerous tools run with no confirmation gate.
    pub power_user_mode: bool,
    /// Directories the filesystem tools may touch (`[]` => home dir).
    pub allowed_fs_roots: Vec<String>,
    /// Persist the rolling conversation transcript across restarts.
    pub persist_history: bool,
