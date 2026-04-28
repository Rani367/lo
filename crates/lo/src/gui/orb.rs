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

