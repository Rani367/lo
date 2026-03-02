//! The conversation/turn state machine, ported from `src/renderer/renderer.ts`.
//!
//! Lives on the winit/UI thread. The epoch counter is the barge-in mechanism: it
//! is bumped on every new listen and every interrupt, and every worker-driven
//! tail is gated on it (an `AppEvent` whose `turn_id` is no longer the active one
//! is dropped), reproducing the renderer's `epoch`/`activeTurnId` discipline.

use lo_core::types::{ChatMessage, ChatRole, LoState};

/// Shortest accepted push-to-talk clip (0.2 s @ 16 kHz) — anything briefer is a
/// misfire (`MIN_PTT_SAMPLES` in renderer.ts).
pub const MIN_PTT_SAMPLES: usize = 3_200;
/// Rolling history cap fed to the brain (`MAX_HISTORY` in renderer.ts).
pub const MAX_HISTORY: usize = 12;
/// How long the captions linger after a turn before fading (`IDLE_FADE_MS`).
pub const IDLE_FADE_SECS: f32 = 4.2;

/// UI-thread session state.
pub struct Session {
    pub state: LoState,
