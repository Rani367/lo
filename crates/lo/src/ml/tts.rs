//! On-device text-to-speech via Kokoro (the `kokoro-tts` 0.3.3 crate, ONNX under
//! the hood).
//!
//! Ports `src/renderer/ml/tts.ts`: load the ~82M Kokoro model once, then turn one
//! chunk of text into 24 kHz mono f32 PCM per call. The caller chunks prose with
//! [`lo_core::text::chunk_for_tts`] and feeds one chunk per [`Tts::synth`].
//!
//! The kokoro-tts crate expects two local files in the `thewh1teagle/kokoro-onnx`
//! release format: the ONNX graph (`kokoro-v1.0.onnx`) and a single combined
//! voice-style binary (`voices-v1.0.bin`). We fetch both from the HF mirror
//! `leonelhs/kokoro-thewh1teagle`. Voice selection and speed are both expressed
//! through the crate's `Voice` enum (each v1.0 variant carries an `f32` speed).
//!
//! Feature-gated behind `tts-kokoro`; with the feature off, [`load_tts`] returns a
//! descriptive error.

use crate::ml::download::Progress;

/// HF mirror hosting Kokoro weights in the `kokoro-onnx` (thewh1teagle) layout
/// that `kokoro-tts` reads: a single ONNX graph + a single combined voices blob.
pub const KOKORO_REPO: &str = "leonelhs/kokoro-thewh1teagle";

/// The full-precision v1.0 ONNX graph file in [`KOKORO_REPO`].
pub const KOKORO_MODEL_FILE: &str = "kokoro-v1.0.onnx";

/// The combined v1.0 voice-style binary (all voices) in [`KOKORO_REPO`].
pub const KOKORO_VOICES_FILE: &str = "voices-v1.0.bin";

/// Kokoro's fixed output sample rate.
pub const KOKORO_SAMPLE_RATE: u32 = 24_000;

// ───────────────────────────── real impl ─────────────────────────────

#[cfg(feature = "tts-kokoro")]
mod imp {
    use super::{Progress, KOKORO_MODEL_FILE, KOKORO_REPO, KOKORO_SAMPLE_RATE, KOKORO_VOICES_FILE};
    use crate::ml::download;
    use anyhow::Context;
    use kokoro_tts::{KokoroTts, Voice};

    /// A loaded Kokoro voice synthesiser.
    ///
    /// `kokoro-tts` is async and `KokoroTts::new` uses `tokio::fs`, so we own a
    /// dedicated current-thread Tokio runtime to drive both load and synthesis.
    /// Holding our own runtime keeps the subsystem self-contained and works whether
    /// or not the caller is already inside a Tokio context (we drive our own
    /// reactor rather than relying on `Handle::current`).
    pub struct Tts {
        tts: KokoroTts,
        /// The configured voice name (e.g. `"af_heart"`), lower-cased.
        voice: String,
        /// Runtime used to poll Kokoro's async methods to completion.
        rt: tokio::runtime::Runtime,
    }

    impl Tts {
        /// Synthesise one chunk of text to `(mono f32 PCM, 24000)`.
        ///
        /// `speed` is the Kokoro speed multiplier (>1 faster, pitch unchanged); it
        /// is carried inside the v1.0 [`Voice`] variant. The `kokoro-tts` API is
        /// async, so we block on it — this runs on the worker thread, never the UI
        /// thread.
        pub fn synth(&mut self, text: &str, speed: f32) -> anyhow::Result<(Vec<f32>, u32)> {
            let trimmed = text.trim();
            if trimmed.is_empty() {
                return Ok((Vec::new(), KOKORO_SAMPLE_RATE));
            }

            let voice = voice_for(&self.voice, speed)
                .with_context(|| format!("unknown Kokoro voice {:?}", self.voice))?;

            let (samples, _duration) = self
                .rt
                .block_on(self.tts.synth(trimmed, voice))
                .context("Kokoro synthesis failed")?;

            Ok((samples, KOKORO_SAMPLE_RATE))
        }
    }

    /// Download the Kokoro graph + voices blob and load the synthesiser once.
    ///
    /// `model_setting` is accepted for parity with the settings/ASR path but is not
