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
        });
    }
    convo
}

/// Build the `/chat/completions` request body (streaming, native tool-calling).
pub fn build_request_body(
    model: &str,
    convo: &[ReqMessage],
    temperature: f64,
) -> serde_json::Value {
    serde_json::json!({
        "model": model,
        "messages": convo,
        "tools": tools::tool_schemas_json(),
        "tool_choice": "auto",
        "temperature": temperature,
        "max_tokens": MAX_TOKENS,
        "stream": true,
    })
}

/// Append the assistant's tool-call turn followed by each tool result, mirroring
/// the loop body in `runBrainTurn`. `results` must be 1:1 with `calls`.
pub fn append_tool_round(
    convo: &mut Vec<ReqMessage>,
    assistant_text: &str,
    calls: Vec<ToolCall>,
    results: Vec<String>,
) {
    let content = if assistant_text.is_empty() {
        None
    } else {
        Some(assistant_text.to_string())
    };
    let ids: Vec<String> = calls.iter().map(|c| c.id.clone()).collect();
    convo.push(ReqMessage::assistant_tool_calls(content, calls));
    for (id, result) in ids.into_iter().zip(results) {
        convo.push(ReqMessage::tool_result(id, result));
    }
}

/// The spoken fallback when the model produced no text after all rounds.
pub const EMPTY_REPLY_FALLBACK: &str = "My apologies, I wasn't able to formulate a response.";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initial_convo_starts_with_system_prompt() {
        let s = LoSettings::default();
        let history = vec![
            ChatMessage {
                role: ChatRole::User,
                content: "hi".into(),
            },
            ChatMessage {
                role: ChatRole::Assistant,
                content: "hello".into(),
            },
        ];
        let convo = initial_convo(&s, &history);
        assert_eq!(convo.len(), 3);
        assert_eq!(convo[0].role, ReqRole::System);
        assert_eq!(convo[1].role, ReqRole::User);
        assert_eq!(convo[2].role, ReqRole::Assistant);
    }

    #[test]
    fn request_body_has_streaming_and_tools() {
        let s = LoSettings::default();
        let convo = initial_convo(&s, &[]);
        let body = build_request_body("my-model", &convo, 0.6);
        assert_eq!(body["model"], "my-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body["tools"].as_array().unwrap().len() >= 20);
    }

    #[test]
    fn tool_round_appends_assistant_then_results() {
        use types::{FunctionCall, ToolCallKind};
        let mut convo = vec![ReqMessage::user("what time is it")];
        let calls = vec![ToolCall {
            id: "call_0".into(),
            kind: ToolCallKind::Function,
            function: FunctionCall {
                name: "get_datetime".into(),
                arguments: "{}".into(),
            },
        }];
        append_tool_round(&mut convo, "", calls, vec!["It is 3pm.".into()]);
        assert_eq!(convo.len(), 3);
        assert_eq!(convo[1].role, ReqRole::Assistant);
        assert!(convo[1].content.is_none()); // pure tool-call turn
        assert_eq!(convo[2].role, ReqRole::Tool);
        assert_eq!(convo[2].tool_call_id.as_deref(), Some("call_0"));
    }
}
