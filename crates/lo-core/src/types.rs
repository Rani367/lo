//! Shared types — the contract that, in the Electron app, crossed the main↔renderer
//! IPC boundary (`src/shared/types.ts`). In the single-process Rust app these are
//! plain values passed over channels (see the eventual `app::events`), but the
//! shapes are preserved 1:1 so `settings.json` and the chat protocol stay
//! byte-compatible with the TypeScript build.

use serde::{Deserialize, Serialize};

/// The renderer/UI state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoState {
    Boot,
    Idle,
    Listening,
    Thinking,
    Speaking,
    Error,
}

/// How a turn is activated.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ActivationMode {
    /// Wake word ("Computer").
    Wake,
    /// Push-to-talk (hold Space).
    Ptt,
    /// Voice-activity detection (auto-segment).
    Vad,
}

/// A concrete LLM engine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendKind {
    Mlx,
    Llama,
    Ollama,
    Custom,
}

impl BackendKind {
    pub fn as_str(self) -> &'static str {
        match self {
            BackendKind::Mlx => "mlx",
            BackendKind::Llama => "llama",
            BackendKind::Ollama => "ollama",
            BackendKind::Custom => "custom",
        }
    }
}

/// The user-selectable backend setting (`auto` resolves by platform/hardware).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BackendChoice {
    Auto,
    Mlx,
    Llama,
    Ollama,
