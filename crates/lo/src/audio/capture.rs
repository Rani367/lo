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
    /// Start at zero level (silence).
    pub fn new() -> Self {
        Self {
            bits: AtomicU32::new(0.0f32.to_bits()),
        }
    }

    /// RT-safe: fold one block's RMS into the EMA. Pure arithmetic — no alloc,
    /// no locks — so it is safe to call from the audio callback.
    pub fn push_block(&self, block: &[f32]) {
        if block.is_empty() {
            return;
        }
        let mut sum = 0.0f64;
        for &s in block {
            sum += (s as f64) * (s as f64);
        }
        let rms = (sum / block.len() as f64).sqrt() as f32;
        let prev = f32::from_bits(self.bits.load(Ordering::Relaxed));
        // Exact EMA from capture-vad.ts: level = level*0.6 + rms*0.4.
        let next = prev * 0.6 + rms * 0.4;
        self.bits.store(next.to_bits(), Ordering::Relaxed);
    }

    /// Reset to silence (e.g. when the mic is paused).
    pub fn reset(&self) {
        self.bits.store(0.0f32.to_bits(), Ordering::Relaxed);
    }

    /// 0..1 smoothed amplitude for the orb — `min(1, level*4)` per the renderer.
    pub fn level(&self) -> f32 {
        let l = f32::from_bits(self.bits.load(Ordering::Relaxed));
        (l * 4.0).min(1.0)
    }
}

impl Default for InputLevel {
    fn default() -> Self {
        Self::new()
    }
}

/// Producer/consumer halves of the capture path plus the shared level.
///
/// The `raw_*` ring carries device-rate mono f32 from the callback to the
/// resample worker; the `cap16k_*` ring carries finished 16 kHz mono f32 from
/// the worker to consumers ([`drain_16k`](CaptureChannel) via the handle).
pub struct CaptureRings {
    /// Device-rate mono f32, written by the input callback.
    pub raw_prod: Producer<f32>,
    pub raw_cons: Consumer<f32>,
    /// 16 kHz mono f32, written by the resample worker, read by ASR/VAD.
    pub cap16k_prod: Producer<f32>,
    pub cap16k_cons: Consumer<f32>,
    /// Smoothed listening level.
    pub level: Arc<InputLevel>,
}

impl CaptureRings {
    /// Allocate the capture rings. `raw_capacity`/`cap_capacity` are in samples.
    pub fn new(raw_capacity: usize, cap_capacity: usize) -> Self {
        let (raw_prod, raw_cons) = RingBuffer::<f32>::new(raw_capacity);
        let (cap16k_prod, cap16k_cons) = RingBuffer::<f32>::new(cap_capacity);
        Self {
            raw_prod,
            raw_cons,
            cap16k_prod,
            cap16k_cons,
            level: Arc::new(InputLevel::new()),
        }
    }
}

/// Drain the raw device-rate ring, resample to 16 kHz, and push into the 16 kHz
/// ring. Runs on a worker thread. Returns the number of 16 kHz samples produced.
///
/// Caller owns the loop/sleep; this performs one pass over whatever is
/// currently buffered. `scratch_in`/`scratch_out` are reusable buffers supplied
/// by the caller to avoid per-call allocation.
pub fn pump_resample(
    raw_cons: &mut Consumer<f32>,
    cap16k_prod: &mut Producer<f32>,
    resampler: &mut MonoResampler,
    scratch_in: &mut Vec<f32>,
    scratch_out: &mut Vec<f32>,
) -> usize {
    scratch_in.clear();
    let available = raw_cons.slots();
    if available == 0 {
        return 0;
    }
    if let Ok(chunk) = raw_cons.read_chunk(available) {
