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
