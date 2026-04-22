//! The GUI subsystem — Lo's whole face.
//!
//! A single full-bleed wgpu surface renders the "living core" orb ([`orb`]) and,
//! composited on top, an egui pass draws the live captions ([`captions`]) plus the
//! minimal chrome (the "lo" wordmark + state dot, and the "hold space to talk"
//! hint) ported from `index.html` / `styles.css`.
//!
//! The orchestrator's event loop owns the [`winit::window::Window`] (and its
//! transparent/frameless styling) and drives this module through the public API:
//! [`Gui::new`], [`Gui::resize`], [`Gui::set_state`], [`Gui::on_window_event`],
//! and [`Gui::render`].

pub mod captions;
pub mod orb;

pub use captions::Captions;
pub use orb::Orb;

use std::sync::Arc;

use anyhow::{anyhow, Context as _};
use lo_core::types::LoState;
use winit::window::Window;

use crate::gui::orb::{DPR_CLAMP, SPEC_BANDS};

/// All GPU + egui state for the window. Created once after the window exists.
pub struct Gui {
    // The window we render into (kept so `render` can pump egui-winit input
    // without the orchestrator threading it through every call).
    window: Arc<Window>,

    // --- wgpu ---
    surface: wgpu::Surface<'static>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,

    // --- subsystems ---
    orb: Orb,

    // --- egui ---
    egui_ctx: egui::Context,
    egui_state: egui_winit::State,
    egui_renderer: egui_wgpu::Renderer,

    // current UI state (drives the chrome accent + dot speed)
    state: LoState,
}

impl Gui {
    /// Build the full GPU + egui stack for `window`.
    ///
    /// Creates a wgpu `Instance` → `Surface` (from the `Arc<Window>`) →
    /// `Adapter`/`Device`/`Queue` (via `pollster::block_on`), configures an
    /// alpha-capable surface, builds the orb pipeline from `ORB_WGSL`, and inits
    /// egui (`Context`, `egui_winit::State`, `egui_wgpu::Renderer`).
    pub fn new(window: Arc<Window>) -> anyhow::Result<Gui> {
        let size = window.inner_size();
        let width = size.width.max(1);
        let height = size.height.max(1);

        let instance = wgpu::Instance::new(&wgpu::InstanceDescriptor::default());

        // The Arc<Window> satisfies `Into<SurfaceTarget<'static>>`, giving a
        // 'static surface that owns its window handle.
        let surface = instance
            .create_surface(window.clone())
            .context("create wgpu surface from window")?;

        let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            force_fallback_adapter: false,
            compatible_surface: Some(&surface),
        }))
        .map_err(|e| anyhow!("no compatible wgpu adapter: {e}"))?;

        let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
            label: Some("lo-device"),
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::downlevel_defaults(),
            memory_hints: wgpu::MemoryHints::default(),
            trace: wgpu::Trace::Off,
        }))
        .context("request wgpu device")?;

        // Configure the surface. Prefer an sRGB format and an alpha-capable
        // composite mode so the orchestrator's transparent/frameless window can
        // blend over the desktop where supported.
        let caps = surface.get_capabilities(&adapter);
        let format = caps
            .formats
            .iter()
            .copied()
            .find(|f| f.is_srgb())
            .unwrap_or_else(|| {
                caps.formats
                    .first()
                    .copied()
                    .unwrap_or(wgpu::TextureFormat::Bgra8UnormSrgb)
            });

        let alpha_mode = pick_alpha_mode(&caps.alpha_modes);

        let config = wgpu::SurfaceConfiguration {
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
            format,
            width,
            height,
            present_mode: wgpu::PresentMode::Fifo,
            desired_maximum_frame_latency: 2,
            alpha_mode,
            view_formats: vec![],
        };
        surface.configure(&device, &config);

        let orb = Orb::new(&device, format);

        // --- egui ---
        let egui_ctx = egui::Context::default();
        let egui_state = egui_winit::State::new(
            egui_ctx.clone(),
            egui::ViewportId::ROOT,
            window.as_ref(),
            Some(window.scale_factor() as f32),
            None,
            None,
        );
        // egui draws with premultiplied alpha; dithering off for the dark field.
        let egui_renderer = egui_wgpu::Renderer::new(&device, format, None, 1, false);

        Ok(Gui {
            window,
            surface,
            device,
            queue,
            config,
            orb,
            egui_ctx,
            egui_state,
            egui_renderer,
            state: LoState::Boot,
        })
    }

    /// Reconfigure the surface to a new size. Zero sizes are ignored.
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
    }

    /// Set the target visual preset for the orb (and the chrome accent).
    pub fn set_state(&mut self, state: LoState) {
        self.state = state;
        self.orb.set_state(state);
    }

    /// Forward a window event to egui. Returns whether egui consumed it (so the
    /// orchestrator can skip its own handling of consumed events).
    pub fn on_window_event(&mut self, window: &Window, event: &winit::event::WindowEvent) -> bool {
        self.egui_state.on_window_event(window, event).consumed
    }

    /// Render one frame: ease the orb, draw it (clearing to the dark bg), then an
    /// egui pass (load) drawing captions + chrome. Submits once and presents.
