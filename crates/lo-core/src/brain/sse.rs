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
}

#[derive(Debug, Deserialize, Default)]
pub struct DeltaToolCall {
    #[serde(default)]
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub function: Option<DeltaFunction>,
}

#[derive(Debug, Deserialize, Default)]
pub struct DeltaFunction {
    #[serde(default)]
    pub name: Option<String>,
    /// May be a JSON string OR a JSON object/value (llama-server `--jinja`).
    #[serde(default)]
    pub arguments: Option<serde_json::Value>,
}

/// The meaning of one SSE line.
#[derive(Debug)]
pub enum Frame {
    Event(SseEvent),
    Done,
    /// Not a `data:` line, a keep-alive, or an unparseable payload — ignored.
    Ignore,
}

/// Parse a single already-line-split SSE line, mirroring `readSse`'s per-line
/// logic: only `data:` lines matter, `[DONE]` terminates, parse failures are
/// silently ignored.
pub fn parse_line(line: &str) -> Frame {
    let line = line.trim();
    let Some(payload) = line.strip_prefix("data:") else {
        return Frame::Ignore;
    };
    let payload = payload.trim();
    if payload == "[DONE]" {
        return Frame::Done;
    }
    match serde_json::from_str::<SseEvent>(payload) {
        Ok(ev) => Frame::Event(ev),
        Err(_) => Frame::Ignore,
    }
}

#[derive(Default, Clone)]
struct AccCall {
    id: String,
    name: String,
    args: String,
}

/// Accumulates a single streamed completion: appends prose deltas and merges
/// `tool_calls` by index, exactly like `streamCompletion`.
#[derive(Default)]
pub struct StreamAccumulator {
    text: String,
    calls: BTreeMap<usize, AccCall>,
}

impl StreamAccumulator {
    pub fn new() -> Self {
        Self::default()
    }

    /// Feed one parsed event. Returns the prose delta (if any) so the caller can
    /// forward it to its `on_delta` callback — matching the TS `cb.onDelta`.
    pub fn push_event(&mut self, ev: &SseEvent) -> Option<String> {
        let delta = ev.choices.first().map(|c| &c.delta)?;
        let mut emitted: Option<String> = None;
        if let Some(content) = &delta.content {
            if !content.is_empty() {
                self.text.push_str(content);
                emitted = Some(content.clone());
            }
        }
        for tc in &delta.tool_calls {
            let acc = self.calls.entry(tc.index).or_default();
            if let Some(id) = &tc.id {
                if !id.is_empty() {
                    acc.id = id.clone();
                }
            }
            if let Some(func) = &tc.function {
                if let Some(name) = &func.name {
                    if !name.is_empty() {
                        acc.name = name.clone();
                    }
                }
