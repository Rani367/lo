//! The "listen" std thread: owns the !Send on-device hearing models (whisper
//! ASR, Silero VAD), continuously drains the 16 kHz capture ring, and turns
//! speech into a transcript that it hands to the UI as `AppEvent::Transcribed`.
//! Ports the activation logic of `src/renderer/audio/capture-vad.ts` (push-to-talk
//! buffering; VAD auto-segmentation).

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lo_core::types::ActivationMode;
use lo_core::LoSettings;
use winit::event_loop::EventLoopProxy;

use crate::app::state::MIN_PTT_SAMPLES;
use crate::audio::AudioHandle;
use crate::events::AppEvent;
use crate::ml::{self, DisabledWake, VadEvent, WakeWord};

const SAMPLE_RATE: usize = 16_000;
/// Reject clips longer than 30 s before they reach the model (parity with the
/// `MAX_SECONDS` cap in servers.ts).
const MAX_CLIP_SAMPLES: usize = SAMPLE_RATE * 30;
/// Silero VAD operates on 512-sample (32 ms) frames at 16 kHz.
const VAD_FRAME: usize = 512;

pub struct ListenCtx {
    pub audio: AudioHandle,
    pub proxy: EventLoopProxy<AppEvent>,
    pub settings: LoSettings,
    pub ptt_active: Arc<AtomicBool>,
}

pub fn spawn(ctx: ListenCtx) {
    let _ = std::thread::Builder::new()
        .name("lo-listen".into())
        .spawn(move || run(ctx));
}

fn run(ctx: ListenCtx) {
    let ListenCtx {
        audio,
        proxy,
        settings,
        ptt_active,
    } = ctx;
    let mode = settings.activation_mode;
    let model = settings.asr_model.clone();

    let mut asr: Option<ml::Asr> = None;
    let mut asr_failed = false;

    let mut vad: Option<ml::Vad> = if mode == ActivationMode::Vad {
        match ml::new_vad(None) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("VAD unavailable, falling back to idle: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Wake word (best-effort): DisabledWake never fires until Porcupine is
    // vendored, so `wake` mode currently idles — use PTT meanwhile.
    let mut wake: Option<Box<dyn WakeWord>> = if mode == ActivationMode::Wake {
        Some(Box::new(DisabledWake))
    } else {
        None
    };

    let mut ptt_clip: Vec<f32> = Vec::new();
    let mut ptt_was = false;
    let mut frame_buf: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();

    loop {
        scratch.clear();
        audio.drain_capture_16k(&mut scratch);
        if scratch.is_empty() {
            std::thread::sleep(Duration::from_millis(8));
        }

        match mode {
            ActivationMode::Wake => {
                if let Some(w) = wake.as_mut() {
                    let n = w.frame_length().max(1);
                    frame_buf.extend_from_slice(&scratch);
                    while frame_buf.len() >= n {
                        let frame: Vec<i16> = frame_buf
                            .drain(0..n)
                            .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                            .collect();
                        if w.process_i16(&frame) {
                            // Future: a wake fires → begin a VAD-style listen.
                            tracing::info!("wake word detected");
                        }
                    }
                }
            }
            ActivationMode::Ptt => {
                let active = ptt_active.load(Ordering::SeqCst);
                if active {
                    ptt_clip.extend_from_slice(&scratch);
                    if ptt_clip.len() > MAX_CLIP_SAMPLES {
                        let cut = ptt_clip.len() - MAX_CLIP_SAMPLES;
                        ptt_clip.drain(0..cut);
                    }
                } else if ptt_was {
                    // Falling edge: finalize the clip.
                    let clip = std::mem::take(&mut ptt_clip);
                    let text = if clip.len() >= MIN_PTT_SAMPLES {
                        transcribe(&mut asr, &mut asr_failed, &model, &clip)
                    } else {
                        String::new()
                    };
                    let _ = proxy.send_event(AppEvent::Transcribed { id: 0, text });
                }
                ptt_was = active;
            }
            ActivationMode::Vad => {
                if let Some(v) = vad.as_mut() {
                    frame_buf.extend_from_slice(&scratch);
                    while frame_buf.len() >= VAD_FRAME {
                        let frame: Vec<f32> = frame_buf.drain(0..VAD_FRAME).collect();
                        for ev in v.push_frame(&frame) {
                            if let VadEvent::SpeechEnd(clip) = ev {
                                let text = transcribe(&mut asr, &mut asr_failed, &model, &clip);
                                if !text.trim().is_empty() {
                                    let _ = proxy.send_event(AppEvent::Transcribed { id: 0, text });
                                }
                            }
                        }
                    }
                } else {
                    // No VAD engine — drop frames so the ring doesn't overflow.
                    frame_buf.clear();
                }
            }
        }
    }
}

/// Lazily load whisper, then transcribe; returns "" on any failure (the UI then
/// simply returns to idle).
fn transcribe(
    asr: &mut Option<ml::Asr>,
    failed: &mut bool,
    model: &str,
    samples: &[f32],
) -> String {
    if asr.is_none() && !*failed {
        match ml::load_asr(model, None) {
            Ok(a) => *asr = Some(a),
            Err(e) => {
                tracing::warn!("ASR unavailable: {e:#}");
                *failed = true;
            }
        }
    }
    match asr.as_mut() {
        Some(a) => a.transcribe(samples).unwrap_or_else(|e| {
            tracing::warn!("transcription failed: {e:#}");
            String::new()
        }),
        None => String::new(),
    }
}
