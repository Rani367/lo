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
    resolve_backend_kind_for(settings.backend, has_custom_url, is_apple_silicon)
}

/// Pure core of [`resolve_backend_kind`] — exposed for exhaustive testing across
/// the env/platform matrix without mutating process env or faking the OS.
pub fn resolve_backend_kind_for(
    choice: BackendChoice,
    has_custom_url: bool,
    is_apple_silicon: bool,
) -> BackendKind {
    if has_custom_url || choice == BackendChoice::Custom {
        return BackendKind::Custom;
    }
    match choice {
        BackendChoice::Auto => {
            if is_apple_silicon {
                BackendKind::Mlx
            } else {
                BackendKind::Llama
            }
        }
        BackendChoice::Mlx => BackendKind::Mlx,
        BackendChoice::Llama => BackendKind::Llama,
        BackendChoice::Ollama => BackendKind::Ollama,
        BackendChoice::Custom => BackendKind::Custom,
    }
}

/// Resolve the endpoint for the active backend, honoring the same `LO_*` env
/// overrides the TS accessors used.
pub fn resolve_endpoint(settings: &LoSettings) -> BackendEndpoint {
    let kind = resolve_backend_kind(settings);
    match kind {
        BackendKind::Mlx => BackendEndpoint {
            kind,
            base_url: format!("http://{HOST}:{}/v1", port("LO_BRAIN_PORT", MLX_PORT)),
            model_id: env_or("LO_ENGINE_MODEL", &settings.model),
            api_key: None,
        },
        BackendKind::Llama => BackendEndpoint {
            kind,
            base_url: format!("http://{HOST}:{}/v1", port("LO_LLAMA_PORT", LLAMA_PORT)),
            model_id: env_or("LO_LLAMA_MODEL_ID", &settings.model),
            api_key: None,
        },
        BackendKind::Ollama => {
            let base = normalize_base(&env_or("LO_OLLAMA_URL", OLLAMA_URL));
            BackendEndpoint {
                kind,
                base_url: format!("{base}/v1"),
                model_id: env_or("LO_OLLAMA_MODEL", &settings.model),
                // Ollama ignores the key but the client wants one.
                api_key: Some(env_or("LO_OLLAMA_KEY", "ollama")),
            }
        }
        BackendKind::Custom => {
            let url = std::env::var("LO_LLM_URL")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .unwrap_or_else(|| settings.llm_url.trim().to_string());
            let key = std::env::var("LO_LLM_KEY")
                .ok()
                .map(|s| s.trim().to_string())
                .filter(|s| !s.is_empty())
                .or_else(|| {
                    let k = settings.llm_key.trim();
                    if k.is_empty() {
                        None
                    } else {
                        Some(k.to_string())
                    }
                });
            BackendEndpoint {
                kind,
                base_url: normalize_base(&url),
                model_id: env_or("LO_LLM_MODEL", &settings.model),
                api_key: key,
            }
        }
    }
}

/// Strip trailing slashes (the user supplies the full OpenAI base, e.g.
/// `http://host:1234/v1`).
pub fn normalize_base(url: &str) -> String {
    url.trim().trim_end_matches('/').to_string()
}

fn env_nonempty(key: &str) -> bool {
    std::env::var(key)
        .map(|v| !v.trim().is_empty())
