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
        /// LSTM hidden state `[2,1,128]`, threaded between frames; zeroed on reset.
        state: Vec<f32>,

        // ── segmentation state (mirrors the TS fields) ──
        /// True once a frame crossed the positive threshold for the current turn.
        is_speaking: bool,
        /// True once a speculative `SilenceStart` fired for the current turn.
        silence_fired: bool,
        /// Speech frames accumulated for the current turn (flattened).
        speech: Vec<f32>,
        /// Number of frames in `speech` (to recover frame boundaries / counts).
        speech_frames: usize,
        /// Rolling pre-speech lead-in (last ≤7 frames before speech began).
        pre_roll: VecDeque<f32>,
        /// Consecutive sub-positive frames seen since the last speech frame.
        redemption: usize,
    }

    impl Vad {
        /// Push one 512-sample 16 kHz mono frame; returns any events it triggers.
        ///
        /// A frame of the wrong length yields no events (the caller is expected to
        /// chunk to [`FRAME_SAMPLES`]); we never panic on bad input.
        pub fn push_frame(&mut self, frame_16k: &[f32]) -> Vec<VadEvent> {
            let mut events = Vec::new();
            if frame_16k.len() != FRAME_SAMPLES {
                return events;
            }

            let prob = match self.infer(frame_16k) {
                Ok(p) => p,
                // A transient inference failure shouldn't crash capture; treat the
                // frame as silence and keep going.
                Err(_) => return events,
            };

            // Maintain the rolling pre-roll while not yet capturing speech.
            if !self.is_speaking {
                for &s in frame_16k {
                    self.pre_roll.push_back(s);
                }
                let cap = PRE_SPEECH_PAD_FRAMES * FRAME_SAMPLES;
                while self.pre_roll.len() > cap {
                    self.pre_roll.pop_front();
                }
            }

            if prob >= POSITIVE_THRESHOLD {
                if self.silence_fired {
                    // Resumed talking after a speculative SilenceStart — it's stale.
                    self.silence_fired = false;
                    self.speech.clear();
                    self.speech_frames = 0;
                    events.push(VadEvent::SpeechResume);
                }
                if !self.is_speaking {
                    self.is_speaking = true;
                    events.push(VadEvent::SpeechStart);
                }
                self.redemption = 0;
                self.speech.extend_from_slice(frame_16k);
                self.speech_frames += 1;
            } else if self.is_speaking {
                // Still within the redemption window — keep accumulating.
                self.speech.extend_from_slice(frame_16k);
                self.speech_frames += 1;
                self.redemption += 1;

                // Fire speculative SilenceStart on the first clearly-silent frame.
                if !self.silence_fired && prob < NEGATIVE_THRESHOLD {
                    self.silence_fired = true;
                    events.push(VadEvent::SilenceStart(self.utterance_clip()));
                }

                // Redemption elapsed → the turn is over.
                if self.redemption >= REDEMPTION_FRAMES {
                    let clip = self.utterance_clip();
                    self.end_turn();
                    if clip.len() >= MIN_UTTERANCE_SAMPLES {
                        events.push(VadEvent::SpeechEnd(clip));
                    } else {
                        events.push(VadEvent::SpeechEnd(Vec::new()));
                    }
                }
            }

            events
        }

        /// Clear all segmentation state and zero the LSTM state (was destroy/init).
        pub fn reset(&mut self) {
            self.state.iter_mut().for_each(|v| *v = 0.0);
            self.end_turn();
        }

        /// Reset only the per-turn segmentation fields (keeps the LSTM warm).
        fn end_turn(&mut self) {
            self.is_speaking = false;
            self.silence_fired = false;
            self.speech.clear();
            self.speech_frames = 0;
            self.pre_roll.clear();
            self.redemption = 0;
        }

        /// Build the utterance clip = pre-roll lead-in + accumulated speech, matching
        /// the canonical `onSpeechEnd` clip the TS library prepended.
        fn utterance_clip(&self) -> Vec<f32> {
            let mut clip = Vec::with_capacity(self.pre_roll.len() + self.speech.len());
            clip.extend(self.pre_roll.iter().copied());
            clip.extend_from_slice(&self.speech);
            clip
        }

        /// Run one frame through the Silero graph, returning the speech probability
        /// and threading the new LSTM state back into `self.state`.
        fn infer(&mut self, frame: &[f32]) -> anyhow::Result<f32> {
            let input = Tensor::from_array(([1usize, FRAME_SAMPLES], frame.to_vec()))
                .context("building VAD input tensor")?;
            let state = Tensor::from_array(([2usize, 1, 128], self.state.clone()))
                .context("building VAD state tensor")?;
            let sr = Tensor::from_array(([1usize], vec![16_000i64]))
                .context("building VAD sr tensor")?;

            let outputs = self
                .session
                .run(ort::inputs![
                    "input" => input,
                    "state" => state,
                    "sr" => sr,
                ])
                .context("running Silero VAD")?;

            // Output 0 is the speech probability `[1,1]`.
            let (_shape, prob_data) = outputs[0]
                .try_extract_tensor::<f32>()
                .context("reading VAD probability")?;
            let prob = prob_data.first().copied().unwrap_or(0.0);

            // The other output is the next LSTM state — pick it up by name-agnostic
            // scan so `stateN`/`state` naming differences don't matter.
            let mut new_state: Option<Vec<f32>> = None;
            for (name, value) in outputs.iter() {
                if name == "output" {
                    continue;
                }
                if let Ok((_s, data)) = value.try_extract_tensor::<f32>() {
                    if data.len() == self.state.len() {
                        new_state = Some(data.to_vec());
                        break;
                    }
                }
            }
            if let Some(s) = new_state {
                self.state = s;
            }

            Ok(prob)
        }
    }

