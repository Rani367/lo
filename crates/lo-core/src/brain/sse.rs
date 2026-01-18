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
