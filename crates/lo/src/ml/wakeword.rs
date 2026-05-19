//! Offline "Hey Computer" wake word.
//!
//! Ports the *shape* of `src/renderer/audio/wakeword.ts`, which used Picovoice
//! Porcupine with the built-in `Computer` keyword. Porcupine has no maintained
//! pure-Rust crate at the locked versions, so its real engine is vendored later;
//! this module provides the stable [`WakeWord`] trait the rest of the app codes
//! against plus a [`DisabledWake`] no-op so the wake-word activation mode compiles
//! and runs (always-off) today.
//!
//! The trait is intentionally tiny and synchronous: the audio thread hands it
//! fixed-size `i16` frames (Porcupine consumes 16 kHz mono `int16`), and it
//! answers "did the wake word just fire?". No feature gate is needed — there is no
//! heavy dependency here yet.

/// A frame-driven wake-word detector. Implementors consume fixed-length 16 kHz
/// mono `i16` frames and report when the keyword is detected.
///
/// `Send` so the always-on wake mic can live on the audio/worker thread.
pub trait WakeWord: Send {
    /// Process exactly one frame of [`frame_length`](WakeWord::frame_length)
    /// samples; returns `true` on the frame where the wake word fires.
    fn process_i16(&mut self, frame_16k_i16: &[i16]) -> bool;

    /// The exact number of `i16` samples each [`process_i16`](WakeWord::process_i16)
    /// call expects (Porcupine's `frame_length`, typically 512 at 16 kHz).
    fn frame_length(&self) -> usize;
}

/// A wake-word detector that never fires. Used when no Picovoice key is configured
/// or the real engine isn't vendored yet — the app falls back to push-to-talk / VAD.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledWake;

impl WakeWord for DisabledWake {
    fn process_i16(&mut self, _frame_16k_i16: &[i16]) -> bool {
        false
    }

    fn frame_length(&self) -> usize {
        512
    }
}
