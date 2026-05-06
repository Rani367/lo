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
