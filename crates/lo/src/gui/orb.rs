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
