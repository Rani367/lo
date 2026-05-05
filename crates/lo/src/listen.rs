//! The "listen" std thread: owns the !Send on-device hearing models (whisper
//! ASR, Silero VAD), continuously drains the 16 kHz capture ring, and turns
//! speech into a transcript that it hands to the UI as `AppEvent::Transcribed`.
//! Ports the activation logic of `src/renderer/audio/capture-vad.ts` (push-to-talk
//! buffering; VAD auto-segmentation).

use std::sync::atomic::{AtomicBool, Ordering};
