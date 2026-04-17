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
