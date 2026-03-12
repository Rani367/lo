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
///
/// **Not `Send`** — keep on the thread that called [`new`]. Dropping it stops
/// and tears down the streams and joins the worker.
pub struct AudioEngine {
    input_device: cpal::Device,
    output_device: cpal::Device,
    input_config: cpal::SupportedStreamConfig,
    output_config: cpal::SupportedStreamConfig,

    /// Producer end of the raw capture ring; moved into the input callback on
    /// `start`. `Option` so it can be taken exactly once.
    raw_prod: Option<rtrb::Producer<f32>>,
    /// Consumer end of the raw capture ring; moved into the worker on `start`.
    raw_cons: Option<rtrb::Consumer<f32>>,
    /// Producer end of the 16 kHz ring; moved into the worker on `start`.
    cap16k_prod: Option<rtrb::Producer<f32>>,
    /// Consumer end of the playback ring; moved into the output callback.
    pcm_cons: Option<rtrb::Consumer<f32>>,
    /// Producer end of the spectrum tee; moved into the output callback.
    tee_prod: Option<rtrb::Producer<f32>>,

    /// Shared mic level (also held by the handle).
    level: Arc<InputLevel>,
    /// Shared playback state (also held by the handle).
    play_state: Arc<PlaybackState>,

    /// Live streams (None until `start`). Kept here so they stay on this thread.
    input_stream: Option<Stream>,
    output_stream: Option<Stream>,
    /// Capture resample worker.
    worker: Option<JoinHandle<()>>,
    worker_stop: Arc<std::sync::atomic::AtomicBool>,
}

/// `Send + Sync + Clone` handle used by every thread other than the engine's.
///
/// Holds the ring endpoints the workers touch (behind mutexes — these methods
/// are *not* the RT callback) plus the shared atomics.
#[derive(Clone)]
pub struct AudioHandle {
    /// Consumer for finished 16 kHz capture samples.
    cap16k_cons: Arc<Mutex<rtrb::Consumer<f32>>>,
    /// Producer for device-rate playback PCM.
    pcm_prod: Arc<Mutex<rtrb::Producer<f32>>>,
    /// Consumer for the output spectrum tee.
    tee_cons: Arc<Mutex<rtrb::Consumer<f32>>>,
    /// Playback-rate resampler (Kokoro/other -> device rate), reused per call.
    out_resampler: Arc<Mutex<Option<MonoResampler>>>,
    /// Spectrum analyser state (256-pt FFT + 16 EMA bands).
    analyzer: Arc<Mutex<SpectrumAnalyzer>>,
    /// Device output sample rate (target for playback resampling).
    output_rate: u32,
    /// Shared mic level.
    level: Arc<InputLevel>,
    /// Shared playback state.
    play_state: Arc<PlaybackState>,
}

/// Build the engine + handle pair.
///
/// Picks the default input/output devices and their default configs. The output
/// stream will be opened at the **device default** sample rate (never assuming
/// 24 kHz is accepted); TTS PCM is resampled to it. Returns an error if no
/// default device is present.
pub fn new() -> anyhow::Result<(AudioEngine, AudioHandle)> {
    let host = cpal::default_host();
    let input_device = host
        .default_input_device()
        .ok_or_else(|| anyhow::anyhow!("no default input (microphone) device"))?;
    let output_device = host
        .default_output_device()
        .ok_or_else(|| anyhow::anyhow!("no default output (speaker) device"))?;

    let input_config = input_device
        .default_input_config()
        .map_err(|e| anyhow::anyhow!("no default input config: {e}"))?;
    let output_config = output_device
        .default_output_config()
        .map_err(|e| anyhow::anyhow!("no default output config: {e}"))?;

    let output_rate = output_config.sample_rate();

    let capture = CaptureRings::new(RAW_CAP, CAP16K_CAP);
    let pb = PlaybackRings::new(PCM_CAP, TEE_CAP);

    let level = capture.level.clone();
    let play_state = pb.state.clone();

    let handle = AudioHandle {
        cap16k_cons: Arc::new(Mutex::new(capture.cap16k_cons)),
        pcm_prod: Arc::new(Mutex::new(pb.pcm_prod)),
        tee_cons: Arc::new(Mutex::new(pb.tee_cons)),
        out_resampler: Arc::new(Mutex::new(None)),
        analyzer: Arc::new(Mutex::new(SpectrumAnalyzer::new())),
        output_rate,
        level: level.clone(),
        play_state: play_state.clone(),
    };

    let engine = AudioEngine {
        input_device,
        output_device,
        input_config,
        output_config,
        raw_prod: Some(capture.raw_prod),
        raw_cons: Some(capture.raw_cons),
        cap16k_prod: Some(capture.cap16k_prod),
        pcm_cons: Some(pb.pcm_cons),
        tee_prod: Some(pb.tee_prod),
        level,
        play_state,
        input_stream: None,
        output_stream: None,
        worker: None,
        worker_stop: Arc::new(std::sync::atomic::AtomicBool::new(false)),
    };

    Ok((engine, handle))
}

impl AudioEngine {
    /// Build and start the input and output streams and the capture resample
    /// worker. Call once, on the owning thread. Idempotent: a second call after
    /// success is a no-op.
    pub fn start(&mut self) -> anyhow::Result<()> {
        if self.input_stream.is_some() && self.output_stream.is_some() {
            return Ok(());
        }
        self.start_output()?;
        self.start_input()?;
        Ok(())
    }

    /// Build + play the output stream at the device default rate.
    fn start_output(&mut self) -> anyhow::Result<()> {
        let sample_format = self.output_config.sample_format();
        let channels = self.output_config.channels() as usize;
        let mut config: StreamConfig = self.output_config.config();
