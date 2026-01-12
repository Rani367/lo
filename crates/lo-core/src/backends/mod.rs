//! LLM backend selection + endpoint resolution (the pure logic of
//! `src/main/backends/index.ts` and the per-backend `baseUrl()/modelId()/apiKey()`
//! accessors). The process supervision (`ManagedServer`), the streaming HTTP
//! client, and the actual downloads live in the `lo` binary crate; this module
//! decides *which* engine serves and *where* to reach it.

pub mod download;
pub mod models;

use crate::config::LoSettings;
use crate::types::{BackendChoice, BackendKind};

