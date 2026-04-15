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
