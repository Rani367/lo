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
        Self {
            _audio_engine: ctx.audio_engine,
            audio: ctx.audio,
            ui_tx: ctx.ui_tx,
            epoch_tx: ctx.epoch_tx,
            ptt_active: ctx.ptt_active,
            settings: ctx.settings,
            session: Session::new(),
            gui: None,
            window: None,
            start: now,
            last_frame: now,
        }
    }

    /// Space pressed/released → push-to-talk (ptt is the default activation mode).
    fn on_key(&mut self, ev: &KeyEvent) {
        let is_space = matches!(ev.logical_key, Key::Named(NamedKey::Space));
        if !is_space {
            return;
        }
        match ev.state {
            ElementState::Pressed => {
                if ev.repeat || self.session.ptt_recording {
                    return;
                }
                // Barge-in: invalidate any in-flight turn, stop playback now.
                let epoch = self.session.begin_listen();
                let _ = self.epoch_tx.send(epoch);
                self.audio.stop_playback();
                let _ = self.ui_tx.send(UiCommand::Cancel { epoch });
                self.ptt_active.store(true, Ordering::SeqCst);
                self.set_gui_state();
            }
            ElementState::Released => {
                if self.session.ptt_recording {
                    // The listen thread sees the falling edge, finalizes the clip,
                    // transcribes, and replies with AppEvent::Transcribed.
                    self.ptt_active.store(false, Ordering::SeqCst);
                }
            }
        }
    }

    fn on_app_event(&mut self, ev: AppEvent) {
        match ev {
            AppEvent::Transcribed { text, .. } => {
                let t = text.trim().to_string();
                if t.is_empty() {
                    // Misfire / too short — return to idle.
                    self.session.ptt_recording = false;
                    self.session.busy = false;
                    self.session.state = LoState::Idle;
                } else {
                    let (turn_id, history) = self.session.begin_turn(&t);
                    let _ = self.ui_tx.send(UiCommand::StartTurn {
                        turn_id,
                        history,
                        epoch: self.session.epoch,
                    });
                }
                self.set_gui_state();
            }
            AppEvent::LlmDelta { turn_id, delta } => {
                self.session.push_delta(&turn_id, &delta);
                self.set_gui_state();
            }
            AppEvent::LlmTool {
                turn_id,
                tool,
                status,
                detail,
            } => {
                tracing::debug!(turn = %turn_id, %tool, ?status, ?detail, "tool event");
            }
            AppEvent::LlmDone { turn_id, result } => {
                let reply = if result.reply.is_empty() {
                    result.error.clone().unwrap_or_default()
                } else {
                    result.reply.clone()
                };
                self.session.finish_turn(&turn_id, &reply);
                self.set_gui_state();
            }
            AppEvent::Announce(text) => {
                // The worker speaks the announcement itself; we just surface it.
                self.session.lo_text = lo_core::text::strip_directives(&text);
                self.session.since_done = 0.0;
            }
            AppEvent::ModelDownload { label, pct } => {
                self.session.lo_text = match pct {
                    Some(p) => format!("Getting {label}… {p}%"),
                    None => format!("Getting {label}…"),
                };
            }
            AppEvent::ServerStatus(_status) => { /* reserved for a HUD status dot */ }
            AppEvent::Error(e) => {
                tracing::error!("worker error: {e}");
                self.session.state = LoState::Error;
                self.session.busy = false;
                self.session.lo_text = e;
                self.set_gui_state();
            }
        }
    }

    fn set_gui_state(&mut self) {
        if let Some(gui) = &mut self.gui {
            gui.set_state(self.session.state);
        }
    }

    fn render(&mut self) {
        let Some(gui) = &mut self.gui else { return };
        let now = Instant::now();
        let dt = (now - self.last_frame).as_secs_f32().min(0.1);
        self.last_frame = now;
        let time = (now - self.start).as_secs_f32();
        self.session.tick(dt);

        let spectrum = self.audio.output_spectrum();
        // The orb's size/brightness is driven by mic level while listening and by
        // the output spectrum energy while Lo is speaking (matches the renderer feel).
        let speaking = self.session.state == LoState::Speaking || self.audio.is_playing();
        let level = if speaking {
            (spectrum.iter().sum::<f32>() / spectrum.len() as f32).clamp(0.0, 1.0)
        } else {
            self.audio.input_level()
        };

        let caps = Captions {
            you: self.session.you_text.clone(),
            lo: self.session.lo_text.clone(),
            fade: self.session.caption_fade(),
        };
        gui.set_state(self.session.state);
        if let Err(e) = gui.render(dt, time, level, &spectrum, &caps) {
            tracing::warn!("render error: {e:#}");
        }
    }
}

impl ApplicationHandler<AppEvent> for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.window.is_some() {
            return;
        }
        let attrs = Window::default_attributes()
            .with_title("Lo")
            .with_decorations(false)
            .with_transparent(true)
            .with_inner_size(LogicalSize::new(1120.0, 720.0))
            .with_min_inner_size(LogicalSize::new(420.0, 360.0));
        let window = match event_loop.create_window(attrs) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                tracing::error!("window creation failed: {e}");
                event_loop.exit();
                return;
            }
        };
