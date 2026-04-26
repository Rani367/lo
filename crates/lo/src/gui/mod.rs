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
    ///
    /// On a lost/outdated surface the surface is reconfigured and the frame is
    /// skipped (returning `Ok`).
    pub fn render(
        &mut self,
        dt: f32,
        time: f32,
        level: f32,
        spectrum: &[f32; SPEC_BANDS],
        caps: &Captions,
    ) -> anyhow::Result<()> {
        let res = [self.config.width as f32, self.config.height as f32];
        self.orb.update(dt, time, level, spectrum, res);

        let frame = match self.surface.get_current_texture() {
            Ok(frame) => frame,
            Err(wgpu::SurfaceError::Lost) | Err(wgpu::SurfaceError::Outdated) => {
                // Reconfigure and skip; the next frame recovers.
                self.surface.configure(&self.device, &self.config);
                return Ok(());
            }
            Err(wgpu::SurfaceError::Timeout) => return Ok(()),
            Err(e) => return Err(anyhow!("acquire surface texture: {e}")),
        };

        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        self.orb.upload(&self.queue);

        let mut encoder = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("lo-encoder"),
            });

        // --- orb pass: clear to the dark dawn bg, draw the full-screen triangle.
        {
            let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("orb-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: orb::BG[0] as f64,
                            g: orb::BG[1] as f64,
                            b: orb::BG[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            self.orb.draw(&mut pass);
        }

        // --- egui pass: build UI, tessellate, upload, then draw on LOAD.
        // Pull accumulated input from egui-winit (sets screen rect + ppp).
        let mut raw_input = self.egui_state.take_egui_input(self.window.as_ref());
        // Honour the DPR clamp (1.25) so high-density panels don't over-render.
        if let Some(ppp) = raw_input.viewport().native_pixels_per_point {
            let clamped = (ppp as f64).min(DPR_CLAMP) as f32;
            let vid = raw_input.viewport_id;
            if let Some(v) = raw_input.viewports.get_mut(&vid) {
                v.native_pixels_per_point = Some(clamped);
            }
            self.egui_ctx.set_pixels_per_point(clamped);
        }

        let state = self.state;
        let full_output = self.egui_ctx.run(raw_input, |ctx| {
            draw_chrome(ctx, state);
            captions::draw(ctx, caps);
        });

        // Apply egui's platform output (cursor, clipboard, etc.).
        self.egui_state
            .handle_platform_output(self.window.as_ref(), full_output.platform_output);

        let clipped = self
            .egui_ctx
            .tessellate(full_output.shapes, full_output.pixels_per_point);

        for (id, image_delta) in &full_output.textures_delta.set {
            self.egui_renderer
                .update_texture(&self.device, &self.queue, *id, image_delta);
        }

        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [self.config.width, self.config.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        let user_cmds = self.egui_renderer.update_buffers(
            &self.device,
            &self.queue,
            &mut encoder,
            &clipped,
            &screen_descriptor,
        );

        {
            let pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("egui-pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            // egui's renderer needs a 'static render pass in 0.32.
            let mut pass = pass.forget_lifetime();
            self.egui_renderer
                .render(&mut pass, &clipped, &screen_descriptor);
        }

        for id in &full_output.textures_delta.free {
            self.egui_renderer.free_texture(id);
        }

        // Submit egui's staging copies first, then our encoder, then present.
        self.queue.submit(
            user_cmds
                .into_iter()
                .chain(std::iter::once(encoder.finish())),
        );
        frame.present();

        Ok(())
    }
}

/// Pick the most-transparent-capable composite alpha mode the surface supports,
/// so a frameless/transparent host window can blend over the desktop. Falls back
/// to `Opaque` (always supported) when nothing better is available.
fn pick_alpha_mode(modes: &[wgpu::CompositeAlphaMode]) -> wgpu::CompositeAlphaMode {
    use wgpu::CompositeAlphaMode::*;
    for preferred in [PreMultiplied, PostMultiplied, Inherit, Auto] {
        if modes.contains(&preferred) {
            return preferred;
        }
    }
    Opaque
}

// --- chrome (wordmark + state dot + hint), ported from index.html / styles.css.

/// Per-state accent colour (the `--accent` CSS var that re-tints the dot/hint).
fn accent(state: LoState) -> egui::Color32 {
    use egui::Color32;
