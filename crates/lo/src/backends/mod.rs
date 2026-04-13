//! Backend selector + brain lifecycle — one OpenAI-compatible client surface over
//! four interchangeable engines (MLX, bundled llama.cpp, a detected Ollama, or any
//! custom endpoint), chosen by platform/hardware/settings.
//!
//! Ported from `src/main/backends/index.ts` + the per-backend modules (`mlx.ts`,
//! `llama.ts`, `ollama.ts`, `custom.ts`). The *selection* and *endpoint* logic is
//! reused from `lo_core::backends`; this module owns the process supervision
//! (spawning MLX / llama-server via [`ManagedServer`]), the first-run downloads,
//! and the health checks for the unmanaged engines.
//!
//! The [`brain`](crate::brain) transport talks only to the [`BackendEndpoint`]
//! this module resolves, so swapping engines never touches the streaming loop.

pub mod download;
pub mod managed_server;

use std::path::PathBuf;
use std::sync::Mutex;
use std::time::Duration;

use lo_core::backends::models::gguf_file_for;
use lo_core::backends::{
    resolve_backend_kind, resolve_endpoint, BackendEndpoint, HOST, LLAMA_PORT, MLX_PORT,
};
use lo_core::config::paths::cache_dir;
use lo_core::types::{BackendKind, LocalStatus};
use lo_core::LoSettings;

use managed_server::{CommandSpec, ManagedServer, ServerSpec, ServerState};

/// Progress callback type for first-run downloads: `(label, pct)`, `pct == None`
/// while indeterminate. Threaded through to [`download`].
pub type ProgressFn<'a> = Option<&'a (dyn Fn(&str, Option<u8>) + Send + Sync)>;

/// The active managed process, tagged with the backend kind it serves so a
/// settings flip (which changes the resolved kind) tears down the old one.
struct Active {
    kind: BackendKind,
    server: ManagedServer,
}

/// The engine façade. Holds the active managed server (for MLX / llama.cpp) and
/// the unmanaged engines' health state, reconstructing when the resolved kind
/// changes.
pub struct Engine {
    active: Mutex<Option<Active>>,
    /// Last health state for the unmanaged backends (Ollama / Custom), since they
    /// have no `ManagedServer` to hold it.
    unmanaged: Mutex<UnmanagedState>,
}

#[derive(Default)]
struct UnmanagedState {
    state: Option<ServerState>,
    last_error: Option<String>,
}

impl Default for Engine {
    fn default() -> Self {
        Self::new()
    }
}

impl Engine {
    /// Create an idle engine. The concrete backend is resolved lazily on the
    /// first [`ensure_ready`](Self::ensure_ready).
    pub fn new() -> Engine {
        Engine {
            active: Mutex::new(None),
            unmanaged: Mutex::new(UnmanagedState::default()),
        }
    }

    /// Resolve the endpoint for the active backend (delegates to
    /// `lo_core::backends::resolve_endpoint`).
    pub fn endpoint(&self, settings: &LoSettings) -> BackendEndpoint {
        resolve_endpoint(settings)
    }

    /// Ensure the active backend is up and the model is loaded + reachable.
    ///
    /// - MLX / llama → spawn (downloading the llama binary + GGUF first) and
    ///   health-poll a [`ManagedServer`].
    /// - Ollama / Custom → a single health GET (never spawned/stopped).
    pub async fn ensure_ready(
        &self,
        settings: &LoSettings,
        progress: ProgressFn<'_>,
    ) -> anyhow::Result<()> {
        let kind = resolve_backend_kind(settings);
        match kind {
            BackendKind::Mlx => {
                let server = self.managed_for(kind, settings);
                server.ensure().await
            }
            BackendKind::Llama => {
                // Acquire the binary + weights before spawning (skipped if env
                // overrides already point at existing files).
                self.ensure_llama_assets(settings, progress).await?;
                let server = self.managed_for(kind, settings);
                server.ensure().await
            }
            BackendKind::Ollama => self.ensure_ollama(settings).await,
            BackendKind::Custom => self.ensure_custom(settings).await,
        }
    }

    /// Restart the active backend. Managed servers are killed + respawned; the
    /// unmanaged ones simply re-health-check.
    pub async fn restart(&self, settings: &LoSettings) -> anyhow::Result<()> {
        let kind = resolve_backend_kind(settings);
        match kind {
            BackendKind::Mlx | BackendKind::Llama => {
                let server = self.managed_for(kind, settings);
                server.restart().await;
                if server.state() == ServerState::Ready {
                    Ok(())
                } else {
                    Err(anyhow::anyhow!(server
                        .last_error()
                        .unwrap_or_else(|| "engine failed to restart".into())))
                }
            }
            BackendKind::Ollama => self.ensure_ollama(settings).await,
            BackendKind::Custom => self.ensure_custom(settings).await,
        }
    }

    /// Stop the active managed server (no-op for the unmanaged backends — they're
    /// not ours to stop).
    pub fn stop(&self) {
        if let Some(active) = self.active.lock().expect("active poisoned").as_ref() {
            active.server.stop();
        }
    }

    /// A non-blocking health snapshot for the HUD (mirrors `getEngineStatus`,
    /// folding only the brain — local ASR health lives in the worker).
    pub fn status(&self, settings: &LoSettings) -> LocalStatus {
        let endpoint = resolve_endpoint(settings);
        let model = endpoint.model_id.clone();
        let kind = endpoint.kind;

        let (state, last_error) = self.current_state(kind);

        match state {
            ServerState::Ready => LocalStatus {
                engine_up: true,
                loading: false,
                backend: Some(kind),
                model,
                detail: None,
            },
            ServerState::Error => LocalStatus {
                engine_up: false,
                loading: false,
                backend: None,
                model,
                detail: last_error,
            },
            // Idle counts as "loading" in the HUD: a load will be kicked off.
            ServerState::Idle | ServerState::Loading => LocalStatus {
                engine_up: false,
                loading: true,
                backend: None,
                model,
                detail: Some("Loading model…".to_string()),
            },
        }
    }

    /// Fire a best-effort 1-token completion to warm prompt-cache / JIT (mirrors
    /// `warmCompletion`). Never fails the caller.
    pub async fn warm(&self, settings: &LoSettings) {
        let endpoint = resolve_endpoint(settings);
        let _ = warm_completion(&endpoint).await;
    }

    /* ---------------- managed (MLX / llama) ---------------- */

    /// Get (constructing/replacing as needed) the [`ManagedServer`] for the given
    /// kind. A kind change stops the prior server first, mirroring the TS cache
    /// invalidation in `getLlmBackend`.
    fn managed_for(&self, kind: BackendKind, settings: &LoSettings) -> ManagedServer {
        let mut guard = self.active.lock().expect("active poisoned");
        if let Some(active) = guard.as_ref() {
            if active.kind == kind {
                return active.server.handle();
            }
            active.server.stop();
        }
        let server = match kind {
            BackendKind::Mlx => build_mlx_server(settings),
            BackendKind::Llama => build_llama_server(settings),
            _ => unreachable!("managed_for is only called for MLX/Llama"),
        };
        let handle = server.handle();
        *guard = Some(Active { kind, server });
        handle
    }

    /// Acquire the llama binary + GGUF weights if missing (honoring the env
    /// overrides), before spawning the server.
    async fn ensure_llama_assets(
        &self,
        settings: &LoSettings,
        progress: ProgressFn<'_>,
    ) -> anyhow::Result<()> {
        let bin = llama_bin_path();
        let bin_override = env_trimmed("LO_LLAMA_BIN").is_some();
        if !bin_override || !bin.exists() {
            download::ensure_llama_binary(&bin, progress).await?;
        }

        let model_path = llama_model_path(settings);
        let model_override = env_trimmed("LO_LLAMA_MODEL").is_some();
        if !model_override || !model_path.exists() {
            download::ensure_gguf_model(&settings.model, &model_path, progress).await?;
        }
        Ok(())
    }

    /* ---------------- unmanaged (Ollama / Custom) ---------------- */

    async fn ensure_ollama(&self, settings: &LoSettings) -> anyhow::Result<()> {
        self.set_unmanaged(ServerState::Loading, None);
        let endpoint = resolve_endpoint(settings);
        // endpoint.base_url ends in `/v1`; the health probe is `/api/tags` on the
        // bare Ollama base (strip exactly one `/v1` suffix).
        let base = endpoint
            .base_url
            .strip_suffix("/v1")
            .unwrap_or(&endpoint.base_url);
        let url = format!("{base}/api/tags");
        match health_get(&url, None, Duration::from_secs(3)).await {
            Ok(status) if (200..300).contains(&status) => {
                self.set_unmanaged(ServerState::Ready, None);
                Ok(())
            }
            _ => {
                let msg = format!(
                    "Ollama is not reachable at {base}. Start it with `ollama serve` and pull a tool-capable model."
                );
                self.set_unmanaged(ServerState::Error, Some(msg.clone()));
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    async fn ensure_custom(&self, settings: &LoSettings) -> anyhow::Result<()> {
        let endpoint = resolve_endpoint(settings);
        if endpoint.base_url.is_empty() {
            let msg =
                "No custom LLM endpoint configured. Set LO_LLM_URL or the endpoint in Settings."
                    .to_string();
            self.set_unmanaged(ServerState::Error, Some(msg.clone()));
            return Err(anyhow::anyhow!(msg));
        }
        self.set_unmanaged(ServerState::Loading, None);
        // Any HTTP response to /models means the host is reachable (some servers
        // 404 it but still serve /chat/completions).
        let url = format!("{}/models", endpoint.base_url);
        match health_get(&url, endpoint.api_key.as_deref(), Duration::from_secs(5)).await {
            Ok(_status) => {
                self.set_unmanaged(ServerState::Ready, None);
                Ok(())
            }
            Err(err) => {
                let msg = format!(
                    "Custom endpoint unreachable at {}: {err}",
                    endpoint.base_url
                );
                self.set_unmanaged(ServerState::Error, Some(msg.clone()));
                Err(anyhow::anyhow!(msg))
            }
        }
    }

    fn set_unmanaged(&self, state: ServerState, err: Option<String>) {
        let mut u = self.unmanaged.lock().expect("unmanaged poisoned");
        u.state = Some(state);
        u.last_error = err;
    }

    /// The current `(state, last_error)` for the resolved kind.
    fn current_state(&self, kind: BackendKind) -> (ServerState, Option<String>) {
        match kind {
            BackendKind::Mlx | BackendKind::Llama => {
                let guard = self.active.lock().expect("active poisoned");
                match guard.as_ref() {
                    Some(active) if active.kind == kind => {
                        (active.server.state(), active.server.last_error())
                    }
                    _ => (ServerState::Idle, None),
                }
            }
            BackendKind::Ollama | BackendKind::Custom => {
                let u = self.unmanaged.lock().expect("unmanaged poisoned");
                (u.state.unwrap_or(ServerState::Idle), u.last_error.clone())
            }
        }
    }
}

/* ---------------- server specs ---------------- */

/// `python -m mlx_lm server --model <id> --host 127.0.0.1 --port 8765
/// --trust-remote-code` — the Apple-Silicon fast path (mirrors `mlx.ts`).
fn build_mlx_server(settings: &LoSettings) -> ManagedServer {
    let port = port_env("LO_BRAIN_PORT", MLX_PORT);
    let model = resolve_endpoint(settings).model_id;
    let spec = ServerSpec {
