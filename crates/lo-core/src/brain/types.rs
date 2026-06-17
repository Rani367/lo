//! OpenAI chat-message shapes for the agent loop (richer than the UI-facing
//! `ChatMessage`: the loop also produces assistant `tool_calls` and `tool`
//! results).

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

/// One message in the request `messages[]` array.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReqMessage {
    pub role: ReqRole,
    /// `null` is valid for an assistant turn that is *only* tool calls.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub content: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool_call_id: Option<String>,
}

impl ReqMessage {
    pub fn system(content: impl Into<String>) -> Self {
        Self {
            role: ReqRole::System,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn user(content: impl Into<String>) -> Self {
        Self {
            role: ReqRole::User,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    pub fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: ReqRole::Assistant,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: None,
        }
    }
    /// The assistant turn that carries `tool_calls` (content may be empty/None).
    pub fn assistant_tool_calls(content: Option<String>, calls: Vec<ToolCall>) -> Self {
        Self {
            role: ReqRole::Assistant,
            content,
            tool_calls: Some(calls),
            tool_call_id: None,
        }
    }
    /// A `tool` result message referencing the originating call.
    pub fn tool_result(tool_call_id: impl Into<String>, content: impl Into<String>) -> Self {
        Self {
            role: ReqRole::Tool,
            content: Some(content.into()),
            tool_calls: None,
            tool_call_id: Some(tool_call_id.into()),
        }
    }
}
