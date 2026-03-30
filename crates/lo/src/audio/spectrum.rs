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
                im: 0.0,
            };
        }
        self.fft.process(&mut self.scratch);

        // Only the first FFT_SIZE/2 bins carry unique real-input information.
        let half = FFT_SIZE / 2;
        // Normalise magnitudes by the window's coherent gain (~N/2 for Hann)
        // and map into a 0..1 range comparable to the byte spectrum the orb
        // shader originally consumed.
        let norm = 1.0 / (half as f32);
        for b in 0..BANDS {
            // Logarithmic band edges across [1, half).
            let lo = band_edge(b, half);
            let hi = band_edge(b + 1, half).max(lo + 1);
            let mut acc = 0.0f32;
            let mut count = 0u32;
            for k in lo..hi {
                let c = self.scratch[k];
                acc += (c.re * c.re + c.im * c.im).sqrt();
                count += 1;
            }
            let mag = if count > 0 {
                (acc / count as f32) * norm
            } else {
                0.0
            };
            let level = mag.min(1.0);
            // EMA: new = old*0.8 + level*0.2 (browser AnalyserNode convention,
            // where smoothingTimeConstant weights the *previous* value).
            self.bands[b] = self.bands[b] * SMOOTHING + level * (1.0 - SMOOTHING);
        }
    }
}

impl Default for SpectrumAnalyzer {
    fn default() -> Self {
        Self::new()
    }
}

/// Logarithmically-spaced band edge `b` (of `BANDS`) over bins `[1, half]`.
fn band_edge(b: usize, half: usize) -> usize {
    let frac = b as f32 / BANDS as f32;
    // Map [0,1] -> [1, half] on a log scale.
    let lo = 1.0f32;
    let hi = half as f32;
    let edge = lo * (hi / lo).powf(frac);
    (edge.round() as usize).clamp(1, half)
}
