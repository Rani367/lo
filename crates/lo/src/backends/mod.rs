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
