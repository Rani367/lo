//! Native audio subsystem: low-latency microphone capture and gapless speech
//! playback, ported from the Electron renderer's `capture-vad.ts` (mic tap) and
//! `playback.ts`.
//!
//! # Threading model
//!
//! cpal's [`Stream`](cpal::Stream) is `!Send`: it must live on — and be dropped
//! on — the thread that created it. We therefore split the subsystem in two:
//!
//! - [`AudioEngine`] owns the input + output `Stream`s and the resample worker.
//!   It is **not** `Send`; construct and keep it on the main/creating thread.
//! - [`AudioHandle`] is `Send + Sync + Clone` and holds only the lock-free ring
//!   endpoints + atomics. All other threads (the brain/worker) use *only* the
//!   handle.
//!
//! Build both with [`new`], then call [`AudioEngine::start`] on the owning
//! thread to bring up the streams.
//!
//! # RT-safety
//!
//! The cpal data callbacks touch only `rtrb` rings and atomics — no allocation,
//! locks, or DSP. Resampling (rubato) and the spectrum FFT (rustfft) run on the
//! resample worker thread or inside the handle's `enqueue_pcm` / `output_spectrum`
//! calls, never in a callback.

pub mod capture;
pub mod playback;
pub mod resample;
pub mod spectrum;

use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{Data, Sample, SampleFormat, Stream, StreamConfig};

use crate::audio::capture::{pump_resample, CaptureRings, InputLevel, CAPTURE_RATE};
use crate::audio::playback::{fill_output, PlaybackRings, PlaybackState};
use crate::audio::resample::{resample_mono, MonoResampler};
use crate::audio::spectrum::{SpectrumAnalyzer, BANDS};

/// Kokoro TTS emits 24 kHz mono f32 PCM.
pub const KOKORO_RATE: u32 = 24_000;

/// Ring capacity (samples) for the device-rate raw capture path (~0.5 s at
/// 48 kHz). Generous so a slow worker pass never overruns the callback.
const RAW_CAP: usize = 48_000;
/// Ring capacity (samples) for the 16 kHz capture path (~4 s of speech).
const CAP16K_CAP: usize = 64_000;
/// Ring capacity (samples) for queued device-rate playback PCM (~5 s @ 48 kHz).
const PCM_CAP: usize = 240_000;
/// Ring capacity (samples) for the output spectrum tee (~0.2 s @ 48 kHz).
const TEE_CAP: usize = 8_192;

/// Smallest/largest sane callback block (frames) — clamps pathological buffer
/// sizes some drivers report.
const MIN_BLOCK: u32 = 16;
const MAX_BLOCK: u32 = 8_192;

/// Owns the cpal input + output streams and the capture resample worker.
