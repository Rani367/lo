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
    Custom,
}

/// A chat message exchanged with the brain (the renderer-facing shape: no
/// `tool`/`tool_calls`; the agent loop's richer message lives in `brain::types`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: ChatRole,
    pub content: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ChatRole {
    System,
    User,
    Assistant,
}

/// The result of one full agent turn.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatTurnResult {
    pub turn_id: String,
    pub reply: String,
    pub used_web_search: bool,
    pub tools_invoked: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

/// Health of the local engine, shown in the HUD.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalStatus {
    /// Sidecar process is running and the model is loaded + warmed.
    pub engine_up: bool,
    /// Model is still loading/warming (not an error — show a spinner).
    pub loading: bool,
    pub backend: Option<BackendKind>,
    pub model: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// First-run engine/model download progress (managed llama.cpp backend).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelDownloadEvent {
    /// `ENGINE` | `WEIGHTS` | …
    pub label: String,
    /// `None` = indeterminate.
    pub pct: Option<u8>,
}

/// Hardware-tiered model recommendation for first-run setup.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelRecommendation {
    /// The logical model id to set (works for both MLX and llama backends).
    pub model: String,
    /// Short human label, e.g. "Qwen3-8B".
    pub label: String,
    /// Detected system memory (GB).
    pub ram_gb: f64,
    /// The engine that would serve it.
    pub backend: BackendKind,
}
