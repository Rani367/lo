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
    initial_convo_with_time(settings, history, None)
}

/// Like [`initial_convo`] but also grounds the model in the current local time
/// (passed in by the binary, which owns the date library), so it can reason about
/// "today"/"now" without a tool round. `now` is a preformatted local timestamp,
/// e.g. `"Sunday, 14 June 2026, 19:46"`.
pub fn initial_convo_with_time(
    settings: &LoSettings,
    history: &[ChatMessage],
    now: Option<&str>,
) -> Vec<ReqMessage> {
    let mut convo = Vec::with_capacity(history.len() + 2);
    convo.push(ReqMessage::system(persona::build_system_prompt(settings)));
    if let Some(now) = now {
        let now = now.trim();
        if !now.is_empty() {
            convo.push(ReqMessage::system(format!(
                "The current local date and time is {now}. Use this for relative references like \"today\" or \"tonight\"; for anything else time-sensitive, use a tool."
            )));
        }
    }
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

/// Decoding parameters for one request, derived from [`LoSettings`]. Kept separate
/// from the model id so the headless and worker paths share one source of truth.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Sampling {
    pub temperature: f64,
    pub top_p: f64,
    pub top_k: u32,
    pub repeat_penalty: f64,
    pub min_p: f64,
}

impl Sampling {
    /// Read the sampling knobs out of the user settings.
    pub fn from_settings(s: &LoSettings) -> Self {
        Self {
            temperature: s.temperature,
            top_p: s.top_p,
            top_k: s.top_k,
            repeat_penalty: s.repeat_penalty,
            min_p: s.min_p,
        }
    }
}

/// Build the `/chat/completions` request body (streaming, native tool-calling).
///
/// `temperature` and `top_p` go to every backend. `top_k`, `min_p`, and the
/// repetition penalty are local-engine extensions, so they are emitted only when
/// set to a non-omitting value; the penalty is sent under both the llama.cpp
/// (`repeat_penalty`) and the MLX / vLLM (`repetition_penalty`) field names so it
/// takes effect whichever local server is serving. Unknown fields are ignored by
/// well-behaved OpenAI-compatible endpoints.
pub fn build_request_body(
    model: &str,
    convo: &[ReqMessage],
    sampling: Sampling,
) -> serde_json::Value {
    let mut body = serde_json::json!({
        "model": model,
        "messages": convo,
        "tools": tools::tool_schemas_json(),
        "tool_choice": "auto",
        "temperature": sampling.temperature,
        "top_p": sampling.top_p,
        "max_tokens": MAX_TOKENS,
        "stream": true,
    });
    let obj = body.as_object_mut().expect("json object");
    if sampling.top_k > 0 {
        obj.insert("top_k".into(), sampling.top_k.into());
    }
    if sampling.min_p > 0.0 {
        obj.insert("min_p".into(), sampling.min_p.into());
    }
    if sampling.repeat_penalty > 1.0 {
        obj.insert("repeat_penalty".into(), sampling.repeat_penalty.into());
        obj.insert("repetition_penalty".into(), sampling.repeat_penalty.into());
    }
    body
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
        let body = build_request_body("my-model", &convo, Sampling::from_settings(&s));
        assert_eq!(body["model"], "my-model");
        assert_eq!(body["stream"], true);
        assert_eq!(body["tool_choice"], "auto");
        assert_eq!(body["max_tokens"], 1024);
        assert!(body["tools"].as_array().unwrap().len() >= 20);
        // Sampling: top_p always present; the penalty is sent under both names.
        assert_eq!(body["top_p"], 0.95);
        assert_eq!(body["top_k"], 40);
        assert_eq!(body["repeat_penalty"], 1.1);
        assert_eq!(body["repetition_penalty"], 1.1);
    }

    #[test]
    fn request_body_omits_unset_local_knobs() {
        let convo = initial_convo(&LoSettings::default(), &[]);
        let sampling = Sampling {
            temperature: 0.6,
            top_p: 0.9,
            top_k: 0,
            repeat_penalty: 1.0,
            min_p: 0.0,
        };
        let body = build_request_body("m", &convo, sampling);
        assert!(body.get("top_k").is_none());
        assert!(body.get("min_p").is_none());
        assert!(body.get("repeat_penalty").is_none());
        assert!(body.get("repetition_penalty").is_none());
        assert_eq!(body["top_p"], 0.9);
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
