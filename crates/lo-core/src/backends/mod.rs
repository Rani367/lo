//! LLM backend selection + endpoint resolution (the pure logic of
//! `src/main/backends/index.ts` and the per-backend `baseUrl()/modelId()/apiKey()`
//! accessors). The process supervision (`ManagedServer`), the streaming HTTP
//! client, and the actual downloads live in the `lo` binary crate; this module
//! decides *which* engine serves and *where* to reach it.

pub mod download;
pub mod models;

use crate::config::LoSettings;
use crate::types::{BackendChoice, BackendKind};

/// Default loopback ports (overridable via `LO_*_PORT`).
pub const MLX_PORT: u16 = 8765;
pub const LLAMA_PORT: u16 = 8770;
pub const OLLAMA_URL: &str = "http://127.0.0.1:11434";
pub const HOST: &str = "127.0.0.1";

/// Where and how to reach the active OpenAI-compatible engine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BackendEndpoint {
    pub kind: BackendKind,
    /// Base URL ending in `/v1`, e.g. `http://127.0.0.1:8765/v1`.
    pub base_url: String,
    /// The exact model id requests must name.
    pub model_id: String,
    /// Bearer token, or `None` for unauthenticated local servers.
    pub api_key: Option<String>,
}

/// Which concrete backend should serve, given env + settings (mirrors
/// `resolveBackendKind`). Reads `LO_LLM_URL` and the host platform/arch.
pub fn resolve_backend_kind(settings: &LoSettings) -> BackendKind {
    let has_custom_url = env_nonempty("LO_LLM_URL");
    let is_apple_silicon = cfg!(all(target_os = "macos", target_arch = "aarch64"));
