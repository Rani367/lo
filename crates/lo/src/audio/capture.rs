//! Microphone capture plumbing.
//!
//! Ports the *mic-tap* side of the original `capture-vad.ts`: per-block RMS is
//! smoothed into a 0..1 "listening" level with the exact EMA the renderer used
//! (`level = level*0.6 + rms*0.4`, exposed as `min(1, level*4)`), and 16 kHz
//! mono f32 samples are made available for the ASR/VAD frontend.
//!
//! RT-safety: the cpal input callback only pushes raw device-rate mono f32 into
//! [`CaptureRings::raw_prod`] and updates the level atomic. The resample from
//! device rate to 16 kHz runs on a worker thread that drains the raw ring and
//! fills the 16 kHz ring (via [`pump_resample`]) — never in the callback.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

use crate::audio::resample::MonoResampler;

/// Target capture rate for the ASR/VAD frontend (matches the renderer).
pub const CAPTURE_RATE: u32 = 16_000;

/// Smoothed microphone amplitude, mirroring `capture-vad.ts`.
///
/// `push_block` is called from the (RT) input callback with one block of mono
/// f32 samples; it computes RMS and folds it into the EMA. The level is stored
/// as `f32::to_bits` in an atomic so [`level`](InputLevel::level) is lock-free
/// from any thread.
pub struct InputLevel {
    bits: AtomicU32,
}

impl InputLevel {
