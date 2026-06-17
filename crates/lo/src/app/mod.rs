//! The winit `ApplicationHandler`: owns the window, the GPU (`Gui`), the cpal
//! `AudioEngine` (kept alive here on the main thread), and the UI-side
//! `Session` state machine. It is the orchestrator — it forwards input to the
//! worker/listen thread and renders the orb + captions each frame.

pub mod state;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};

use lo_core::types::LoState;
use tokio::sync::{mpsc::UnboundedSender, watch};
use winit::application::ApplicationHandler;
use winit::dpi::LogicalSize;
use winit::event::{ElementState, KeyEvent, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow};

/// Idle redraw cadence (~30 fps). While active (listening/thinking/speaking,
/// audio playing, or the boot reveal) we redraw at the display's full rate.
const IDLE_FRAME: Duration = Duration::from_millis(33);
/// The orb's boot-reveal duration; redraw at full rate until it finishes. Matches
/// the reveal easing in `gui::orb` so full-rate rendering lasts exactly the reveal.
const BOOT_SECONDS: f32 = 1.1;
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
    /// `--say "<text>"`: a synthetic transcript injected once the window is up.
    pub pending_say: Option<String>,
}

pub struct App {
    // Kept alive so the cpal streams keep running (AudioEngine is !Send and must
    // live on the main thread).
    _audio_engine: AudioEngine,
    audio: AudioHandle,
    ui_tx: UnboundedSender<UiCommand>,
    epoch_tx: watch::Sender<u64>,
    ptt_active: Arc<AtomicBool>,

    session: Session,
    gui: Option<Gui>,
    window: Option<Arc<Window>>,
    start: Instant,
    last_frame: Instant,
    /// Next scheduled idle redraw (used to cap idle rendering to ~30 fps).
    next_idle: Instant,
    pending_say: Option<String>,
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
            session: Session::new(),
            gui: None,
            window: None,
            start: now,
            last_frame: now,
            next_idle: now,
            pending_say: ctx.pending_say,
        }
    }

    /// Whether the scene needs full-rate (vsync) redraws right now. When false the
    /// orb's slow idle breathing/status pulse still animate, just throttled.
    fn animating(&self) -> bool {
        self.session.state != LoState::Idle
            || self.audio.is_playing()
            || self.start.elapsed().as_secs_f32() < BOOT_SECONDS
    }

    /// Space pressed/released → push-to-talk. Works in every activation mode as an
    /// override on top of hands-free; pressing it also barges in on any reply.
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
                let _ = self.ui_tx.send(UiCommand::Cancel);
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
            AppEvent::Transcribed { text } => {
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
        // the output spectrum energy while Lo is speaking.
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
        match Gui::new(window.clone()) {
            Ok(gui) => self.gui = Some(gui),
            Err(e) => {
                tracing::error!("GPU init failed: {e:#}");
                event_loop.exit();
                return;
            }
        }
        self.window = Some(window);
        // Boot → idle once the surface is live.
        self.session.set_state(LoState::Idle);
        self.set_gui_state();
        if let Some(w) = &self.window {
            w.request_redraw();
        }

        // `--say`: inject a synthetic transcript to drive one full turn (for
        // visual / end-to-end testing without a microphone).
        if let Some(text) = self.pending_say.take() {
            tracing::info!("injecting --say transcript: {text:?}");
            self.on_app_event(AppEvent::Transcribed { text });
        }
    }

    fn window_event(&mut self, event_loop: &ActiveEventLoop, _id: WindowId, event: WindowEvent) {
        // Let egui see the event first (for any interactive chrome).
        if let (Some(gui), Some(window)) = (&mut self.gui, &self.window) {
            let _consumed = gui.on_window_event(window, &event);
        }
        match event {
            WindowEvent::CloseRequested => {
                let _ = self.ui_tx.send(UiCommand::Shutdown);
                event_loop.exit();
            }
            WindowEvent::Resized(size) => {
                if let Some(gui) = &mut self.gui {
                    gui.resize(size.width, size.height);
                }
            }
            WindowEvent::KeyboardInput { event, .. } => self.on_key(&event),
            WindowEvent::RedrawRequested => self.render(),
            _ => {}
        }
    }

    fn user_event(&mut self, _event_loop: &ActiveEventLoop, event: AppEvent) {
        self.on_app_event(event);
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let Some(w) = &self.window else { return };
        if self.animating() {
            // Full rate: each redraw renders and loops straight back here; the
            // Fifo present mode caps it to the display refresh.
            w.request_redraw();
            event_loop.set_control_flow(ControlFlow::Wait);
        } else {
            // Idle: redraw only when the ~30 fps slot is due, then sleep until the
            // next one — keeps the idle orb alive without pinning the GPU.
            let now = Instant::now();
            if now >= self.next_idle {
                w.request_redraw();
                self.next_idle = now + IDLE_FRAME;
            }
            event_loop.set_control_flow(ControlFlow::WaitUntil(self.next_idle));
        }
    }
}
