//! The winit `ApplicationHandler`: owns the window, the GPU (`Gui`), the cpal
//! `AudioEngine` (kept alive here on the main thread), and the UI-side
//! `Session` state machine. It forwards input to the worker/listen thread and
//! renders the orb + captions each frame. Ported from the orchestration in
//! `src/renderer/renderer.ts`.

pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Instant;

use lo_core::types::LoState;
use lo_core::LoSettings;
use tokio::sync::{mpsc::UnboundedSender, watch};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::ActiveEventLoop;
use winit::keyboard::{Key, NamedKey};
use winit::window::{Window, WindowId};

use crate::audio::{AudioEngine, AudioHandle};
use crate::events::{AppEvent, UiCommand};
use crate::gui::{Captions, Gui};
use state::Session;

/// Everything `App::new` needs from `main`.
pub struct AppCtx {
    pub audio_engine: AudioEngine,
    pub audio: AudioHandle,
    pub ui_tx: UnboundedSender<UiCommand>,
    pub epoch_tx: watch::Sender<u64>,
    pub ptt_active: Arc<AtomicBool>,
    pub settings: LoSettings,
}

pub struct App {
    // Kept alive so the cpal streams keep running (AudioEngine is !Send and must
    // live on the main thread).
    _audio_engine: AudioEngine,
    audio: AudioHandle,
    ui_tx: UnboundedSender<UiCommand>,
    epoch_tx: watch::Sender<u64>,
    ptt_active: Arc<AtomicBool>,
    settings: LoSettings,

    session: Session,
    gui: Option<Gui>,
    window: Option<Arc<Window>>,
    start: Instant,
    last_frame: Instant,
}

impl App {
    pub fn new(ctx: AppCtx) -> Self {
        let now = Instant::now();
