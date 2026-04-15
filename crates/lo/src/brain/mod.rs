//! The brain transport — the async streaming HTTP client that talks to the
//! active OpenAI-compatible engine. This is the `streamCompletion` + `readSse`
//! half of `src/main/brain.ts`; the `MAX_ROUNDS` agent loop itself lives in the
//! orchestrator (the worker), which calls [`stream_completion`] once per round.
//!
//! All of the *parsing* (turning raw `data:` lines into prose + a `Vec<ToolCall>`)
//! is reused from `lo_core::brain::sse::StreamAccumulator`; this module only owns
//! the transport: POST the body, stream `bytes_stream()`, split on `\n`, and feed
//! each line into the accumulator until the terminal `[DONE]`.

use anyhow::{anyhow, Context};
use futures_util::StreamExt;
use lo_core::backends::BackendEndpoint;
use lo_core::brain::sse::StreamAccumulator;
use lo_core::brain::types::ToolCall;

/// How much of a non-2xx error body to surface (the rest is noise).
const ERROR_BODY_LIMIT: usize = 300;

/// Stream a single completion from `{endpoint.base_url}/chat/completions`.
///
/// Forwards each assistant prose delta to `on_delta` as it arrives (matching the
/// TS `cb.onDelta`) and accumulates any native `tool_calls`. Returns the final
/// `(prose, tool_calls)` once the stream reaches its terminal `[DONE]` (or the
/// connection closes). On a non-2xx response, returns an error carrying the
/// status and a truncated body.
pub async fn stream_completion(
    endpoint: &BackendEndpoint,
    body: serde_json::Value,
    on_delta: &mut (dyn FnMut(&str) + Send),
) -> anyhow::Result<(String, Vec<ToolCall>)> {
    let url = format!("{}/chat/completions", endpoint.base_url);

    let client = reqwest::Client::builder()
        .build()
        .context("failed to build HTTP client")?;

    let mut req = client.post(&url).json(&body);
    if let Some(key) = endpoint.api_key.as_deref() {
        // `authorization: Bearer <key>` only when a key is configured (the TS
        // `authHeader` helper); local servers stay unauthenticated.
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
    }

    let res = req
        .send()
