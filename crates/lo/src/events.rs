//! The in-process message contract between the two halves of the app. The
//! winit/UI thread and the tokio worker communicate over two channels:
//!   - UI → worker: `mpsc::UnboundedSender<UiCommand>`
//!   - worker/ML → UI: `EventLoopProxy<AppEvent>` (delivered to `user_event`)
//!
//! [`UiCommand`] carries requests from the UI (start/cancel a turn, shut down);
//! [`AppEvent`] carries the worker's streamed results back. Barge-in is keyed by an
//! `epoch`: a new listen bumps the epoch (over a separate watch channel) and a
//! `Cancel` tells the worker to abandon the in-flight turn.

use lo_core::{ChatMessage, ChatTurnResult, LocalStatus};

/// UI → worker: requests issued by the UI thread.
#[derive(Debug, Clone)]
pub enum UiCommand {
    /// Run an agent turn for `history`. `epoch` lets a later `Cancel` invalidate
    /// this turn (barge-in).
    StartTurn {
        turn_id: String,
        history: Vec<ChatMessage>,
        epoch: u64,
    },
    /// Barge-in: abandon any in-flight turn (the epoch watch channel carries which).
    Cancel,
    /// App is exiting — stop all child servers.
    Shutdown,
}

/// Progress of a single tool invocation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolStatus {
    Start,
    Done,
}

/// worker/ML → UI: results streamed back to the UI thread. Delivered into
/// winit's `ApplicationHandler::user_event`.
#[derive(Debug, Clone)]
pub enum AppEvent {
    /// A streamed assistant prose delta.
    LlmDelta { turn_id: String, delta: String },
    /// A tool started / finished.
    LlmTool {
        turn_id: String,
        tool: String,
        status: ToolStatus,
        detail: Option<String>,
    },
    /// The turn finished.
    LlmDone {
        turn_id: String,
        result: ChatTurnResult,
    },
    /// A timer fired; speak this when idle.
    Announce(String),
    /// First-run engine/model download progress.
    ModelDownload { label: String, pct: Option<u8> },
    /// A finished transcript, ready to drive a turn.
    Transcribed { text: String },
    /// Engine health for the HUD status dot.
    ServerStatus(LocalStatus),
    /// A worker-level error to surface.
    Error(String),
}
