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
