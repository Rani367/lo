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

