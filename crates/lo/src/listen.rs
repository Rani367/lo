//! The "listen" std thread: owns the !Send on-device hearing models (whisper
//! ASR, Silero VAD), continuously drains the 16 kHz capture ring, and turns
//! speech into a transcript that it hands to the UI as `AppEvent::Transcribed`.
//!
//! Two activation paths coexist:
//!   - **Hands-free (`Vad`, the default):** a Silero segmenter watches the mic and
//!     fires a transcript when you stop talking. Lo's own TTS is suppressed so it
//!     never hears itself.
//!   - **Push-to-talk:** holding Space buffers exactly what's said and transcribes
//!     it on release. PTT takes precedence in every mode, so it's always available
//!     as an override on top of hands-free.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use lo_core::types::ActivationMode;
use lo_core::LoSettings;
use winit::event_loop::EventLoopProxy;

use crate::app::state::MIN_PTT_SAMPLES;
use crate::audio::AudioHandle;
use crate::events::AppEvent;
use crate::ml::{self, VadEvent};

const SAMPLE_RATE: usize = 16_000;
/// Reject clips longer than 30 s before they reach the model, so a stuck key or a
/// noisy room can't hand whisper an unbounded buffer.
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

    // Hands-free mode runs an always-on Silero segmenter; pure push-to-talk does
    // not (audio is only transcribed while Space is held). Best-effort: if the VAD
    // model can't load, hands-free goes quiet but push-to-talk still works.
    let mut vad: Option<ml::Vad> = if mode == ActivationMode::Vad {
        match ml::new_vad(None, vad_tuning(&settings)) {
            Ok(v) => Some(v),
            Err(e) => {
                tracing::warn!("VAD unavailable; hands-free off (push-to-talk still works): {e:#}");
                None
            }
        }
    } else {
        None
    };

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
            // Idle briefly so we don't busy-spin; well under one VAD frame (32 ms).
            std::thread::sleep(Duration::from_millis(8));
        }

        // Push-to-talk takes precedence in every mode: while Space is held we buffer
        // exactly what's said and transcribe it on release. The app already bumped
        // the barge-in epoch and stopped playback on the key press.
        let ptt = ptt_active.load(Ordering::SeqCst);
        if ptt {
            if !ptt_was {
                // Rising edge: suppress the hands-free segmenter for this hold and
                // start the clip fresh.
                if let Some(v) = vad.as_mut() {
                    v.reset();
                }
                frame_buf.clear();
                ptt_clip.clear();
            }
            ptt_clip.extend_from_slice(&scratch);
            if ptt_clip.len() > MAX_CLIP_SAMPLES {
                let cut = ptt_clip.len() - MAX_CLIP_SAMPLES;
                ptt_clip.drain(0..cut);
            }
            ptt_was = true;
            continue;
        } else if ptt_was {
            // Falling edge: finalize the held clip and transcribe it.
            let clip = std::mem::take(&mut ptt_clip);
            let text = if clip.len() >= MIN_PTT_SAMPLES {
                transcribe(&mut asr, &mut asr_failed, &model, &clip)
            } else {
                String::new()
            };
            let _ = proxy.send_event(AppEvent::Transcribed { text });
            ptt_was = false;
            // Resume hands-free with a clean segmenter.
            if let Some(v) = vad.as_mut() {
                v.reset();
            }
            frame_buf.clear();
            continue;
        }

        // Hands-free segmentation (only when not in a push-to-talk hold).
        if mode == ActivationMode::Vad {
            if audio.is_playing() {
                // Don't let Lo's own TTS trip the VAD: discard captured audio while
                // speaking and keep the segmenter reset for the next turn.
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
                                let _ = proxy.send_event(AppEvent::Transcribed { text });
                            }
                        }
                    }
                }
            } else {
                // No VAD engine — drop frames so the ring doesn't overflow.
                frame_buf.clear();
            }
        }
        // Pure push-to-talk mode just idles here between holds; `scratch` was
        // already drained above, so the capture ring won't overflow.
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
