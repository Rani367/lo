//! Server-Sent-Events parsing + native tool-call reconstruction (ported from
//! `readSse` and the tool-call accumulation in `streamCompletion`).
//!
//! The transport (reqwest `bytes_stream`) lives in the `lo` binary; everything
//! that turns raw `data:` lines into accumulated prose + a `Vec<ToolCall>` is
//! here and exhaustively tested — including the two compatibility quirks that
//! every real backend trips over:
//!   1. `tool_calls` arrive *delta-encoded by index*, split across many chunks.
//!   2. `arguments` may arrive as a JSON *object* in a single delta (llama-server
//!      `--jinja`) rather than a streamed string — it must be coerced to a string.

use super::types::{FunctionCall, ToolCall, ToolCallKind};
use serde::Deserialize;
use std::collections::BTreeMap;

/// One OpenAI streaming chunk (permissive: every field optional / defaulted so a
/// keep-alive, a usage-only chunk, or an unknown field never breaks parsing).
#[derive(Debug, Deserialize, Default)]
pub struct SseEvent {
    #[serde(default)]
    pub choices: Vec<SseChoice>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SseChoice {
    #[serde(default)]
    pub delta: SseDelta,
    #[serde(default)]
    pub finish_reason: Option<String>,
}

#[derive(Debug, Deserialize, Default)]
pub struct SseDelta {
    #[serde(default)]
    pub content: Option<String>,
    #[serde(default)]
    pub tool_calls: Vec<DeltaToolCall>,
