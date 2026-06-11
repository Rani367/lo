//! The tokio worker — the native function-calling agent loop (the orchestration
//! half of `src/main/brain.ts`, with the streaming transport in `crate::brain`).
//! Owns the backend `Engine`, dispatches tools, and drives Kokoro TTS on a
//! dedicated std thread (Kokoro's session is !Send, so it can't live on the
//! tokio pool). Emits `AppEvent`s to the UI via the `EventLoopProxy`.

use std::sync::mpsc::{Receiver as StdReceiver, Sender as StdSender};

use lo_core::brain::{
    append_tool_round, build_request_body, initial_convo, EMPTY_REPLY_FALLBACK, MAX_ROUNDS,
};
use lo_core::text::{chunk_for_tts_default, strip_directives};
use lo_core::types::{ChatMessage, ChatTurnResult};
use lo_core::LoSettings;
use tokio::sync::mpsc::{unbounded_channel, UnboundedReceiver, UnboundedSender};
use tokio::sync::watch;
use winit::event_loop::EventLoopProxy;

use crate::audio::AudioHandle;
use crate::backends::Engine;
use crate::brain::stream_completion;
use crate::events::{AppEvent, ToolStatus, UiCommand};
use crate::ml;
use crate::tools;

/// Everything the worker needs from `main`.
pub struct WorkerCtx {
    pub ui_rx: UnboundedReceiver<UiCommand>,
    pub proxy: EventLoopProxy<AppEvent>,
    pub settings: LoSettings,
    pub audio: AudioHandle,
    pub epoch_rx: watch::Receiver<u64>,
}

/// A sentence queued for synthesis, tagged with the epoch it belongs to so the
/// TTS thread can drop it after a barge-in.
struct TtsMsg {
    text: String,
    speed: f32,
    epoch: u64,
}

pub async fn run(mut ctx: WorkerCtx) {
    let engine = Engine::new();
    let mut settings = ctx.settings;

    // tools (e.g. set_timer) push announcements here.
    let (announce_tx, mut announce_rx) = unbounded_channel::<String>();
    // Sentences for the Kokoro TTS thread.
    let (tts_tx, tts_rx) = std::sync::mpsc::channel::<TtsMsg>();

    // TTS thread (owns the !Send Kokoro engine, loaded lazily).
    {
        let audio = ctx.audio.clone();
        let epoch_rx = ctx.epoch_rx.clone();
        let model = settings.tts_model.clone();
        let voice = settings.voice.clone();
        let _ = std::thread::Builder::new()
            .name("lo-tts".into())
            .spawn(move || tts_thread(tts_rx, audio, epoch_rx, model, voice));
    }

    // Announcement drainer: surface it as a caption AND speak it.
    {
        let proxy = ctx.proxy.clone();
        let tts_tx = tts_tx.clone();
        let speed = settings.speech_rate as f32;
        let epoch_rx = ctx.epoch_rx.clone();
        tokio::spawn(async move {
            while let Some(text) = announce_rx.recv().await {
                let _ = proxy.send_event(AppEvent::Announce(text.clone()));
                let ep = *epoch_rx.borrow();
                for chunk in chunk_for_tts_default(&strip_directives(&text)) {
                    let _ = tts_tx.send(TtsMsg {
                        text: chunk,
                        speed,
                        epoch: ep,
                    });
                }
            }
        });
    }

