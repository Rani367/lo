//! OpenAI chat-message shapes for the agent loop (richer than the renderer-facing
//! `ChatMessage`: the loop also produces assistant `tool_calls` and `tool`
//! results). Ported from the `ReqMessage`/`ToolCall` interfaces in `brain.ts`.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ReqRole {
    System,
    User,
    Assistant,
    Tool,
}

