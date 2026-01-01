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
