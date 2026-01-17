//! The brain — the native function-calling agent loop (ported from
//! `src/main/brain.ts`). The async transport (streaming `reqwest` to
//! `{base_url}/chat/completions`) lives in the `lo` binary crate, which drives
//! these pure building blocks: shaping the conversation, building the request
//! body, accumulating the SSE stream (`sse`), and appending each tool round.

pub mod sse;
pub mod types;

use crate::config::{persona, LoSettings};
use crate::tools;
use crate::types::{ChatMessage, ChatRole};
use types::{ReqMessage, ReqRole, ToolCall};

/// Native calls are reliable, so a few tool rounds are safe.
pub const MAX_ROUNDS: usize = 6;
/// Generous ceiling; the persona keeps the spoken reply short on its own.
pub const MAX_TOKENS: u32 = 1024;

/// Build the opening conversation: the system prompt followed by the rolling
/// history (mapped to request messages).
pub fn initial_convo(settings: &LoSettings, history: &[ChatMessage]) -> Vec<ReqMessage> {
    let mut convo = Vec::with_capacity(history.len() + 1);
    convo.push(ReqMessage::system(persona::build_system_prompt(settings)));
    for m in history {
        let role = match m.role {
            ChatRole::System => ReqRole::System,
            ChatRole::User => ReqRole::User,
            ChatRole::Assistant => ReqRole::Assistant,
        };
        convo.push(ReqMessage {
            role,
            content: Some(m.content.clone()),
            tool_calls: None,
            tool_call_id: None,
