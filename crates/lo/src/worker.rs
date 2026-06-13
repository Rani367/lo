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

    while let Some(cmd) = ctx.ui_rx.recv().await {
        match cmd {
            UiCommand::StartTurn {
                turn_id,
                history,
                epoch,
            } => {
                let mut erx = ctx.epoch_rx.clone();
                let turn = handle_turn(
                    &engine,
                    &settings,
                    &ctx.proxy,
                    &announce_tx,
                    &tts_tx,
                    &turn_id,
                    &history,
                    epoch,
                );
                tokio::select! {
                    res = turn => {
                        let result = match res {
                            Ok(r) => r,
                            Err(e) => {
                                let _ = ctx.proxy.send_event(AppEvent::Error(format!("{e:#}")));
                                ChatTurnResult { turn_id: turn_id.clone(), reply: EMPTY_REPLY_FALLBACK.to_string(), ..Default::default() }
                            }
                        };
                        let _ = ctx.proxy.send_event(AppEvent::LlmDone { turn_id: turn_id.clone(), result });
                    }
                    _ = wait_epoch_change(&mut erx, epoch) => {
                        tracing::debug!("turn {turn_id} cancelled (barge-in)");
                    }
                }
            }
            UiCommand::Cancel { .. } => { /* the epoch watch already aborts the in-flight turn */ }
            UiCommand::Transcribe { .. } => { /* transcription runs on the listen thread */ }
            UiCommand::UpdateSettings(s) => {
                settings = *s;
                if let Err(e) = engine.restart(&settings).await {
                    tracing::warn!("engine restart failed: {e:#}");
                }
            }
            UiCommand::Shutdown => {
                engine.stop();
                break;
            }
        }
    }
    engine.stop();
}

#[allow(clippy::too_many_arguments)]
async fn handle_turn(
    engine: &Engine,
    settings: &LoSettings,
    proxy: &EventLoopProxy<AppEvent>,
    announce_tx: &UnboundedSender<String>,
    tts_tx: &StdSender<TtsMsg>,
    turn_id: &str,
    history: &[ChatMessage],
    epoch: u64,
) -> anyhow::Result<ChatTurnResult> {
    // Ensure the engine is up (downloads on first run report via ModelDownload).
    {
        let p = proxy.clone();
        let prog = move |label: &str, pct: Option<u8>| {
            let _ = p.send_event(AppEvent::ModelDownload {
                label: label.to_string(),
                pct,
            });
        };
        engine.ensure_ready(settings, Some(&prog)).await?;
    }
    // Surface engine health to the HUD status dot.
    let _ = proxy.send_event(AppEvent::ServerStatus(engine.status(settings)));

    let endpoint = engine.endpoint(settings);
    let mut convo = initial_convo(settings, history);
    let mut tools_invoked: Vec<String> = Vec::new();
    let mut used_web = false;
    let mut final_text = String::new();

    for _round in 0..MAX_ROUNDS {
        let body = build_request_body(&endpoint.model_id, &convo, settings.temperature);
        let (text, calls) = {
            let p = proxy.clone();
            let tid = turn_id.to_string();
            let mut on_delta = move |d: &str| {
                let _ = p.send_event(AppEvent::LlmDelta {
                    turn_id: tid.clone(),
                    delta: d.to_string(),
                });
            };
            stream_completion(&endpoint, body, &mut on_delta).await?
        };

        if calls.is_empty() {
            final_text = text.trim().to_string();
            break;
        }

        let mut results = Vec::with_capacity(calls.len());
        for call in &calls {
            let _ = proxy.send_event(AppEvent::LlmTool {
                turn_id: turn_id.to_string(),
                tool: call.function.name.clone(),
                status: ToolStatus::Start,
                detail: None,
            });
            let res = tools::dispatch(
                &call.function.name,
                &call.function.arguments,
                settings,
                announce_tx,
            )
            .await;
            tools_invoked.push(call.function.name.clone());
            if call.function.name == "web_search" {
                used_web = true;
            }
            let _ = proxy.send_event(AppEvent::LlmTool {
                turn_id: turn_id.to_string(),
                tool: call.function.name.clone(),
                status: ToolStatus::Done,
                detail: None,
            });
            results.push(res);
        }
        append_tool_round(&mut convo, &text, calls, results);
    }

    if final_text.is_empty() {
