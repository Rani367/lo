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
        .await
        .with_context(|| format!("brain request failed to send to {url}"))?;

    let status = res.status();
    if !status.is_success() {
        // Mirror `Brain request failed (status): detail.slice(0,300)`.
        let detail = res.text().await.unwrap_or_default();
        let truncated: String = detail.chars().take(ERROR_BODY_LIMIT).collect();
        return Err(anyhow!(
            "Brain request failed ({}): {}",
            status.as_u16(),
            truncated
        ));
    }

    let mut acc = StreamAccumulator::new();
    // Buffer raw bytes (not a lossy `String`): a multi-byte UTF-8 char split
    // across two chunks must not be corrupted. We only decode *complete*,
    // `\n`-terminated lines — by then any split char has been reassembled.
    let mut buffer: Vec<u8> = Vec::new();
    let mut stream = res.bytes_stream();

    'outer: while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("error reading brain response stream")?;
        buffer.extend_from_slice(&chunk);

        // Split on '\n', feeding each complete line to the accumulator. The
        // trailing (possibly partial) segment stays in `buffer`.
        while let Some(nl) = buffer.iter().position(|&b| b == b'\n') {
            let line: Vec<u8> = buffer.drain(..=nl).collect();
            let line = String::from_utf8_lossy(&line);
            let (delta, done) = acc.push_line(&line);
            if let Some(text) = delta {
                on_delta(&text);
            }
            if done {
                break 'outer;
            }
        }
    }

    // Flush any final line the stream left without a trailing newline.
    if !buffer.is_empty() {
        let line = String::from_utf8_lossy(&buffer);
        let (delta, _done) = acc.push_line(&line);
        if let Some(text) = delta {
