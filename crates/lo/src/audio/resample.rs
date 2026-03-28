//! Mono f32 sample-rate conversion built on rubato 3.0.0's asynchronous sinc
//! resampler.
//!
//! Both the capture downsample (device rate -> 16 kHz) and the playback upsample
//! (Kokoro 24 kHz / device rate) run here, on worker threads or the
//! enqueue/drain calls — NEVER inside an audio callback.
//!
//! rubato's [`Async`] resampler is "fixed input": it consumes a fixed number of
//! input frames per `process` call and emits a variable number of output frames.
//! We buffer leftover input across calls so callers can hand us arbitrary-length
//! mono chunks.

use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{
    Async, FixedAsync, Resampler, SincInterpolationParameters, SincInterpolationType,
    WindowFunction,
};

/// Streaming mono f32 resampler. Push arbitrary-length input; pull resampled
/// output. Stateful (keeps filter history + a leftover-input buffer between
/// calls), so reuse one instance per logical stream and direction.
pub struct MonoResampler {
    inner: Async<f32>,
    from_rate: u32,
    to_rate: u32,
    /// Input frames not yet consumed (rubato needs exactly `chunk` frames/call).
    pending: Vec<f32>,
    /// Fixed number of input frames consumed per `process` call.
    chunk: usize,
    /// Scratch holding the produced output of the most recent `process` call.
    out_scratch: Vec<f32>,
}

/// Input chunk size (frames) fed to rubato per process call. Small enough to
/// keep latency low, large enough to amortise the sinc filter cost.
const CHUNK: usize = 1024;

impl MonoResampler {
    /// Build a mono resampler from `from_rate` to `to_rate` (both in Hz).
    pub fn new(from_rate: u32, to_rate: u32) -> anyhow::Result<Self> {
        let params = SincInterpolationParameters {
            sinc_len: 256,
            f_cutoff: 0.95,
            oversampling_factor: 128,
            interpolation: SincInterpolationType::Linear,
            window: WindowFunction::Hann,
        };
        let ratio = to_rate as f64 / from_rate as f64;
        // `max_resample_ratio_relative` = 1.0: we never change the ratio at
        // runtime (rate pairs are fixed for a stream's lifetime).
        let inner = Async::<f32>::new_sinc(ratio, 1.0, &params, CHUNK, 1, FixedAsync::Input)
            .map_err(|e| anyhow::anyhow!("rubato construction failed: {e}"))?;
        Ok(Self {
            inner,
            from_rate,
            to_rate,
            pending: Vec::with_capacity(CHUNK * 2),
            chunk: CHUNK,
            out_scratch: Vec::new(),
        })
    }

    /// Source sample rate this resampler was built for.
    #[allow(clippy::wrong_self_convention)] // `from_rate` is a getter, not a constructor
    pub fn from_rate(&self) -> u32 {
        self.from_rate
    }

    /// Destination sample rate this resampler was built for.
    pub fn to_rate(&self) -> u32 {
        self.to_rate
    }

    /// Resample `input` (mono f32 at `from_rate`), appending the produced
    /// samples (mono f32 at `to_rate`) to `out`. Any input frames that do not
    /// fill a complete processing chunk are retained internally and consumed on
    /// the next call, so feeding the stream in arbitrary-sized pieces is safe.
    pub fn process(&mut self, input: &[f32], out: &mut Vec<f32>) {
        self.pending.extend_from_slice(input);
        // The fixed-input resampler may want a different number of frames per
        // call after a ratio change; for our fixed ratio it stays at `chunk`,
        // but we honour `input_frames_next` defensively.
        loop {
            let need = self.inner.input_frames_next().max(1);
            if self.pending.len() < need {
                break;
            }
            self.process_one(need, out);
        }
    }

    /// Flush any buffered input by zero-padding the final partial chunk, so
    /// trailing audio is not lost at end-of-stream. Only the output frames that
    /// correspond to the *real* (unpadded) leftover input are kept — the padding
    /// silence is discarded — so the total output length stays ≈ input·ratio (no
    /// spurious trailing silence). Resets internal state after flushing so the
    /// instance can be reused.
    pub fn flush(&mut self, out: &mut Vec<f32>) {
        if !self.pending.is_empty() {
            let real = self.pending.len();
            let need = self.inner.input_frames_next().max(1);
            if self.pending.len() < need {
                self.pending.resize(need, 0.0);
            }
            let mut tail = Vec::new();
            self.process_one(need, &mut tail);
            let ratio = self.to_rate as f64 / self.from_rate as f64;
            let keep = (((real as f64) * ratio).round() as usize).min(tail.len());
            out.extend_from_slice(&tail[..keep]);
        }
        self.pending.clear();
        self.inner.reset();
    }

    /// Process exactly `need` input frames from `pending` into `out`.
    fn process_one(&mut self, need: usize, out: &mut Vec<f32>) {
        let take = need.min(self.pending.len());
        // SequentialSlice wants a `&[f32]` of length channels*frames; mono so
        // exactly `take` frames.
        let input_block: Vec<f32> = self.pending.drain(..take).collect();
        let adapter = match SequentialSlice::new(&input_block[..], 1, take) {
            Ok(a) => a,
            Err(_) => return,
        };
        match self.inner.process(&adapter, 0, None) {
            Ok(produced) => {
                let frames = produced.frames();
                self.out_scratch.clear();
                self.out_scratch.reserve(frames);
                for f in 0..frames {
                    self.out_scratch
                        .push(produced.read_sample(0, f).unwrap_or(0.0));
                }
                out.extend_from_slice(&self.out_scratch);
            }
            Err(_) => {
                // On a transient processing error, drop this block rather than
                // poison the stream.
            }
        }
    }
}

/// One-shot mono resample of a fully-buffered clip (e.g. a complete Kokoro TTS
/// chunk). Builds a fresh resampler, processes, and flushes the tail. Returns
/// `input` cloned unchanged when the rates already match.
pub fn resample_mono(input: &[f32], from_rate: u32, to_rate: u32) -> anyhow::Result<Vec<f32>> {
    if from_rate == to_rate {
        return Ok(input.to_vec());
    }
    let mut r = MonoResampler::new(from_rate, to_rate)?;
    let mut out = Vec::with_capacity(
