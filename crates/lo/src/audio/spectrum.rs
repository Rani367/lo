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
