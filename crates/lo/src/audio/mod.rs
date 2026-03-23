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
        clamp_buffer_size(&mut config);

        let mut pcm_cons = self
            .pcm_cons
            .take()
            .ok_or_else(|| anyhow::anyhow!("output already started"))?;
        let mut tee_prod = self
            .tee_prod
            .take()
            .ok_or_else(|| anyhow::anyhow!("output already started"))?;
        let state = self.play_state.clone();

        // Scratch f32 block reused inside the callback (sized once on first
        // call) so we never allocate in the RT path after warm-up.
        let mut scratch: Vec<f32> = Vec::new();

        let err_fn = |e| tracing::error!(target: "audio", "output stream error: {e}");

        let device = &self.output_device;
        let stream = match sample_format {
            SampleFormat::F32 => device.build_output_stream_raw(
                config,
                SampleFormat::F32,
                move |data: &mut Data, _| {
                    if let Some(out) = data.as_slice_mut::<f32>() {
                        fill_output(out, channels, &mut pcm_cons, &mut tee_prod, &state);
                    }
                },
                err_fn,
                None,
            )?,
            SampleFormat::I16 => device.build_output_stream_raw(
                config,
                SampleFormat::I16,
                move |data: &mut Data, _| {
                    let out = match data.as_slice_mut::<i16>() {
                        Some(s) => s,
                        None => return,
                    };
                    ensure_len(&mut scratch, out.len());
                    fill_output(&mut scratch, channels, &mut pcm_cons, &mut tee_prod, &state);
                    for (d, s) in out.iter_mut().zip(scratch.iter()) {
                        *d = i16::from_sample(*s);
                    }
                },
                err_fn,
                None,
            )?,
            SampleFormat::U16 => device.build_output_stream_raw(
                config,
                SampleFormat::U16,
                move |data: &mut Data, _| {
                    let out = match data.as_slice_mut::<u16>() {
                        Some(s) => s,
                        None => return,
                    };
                    ensure_len(&mut scratch, out.len());
                    fill_output(&mut scratch, channels, &mut pcm_cons, &mut tee_prod, &state);
                    for (d, s) in out.iter_mut().zip(scratch.iter()) {
                        *d = u16::from_sample(*s);
                    }
                },
                err_fn,
                None,
            )?,
            other => {
                return Err(anyhow::anyhow!(
                    "unsupported output sample format: {other:?}"
                ));
            }
        };
        stream.play()?;
        self.output_stream = Some(stream);
        Ok(())
    }

    /// Build + play the input stream and spawn the resample worker.
    fn start_input(&mut self) -> anyhow::Result<()> {
        let sample_format = self.input_config.sample_format();
        let channels = self.input_config.channels() as usize;
        let device_rate = self.input_config.sample_rate();
        let mut config: StreamConfig = self.input_config.config();
        clamp_buffer_size(&mut config);

        let mut raw_prod = self
            .raw_prod
            .take()
            .ok_or_else(|| anyhow::anyhow!("input already started"))?;
        let level = self.level.clone();

        // Reusable mono downmix scratch for the callback (no alloc after warm-up).
        let mut mono: Vec<f32> = Vec::new();

        let err_fn = |e| tracing::error!(target: "audio", "input stream error: {e}");

        let device = &self.input_device;
        let stream = match sample_format {
            SampleFormat::F32 => device.build_input_stream_raw(
                config,
                SampleFormat::F32,
                move |data: &Data, _| {
                    if let Some(samples) = data.as_slice::<f32>() {
                        capture_block(samples, channels, &mut mono, &mut raw_prod, &level, |s| s);
                    }
                },
                err_fn,
                None,
            )?,
            SampleFormat::I16 => device.build_input_stream_raw(
                config,
                SampleFormat::I16,
                move |data: &Data, _| {
                    if let Some(samples) = data.as_slice::<i16>() {
                        capture_block(samples, channels, &mut mono, &mut raw_prod, &level, |s| {
                            s.to_sample::<f32>()
                        });
                    }
                },
                err_fn,
                None,
            )?,
            SampleFormat::U16 => device.build_input_stream_raw(
                config,
                SampleFormat::U16,
                move |data: &Data, _| {
                    if let Some(samples) = data.as_slice::<u16>() {
                        capture_block(samples, channels, &mut mono, &mut raw_prod, &level, |s| {
                            s.to_sample::<f32>()
                        });
                    }
                },
                err_fn,
                None,
            )?,
            other => {
                return Err(anyhow::anyhow!(
                    "unsupported input sample format: {other:?}"
                ));
            }
        };
        stream.play()?;
        self.input_stream = Some(stream);

        // Spawn the capture resample worker (device rate -> 16 kHz).
        let mut raw_cons = self
            .raw_cons
            .take()
            .ok_or_else(|| anyhow::anyhow!("input already started"))?;
        let mut cap16k_prod = self
            .cap16k_prod
            .take()
            .ok_or_else(|| anyhow::anyhow!("input already started"))?;
        let stop = self.worker_stop.clone();
        self.worker = Some(std::thread::spawn(move || {
            let mut resampler = match MonoResampler::new(device_rate, CAPTURE_RATE) {
                Ok(r) => r,
                Err(e) => {
                    tracing::error!(target: "audio", "capture resampler init failed: {e}");
                    return;
                }
            };
            let mut scratch_in: Vec<f32> = Vec::with_capacity(RAW_CAP);
            let mut scratch_out: Vec<f32> = Vec::with_capacity(CAP16K_CAP);
            while !stop.load(Ordering::Relaxed) {
                let produced = pump_resample(
                    &mut raw_cons,
                    &mut cap16k_prod,
                    &mut resampler,
                    &mut scratch_in,
                    &mut scratch_out,
                );
                if produced == 0 {
                    // Idle poll cadence (~5 ms) — small enough that capture
                    // latency stays well under one VAD frame.
                    std::thread::sleep(Duration::from_millis(5));
                }
            }
        }));

        Ok(())
    }
}

impl Drop for AudioEngine {
    fn drop(&mut self) {
        // Stop the worker first so it stops touching the rings.
        self.worker_stop.store(true, Ordering::Relaxed);
        if let Some(w) = self.worker.take() {
            let _ = w.join();
        }
        // Streams drop here on the owning thread (cpal requirement).
        self.input_stream = None;
        self.output_stream = None;
    }
}

impl AudioHandle {
    /// Move all currently-available 16 kHz mono f32 capture samples into `out`
    /// (appended). Lock-free ring drain behind a mutex (worker-thread safe).
    pub fn drain_capture_16k(&self, out: &mut Vec<f32>) {
        let mut cons = match self.cap16k_cons.lock() {
            Ok(c) => c,
            Err(p) => p.into_inner(),
        };
        let n = cons.slots();
        if n == 0 {
            return;
        }
        if let Ok(chunk) = cons.read_chunk(n) {
            let (a, b) = chunk.as_slices();
            out.extend_from_slice(a);
            out.extend_from_slice(b);
            chunk.commit_all();
        }
    }

    /// 0..1 smoothed microphone amplitude — `min(1, level*4)` over the EMA
    /// `level = level*0.6 + rms*0.4`, matching `capture-vad.ts`.
    pub fn input_level(&self) -> f32 {
        self.level.level()
    }

    /// Resample `samples` (mono f32 at `sample_rate`) to the device output rate
    /// and queue it for gapless playback. Kokoro PCM arrives at 24 kHz; any
    /// rate is accepted. Resampling runs here (worker/caller thread), never in
    /// the audio callback. Clearing the flush barrier first lets audio enqueued
    /// after a barge-in play immediately.
    pub fn enqueue_pcm(&self, samples: &[f32], sample_rate: u32) {
        if samples.is_empty() {
            return;
        }
        // A fresh enqueue after a stop should not be eaten by a stale flush.
        self.play_state.flush.store(false, Ordering::Relaxed);

        // Fast path: device already at the source rate, no resampling needed.
        if sample_rate == self.output_rate {
            self.push_pcm(samples);
            return;
        }

        // Reuse a persistent resampler when the source rate is stable (Kokoro is
        // always 24 kHz); rebuild only when the rate changes.
        let mut guard = match self.out_resampler.lock() {
            Ok(g) => g,
            Err(p) => p.into_inner(),
        };
        let need_new = match guard.as_ref() {
            Some(r) => r.from_rate() != sample_rate || r.to_rate() != self.output_rate,
            None => true,
        };
        if need_new {
            match MonoResampler::new(sample_rate, self.output_rate) {
                Ok(r) => *guard = Some(r),
                Err(e) => {
                    tracing::error!(target: "audio", "playback resampler init failed: {e}");
                    *guard = None;
                    drop(guard);
                    // One-shot fallback so audio still plays.
                    if let Ok(v) = resample_mono(samples, sample_rate, self.output_rate) {
                        self.push_pcm(&v);
                    }
                    return;
                }
            }
        }
        let r = guard.as_mut().expect("resampler present after init");
        let mut out = Vec::with_capacity(
            (samples.len() as u64 * self.output_rate as u64 / sample_rate as u64) as usize + 1024,
        );
        r.process(samples, &mut out);
        drop(guard);
        self.push_pcm(&out);
    }

    /// Push device-rate mono f32 into the playback ring (best-effort; drops the
    /// overflow tail if the ring is momentarily full). Updates the queued count.
    fn push_pcm(&self, samples: &[f32]) {
        if samples.is_empty() {
            return;
        }
        let mut prod = match self.pcm_prod.lock() {
            Ok(p) => p,
            Err(p) => p.into_inner(),
        };
        let room = prod.slots();
        let n = samples.len().min(room);
        if n == 0 {
            return;
        }
        if let Ok(chunk) = prod.write_chunk_uninit(n) {
            chunk.fill_from_iter(samples[..n].iter().copied());
            self.play_state
                .queued
                .fetch_add(n as u64, Ordering::Relaxed);
        }
    }

    /// Request instant barge-in: the output callback drops everything queued and
    /// emits silence until the ring is empty (then lifts the barrier itself).
    pub fn stop_playback(&self) {
        self.play_state.flush.store(true, Ordering::Relaxed);
    }

    /// True while speech is queued/playing (mirrors `playback.ts` isPlaying).
    pub fn is_playing(&self) -> bool {
        self.play_state.is_playing()
    }

    /// 16-band smoothed log spectrum of the output (256-pt Hann FFT, EMA 0.8) —
    /// the native replacement for the Web Audio `AnalyserNode` that drove the
    /// speaking orb. Consumes whatever the playback callback has teed since the
    /// last call.
    pub fn output_spectrum(&self) -> [f32; BANDS] {
        // Drain the tee ring into a scratch buffer, then run the FFT.
        let mut fresh: Vec<f32> = Vec::new();
        {
            let mut cons = match self.tee_cons.lock() {
                Ok(c) => c,
                Err(p) => p.into_inner(),
            };
            let n = cons.slots();
            if n > 0 {
                if let Ok(chunk) = cons.read_chunk(n) {
                    let (a, b) = chunk.as_slices();
                    fresh.extend_from_slice(a);
                    fresh.extend_from_slice(b);
                    chunk.commit_all();
                }
            }
        }
        let mut analyzer = match self.analyzer.lock() {
            Ok(a) => a,
            Err(p) => p.into_inner(),
        };
        analyzer.compute(&fresh)
    }
}

/// RT-safe capture-block handler shared by the i16/u16/f32 input callbacks:
/// down-mix interleaved `samples` (with `channels` channels) to mono via a
/// reusable `mono` scratch, fold the block RMS into `level`, and push the mono
/// block into the raw ring. `conv` converts the device sample type to f32.
fn capture_block<T, F>(
    samples: &[T],
    channels: usize,
    mono: &mut Vec<f32>,
    raw_prod: &mut rtrb::Producer<f32>,
    level: &InputLevel,
    conv: F,
) where
    T: Copy,
    F: Fn(T) -> f32,
{
    let channels = channels.max(1);
    let frames = samples.len() / channels;
    mono.clear();
    mono.reserve(frames);
    if channels == 1 {
        for &s in samples {
            mono.push(conv(s));
        }
    } else {
        for frame in 0..frames {
            let base = frame * channels;
            let mut acc = 0.0f32;
            for c in 0..channels {
                acc += conv(samples[base + c]);
            }
            mono.push(acc / channels as f32);
        }
    }
    // Block RMS -> smoothed listening level (pure arithmetic, RT-safe).
    level.push_block(&mono[..]);
    // Push to the raw ring; drop the overflow tail rather than block.
    let room = raw_prod.slots();
    let n = mono.len().min(room);
    if n > 0 {
        if let Ok(chunk) = raw_prod.write_chunk_uninit(n) {
            chunk.fill_from_iter(mono[..n].iter().copied());
        }
    }
}

/// Resize `buf` to exactly `len`, reusing capacity (RT-safe after first call
/// once the capacity is reached).
fn ensure_len(buf: &mut Vec<f32>, len: usize) {
    if buf.len() != len {
        buf.resize(len, 0.0);
    }
}

/// Clamp a pathological driver-reported fixed buffer size into a sane range.
fn clamp_buffer_size(config: &mut StreamConfig) {
    if let cpal::BufferSize::Fixed(frames) = config.buffer_size {
        let clamped = frames.clamp(MIN_BLOCK, MAX_BLOCK);
        config.buffer_size = cpal::BufferSize::Fixed(clamped);
    }
    // The sample rate is whatever the device default reported; we never assume a
    // specific rate, so it is left untouched.
}

/// Compile-time assertions that the handle is `Send + Sync + Clone` and the
/// engine is intentionally *not* `Send`.
#[allow(dead_code)]
fn _assert_traits() {
    fn is_send_sync_clone<T: Send + Sync + Clone>() {}
    is_send_sync_clone::<AudioHandle>();
}

#[cfg(test)]
mod tests {
    use crate::audio::capture::InputLevel;
    use crate::audio::playback::{fill_output, PlaybackRings};
    use crate::audio::resample::{resample_mono, MonoResampler};
    use crate::audio::spectrum::SpectrumAnalyzer;

    #[test]
    fn level_matches_capture_vad_ema() {
        // Reproduce capture-vad.ts: level = level*0.6 + rms*0.4, exposed as
        // min(1, level*4). Feed a constant-amplitude block; the level rises
        // monotonically toward rms and saturates the exposed value at 1.
        let lvl = InputLevel::new();
        let block = vec![0.5f32; 512];
        let rms = 0.5f32; // RMS of a constant 0.5 signal.
        let mut expected = 0.0f32;
        for _ in 0..50 {
            lvl.push_block(&block);
            expected = expected * 0.6 + rms * 0.4;
        }
        // The exposed value is min(1, level*4): with rms 0.5 it saturates at 1.
        assert!((lvl.level() - (expected * 4.0).min(1.0)).abs() < 1e-4);
    }

    #[test]
    fn silence_is_zero_level() {
        let lvl = InputLevel::new();
        lvl.push_block(&[0.0f32; 256]);
        assert_eq!(lvl.level(), 0.0);
    }

    #[test]
    fn resample_24k_to_48k_roughly_doubles() {
        // Kokoro 24 kHz -> a 48 kHz device should roughly double the frame count.
        let input = vec![0.0f32; 2400]; // 0.1 s @ 24 kHz
        let out = resample_mono(&input, 24_000, 48_000).expect("resample");
        let ratio = out.len() as f64 / input.len() as f64;
        assert!(ratio > 1.6 && ratio < 2.4, "ratio was {ratio}");
    }

    #[test]
    fn resample_identity_when_rates_match() {
        let input = vec![0.1, 0.2, 0.3, 0.4];
        let out = resample_mono(&input, 16_000, 16_000).expect("resample");
        assert_eq!(out, input);
    }

    #[test]
    fn streaming_resampler_accepts_arbitrary_chunks() {
        let mut r = MonoResampler::new(44_100, 16_000).expect("resampler");
        let mut out = Vec::new();
        // Feed in odd-sized pieces; the resampler buffers leftovers internally.
        for chunk in [100usize, 333, 777, 50, 4096] {
            let piece = vec![0.0f32; chunk];
            r.process(&piece, &mut out);
        }
        r.flush(&mut out);
        // Downsampling 44.1k -> 16k should produce fewer samples than fed in.
        let fed = 100 + 333 + 777 + 50 + 4096;
        assert!(out.len() < fed);
        assert!(!out.is_empty());
    }

    #[test]
    fn spectrum_is_16_bands_and_smooths() {
        let mut sa = SpectrumAnalyzer::new();
        // Silence -> all-zero bands.
        let bands = sa.compute(&[0.0f32; 256]);
        assert_eq!(bands.len(), 16);
        assert!(bands.iter().all(|&b| b == 0.0));
        // A loud tone-ish burst should raise at least one band over repeated
        // frames (EMA ramp).
        let tone: Vec<f32> = (0..512).map(|n| (n as f32 * 0.2).sin() * 0.9).collect();
        let mut peak = 0.0f32;
        for _ in 0..20 {
            let b = sa.compute(&tone);
            peak = peak.max(b.iter().cloned().fold(0.0, f32::max));
        }
        assert!(peak > 0.0);
        assert!(bands.iter().all(|&b| (0.0..=1.0).contains(&b)));
    }

    #[test]
    fn fill_output_silence_on_empty_ring() {
        let mut pb = PlaybackRings::new(1024, 1024);
        let mut out = vec![1.0f32; 64]; // stereo: 32 frames, 2 ch
        fill_output(&mut out, 2, &mut pb.pcm_cons, &mut pb.tee_prod, &pb.state);
        assert!(out.iter().all(|&s| s == 0.0), "underrun must be silence");
    }

    #[test]
    fn fill_output_duplicates_mono_across_channels() {
        let mut pb = PlaybackRings::new(1024, 1024);
        // Queue 4 mono samples.
        {
            let chunk = pb.pcm_prod.write_chunk_uninit(4).unwrap();
            chunk.fill_from_iter([0.1f32, 0.2, 0.3, 0.4]);
