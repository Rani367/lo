//! The conversation/turn state machine.
//!
//! Lives on the winit/UI thread. The epoch counter is the barge-in mechanism: it
//! is bumped on every new listen and every interrupt, and every worker-driven
//! tail is gated on it (an `AppEvent` whose `turn_id` is no longer the active one
//! is dropped), so a superseded turn can never write into the live conversation.

use lo_core::types::{ChatMessage, ChatRole, LoState};

/// Shortest accepted push-to-talk clip (0.2 s @ 16 kHz) — anything briefer is a
/// misfire.
pub const MIN_PTT_SAMPLES: usize = 3_200;
/// Rolling history cap fed to the brain.
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
}

impl Session {
    pub fn new() -> Self {
        Self {
            state: LoState::Boot,
            epoch: 0,
            active_turn_id: String::new(),
            busy: false,
            ptt_recording: false,
            history: Vec::new(),
            you_text: String::new(),
            lo_text: String::new(),
            since_done: 0.0,
            turn_counter: 0,
        }
    }

    /// Set the visual state directly (e.g. boot → idle once the GPU is live).
    pub fn set_state(&mut self, s: LoState) {
        self.state = s;
    }

    /// Barge-in / new listen: invalidate any in-flight turn. Returns the new epoch
    /// the caller should pass to `UiCommand::Cancel` and carry into the next turn.
    pub fn interrupt(&mut self) -> u64 {
        self.epoch += 1;
        self.active_turn_id.clear();
        self.busy = false;
        self.epoch
    }

    /// Start listening (push-to-talk pressed). Bumps the epoch so a turn in
    /// progress is abandoned (barge-in).
    pub fn begin_listen(&mut self) -> u64 {
        let e = self.interrupt();
        self.ptt_recording = true;
        self.you_text.clear();
        self.lo_text.clear();
        self.state = LoState::Listening;
        e
    }

    /// The user finished a clip and it transcribed to `transcript`. Shapes the
    /// outgoing history (drop a dangling trailing `user` turn, push the new user
    /// message, cap to `MAX_HISTORY`) and mints a fresh `turn_id`. Returns
    /// `(turn_id, history_snapshot)` for `UiCommand::StartTurn`.
    pub fn begin_turn(&mut self, transcript: &str) -> (String, Vec<ChatMessage>) {
        self.ptt_recording = false;
        self.you_text = transcript.to_string();
        self.lo_text.clear();
        self.busy = true;
        self.state = LoState::Thinking;

        // Drop a dangling trailing user message (a prior turn that never got a
        // reply) before pushing this one.
        if matches!(self.history.last(), Some(m) if m.role == ChatRole::User) {
            self.history.pop();
        }
        self.history.push(ChatMessage {
            role: ChatRole::User,
            content: transcript.to_string(),
        });
        if self.history.len() > MAX_HISTORY {
            let drop = self.history.len() - MAX_HISTORY;
            self.history.drain(0..drop);
        }

        self.turn_counter += 1;
        let turn_id = format!("t{}-{}", self.epoch, self.turn_counter);
        self.active_turn_id = turn_id.clone();
        (turn_id, self.history.clone())
    }

    /// Append a streamed prose delta if it belongs to the active turn.
    pub fn push_delta(&mut self, turn_id: &str, delta: &str) {
        if turn_id == self.active_turn_id {
            self.lo_text.push_str(delta);
            if self.state != LoState::Speaking {
                self.state = LoState::Speaking;
            }
        }
    }

    /// Finish the active turn: record the assistant reply in history, return to
    /// idle, and start the caption fade timer.
    pub fn finish_turn(&mut self, turn_id: &str, reply: &str) {
        if turn_id != self.active_turn_id {
            return; // a superseded turn — ignore
        }
        if !reply.is_empty() {
            self.history.push(ChatMessage {
                role: ChatRole::Assistant,
                content: reply.to_string(),
            });
            if self.history.len() > MAX_HISTORY {
                let drop = self.history.len() - MAX_HISTORY;
                self.history.drain(0..drop);
            }
        }
        self.active_turn_id.clear();
        self.busy = false;
        self.since_done = 0.0;
        self.state = LoState::Idle;
    }

    /// Caption alpha (1.0 while active/recent, fading to 0 over `IDLE_FADE_SECS`).
    pub fn caption_fade(&self) -> f32 {
        if self.busy || self.ptt_recording || self.since_done < IDLE_FADE_SECS {
            1.0
        } else {
            (1.0 - (self.since_done - IDLE_FADE_SECS) / 0.7).clamp(0.0, 1.0)
        }
    }

    /// Advance per-frame timers.
    pub fn tick(&mut self, dt: f32) {
        if !self.busy && !self.ptt_recording {
            self.since_done += dt;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn barge_in_bumps_epoch_and_clears_active_turn() {
        let mut s = Session::new();
        let (tid, _h) = s.begin_turn("hello");
        assert_eq!(s.active_turn_id, tid);
        assert!(s.busy);
        let e = s.interrupt();
        assert_eq!(e, s.epoch);
        assert!(s.active_turn_id.is_empty());
        assert!(!s.busy);
    }

    #[test]
    fn dangling_user_turn_is_dropped_before_new_user_msg() {
        let mut s = Session::new();
        s.begin_turn("first question"); // pushes a user msg, no reply recorded
        s.begin_turn("second question"); // should drop the dangling first user msg
        let users: Vec<_> = s
            .history
            .iter()
            .filter(|m| m.role == ChatRole::User)
            .collect();
        assert_eq!(users.len(), 1);
        assert_eq!(users[0].content, "second question");
    }

    #[test]
    fn deltas_for_stale_turns_are_ignored() {
        let mut s = Session::new();
        let (tid, _) = s.begin_turn("q");
        s.push_delta(&tid, "Hello");
        assert_eq!(s.lo_text, "Hello");
        s.interrupt(); // active turn cleared
        s.push_delta(&tid, " world"); // stale
        assert_eq!(s.lo_text, "Hello");
    }

    #[test]
    fn history_is_capped() {
        let mut s = Session::new();
        for i in 0..20 {
            let (tid, _) = s.begin_turn(&format!("q{i}"));
            s.finish_turn(&tid, &format!("a{i}"));
        }
        assert!(s.history.len() <= MAX_HISTORY);
    }
}
