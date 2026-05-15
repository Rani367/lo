//! Voice-activity detection via Silero VAD v5 (ONNX, run through `ort` 2.0).
//!
//! A faithful Rust port of `src/renderer/audio/capture-vad.ts`. The browser used
//! `@ricky0123/vad-web` (`model: 'v5'`) which wraps the same Silero v5 ONNX graph;
//! here we drive that graph directly with `ort` and re-implement the segmentation
//! state machine — including the speculative `SilenceStart` / `SpeechResume`
//! behaviour the TS relied on for low-latency speculative transcription.
//!
//! Frame contract (identical to the TS): **512-sample, 16 kHz mono f32** frames.
//! At 16 kHz, 512 samples ≈ 32 ms, so the timing constants convert to frame counts:
//!   - `positiveSpeechThreshold = 0.6`, `negativeSpeechThreshold = 0.4`
//!   - `minSpeechMs = 150`  (not separately enforced here; see note below)
//!   - `redemptionMs  = 900` → ~28 frames of sub-threshold audio ends the turn
//!   - `preSpeechPadMs = 250` → 7-frame pre-roll prepended to the utterance
//!   - `MIN_UTTERANCE_SAMPLES = 6400` (0.4 s) → shorter clips are misfires
//!
//! The Silero v5 graph keeps an LSTM state between frames: inputs are `input`
//! `[1,512] f32`, `state` `[2,1,128] f32` (zeroed at reset), `sr` scalar `i64`
//! (16000); outputs are the speech probability and the next state, which we thread
//! back in. We read the probability by output position (0) and pick up the new
//! state by the non-probability output, so the exact state-output name
//! (`stateN` vs `state`) doesn't matter.
//!
//! Feature-gated behind `vad-silero`; with the feature off, [`new_vad`] returns a
//! descriptive error.

use crate::ml::download::Progress;

/// HF repo hosting the Silero VAD v5 ONNX graph (full precision `onnx/model.onnx`).
pub const SILERO_REPO: &str = "onnx-community/silero-vad";

/// The full-precision Silero VAD v5 ONNX file in [`SILERO_REPO`].
pub const SILERO_FILE: &str = "onnx/model.onnx";

/// Exact frame size the Silero v5 16 kHz model consumes.
pub const FRAME_SAMPLES: usize = 512;

/// Speech probability at/above which a frame counts as speech (TS: 0.6).
const POSITIVE_THRESHOLD: f32 = 0.6;
/// Speech probability below which a frame counts as silence (TS: 0.4).
const NEGATIVE_THRESHOLD: f32 = 0.4;
/// Pre-roll prepended to each utterance: `preSpeechPadMs(250) / 32ms ≈ 7 frames`.
const PRE_SPEECH_PAD_FRAMES: usize = 7;
/// Silence frames that end a turn: `redemptionMs(900) / 32ms ≈ 28 frames`.
const REDEMPTION_FRAMES: usize = 28;
/// Ignore utterances shorter than 0.4 s (`16000 * 0.4`).
const MIN_UTTERANCE_SAMPLES: usize = 6400;

/// Events emitted as frames are pushed, mirroring the TS `VadHandlers` callbacks.
#[derive(Debug, Clone, PartialEq)]
pub enum VadEvent {
    /// Speech began (was `onSpeechStart`).
    SpeechStart,
    /// Silence first detected after speech — *speculative*. Carries the pre-roll +
    /// speech-so-far so a speculative transcription can start before the turn is
    /// confirmed over (was `onSilenceStart`). Invalidated by [`VadEvent::SpeechResume`].
    SilenceStart(Vec<f32>),
    /// The user resumed talking after a [`VadEvent::SilenceStart`]; discard the
    /// speculative clip (was `onSpeechResume`).
    SpeechResume,
    /// The turn ended after the redemption window. Carries the full utterance
    /// (pre-roll + all speech frames) **iff** it is ≥ [`MIN_UTTERANCE_SAMPLES`];
    /// otherwise it is empty (a misfire) (was `onSpeechEnd` / `onVADMisfire`).
    SpeechEnd(Vec<f32>),
}

// ───────────────────────────── real impl ─────────────────────────────

#[cfg(feature = "vad-silero")]
mod imp {
    use super::{
        Progress, VadEvent, FRAME_SAMPLES, MIN_UTTERANCE_SAMPLES, NEGATIVE_THRESHOLD,
        POSITIVE_THRESHOLD, PRE_SPEECH_PAD_FRAMES, REDEMPTION_FRAMES, SILERO_FILE, SILERO_REPO,
    };
    use crate::ml::download;
    use anyhow::Context;
    use ort::session::Session;
    use ort::value::Tensor;
    use std::collections::VecDeque;

    /// Silero VAD streaming segmenter.
    pub struct Vad {
        session: Session,
