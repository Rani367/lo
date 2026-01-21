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

/// A native function call emitted by the model (and echoed back in the assistant
/// turn). `arguments` is a JSON *string* (the OpenAI contract).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ToolCallKind,
    pub function: FunctionCall,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolCallKind {
    Function,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    /// Raw JSON arguments string (may be `{}` when the model sent none).
    pub arguments: String,
}

