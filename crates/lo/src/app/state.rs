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
    /// Bumped on every new listen / barge-in; gates async tails.
    pub epoch: u64,
    /// The turn whose streamed deltas/tools the UI currently accepts.
    pub active_turn_id: String,
    /// A turn is in flight (thinking/speaking).
    pub busy: bool,
    /// Space is held and we're buffering a PTT clip.
    pub ptt_recording: bool,
    /// Rolling transcript shared with the brain.
    pub history: Vec<ChatMessage>,
    /// The user's last utterance (top caption line).
    pub you_text: String,
    /// Lo's streaming reply (bottom caption line).
    pub lo_text: String,
    /// Seconds since the last turn finished (drives the caption fade).
    pub since_done: f32,
    turn_counter: u64,
}

impl Default for Session {
    fn default() -> Self {
        Self::new()
    }
