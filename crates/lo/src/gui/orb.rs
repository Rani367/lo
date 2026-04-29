//! The living core — a full-screen WGSL fragment shader rendered with wgpu.
//!
//! Ported from the WebGL2 build in `src/renderer/ui/core.ts`: a single organic
//! body of warm light over a deep dawn field. The GPU shader source lives in
//! [`lo_core::shaders::ORB_WGSL`]; this module owns the host-side render pipeline,
//! the eased visual state ([`Vis`]), and the uniform buffer whose byte layout must
//! mirror the shader's `Uniforms` struct EXACTLY (see [`Uniforms`]).

use bytemuck::{Pod, Zeroable};
use lo_core::types::LoState;
use wgpu::util::DeviceExt;

/// A linear-space RGB triple (0..1), matching the GLSL `vec3` palette entries.
pub type Rgb = [f32; 3];

// ---------------------------------------------------------------------------
// Warm "dawn" palette (linear 0..1) — ported 1:1 from `core.ts`.
// ---------------------------------------------------------------------------

/// Hot near-white centre.
pub const WARM_WHITE: Rgb = [1.0, 0.93, 0.84];
/// Soft peach-white (used for `thinking` core).
pub const PEACHWHITE: Rgb = [1.0, 0.86, 0.93];
/// Coral mid.
pub const CORAL: Rgb = [1.0, 0.49, 0.42];
/// Rose mid/edge.
pub const ROSE: Rgb = [1.0, 0.36, 0.56];
/// Violet edge.
pub const VIOLET: Rgb = [0.55, 0.36, 0.96];
/// Indigo edge (cool states).
pub const INDIGO: Rgb = [0.3, 0.31, 0.62];
/// Desaturated red (error mid).
pub const RED_DESAT: Rgb = [0.96, 0.45, 0.4];
/// Deep dawn background.
pub const BG: Rgb = [0.039, 0.027, 0.031];

/// Number of folded spectrum bands fed into the uniform (kept for host parity;
/// the shader does not currently read them).
pub const SPEC_BANDS: usize = 16;

/// Broad ambient lift on the dark field — see the long comment in `core.ts`. It
/// keeps the upper field a dim glow so high-contrast panels don't crush it into a
/// hard black rectangle. `0.0` is the original look.
pub const FIELD_LIFT: f32 = 0.06;

/// Clamp the device-pixel-ratio so 4K/Retina panels don't over-render the field.
pub const DPR_CLAMP: f64 = 1.25;

/// How long (seconds) the body takes to bloom up on launch (`reveal`).
const BOOT_SECONDS: f32 = 1.1;

/// The eased visual parameters for a single render frame. State changes ease the
/// `cur` toward the `target` preset; the shader reads these every frame.
#[derive(Debug, Clone, Copy)]
pub struct Vis {
    /// Overall brightness multiplier.
    pub intensity: f32,
    /// Boundary turbulence.
    pub turb: f32,
    /// Heartbeat amount.
    pub pulse: f32,
    /// Slow idle breath.
    pub breathe: f32,
    /// How hard audio drives the surface (folded into the spectrum; host-side only).
    pub gain: f32,
    /// Hot centre colour.
    pub core: Rgb,
    /// Body colour.
    pub mid: Rgb,
    /// Rim / halo colour.
    pub edge: Rgb,
}

impl Vis {
    /// The preset for a given UI state (the 6 `STATES` entries from `core.ts`).
    pub fn preset(state: LoState) -> Vis {
        match state {
            LoState::Boot => Vis {
                intensity: 0.32,
                turb: 0.5,
                pulse: 0.3,
                breathe: 0.6,
                gain: 0.2,
                core: WARM_WHITE,
                mid: ROSE,
                edge: VIOLET,
            },
            LoState::Idle => Vis {
                intensity: 0.62,
                turb: 0.55,
                pulse: 0.35,
                breathe: 1.0,
                gain: 0.25,
                core: WARM_WHITE,
                mid: ROSE,
                edge: VIOLET,
            },
            LoState::Listening => Vis {
                intensity: 0.98,
                turb: 0.85,
                pulse: 0.5,
                breathe: 0.4,
                gain: 1.0,
                core: WARM_WHITE,
                mid: CORAL,
                edge: ROSE,
            },
            LoState::Thinking => Vis {
                intensity: 0.8,
                turb: 1.3,
                pulse: 0.8,
                breathe: 0.3,
                gain: 0.4,
                core: PEACHWHITE,
                mid: VIOLET,
                edge: INDIGO,
            },
            LoState::Speaking => Vis {
                intensity: 1.05,
                turb: 0.95,
                pulse: 0.55,
                breathe: 0.45,
                gain: 1.25,
                core: WARM_WHITE,
                mid: ROSE,
                edge: VIOLET,
            },
            LoState::Error => Vis {
                intensity: 0.58,
                turb: 0.32,
                pulse: 0.2,
                breathe: 0.7,
                gain: 0.2,
                core: [1.0, 0.7, 0.66],
                mid: RED_DESAT,
                edge: INDIGO,
            },
        }
    }

    /// Ease `self` toward `target` by factor `k` (`k = (dt*4).min(1.0)` upstream).
    pub fn ease_toward(&mut self, target: &Vis, k: f32) {
        self.intensity += (target.intensity - self.intensity) * k;
        self.turb += (target.turb - self.turb) * k;
        self.pulse += (target.pulse - self.pulse) * k;
        self.breathe += (target.breathe - self.breathe) * k;
        self.gain += (target.gain - self.gain) * k;
        for i in 0..3 {
            self.core[i] += (target.core[i] - self.core[i]) * k;
            self.mid[i] += (target.mid[i] - self.mid[i]) * k;
            self.edge[i] += (target.edge[i] - self.edge[i]) * k;
        }
    }
}

