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

/// The literal `model` value that means "pick the best model for this machine".
pub const AUTO_MODEL: &str = "auto";

/// The model id shipped in `LoSettings::default()`. When the settings still carry
/// it (i.e. the user never chose a model), treat selection as automatic and pick
/// by RAM rather than forcing the 30B everywhere — the bug this fixes.
pub const DEFAULT_MODEL_SENTINEL: &str = "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ";

/// Resolve the logical model id to actually use, applying the hardware RAM ladder
/// when the user left the model on `auto`/the shipped default. An explicit
/// non-default `settings_model` is always honored verbatim.
///
/// `ram_bytes` is the OS-reported total memory (the bin crate passes
/// `sysinfo`'s value; `LO_RAM_GB` still overrides inside [`models::total_ram_gb`]).
/// The tier maps to the right artifact for the active backend: the MLX weight,
/// the GGUF ref, or the Ollama tag. Custom endpoints define their own model, so
/// the value is passed through unchanged.
pub fn resolve_model_id(settings_model: &str, kind: BackendKind, ram_bytes: u64) -> String {
    let m = settings_model.trim();
    let automatic =
        m.is_empty() || m.eq_ignore_ascii_case(AUTO_MODEL) || m == DEFAULT_MODEL_SENTINEL;
    if !automatic {
        return m.to_string();
    }
    let tier = models::recommend_tier(models::total_ram_gb(ram_bytes));
    match kind {
        BackendKind::Mlx => tier.mlx.to_string(),
        BackendKind::Llama => tier.gguf.to_string(),
        BackendKind::Ollama => tier.ollama.to_string(),
        // A custom endpoint's model is whatever it serves; don't invent one.
        BackendKind::Custom => m.to_string(),
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
        .unwrap_or(false)
}

fn env_or(key: &str, fallback: &str) -> String {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| fallback.to_string())
}

fn port(key: &str, default: u16) -> u16 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.trim().parse::<u16>().ok())
        .filter(|&p| p != 0)
        .unwrap_or(default)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn auto_selects_mlx_on_apple_silicon_else_llama() {
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Auto, false, true),
            BackendKind::Mlx
        );
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Auto, false, false),
            BackendKind::Llama
        );
    }

    #[test]
    fn custom_url_or_choice_forces_custom() {
        // A custom URL in env overrides even an explicit non-custom choice.
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Mlx, true, true),
            BackendKind::Custom
        );
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Custom, false, false),
            BackendKind::Custom
        );
    }

    #[test]
    fn explicit_choices_pass_through() {
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Ollama, false, true),
            BackendKind::Ollama
        );
        assert_eq!(
            resolve_backend_kind_for(BackendChoice::Llama, false, true),
            BackendKind::Llama
        );
    }

    #[test]
    fn auto_model_uses_ram_ladder_per_backend() {
        // 8 GB → smallest tier (Qwen3-4B), mapped to each backend's artifact.
        let gb8 = 8u64 * 1_000_000_000;
        assert_eq!(
            resolve_model_id(AUTO_MODEL, BackendKind::Mlx, gb8),
            "mlx-community/Qwen3-4B-4bit"
        );
        assert_eq!(
            resolve_model_id(AUTO_MODEL, BackendKind::Ollama, gb8),
            "qwen3:4b"
        );
        assert!(resolve_model_id(AUTO_MODEL, BackendKind::Llama, gb8).contains("Qwen3-4B"));
        // The shipped default sentinel is treated as "auto" too.
        let gb64 = 64u64 * 1_000_000_000;
        assert_eq!(
            resolve_model_id(DEFAULT_MODEL_SENTINEL, BackendKind::Mlx, gb64),
            "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ"
        );
    }

    #[test]
    fn explicit_model_is_honored_verbatim() {
        let gb8 = 8u64 * 1_000_000_000;
        assert_eq!(
            resolve_model_id("mlx-community/Qwen3-14B-4bit", BackendKind::Mlx, gb8),
            "mlx-community/Qwen3-14B-4bit"
        );
        // Custom endpoints define their own model; auto passes through.
        assert_eq!(
            resolve_model_id(AUTO_MODEL, BackendKind::Custom, gb8),
            AUTO_MODEL
        );
    }

    #[test]
    fn normalize_base_strips_trailing_slashes() {
        assert_eq!(
            normalize_base("http://host:1234/v1/"),
            "http://host:1234/v1"
        );
        assert_eq!(
            normalize_base("http://host:1234/v1///"),
            "http://host:1234/v1"
        );
        assert_eq!(normalize_base("  http://h/v1  "), "http://h/v1");
    }
}
