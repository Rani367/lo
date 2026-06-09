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
