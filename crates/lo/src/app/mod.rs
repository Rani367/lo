//! The winit `ApplicationHandler`: owns the window, the GPU (`Gui`), the cpal
//! `AudioEngine` (kept alive here on the main thread), and the UI-side
//! `Session` state machine. It forwards input to the worker/listen thread and
//! renders the orb + captions each frame. Ported from the orchestration in
//! `src/renderer/renderer.ts`.

pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;
