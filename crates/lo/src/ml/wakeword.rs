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
