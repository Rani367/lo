//! Offline "Hey Jarvis" wake word via **openWakeWord** (ONNX, run through `ort`).
//!
//! openWakeWord is a 3-stage pipeline, faithfully ported here from the reference
//! Python (`github.com/dscripka/openWakeWord`):
//!   raw 16 kHz int16 audio → melspectrogram model → shared Google speech-embedding
//!   model → a per-phrase classifier (`hey_jarvis`). One score is produced per
//!   1280-sample (80 ms) chunk; a score ≥ threshold (default 0.5) fires the word.
//!
//! No API key and no cloud — the three ONNX graphs are fetched once over HTTPS from
//! the openWakeWord GitHub release into Lo's model cache. Feature-gated behind
//! `wake-openwakeword`; with the feature off, [`load_wakeword`] returns a
//! descriptive error and the wake activation mode simply idles (use PTT/VAD).
//!
//! The trait is intentionally tiny and synchronous: the listen thread hands it
//! fixed-size `i16` frames and it answers "did the wake word just fire?".

/// A frame-driven wake-word detector. Implementors consume fixed-length 16 kHz
/// mono `i16` frames and report when the keyword is detected.
///
/// `Send` so the always-on wake mic can live on the listen thread.
pub trait WakeWord: Send {
    /// Process exactly one frame of [`frame_length`](WakeWord::frame_length)
    /// samples; returns `true` on the frame where the wake word fires.
    fn process_i16(&mut self, frame_16k_i16: &[i16]) -> bool;

    /// The exact number of `i16` samples each [`process_i16`](WakeWord::process_i16)
    /// call expects (openWakeWord's 80 ms step = 1280 samples at 16 kHz).
    fn frame_length(&self) -> usize;
}

/// A wake-word detector that never fires. Used when the `wake-openwakeword` feature
/// is off — the app falls back to push-to-talk / VAD.
#[derive(Debug, Default, Clone, Copy)]
pub struct DisabledWake;

impl WakeWord for DisabledWake {
    fn process_i16(&mut self, _frame_16k_i16: &[i16]) -> bool {
        false
    }

    fn frame_length(&self) -> usize {
        1280
    }
}

use crate::ml::download::Progress;

/// openWakeWord GitHub release hosting the ONNX graphs.
pub const OWW_BASE: &str = "https://github.com/dscripka/openWakeWord/releases/download/v0.5.1";
/// Shared melspectrogram front-end.
pub const MEL_FILE: &str = "melspectrogram.onnx";
/// Shared Google speech-embedding model.
pub const EMB_FILE: &str = "embedding_model.onnx";
/// The "Hey Jarvis" classifier.
pub const HEY_JARVIS_FILE: &str = "hey_jarvis_v0.1.onnx";

/// Load the "Hey Jarvis" openWakeWord detector, fetching its three ONNX graphs on
/// first use. `threshold` is the classifier score (0..1) that fires the word.
#[cfg(feature = "wake-openwakeword")]
pub fn load_wakeword(threshold: f32, progress: Progress<'_>) -> anyhow::Result<Box<dyn WakeWord>> {
    Ok(Box::new(oww::OpenWakeWord::load(threshold, progress)?))
}

/// Stub when the feature is off — the listen thread treats the `Err` as "no wake
/// word" and idles in wake mode.
#[cfg(not(feature = "wake-openwakeword"))]
pub fn load_wakeword(
    _threshold: f32,
    _progress: Progress<'_>,
) -> anyhow::Result<Box<dyn WakeWord>> {
    anyhow::bail!("wake word unavailable: built without the `wake-openwakeword` feature")
}

// ───────────────────────────── real impl ─────────────────────────────

#[cfg(feature = "wake-openwakeword")]
mod oww {
    use super::{WakeWord, EMB_FILE, HEY_JARVIS_FILE, MEL_FILE, OWW_BASE};
    use crate::ml::download::{self, Progress};
    use anyhow::Context;
    use ort::session::Session;
    use ort::value::Tensor;
    use std::path::Path;

    const CHUNK: usize = 1280; // 80 ms @ 16 kHz — the fundamental step
    const LOOKBACK: usize = 480; // 160*3 extra raw samples for mel edge alignment
    const RAW_KEEP: usize = CHUNK + LOOKBACK; // samples fed to the mel model per step
    const MEL_BINS: usize = 32;
    const EMB_DIM: usize = 96;
    const EMB_WINDOW: usize = 76; // mel frames per embedding window
    const CLS_WINDOW: usize = 16; // embeddings per classifier input
    const MEL_MAX_ROWS: usize = 970; // ~10 s of mel frames
    const FEAT_MAX_ROWS: usize = 120; // rolling embedding buffer
    const WARMUP_FRAMES: usize = 5; // force first scores to 0 (reference behaviour)
    const COOLDOWN_FRAMES: usize = 20; // ~1.6 s refractory after a fire (debounce)

    /// Streaming openWakeWord detector. Holds the three ONNX sessions and the
    /// rolling buffers needed to reproduce one score per 1280-sample chunk.
    pub struct OpenWakeWord {
        mel: Session,
        emb: Session,
        cls: Session,
        threshold: f32,

        /// Last `RAW_KEEP` raw int16 samples (for the mel model's overlap window).
        raw: Vec<i16>,
        /// Rolling mel buffer, row-major `rows × 32`, seeded with 76 rows of 1.0.
        mel_buf: Vec<f32>,
        mel_rows: usize,
        /// Rolling embedding buffer, row-major `rows × 96`, seeded with 16 zero rows.
        feat_buf: Vec<f32>,
        feat_rows: usize,

        frames_seen: usize,
        cooldown: usize,
    }

    impl OpenWakeWord {
        pub fn load(threshold: f32, progress: Progress<'_>) -> anyhow::Result<Self> {
            let mel_path = download::fetch_http(
                &format!("{OWW_BASE}/{MEL_FILE}"),
                MEL_FILE,
                "WAKE",
                progress,
            )?;
            let emb_path = download::fetch_http(
                &format!("{OWW_BASE}/{EMB_FILE}"),
                EMB_FILE,
                "WAKE",
                progress,
            )?;
            let cls_path = download::fetch_http(
                &format!("{OWW_BASE}/{HEY_JARVIS_FILE}"),
                HEY_JARVIS_FILE,
                "WAKE",
                progress,
            )?;

            let mel = session_for(&mel_path)?;
            let emb = session_for(&emb_path)?;
            let cls = session_for(&cls_path)?;

            // Seed the mel buffer with 76 rows of 1.0 so the first embedding window
            // is well-formed, and the embedding buffer with 16 zero rows so the
            // classifier always has its [1,16,96] input (warmup zeros the scores).
            let mel_buf = vec![1.0f32; EMB_WINDOW * MEL_BINS];
            let feat_buf = vec![0.0f32; CLS_WINDOW * EMB_DIM];

            Ok(Self {
                mel,
                emb,
                cls,
                threshold,
                raw: Vec::with_capacity(RAW_KEEP),
                mel_buf,
                mel_rows: EMB_WINDOW,
                feat_buf,
                feat_rows: CLS_WINDOW,
                frames_seen: 0,
                cooldown: 0,
            })
        }

        /// Run one 1280-sample chunk through the pipeline, returning the score.
        fn score_chunk(&mut self, frame: &[i16]) -> anyhow::Result<f32> {
            // 1) Accumulate raw audio, keeping the last RAW_KEEP samples.
            self.raw.extend_from_slice(frame);
            if self.raw.len() > RAW_KEEP {
                let cut = self.raw.len() - RAW_KEEP;
                self.raw.drain(0..cut);
            }
            // Left-pad with zeros early on so the mel model gets a full window.
            let mut audio = vec![0.0f32; RAW_KEEP.saturating_sub(self.raw.len())];
            audio.extend(self.raw.iter().map(|&s| s as f32));

            // 2) Melspectrogram → frames×32, transform x/10 + 2, append to buffer.
            let n = audio.len();
            let mel_in = Tensor::from_array(([1usize, n], audio)).context("mel input")?;
            let mel_out = self.mel.run(ort::inputs!["input" => mel_in])?;
            let (_shape, mel_data) = mel_out[0]
                .try_extract_tensor::<f32>()
                .context("mel output")?;
            let frames = mel_data.len() / MEL_BINS;
            for f in 0..frames {
                for b in 0..MEL_BINS {
                    self.mel_buf.push(mel_data[f * MEL_BINS + b] / 10.0 + 2.0);
                }
            }
            self.mel_rows += frames;
            if self.mel_rows > MEL_MAX_ROWS {
                let drop = self.mel_rows - MEL_MAX_ROWS;
                self.mel_buf.drain(0..drop * MEL_BINS);
                self.mel_rows = MEL_MAX_ROWS;
            }

            // 3) Embedding from the last 76 mel frames → 96-d vector.
            let start = (self.mel_rows - EMB_WINDOW) * MEL_BINS;
            let window: Vec<f32> = self.mel_buf[start..start + EMB_WINDOW * MEL_BINS].to_vec();
            let emb_in = Tensor::from_array(([1usize, EMB_WINDOW, MEL_BINS, 1usize], window))
                .context("embedding input")?;
            let emb_out = self.emb.run(ort::inputs!["input_1" => emb_in])?;
            let (_s, emb_data) = emb_out[0]
                .try_extract_tensor::<f32>()
                .context("embedding output")?;
            self.feat_buf
                .extend_from_slice(&emb_data[..EMB_DIM.min(emb_data.len())]);
            self.feat_rows += 1;
            if self.feat_rows > FEAT_MAX_ROWS {
                let drop = self.feat_rows - FEAT_MAX_ROWS;
                self.feat_buf.drain(0..drop * EMB_DIM);
                self.feat_rows = FEAT_MAX_ROWS;
            }

            // 4) Classifier over the last 16 embeddings → a single score.
            let cstart = (self.feat_rows - CLS_WINDOW) * EMB_DIM;
            let cwin: Vec<f32> = self.feat_buf[cstart..cstart + CLS_WINDOW * EMB_DIM].to_vec();
            let cls_in = Tensor::from_array(([1usize, CLS_WINDOW, EMB_DIM], cwin))
                .context("classifier input")?;
            // The classifier has a single input; pass it positionally so we don't
            // depend on the model's (private, version-specific) input-name metadata.
            let cls_out = self.cls.run(ort::inputs![cls_in])?;
            let (_cs, cls_data) = cls_out[0]
                .try_extract_tensor::<f32>()
                .context("classifier output")?;
            Ok(cls_data.first().copied().unwrap_or(0.0))
        }
    }

    impl WakeWord for OpenWakeWord {
        fn process_i16(&mut self, frame_16k_i16: &[i16]) -> bool {
            if frame_16k_i16.len() != CHUNK {
                return false;
            }
            let score = match self.score_chunk(frame_16k_i16) {
                Ok(s) => s,
                // A transient inference error shouldn't crash the listen thread.
                Err(e) => {
                    tracing::warn!("wake-word inference failed: {e:#}");
                    return false;
                }
            };
            self.frames_seen += 1;
            if self.frames_seen <= WARMUP_FRAMES {
                return false;
            }
            if self.cooldown > 0 {
                self.cooldown -= 1;
                return false;
            }
            if score >= self.threshold {
                self.cooldown = COOLDOWN_FRAMES;
                tracing::info!(score, "wake word detected");
                true
            } else {
                false
            }
        }

        fn frame_length(&self) -> usize {
            CHUNK
        }
    }

    /// Build an ort session with the same EP preference + CPU fallback as the VAD.
    #[allow(clippy::vec_init_then_push)] // EP vec is conditionally cfg-populated
    fn session_for(path: &Path) -> anyhow::Result<Session> {
        use ort::execution_providers::{CPUExecutionProvider, ExecutionProviderDispatch};
        // Conditionally populated by cfg, so the "init then push" lint misfires.
        let mut eps: Vec<ExecutionProviderDispatch> = Vec::new();
        #[cfg(feature = "vad-cuda")]
        eps.push(ort::execution_providers::CUDAExecutionProvider::default().build());
        #[cfg(feature = "vad-directml")]
        eps.push(ort::execution_providers::DirectMLExecutionProvider::default().build());
        #[cfg(target_os = "macos")]
        eps.push(ort::execution_providers::CoreMLExecutionProvider::default().build());
        eps.push(CPUExecutionProvider::default().build());

        let mut builder = Session::builder()
            .context("creating ort session builder")?
            .with_execution_providers(eps)
            .map_err(|e| anyhow::anyhow!("registering wake-word execution providers: {e}"))?;
        builder
            .commit_from_file(path)
            .with_context(|| format!("loading wake-word model {}", path.display()))
    }
}
