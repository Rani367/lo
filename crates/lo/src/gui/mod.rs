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
