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
/// Mirrors the TS default `onnx-community/whisper-base.en`.
pub const DEFAULT_GGML: &str = "ggml-base.en.bin";

/// Map an `asr_model` setting to a GGML filename in [`WHISPER_REPO`].
///
/// The settings ship MLX ids (`mlx-community/parakeet-*`, `mlx-community/whisper-*`)
/// that whisper.cpp can't load, so we translate them to the nearest GGML weight.
/// Anything already shaped like a `ggml-*.bin` filename is passed through verbatim.
pub fn ggml_file_for(model_setting: &str) -> String {
    let m = model_setting.trim();
    let lower = m.to_lowercase();

    // Explicit ggml filename — honour it.
    if lower.ends_with(".bin") && lower.contains("ggml") {
        return m.to_string();
    }

    // Map by capability keywords found in the (MLX/HF) id.
    if lower.contains("large") || lower.contains("turbo") {
        "ggml-large-v3-turbo.bin".to_string()
    } else if lower.contains("medium") {
        if lower.contains(".en") {
            "ggml-medium.en.bin".to_string()
        } else {
            "ggml-medium.bin".to_string()
        }
    } else if lower.contains("small") {
        if lower.contains(".en") {
            "ggml-small.en.bin".to_string()
        } else {
            "ggml-small.bin".to_string()
        }
    } else if lower.contains("tiny") {
        if lower.contains(".en") {
            "ggml-tiny.en.bin".to_string()
        } else {
            "ggml-tiny.bin".to_string()
        }
    } else if lower.contains("base") && !lower.contains(".en") && lower.contains("whisper") {
        // A multilingual "base" was explicitly requested.
        "ggml-base.bin".to_string()
    } else {
        // Parakeet (Apple Silicon default) and everything unrecognised → base.en.
        DEFAULT_GGML.to_string()
    }
}

/// Whether a GGML filename is an English-only model (`*.en.bin`), used to pin the
/// decode language to `en` and skip language auto-detection.
fn is_english_only(ggml_file: &str) -> bool {
    ggml_file.to_lowercase().contains(".en.")
}

// ───────────────────────────── real impl ─────────────────────────────

#[cfg(feature = "asr-whisper")]
mod imp {
    use super::{ggml_file_for, is_english_only, Progress, WHISPER_REPO};
    use crate::ml::download;
    use anyhow::Context;
    use whisper_rs::{
        FullParams, SamplingStrategy, WhisperContext, WhisperContextParameters, WhisperState,
    };

    /// True when this build compiled a whisper.cpp GPU backend. Metal is always
    /// compiled on macOS here; CUDA/Vulkan/hipBLAS are opt-in on Linux/Windows.
    const GPU_BUILD: bool = cfg!(any(
        target_os = "macos",
        feature = "asr-cuda",
        feature = "asr-vulkan",
        feature = "asr-hipblas",
    ));

    /// A loaded whisper.cpp model. The [`WhisperContext`] holds the weights; a
    /// fresh [`WhisperState`] is created per `transcribe` call (cheap relative to
    /// loading, and keeps decodes independent — matching the TS one-shot calls).
    pub struct Asr {
        ctx: WhisperContext,
        english_only: bool,
    }

    impl Asr {
        /// Transcribe a 16 kHz mono f32 clip to trimmed text.
        pub fn transcribe(&mut self, samples_16k_mono: &[f32]) -> anyhow::Result<String> {
            if samples_16k_mono.is_empty() {
                return Ok(String::new());
            }

            let mut state: WhisperState = self
                .ctx
                .create_state()
                .context("creating whisper decode state")?;

            let mut params = FullParams::new(SamplingStrategy::Greedy { best_of: 1 });
            // English-only weights: pin the language so no detection pass runs.
            if self.english_only {
                params.set_language(Some("en"));
            }
            params.set_translate(false);
            // Quiet: whisper.cpp otherwise prints to stdout/stderr.
            params.set_print_special(false);
            params.set_print_progress(false);
            params.set_print_realtime(false);
            params.set_print_timestamps(false);
            params.set_suppress_blank(true);
            // Reject clips the model scores as non-speech, cutting hallucinated
            // transcripts on silence/noise (PTT release with no speech, room tone).
            params.set_no_speech_thold(0.6);
            // Each clip is an independent utterance.
            params.set_no_context(true);
            // Use all but one core, like whisper.cpp's defaults, min 1.
            let threads = std::thread::available_parallelism()
                .map(|n| (n.get() as i32 - 1).max(1))
                .unwrap_or(4);
            params.set_n_threads(threads);

            state
                .full(params, samples_16k_mono)
                .context("running whisper inference")?;

            let n = state.full_n_segments();
            let mut out = String::new();
            for i in 0..n {
                if let Some(seg) = state.get_segment(i) {
                    // Lossy: a rare invalid byte shouldn't drop the whole clip.
                    if let Ok(text) = seg.to_str_lossy() {
                        out.push_str(&text);
                    }
                }
            }
            Ok(out.trim().to_string())
        }
    }

    /// Download the mapped GGML weight and build a [`WhisperContext`] once.
    ///
    /// Uses the compiled GPU backend (Metal/CUDA/Vulkan/hipBLAS) when available,
    /// falling back to CPU if GPU context creation fails or `LO_WHISPER_CPU=1` is
    /// set (a support escape hatch). CPU is always correct, just slower.
    pub fn load_asr(model_setting: &str, progress: Progress<'_>) -> anyhow::Result<Asr> {
        let ggml_file = ggml_file_for(model_setting);
        let english_only = is_english_only(&ggml_file);

        let path = download::fetch(WHISPER_REPO, &ggml_file, "HEARING", progress)
            .context("fetching whisper GGML weights")?;
        let path_str = path
            .to_str()
            .context("whisper model path is not valid UTF-8")?;

        let force_cpu = std::env::var("LO_WHISPER_CPU").is_ok();
        let ctx = if GPU_BUILD && !force_cpu {
            let mut params = WhisperContextParameters::default();
            params.use_gpu(true);
            match WhisperContext::new_with_params(path_str, params) {
                Ok(ctx) => {
                    tracing::info!("whisper: GPU backend active");
                    ctx
                }
                Err(e) => {
                    tracing::warn!("whisper GPU init failed ({e}); falling back to CPU");
                    WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                        .with_context(|| {
                            format!("loading whisper model {ggml_file} (CPU fallback)")
                        })?
                }
            }
        } else {
            WhisperContext::new_with_params(path_str, WhisperContextParameters::default())
                .with_context(|| format!("loading whisper model {ggml_file}"))?
        };

        Ok(Asr { ctx, english_only })
    }
}

// ───────────────────────────── stub ─────────────────────────────

#[cfg(not(feature = "asr-whisper"))]
mod imp {
    use super::Progress;

    /// Placeholder ASR that exists only so the public type names resolve when the
    /// `asr-whisper` feature is off. It is never constructed — [`load_asr`] errs
    /// before producing one.
    pub struct Asr {
        _never: std::convert::Infallible,
    }

    impl Asr {
        pub fn transcribe(&mut self, _samples_16k_mono: &[f32]) -> anyhow::Result<String> {
            match self._never {}
        }
    }

    pub fn load_asr(_model_setting: &str, _progress: Progress<'_>) -> anyhow::Result<Asr> {
        anyhow::bail!("speech-to-text unavailable: built without the `asr-whisper` feature")
    }
}

pub use imp::{load_asr, Asr};
