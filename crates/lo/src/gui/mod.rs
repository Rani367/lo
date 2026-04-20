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

