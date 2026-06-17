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
use crate::ml::{self, VadEvent, WakeWord};

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
    /// Set on shutdown so the listen loop exits and the thread can be joined.
    pub shutdown: Arc<AtomicBool>,
}

/// Spawn the listen thread, returning its handle so `main` can join it on exit.
pub fn spawn(ctx: ListenCtx) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("lo-listen".into())
        .spawn(move || run(ctx))
        .expect("failed to spawn listen thread")
}

fn run(ctx: ListenCtx) {
    let ListenCtx {
        audio,
        proxy,
        settings,
        ptt_active,
        shutdown,
    } = ctx;
    let mode = settings.activation_mode;
    let model = settings.asr_model.clone();

    let mut asr: Option<ml::Asr> = None;
    let mut asr_failed = false;

    let mut vad: Option<ml::Vad> = if mode == ActivationMode::Vad {
        match ml::new_vad(None, vad_tuning(&settings)) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("VAD unavailable, falling back to idle: {e:#}");
                None
            }
        }
    } else {
        None
    };

    // Wake word ("Hey Jarvis", openWakeWord). In wake mode we also keep a VAD to
    // segment the utterance that follows a detection. Both are best-effort: if the
    // model can't load, wake mode simply idles (use PTT meanwhile).
    let mut wake: Option<Box<dyn WakeWord>> = if mode == ActivationMode::Wake {
        match ml::load_wakeword(settings.wake_threshold, None) {
            Ok(w) => Some(w),
            Err(e) => {
                tracing::warn!("wake word unavailable, idling: {e:#}");
                None
            }
        }
    } else {
        None
    };
    let mut wake_vad: Option<ml::Vad> = if mode == ActivationMode::Wake {
        ml::new_vad(None, vad_tuning(&settings)).ok()
    } else {
        None
    };
    // True once the wake word has fired and we're capturing the user's utterance.
    let mut armed = false;
    let mut armed_frames: usize = 0;
    // Auto-disarm if no utterance arrives within ~8 s of the wake word.
    const ARM_TIMEOUT_FRAMES: usize = 8000 / 32;

    let mut ptt_clip: Vec<f32> = Vec::new();
    let mut ptt_was = false;
    let mut frame_buf: Vec<f32> = Vec::new();
    let mut scratch: Vec<f32> = Vec::new();

    loop {
        if shutdown.load(Ordering::SeqCst) {
            break;
        }
        scratch.clear();
        audio.drain_capture_16k(&mut scratch);
        if scratch.is_empty() {
            std::thread::sleep(Duration::from_millis(8));
        }

        match mode {
            ActivationMode::Wake => {
                frame_buf.extend_from_slice(&scratch);
                if !armed {
                    // Listen for "Hey Jarvis" in 1280-sample (80 ms) chunks.
                    if let Some(w) = wake.as_mut() {
                        let n = w.frame_length().max(1);
                        while frame_buf.len() >= n {
                            let frame: Vec<i16> = frame_buf
                                .drain(0..n)
                                .map(|s| (s.clamp(-1.0, 1.0) * 32767.0) as i16)
                                .collect();
                            if w.process_i16(&frame) {
                                armed = true;
                                armed_frames = 0;
                                if let Some(v) = wake_vad.as_mut() {
                                    v.reset();
                                }
                                frame_buf.clear(); // capture the utterance fresh
                                break;
                            }
                        }
                    } else {
                        frame_buf.clear(); // no detector — don't grow the buffer
                    }
                } else if audio.is_playing() {
                    // Don't capture Lo's own reply as the user's utterance.
                    frame_buf.clear();
                } else if let Some(v) = wake_vad.as_mut() {
                    // After the wake word: segment the utterance with the VAD.
                    while frame_buf.len() >= VAD_FRAME {
                        let frame: Vec<f32> = frame_buf.drain(0..VAD_FRAME).collect();
                        armed_frames += 1;
                        for ev in v.push_frame(&frame) {
                            if let VadEvent::SpeechEnd(clip) = ev {
                                armed = false;
                                if !clip.is_empty() {
                                    let text = transcribe(&mut asr, &mut asr_failed, &model, &clip);
                                    if !text.trim().is_empty() {
                                        let _ =
                                            proxy.send_event(AppEvent::Transcribed { id: 0, text });
                                    }
                                }
                            }
                        }
                    }
                    if armed && armed_frames > ARM_TIMEOUT_FRAMES {
                        armed = false; // user said nothing — go back to waiting
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
                if audio.is_playing() {
                    // Don't let Lo's own TTS trip the VAD: discard captured audio
                    // while speaking and keep the segmenter reset for the next turn.
                    frame_buf.clear();
                    if let Some(v) = vad.as_mut() {
                        v.reset();
                    }
                } else if let Some(v) = vad.as_mut() {
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

/// Build VAD thresholds from settings, with `LO_VAD_*` env overrides for quick
/// field tuning in noisy rooms (no UI). Redemption is given in ms and converted
/// to whole frames.
fn vad_tuning(s: &LoSettings) -> ml::VadTuning {
    let positive = env_f32("LO_VAD_POSITIVE").unwrap_or(s.vad_positive_threshold);
    let negative = env_f32("LO_VAD_NEGATIVE").unwrap_or(s.vad_negative_threshold);
    let redemption_ms = env_u32("LO_VAD_REDEMPTION_MS").unwrap_or(s.vad_redemption_ms);
    let redemption_frames = ((redemption_ms as f32 / ml::vad::FRAME_MS).round() as usize).max(1);
    ml::VadTuning {
        positive,
        negative,
        redemption_frames,
    }
}

fn env_f32(key: &str) -> Option<f32> {
    std::env::var(key).ok()?.trim().parse().ok()
}

fn env_u32(key: &str) -> Option<u32> {
    std::env::var(key).ok()?.trim().parse().ok()
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
