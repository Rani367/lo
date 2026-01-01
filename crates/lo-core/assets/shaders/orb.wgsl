// Lo — the living core, ported from the WebGL2 GLSL ES 300 shader in
// `src/renderer/ui/core.ts` (Phase-0 Spike A).
//
// A single organic body of warm light: domain-warped fractal noise forms a
// fluid metaball, lit with a warm peach→coral→rose→violet ramp, wrapped in
// additive bloom and a drift of embers over a deep dawn field. State changes ease
// smoothly (the host interpolates the uniforms); the body reveals over ~1.1s on
// launch (`reveal`). `level` (0..1) inflates and brightens the body.
//
// PORTING NOTES (vs the GLSL original):
//   * The full-screen triangle comes from @builtin(vertex_index) (was gl_VertexID).
//   * gl_FragCoord (WebGL, bottom-left origin) → we flip @builtin(position).y so
//     `frag` matches the GLSL coordinate space exactly: the bg gradient, ember
//     drift, and grain are then 1:1 with the Electron build.
//   * The per-angle 16-band spectrum ripple was deliberately removed upstream (it
//     amplified into ray artifacts under the steep bloom). The orb reacts to voice
//     via `level` only; `spec` is kept in the uniform for host-layout parity but
//     is intentionally unread here.
//
// UNIFORM LAYOUT (R7 — std140-style; the host Rust struct must mirror this with
// #[repr(C)] + bytemuck and the SAME padding, or values silently corrupt):
//   offset  field
//      0    res    : vec2<f32>
//      8    time   : f32
//     12    level  : f32
//     16    intensity, turb, pulse, breathe : 4×f32
//     32    reveal, lift : 2×f32
//     40    _pad0  : vec2<f32>            (aligns the vec4 block to 48)
//     48    core   : vec4<f32>   (.xyz used)
//     64    mid    : vec4<f32>
//     80    edge   : vec4<f32>
//     96    bg     : vec4<f32>
//    112    spec   : array<vec4<f32>, 4>  (16 bands; currently unused)
//    -> total size 176 bytes.
