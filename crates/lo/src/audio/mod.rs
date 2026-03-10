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
