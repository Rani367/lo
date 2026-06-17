// Lo — the living core.
//
// A single organic body of warm light: domain-warped fractal noise forms a
// fluid metaball, lit with a warm peach→coral→rose→violet ramp, wrapped in
// additive bloom and a drift of embers over a deep dawn field. State changes ease
// smoothly (the host interpolates the uniforms); the body reveals over ~1.1s on
// launch (`reveal`). `level` (0..1) inflates and brightens the body.
//
// RENDERING NOTES:
//   * The full-screen triangle is generated from @builtin(vertex_index).
//   * @builtin(position).y is flipped so `frag` uses a bottom-left origin,
//     keeping the bg gradient, ember drift, and grain oriented as designed.
//   * The orb reacts to voice via `level` only; `spec` is kept in the uniform for
//     host-layout stability but is intentionally unread here.
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

struct Uniforms {
    res: vec2<f32>,
    time: f32,
    level: f32,
    intensity: f32,
    turb: f32,
    pulse: f32,
    breathe: f32,
    reveal: f32,
    lift: f32,
    _pad0: vec2<f32>,
    core: vec4<f32>,
    mid: vec4<f32>,
    edge: vec4<f32>,
    bg: vec4<f32>,
    spec: array<vec4<f32>, 4>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

// Full-screen triangle: vertex 0 → (-1,-1), 1 → (-1, 3), 2 → (3, -1).
@vertex
fn vs_main(@builtin(vertex_index) vid: u32) -> @builtin(position) vec4<f32> {
    let x = select(-1.0, 3.0, vid == 2u);
    let y = select(-1.0, 3.0, vid == 1u);
    return vec4<f32>(x, y, 0.0, 1.0);
}

fn hash21(p_in: vec2<f32>) -> f32 {
    var p = fract(p_in * vec2<f32>(123.34, 345.45));
    p = p + dot(p, p + 34.345);
    return fract(p.x * p.y);
}

fn vnoise(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let uu = f * f * (3.0 - 2.0 * f);
    let a = hash21(i);
    let b = hash21(i + vec2<f32>(1.0, 0.0));
    let c = hash21(i + vec2<f32>(0.0, 1.0));
    let d = hash21(i + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, uu.x), mix(c, d, uu.x), uu.y);
}

fn fbm(p_in: vec2<f32>) -> f32 {
    var s = 0.0;
    var a = 0.5;
    var p = p_in;
    // Rotate/scale between FBM octaves; column-major: columns (1.6,1.2) & (-1.2,1.6).
    let m = mat2x2<f32>(vec2<f32>(1.6, 1.2), vec2<f32>(-1.2, 1.6));
    for (var i = 0; i < 5; i = i + 1) {
        s = s + a * vnoise(p);
        p = m * p;
        a = a * 0.5;
    }
    return s;
}

@fragment
fn fs_main(@builtin(position) pos: vec4<f32>) -> @location(0) vec4<f32> {
    // Flip Y so `frag` uses a bottom-left origin (matches the header layout).
    let frag = vec2<f32>(pos.x, u.res.y - pos.y);
    let p = (frag - 0.5 * u.res) / u.res.y; // y-normalised, centred
    let t = u.time;
    let d = length(p);

    // domain-warped organic field + a finer internal flow
    let q = vec2<f32>(
        fbm(p * 1.6 + vec2<f32>(0.0, t * 0.06)),
        fbm(p * 1.6 + vec2<f32>(5.2, -t * 0.05)),
    );
    let n = fbm(p * 2.1 + q * (0.6 + u.turb) + t * 0.05);
    let flow = fbm(p * 3.4 + q * 1.7 - vec2<f32>(0.0, t * 0.13));

    var baseR = 0.25 * (1.0 + 0.05 * sin(t * 1.3) * u.breathe + 0.11 * u.level + u.pulse * 0.04 * sin(t * 2.1));
    baseR = baseR * mix(0.6, 1.0, u.reveal);

    // organic, breathing boundary (voice-reactive via `level` only)
    let bnd = baseR + (n - 0.5) * 0.22 * (0.5 + u.turb) + (fbm(p * 4.2 - vec2<f32>(t * 0.1, 0.0)) - 0.5) * 0.05;
    let body = smoothstep(bnd + 0.12, bnd - 0.05, d);

    // colour by radius, with flowing internal filaments
    let cm = clamp(d / max(bnd * 1.05, 0.001), 0.0, 1.0);
    var col = mix(u.core.xyz, u.mid.xyz, smoothstep(0.0, 0.62, cm + (flow - 0.5) * 0.5));
    col = mix(col, u.edge.xyz, smoothstep(0.5, 1.0, cm + (n - 0.5) * 0.15));
    let fil = smoothstep(0.5, 0.95, flow) * (1.0 - cm); // hot tendrils toward the centre
    col = col + u.core.xyz * fil * 0.45;
    col = col * body;

    // inner hot core
    let hot = smoothstep(bnd * 0.6, 0.0, d);
    col = col + u.core.xyz * hot * (0.3 + 0.7 * u.level);

    // additive bloom halo
    let halo = exp(-max(d - bnd, 0.0) * 6.5);
    let bloom = mix(u.mid.xyz, u.edge.xyz, 0.4) * halo * 0.75;

    var lit = (col + bloom) * u.intensity * (0.45 + 0.55 * u.reveal);

    // drifting embers (3 layers)
    var emb = 0.0;
    for (var i = 0; i < 3; i = i + 1) {
        let fi = f32(i);
        var gp = p * (3.0 + fi * 1.4);
        gp.y = gp.y + t * (0.05 + 0.025 * fi);
        let cell = floor(gp);
        let h = hash21(cell + fi * 7.1);
        let cc = fract(gp) - 0.5;
        let spark = smoothstep(0.08, 0.0, length(cc)) * step(0.968, h);
        emb = emb + spark * (0.5 + 0.5 * sin(t * 3.0 + h * 30.0));
    }
    lit = lit + mix(u.mid.xyz, u.core.xyz, 0.6) * emb * 0.6 * smoothstep(0.6, 0.12, d) * u.reveal;

    // background dawn field
    let bgGlow = exp(-d * 1.4) * 0.5;
    var bg = u.bg.xyz + mix(u.edge.xyz, u.mid.xyz, 0.3) * bgGlow * 0.22;
    bg = bg + u.bg.xyz * (1.0 - frag.y / u.res.y) * 0.25;
    // broad, smooth ambient lift so the upper field never crushes to a flat
    // near-black plateau on high-contrast panels (lift == 0.0 disables it).
    bg = bg + mix(u.mid.xyz, u.edge.xyz, 0.6) * exp(-d * 0.6) * u.lift;

    var outc = bg + lit;

    // fine grain + warm vignette
    outc = outc + (hash21(frag + fract(t) * vec2<f32>(13.0, 7.0)) - 0.5) * 0.018;
    let vig = smoothstep(1.3, 0.35, d);
    outc = outc * mix(0.8, 1.0, vig);

    return vec4<f32>(max(outc, vec3<f32>(0.0)), 1.0);
}
