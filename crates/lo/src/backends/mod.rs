//! Backend selector + brain lifecycle — one OpenAI-compatible client surface over
//! four interchangeable engines (MLX, bundled llama.cpp, a detected Ollama, or any
//! custom endpoint), chosen by platform/hardware/settings.
//!
//! The *selection* and *endpoint* logic lives in `lo_core::backends`; this module
//! owns the process supervision (spawning MLX / llama-server via
//! [`ManagedServer`]), the first-run downloads, and the health checks for the
//! unmanaged engines.
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

    /// Stop the active managed server (no-op for the unmanaged backends — they're
    /// not ours to stop).
    pub fn stop(&self) {
        if let Some(active) = self.active.lock().expect("active poisoned").as_ref() {
            active.server.stop();
        }
    }

    /// A non-blocking health snapshot for the HUD, folding only the brain — local
    /// ASR health lives in the worker.
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

    /* ---------------- managed (MLX / llama) ---------------- */

    /// Get (constructing/replacing as needed) the [`ManagedServer`] for the given
    /// kind. A kind change stops the prior server first, so a settings flip never
    /// leaves two engines contending for the port.
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
/// --trust-remote-code` — the Apple-Silicon fast path.
fn build_mlx_server(settings: &LoSettings) -> ManagedServer {
    let port = port_env("LO_BRAIN_PORT", MLX_PORT);
    let model = resolve_endpoint(settings).model_id;
    let spec = ServerSpec {
        name: "brain".to_string(),
        health_url: format!("http://{HOST}:{port}/health"),
        // mlx_lm server loads the model before it serves; 200 = ready.
        is_ready: Box::new(|status| status == 200),
        build: Box::new(move || CommandSpec {
            program: python_command(),
            args: vec![
                "-m".into(),
                "mlx_lm".into(),
                "server".into(),
                "--model".into(),
                model.clone(),
                "--host".into(),
                HOST.to_string(),
                "--port".into(),
                port.to_string(),
                "--trust-remote-code".into(),
            ],
            envs: vec![("PYTHONUNBUFFERED".to_string(), "1".to_string())],
        }),
    };
    ManagedServer::new(spec)
}

/// `llama-server --model <path> --host 127.0.0.1 --port 8770 --jinja -fa
/// --no-webui -ngl 999 -c <ctx>` — the universal cross-platform engine. Args are a
/// tuned, deliberately fixed set.
fn build_llama_server(settings: &LoSettings) -> ManagedServer {
    let port = port_env("LO_LLAMA_PORT", LLAMA_PORT);
    let bin = llama_bin_path();
    let model_path = llama_model_path(settings);
    let ctx = env_trimmed("LO_LLAMA_CTX")
        .and_then(|v| v.parse::<u32>().ok())
        .filter(|&c| c != 0)
        .unwrap_or(8192);

    let bin_str = bin.to_string_lossy().to_string();
    let model_str = model_path.to_string_lossy().to_string();

    let spec = ServerSpec {
        name: "llama".to_string(),
        health_url: format!("http://{HOST}:{port}/health"),
        // llama-server /health is 200 once the model is loaded.
        is_ready: Box::new(|status| status == 200),
        build: Box::new(move || CommandSpec {
            program: bin_str.clone(),
            args: vec![
                "--model".into(),
                model_str.clone(),
                "--host".into(),
                HOST.to_string(),
                "--port".into(),
                port.to_string(),
                "--jinja".into(),
                "-fa".into(),
                "--no-webui".into(),
                "-ngl".into(),
                "999".into(),
                "-c".into(),
                ctx.to_string(),
            ],
            envs: vec![],
        }),
    };
    ManagedServer::new(spec)
}

/* ---------------- path / env helpers ---------------- */

/// Resolve the Python interpreter for the MLX server: `LO_PYTHON` (or the
/// `LO_GEMMA_AUDIO_PYTHON` alias), else the project-local venv if present, else
/// `python3`.
fn python_command() -> String {
    if let Some(explicit) =
        env_trimmed("LO_PYTHON").or_else(|| env_trimmed("LO_GEMMA_AUDIO_PYTHON"))
    {
        return explicit;
    }
    let venv = PathBuf::from(".venv-gemma-audio")
        .join("bin")
        .join("python");
    if venv.exists() {
        return venv.to_string_lossy().to_string();
    }
    "python3".to_string()
}

/// Path to the llama-server binary: `LO_LLAMA_BIN` override, else the managed
/// location under the cache dir (`engine/llama/llama-server[.exe]`).
fn llama_bin_path() -> PathBuf {
    if let Some(explicit) = env_trimmed("LO_LLAMA_BIN") {
        return PathBuf::from(explicit);
    }
    let exe = if cfg!(windows) {
        "llama-server.exe"
    } else {
        "llama-server"
    };
    cache_dir().join("engine").join("llama").join(exe)
}

/// Path to the GGUF weights: `LO_LLAMA_MODEL` override, else the managed download
/// under the cache dir (`models/<gguf_file_for(model)>`).
fn llama_model_path(settings: &LoSettings) -> PathBuf {
    if let Some(explicit) = env_trimmed("LO_LLAMA_MODEL") {
        return PathBuf::from(explicit);
    }
    cache_dir()
        .join("models")
        .join(gguf_file_for(&settings.model))
}

fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

fn port_env(key: &str, default: u16) -> u16 {
    env_trimmed(key)
        .and_then(|v| v.parse::<u16>().ok())
        .filter(|&p| p != 0)
        .unwrap_or(default)
}

/* ---------------- HTTP helpers ---------------- */

/// A bare health GET; returns the HTTP status code on any response.
async fn health_get(url: &str, api_key: Option<&str>, timeout: Duration) -> anyhow::Result<u16> {
    let client = reqwest::Client::builder()
        .build()
        .map_err(|e| anyhow::anyhow!("failed to build client: {e}"))?;
    let mut req = client.get(url).timeout(timeout);
    if let Some(key) = api_key {
        req = req.header(reqwest::header::AUTHORIZATION, format!("Bearer {key}"));
    }
    let res = req.send().await?;
    Ok(res.status().as_u16())
}
