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

/// Host mirror of the shader's `Uniforms` struct. The byte layout MUST match
/// `orb.wgsl` exactly (std140-style); `#[repr(C)]` + the explicit padding fields
/// below keep the vec4 block 16-byte aligned. Total size is asserted to be 176.
///
/// ```text
///   0   res:vec2  8 time  12 level
///   16  intensity turb pulse breathe
///   32  reveal lift  40 _pad0:vec2
///   48  core:vec4  64 mid  80 edge  96 bg
///   112 spec: [vec4;4]   -> 176 bytes
/// ```
#[repr(C)]
#[derive(Debug, Clone, Copy, Pod, Zeroable)]
pub struct Uniforms {
    /// Render-target resolution in physical pixels.
    pub res: [f32; 2],
    /// Animation time (seconds).
    pub time: f32,
    /// Smoothed audio level (0..1).
    pub level: f32,
    /// Eased intensity.
    pub intensity: f32,
    /// Eased turbulence.
    pub turb: f32,
    /// Eased pulse.
    pub pulse: f32,
    /// Eased breathe.
    pub breathe: f32,
    /// Boot reveal (0..1, eased over ~1.1s).
    pub reveal: f32,
    /// Ambient field lift.
    pub lift: f32,
    /// Padding to align the vec4 block to offset 48.
    pub _pad0: [f32; 2],
    /// Hot centre colour (xyz used).
    pub core: [f32; 4],
    /// Body colour (xyz used).
    pub mid: [f32; 4],
    /// Rim colour (xyz used).
    pub edge: [f32; 4],
    /// Background colour (xyz used).
    pub bg: [f32; 4],
    /// 16 spectrum bands packed as 4×vec4 (currently unread by the shader).
    pub spec: [[f32; 4]; 4],
}

// Compile-time guarantee that the host struct matches the documented WGSL layout.
const _: () = assert!(core::mem::size_of::<Uniforms>() == 176);

impl Default for Uniforms {
    fn default() -> Self {
        Uniforms {
            res: [1.0, 1.0],
            time: 0.0,
            level: 0.0,
            intensity: 0.0,
            turb: 0.0,
            pulse: 0.0,
            breathe: 0.0,
            reveal: 0.0,
            lift: FIELD_LIFT,
            _pad0: [0.0, 0.0],
            core: [0.0; 4],
            mid: [0.0; 4],
            edge: [0.0; 4],
            bg: [BG[0], BG[1], BG[2], 1.0],
            spec: [[0.0; 4]; 4],
        }
    }
