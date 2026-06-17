//! On-device ML subsystem: speech-to-text, text-to-speech, voice-activity
//! detection, and the wake word.
//!
//! Everything here is **feature-gated per engine** so a host without a
//! C/C++/CMake toolchain (or building offline) still produces a working binary:
//!   - `asr-whisper` → whisper.cpp ASR ([`asr`])
//!   - `tts-kokoro`  → Kokoro TTS ([`tts`])
//!   - `vad-silero`  → Silero VAD ([`vad`])
//!
//! With an engine's feature off, its loader returns a descriptive `Err` and the
//! type names still resolve, so callers compile unchanged. The wake word
//! ([`wakeword`]) needs no feature gate yet — only a trait + a disabled no-op.
//!
//! Models download once into [`lo_core::config::paths::cache_dir`] via the shared
//! hf-hub helper in [`download`], reporting through the [`Progress`] callback.
//!
//! This subsystem is self-contained: it depends only on `std`, `lo_core`, the
//! external ML crates, and its own submodules. It communicates purely through the
//! plain structs / enums / closures re-exported below — no imports from the app,
//! worker, or event layers.

pub mod asr;
pub mod download;
pub mod tts;
pub mod vad;
pub mod wakeword;

pub use asr::{load_asr, Asr};
pub use tts::{load_tts, Tts};
pub use vad::{new_vad, Vad, VadEvent, VadTuning};
pub use wakeword::{load_wakeword, WakeWord};
