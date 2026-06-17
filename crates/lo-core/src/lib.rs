//! `lo-core` — the pure, dependency-light heart of Lo.
//!
//! It holds everything that needs no GPU, audio, or ML native toolchain, so it
//! builds fast and is exhaustively unit-tested:
//! - [`config`] — settings (+defaults/merge), paths, persona, history, options.
//! - [`text`] — TTS sentence/clause chunking and directive stripping.
//! - [`brain`] — the agent-loop building blocks + SSE parsing/tool-call merge.
//! - [`backends`] — engine selection, endpoint resolution, the RAM ladder, and download-URL resolution.
//! - [`tools`] — the tool registry, the safety gate, the audit log, the SSRF guard, the filesystem sandbox, and argv validation.
//! - [`types`] — the shared data contract passed between the core and the binary.
//!
//! The `lo` binary crate layers winit + wgpu + cpal + whisper-rs + ort on top and
//! drives these building blocks.

pub mod backends;
pub mod brain;
pub mod config;
pub mod shaders;
pub mod text;
pub mod tools;
pub mod types;

pub use config::LoSettings;
pub use types::{
    ActivationMode, BackendChoice, BackendKind, ChatMessage, ChatRole, ChatTurnResult, LoState,
    LocalStatus, ModelRecommendation,
};
