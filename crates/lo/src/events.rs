//! The in-process message contract that replaces Electron's IPC. The winit/UI
//! thread and the tokio worker communicate over two channels:
//!   - UI → worker: `mpsc::UnboundedSender<UiCommand>`
//!   - worker/ML → UI: `EventLoopProxy<AppEvent>` (delivered to `user_event`)
//!
//! These map 1:1 onto the old `IPC` channels in `src/shared/types.ts`.

use lo_core::{ChatMessage, ChatTurnResult, LocalStatus};
use std::sync::Arc;

/// UI → worker (was `ipcMain.handle` invocations + renderer-local actions).
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Run an agent turn for `history` (was `IPC.chat`). `epoch` lets a later
    /// `Cancel` invalidate this turn (barge-in).
    StartTurn {
        turn_id: String,
        history: Vec<ChatMessage>,
        epoch: u64,
    },
    /// Barge-in: cancel any in-flight turn whose epoch is `<= epoch`.
    Cancel { epoch: u64 },
    /// Transcribe a PTT/VAD clip (was `IPC.transcribeAudio`). The `id` lets a
    /// superseded speculative result be dropped.
    Transcribe { id: u64, samples: Arc<[f32]> },
    /// Push a settings change through the worker (backend/model restart, etc.).
    UpdateSettings(Box<lo_core::LoSettings>),
    /// App is exiting — stop all child servers (was `stopAllServers`).
    Shutdown,
}

/// Progress of a single tool invocation (was the `status` field of `LlmToolEvent`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Start,
    Done,
    Error,
}

/// worker/ML → UI (was `webContents.send` events). Delivered into winit's
/// `ApplicationHandler::user_event`.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A streamed assistant prose delta (was `IPC.evtLlmDelta`).
    LlmDelta { turn_id: String, delta: String },
    /// A tool started / finished / errored (was `IPC.evtLlmTool`).
    LlmTool {
        turn_id: String,
        tool: String,
        status: ToolStatus,
        detail: Option<String>,
    },
    /// The turn finished (was `IPC.evtLlmDone`).
    LlmDone {
        turn_id: String,
        result: ChatTurnResult,
    },
    /// A timer fired; speak this when idle (was `IPC.evtAnnounce`).
    Announce(String),
    /// First-run engine/model download progress (was `IPC.evtModelDownload`).
    ModelDownload { label: String, pct: Option<u8> },
    /// Result of a `Transcribe` command, matched by `id`.
    Transcribed { id: u64, text: String },
    /// Engine health for the HUD status dot.
    ServerStatus(LocalStatus),
    /// A worker-level error to surface.
    Error(String),
}
