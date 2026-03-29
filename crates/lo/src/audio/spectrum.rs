//! Output spectrum analyser — the native replacement for the Web Audio
//! `AnalyserNode` (fftSize = 256, smoothingTimeConstant = 0.8) that drove the
//! "speaking" orb in the original renderer.
//!
//! A 256-point Hann-windowed FFT is run over the most recent output samples
//! (teed off the playback stream), then the 128 magnitude bins are folded into
//! 16 logarithmically-spaced bands with per-band EMA smoothing (0.8), matching
//! the browser AnalyserNode's smoothing behaviour. All DSP runs on the caller's
//! thread inside [`SpectrumAnalyzer::compute`] — never in an audio callback.

use std::f32::consts::PI;

use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// FFT size — mirrors the Web Audio `AnalyserNode.fftSize = 256`.
pub const FFT_SIZE: usize = 256;
/// Number of output bands the orb shader consumes.
pub const BANDS: usize = 16;
/// EMA smoothing constant — mirrors `smoothingTimeConstant = 0.8`.
const SMOOTHING: f32 = 0.8;

/// Computes a smoothed 16-band log spectrum from a rolling window of output PCM.
pub struct SpectrumAnalyzer {
    fft: Arc<dyn Fft<f32>>,
    /// Precomputed Hann window of length [`FFT_SIZE`].
    window: Vec<f32>,
    /// Reusable complex scratch buffer for the in-place FFT.
    scratch: Vec<Complex<f32>>,
    /// Most recent [`FFT_SIZE`] mono samples, used when fewer fresh samples are
    /// available than a full frame.
    history: Vec<f32>,
    /// Per-band EMA state.
    bands: [f32; BANDS],
}

impl SpectrumAnalyzer {
    /// Build a fresh analyser with a planned 256-point forward FFT.
    pub fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let fft = planner.plan_fft_forward(FFT_SIZE);
        let window = (0..FFT_SIZE)
            .map(|n| 0.5 - 0.5 * (2.0 * PI * n as f32 / (FFT_SIZE as f32 - 1.0)).cos())
            .collect();
        Self {
            fft,
            window,
            scratch: vec![Complex { re: 0.0, im: 0.0 }; FFT_SIZE],
            history: vec![0.0; FFT_SIZE],
            bands: [0.0; BANDS],
        }
    }

    /// Feed the latest output samples and return the smoothed 16-band spectrum.
    ///
    /// `fresh` is whatever was teed off the playback stream since the last call
    /// (may be empty, shorter than, or longer than [`FFT_SIZE`]). The analyser
    /// keeps a rolling [`FFT_SIZE`]-sample history so it always has a full frame
    /// to transform.
    pub fn compute(&mut self, fresh: &[f32]) -> [f32; BANDS] {
        self.push_history(fresh);
        self.transform();
        self.bands
    }

    /// Slide `fresh` into the rolling history window (keep the last FFT_SIZE).
    fn push_history(&mut self, fresh: &[f32]) {
        if fresh.is_empty() {
            return;
        }
        if fresh.len() >= FFT_SIZE {
            self.history
                .copy_from_slice(&fresh[fresh.len() - FFT_SIZE..]);
        } else {
            let keep = FFT_SIZE - fresh.len();
            self.history.copy_within(FFT_SIZE - keep.., 0);
            self.history[keep..].copy_from_slice(fresh);
        }
    }

    /// Run the windowed FFT over the history window and fold into bands.
    fn transform(&mut self) {
        for i in 0..FFT_SIZE {
            self.scratch[i] = Complex {
                re: self.history[i] * self.window[i],
