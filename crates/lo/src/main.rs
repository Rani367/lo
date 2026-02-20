//! Lo — pure-native Rust voice agent. Entry point + threading model.
//!
//! The winit event loop owns the main thread (it is intentionally NOT async). A
//! multi-thread tokio runtime hosts the worker (brain loop, backends, tools,
//! downloads). On-device ML (whisper ASR, Silero VAD) runs on a dedicated
//! "listen" std thread that owns the !Send models; Kokoro TTS runs on its own
//! std thread (spawned by the worker). cpal audio streams (also !Send) live on
//! the main thread inside the `App`.
//!
//! Bridges (replacing Electron IPC, see `events`):
//!   - UI/listen → worker: `mpsc::UnboundedSender<UiCommand>`
//!   - worker/ML → UI: `EventLoopProxy<AppEvent>` → `ApplicationHandler::user_event`
//!   - barge-in epoch: a `watch::channel<u64>` the UI bumps and the worker/TTS
//!     observe to abandon stale work.

// Don't pop a console window on Windows release builds.
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]
// The binary carries APIs that are wired up in later phases (wake-word
// activation, settings hot-reload, engine prewarm, the status-HUD dot). Allow
// dead_code crate-wide so CI's `-D warnings` stays green without prematurely
// deleting that scaffolding; `lo-core` (the tested core) stays strictly clean.
#![allow(dead_code)]

mod app;
mod audio;
mod backends;
mod brain;
mod events;
mod gui;
mod listen;
mod ml;
mod tools;
mod worker;
