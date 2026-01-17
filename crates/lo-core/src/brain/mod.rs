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
