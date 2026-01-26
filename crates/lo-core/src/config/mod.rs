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
}

impl Default for LoSettings {
    fn default() -> Self {
        Self {
            // A Qwen3-Coder MoE: ~30B total but ~3B active params/token. Override
            // with LO_ENGINE_MODEL.
            model: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ".to_string(),
            asr_model: "mlx-community/parakeet-tdt-0.6b-v3".to_string(),
            tts_model: "onnx-community/Kokoro-82M-v1.0-ONNX".to_string(),
            voice: "af_heart".to_string(),
            activation_mode: ActivationMode::Ptt,
            user_name: "there".to_string(),
            voice_enabled: true,
            temperature: 0.6,
            speech_rate: 1.15,
            backend: BackendChoice::Auto,
            llm_url: String::new(),
            llm_key: String::new(),
            power_user_mode: false,
            allowed_fs_roots: Vec::new(),
            persist_history: false,
        }
    }
}

impl LoSettings {
    /// Load settings, merging the on-disk `settings.json` over the defaults. Any
    /// error (missing file, bad JSON) yields the defaults — matching the TS
    /// behavior where a corrupt file silently falls back.
    pub fn load() -> Self {
        Self::load_from(paths::settings_file())
    }

    /// Load from an explicit path (used by tests).
    pub fn load_from<P: AsRef<Path>>(path: P) -> Self {
        match fs::read_to_string(path.as_ref()) {
            Ok(raw) => serde_json::from_str(&raw).unwrap_or_default(),
            Err(_) => Self::default(),
        }
    }

    /// Persist to the default settings path (pretty JSON, like the TS writer).
    pub fn save(&self) -> std::io::Result<()> {
        self.save_to(paths::settings_file())
    }

    pub fn save_to<P: AsRef<Path>>(&self, path: P) -> std::io::Result<()> {
        let path = path.as_ref();
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self).expect("LoSettings is serializable");
        fs::write(path, json)
    }
}

/// The optional Picovoice access key for the on-device wake word.
pub fn porcupine_key() -> Option<String> {
    std::env::var("PICOVOICE_ACCESS_KEY")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_electron_app() {
        let s = LoSettings::default();
        assert_eq!(s.voice, "af_heart");
        assert_eq!(s.activation_mode, ActivationMode::Ptt);
        assert_eq!(s.backend, BackendChoice::Auto);
        assert!(!s.power_user_mode);
        assert!(s.allowed_fs_roots.is_empty());
        assert_eq!(s.temperature, 0.6);
        assert_eq!(s.speech_rate, 1.15);
    }

    #[test]
    fn partial_json_merges_over_defaults() {
        // Only two keys present; everything else must fall back to defaults.
        let json = r#"{ "voice": "am_michael", "powerUserMode": true }"#;
        let s: LoSettings = serde_json::from_str(json).unwrap();
        assert_eq!(s.voice, "am_michael");
        assert!(s.power_user_mode);
        // untouched keys keep defaults
        assert_eq!(s.user_name, "there");
        assert_eq!(s.backend, BackendChoice::Auto);
    }

    #[test]
    fn camel_case_round_trips_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = LoSettings {
            user_name: "Rani".to_string(),
            persist_history: true,
            ..Default::default()
        };
        s.save_to(&path).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        assert!(
            raw.contains("\"userName\""),
            "expected camelCase keys: {raw}"
        );
        assert!(raw.contains("\"persistHistory\""));

        let back = LoSettings::load_from(&path);
        assert_eq!(back, s);
    }

