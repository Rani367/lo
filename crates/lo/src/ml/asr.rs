//! On-device speech-to-text via whisper.cpp (the `whisper-rs` 0.16 bindings).
//!
//! Ports `src/renderer/ml/asr.ts`: a single model loads once, each clip creates a
//! fresh decode state, runs greedy whisper, and the segments are joined + trimmed.
//! Input is **already 16 kHz mono f32** (the rate whisper.cpp wants), so no
//! resampling happens here — the caller (cpal capture / VAD) delivers it that way.
//!
//! The GGML weights come from the canonical `ggerganov/whisper.cpp` HF repo. The
//! `asr_model` setting is mapped to a GGML filename; the old MLX Parakeet id (the
//! default on Apple Silicon, which has no whisper.cpp equivalent) falls back to
//! `base.en`, mirroring the TS default of `whisper-base.en`.
//!
//! Feature-gated behind `asr-whisper`; with the feature off, [`load_asr`] returns
//! a descriptive error so the crate still builds (e.g. on a host without a
//! C/C++/CMake toolchain).

use crate::ml::download::Progress;

/// HuggingFace repo hosting the GGML whisper.cpp weights.
pub const WHISPER_REPO: &str = "ggerganov/whisper.cpp";

/// Default GGML model — small, fast, accurate for short push-to-talk clips.
