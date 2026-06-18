/* ===========================================================================
   Lo's living core — a faithful WebGL2 port of crates/lo-core/assets/shaders/
   orb.wgsl. Same domain-warped fractal body, warm peach→coral→rose→violet ramp,
   additive bloom, ember drift, and ~1.1s reveal. The palette and per-state
   presets are copied verbatim from crates/lo/src/gui/orb.rs.

   It exposes window.LoOrb.setState('idle'|'listening'|'thinking'|'speaking'|...)
   so main.js can ease the orb through Lo's real conversation states on scroll.
   =========================================================================== */
(function () {
  "use strict";

  var canvas = document.getElementById("orb");
  if (!canvas) return;

  var gl = canvas.getContext("webgl2", {
    antialias: false,
    alpha: false,
    depth: false,
    stencil: false,
    powerPreference: "low-power",
  });
  if (!gl) {
    document.body.classList.add("no-webgl");
    return;
  }

  var reduceMotion = window.matchMedia("(prefers-reduced-motion: reduce)").matches;

  /* ---- palette (linear 0..1) — gui/orb.rs ---- */
  var WARM_WHITE = [1.0, 0.93, 0.84];
  var PEACHWHITE = [1.0, 0.86, 0.93];
  var CORAL = [1.0, 0.49, 0.42];
  var ROSE = [1.0, 0.36, 0.56];
  var VIOLET = [0.55, 0.36, 0.96];
  var INDIGO = [0.3, 0.31, 0.62];
  var RED_DESAT = [0.96, 0.45, 0.4];
  var BG = [0.039, 0.027, 0.031];
  var ERR_CORE = [1.0, 0.7, 0.66];

  var FIELD_LIFT = 0.06;
  var DPR_CLAMP = 1.25;
  var BOOT_SECONDS = 1.1;

  /* ---- per-state presets — Vis::preset() in gui/orb.rs ---- */
  var PRESETS = {
    boot:      { intensity: 0.32, turb: 0.5,  pulse: 0.3,  breathe: 0.6,  gain: 0.2,  core: WARM_WHITE, mid: ROSE,     edge: VIOLET },
    idle:      { intensity: 0.62, turb: 0.55, pulse: 0.35, breathe: 1.0,  gain: 0.25, core: WARM_WHITE, mid: ROSE,     edge: VIOLET },
    listening: { intensity: 0.98, turb: 0.85, pulse: 0.5,  breathe: 0.4,  gain: 1.0,  core: WARM_WHITE, mid: CORAL,    edge: ROSE },
    thinking:  { intensity: 0.8,  turb: 1.3,  pulse: 0.8,  breathe: 0.3,  gain: 0.4,  core: PEACHWHITE, mid: VIOLET,   edge: INDIGO },
    speaking:  { intensity: 1.05, turb: 0.95, pulse: 0.55, breathe: 0.45, gain: 1.25, core: WARM_WHITE, mid: ROSE,     edge: VIOLET },
    error:     { intensity: 0.58, turb: 0.32, pulse: 0.2,  breathe: 0.7,  gain: 0.2,  core: ERR_CORE,   mid: RED_DESAT, edge: INDIGO },
  };

  /* ---------- shaders ---------- */

  var VERT = [
    "#version 300 es",
    "void main() {",
    "  float x = (gl_VertexID == 2) ? 3.0 : -1.0;",
    "  float y = (gl_VertexID == 1) ? 3.0 : -1.0;",
    "  gl_Position = vec4(x, y, 0.0, 1.0);",
    "}",
  ].join("\n");

  var FRAG = [
    "#version 300 es",
    "precision highp float;",
    "out vec4 fragColor;",
    "uniform vec2 u_res;",
    "uniform float u_time;",
    "uniform float u_level;",
    "uniform float u_intensity;",
    "uniform float u_turb;",
    "uniform float u_pulse;",
    "uniform float u_breathe;",
    "uniform float u_reveal;",
    "uniform float u_lift;",
    "uniform vec3 u_core;",
    "uniform vec3 u_mid;",
    "uniform vec3 u_edge;",
    "uniform vec3 u_bg;",
    "uniform vec2 u_offset;",
    "",
    "float hash21(vec2 p_in){",
    "  vec2 p = fract(p_in * vec2(123.34, 345.45));",
    "  p += dot(p, p + 34.345);",
    "  return fract(p.x * p.y);",
    "}",
    "float vnoise(vec2 p){",
    "  vec2 i = floor(p); vec2 f = fract(p);",
    "  vec2 uu = f*f*(3.0-2.0*f);",
    "  float a = hash21(i);",
    "  float b = hash21(i+vec2(1.0,0.0));",
    "  float c = hash21(i+vec2(0.0,1.0));",
    "  float d = hash21(i+vec2(1.0,1.0));",
    "  return mix(mix(a,b,uu.x), mix(c,d,uu.x), uu.y);",
    "}",
    "float fbm(vec2 p){",
    "  float s=0.0; float a=0.5;",
    "  mat2 m = mat2(1.6,1.2,-1.2,1.6);",
    "  for(int i=0;i<5;i++){ s += a*vnoise(p); p = m*p; a *= 0.5; }",
    "  return s;",
    "}",
    "void main(){",
    "  vec2 frag = gl_FragCoord.xy;",
    "  vec2 p = (frag - 0.5*u_res - u_offset) / u_res.y;",
    "  float t = u_time;",
    "  float d = length(p);",
    "  vec2 q = vec2(",
    "    fbm(p*1.6 + vec2(0.0, t*0.06)),",
    "    fbm(p*1.6 + vec2(5.2, -t*0.05)));",
    "  float n = fbm(p*2.1 + q*(0.6+u_turb) + t*0.05);",
    "  float flow = fbm(p*3.4 + q*1.7 - vec2(0.0, t*0.13));",
    "  float baseR = 0.25*(1.0 + 0.05*sin(t*1.3)*u_breathe + 0.11*u_level + u_pulse*0.04*sin(t*2.1));",
    "  baseR *= mix(0.6,1.0,u_reveal);",
    "  float bnd = baseR + (n-0.5)*0.22*(0.5+u_turb) + (fbm(p*4.2 - vec2(t*0.1,0.0))-0.5)*0.05;",
    "  float body = smoothstep(bnd+0.12, bnd-0.05, d);",
    "  float cm = clamp(d / max(bnd*1.05, 0.001), 0.0, 1.0);",
    "  vec3 col = mix(u_core, u_mid, smoothstep(0.0,0.62, cm + (flow-0.5)*0.5));",
    "  col = mix(col, u_edge, smoothstep(0.5,1.0, cm + (n-0.5)*0.15));",
    "  float fil = smoothstep(0.5,0.95,flow)*(1.0-cm);",
    "  col += u_core*fil*0.45;",
    "  col *= body;",
    "  float hot = smoothstep(bnd*0.6, 0.0, d);",
    "  col += u_core*hot*(0.3+0.7*u_level);",
    "  float halo = exp(-max(d-bnd,0.0)*6.5);",
    "  vec3 bloom = mix(u_mid,u_edge,0.4)*halo*0.75;",
    "  vec3 lit = (col+bloom)*u_intensity*(0.45+0.55*u_reveal);",
    "  float emb = 0.0;",
    "  for(int i=0;i<3;i++){",
    "    float fi = float(i);",
    "    vec2 gp = p*(3.0+fi*1.4);",
    "    gp.y += t*(0.05+0.025*fi);",
    "    vec2 cell = floor(gp);",
    "    float h = hash21(cell + fi*7.1);",
    "    vec2 cc = fract(gp)-0.5;",
    "    float spark = smoothstep(0.08,0.0,length(cc))*step(0.968,h);",
    "    emb += spark*(0.5+0.5*sin(t*3.0+h*30.0));",
    "  }",
    "  lit += mix(u_mid,u_core,0.6)*emb*0.6*smoothstep(0.6,0.12,d)*u_reveal;",
    "  float bgGlow = exp(-d*1.4)*0.5;",
    "  vec3 bg = u_bg + mix(u_edge,u_mid,0.3)*bgGlow*0.22;",
    "  bg += u_bg*(1.0 - frag.y/u_res.y)*0.25;",
    "  bg += mix(u_mid,u_edge,0.6)*exp(-d*0.6)*u_lift;",
    "  vec3 outc = bg + lit;",
    "  outc += (hash21(frag + fract(t)*vec2(13.0,7.0))-0.5)*0.018;",
    "  float vig = smoothstep(1.3,0.35,d);",
    "  outc *= mix(0.8,1.0,vig);",
    "  fragColor = vec4(max(outc, vec3(0.0)), 1.0);",
    "}",
  ].join("\n");

  function compile(type, src) {
    var sh = gl.createShader(type);
    gl.shaderSource(sh, src);
    gl.compileShader(sh);
    if (!gl.getShaderParameter(sh, gl.COMPILE_STATUS)) {
      console.error("orb shader error:", gl.getShaderInfoLog(sh));
      return null;
    }
    return sh;
  }

  var vs = compile(gl.VERTEX_SHADER, VERT);
  var fs = compile(gl.FRAGMENT_SHADER, FRAG);
  if (!vs || !fs) {
    document.body.classList.add("no-webgl");
    return;
  }
  var prog = gl.createProgram();
  gl.attachShader(prog, vs);
  gl.attachShader(prog, fs);
  gl.linkProgram(prog);
  if (!gl.getProgramParameter(prog, gl.LINK_STATUS)) {
    console.error("orb link error:", gl.getProgramInfoLog(prog));
    document.body.classList.add("no-webgl");
    return;
  }
  gl.useProgram(prog);

  var U = {};
  ["u_res","u_time","u_level","u_intensity","u_turb","u_pulse","u_breathe",
   "u_reveal","u_lift","u_core","u_mid","u_edge","u_bg","u_offset"].forEach(function (n) {
    U[n] = gl.getUniformLocation(prog, n);
  });

  var vao = gl.createVertexArray();
  gl.bindVertexArray(vao);

  /* ---------- eased visual state ---------- */

  function clonePreset(p) {
    return {
      intensity: p.intensity, turb: p.turb, pulse: p.pulse, breathe: p.breathe,
      gain: p.gain, core: p.core.slice(), mid: p.mid.slice(), edge: p.edge.slice(),
    };
  }

  var cur = clonePreset(PRESETS.boot);
  var target = PRESETS.idle;
  var stateName = "idle";
  var level = 0.0;
  var bootT = reduceMotion ? 1.0 : 0.0;

  // base offset lifts the core into the upper third so hero text sits below it
  var baseOffX = 0, baseOffY = 0;
  var parX = 0, parY = 0, parTX = 0, parTY = 0; // parallax (eased)

  function easeColor(c, tgt, k) {
    c[0] += (tgt[0] - c[0]) * k;
    c[1] += (tgt[1] - c[1]) * k;
    c[2] += (tgt[2] - c[2]) * k;
  }
  function easeToward(k) {
    cur.intensity += (target.intensity - cur.intensity) * k;
    cur.turb += (target.turb - cur.turb) * k;
    cur.pulse += (target.pulse - cur.pulse) * k;
    cur.breathe += (target.breathe - cur.breathe) * k;
    cur.gain += (target.gain - cur.gain) * k;
    easeColor(cur.core, target.core, k);
    easeColor(cur.mid, target.mid, k);
    easeColor(cur.edge, target.edge, k);
  }

  // A synthetic "voice level" so the orb feels alive without microphone access:
  // a layered, syllable-like envelope while listening/speaking; near-still idle.
  function levelFor(name, t) {
    if (name === "listening") {
      var s = 0.5 + 0.5 * Math.sin(t * 6.5);
      var g = 0.5 + 0.5 * Math.sin(t * 11.3 + 1.7);
      return 0.22 + 0.6 * (0.55 * s + 0.45 * g * g);
    }
    if (name === "speaking") {
      var a = 0.5 + 0.5 * Math.sin(t * 8.2);
      var b = 0.5 + 0.5 * Math.sin(t * 3.1 + 0.6);
      var c = 0.5 + 0.5 * Math.sin(t * 13.0 + 2.2);
      return 0.28 + 0.62 * (0.5 * a * b + 0.5 * c * c);
    }
    if (name === "thinking") return 0.12 + 0.05 * Math.sin(t * 1.6);
    return 0.045 + 0.03 * Math.sin(t * 0.9); // idle / boot / error
  }

  function easeOutCubic(x) {
    var inv = 1.0 - x;
    return 1.0 - inv * inv * inv;
  }

  /* ---------- sizing ---------- */

  var W = 1, H = 1;
  function resize() {
    var dpr = Math.min(window.devicePixelRatio || 1, DPR_CLAMP);
    var cw = canvas.clientWidth || window.innerWidth;
    var ch = canvas.clientHeight || window.innerHeight;
    W = Math.max(1, Math.round(cw * dpr));
    H = Math.max(1, Math.round(ch * dpr));
    if (canvas.width !== W || canvas.height !== H) {
      canvas.width = W;
      canvas.height = H;
    }
    gl.viewport(0, 0, W, H);
    baseOffX = 0;
    baseOffY = 0.12 * H; // push the core ~12% up the viewport
  }
  resize();
  window.addEventListener("resize", function () {
    resize();
    if (reduceMotion) renderOnce();
  });

  /* ---------- pointer parallax ---------- */
  if (!reduceMotion) {
    window.addEventListener("pointermove", function (e) {
      var dpr = Math.min(window.devicePixelRatio || 1, DPR_CLAMP);
      var nx = (e.clientX / window.innerWidth - 0.5);
      var ny = (e.clientY / window.innerHeight - 0.5);
      parTX = -nx * 36 * dpr;
      parTY = ny * 36 * dpr; // GL y is bottom-up; invert
    }, { passive: true });
  }

  /* ---------- public API ---------- */
  window.LoOrb = {
    setState: function (name) {
      if (PRESETS[name]) {
        target = PRESETS[name];
        stateName = name;
      }
    },
    reduceMotion: reduceMotion,
  };

  /* ---------- draw ---------- */

  function setUniforms(time) {
    gl.uniform2f(U.u_res, W, H);
    gl.uniform1f(U.u_time, time);
    gl.uniform1f(U.u_level, level);
    gl.uniform1f(U.u_intensity, cur.intensity);
    gl.uniform1f(U.u_turb, cur.turb);
    gl.uniform1f(U.u_pulse, cur.pulse);
    gl.uniform1f(U.u_breathe, cur.breathe);
    gl.uniform1f(U.u_reveal, easeOutCubic(bootT));
    gl.uniform1f(U.u_lift, FIELD_LIFT);
    gl.uniform3fv(U.u_core, cur.core);
    gl.uniform3fv(U.u_mid, cur.mid);
    gl.uniform3fv(U.u_edge, cur.edge);
    gl.uniform3fv(U.u_bg, BG);
    gl.uniform2f(U.u_offset, baseOffX + parX, baseOffY + parY);
  }

  function renderOnce() {
    // static frame for reduced-motion / context restore
    cur = clonePreset(PRESETS.idle);
    level = 0.05;
    bootT = 1.0;
    setUniforms(2.4);
    gl.drawArrays(gl.TRIANGLES, 0, 3);
  }

  var raf = 0;
  var last = 0;

  function frame(now) {
    var t = now * 0.001;
    var dt = last ? Math.min(0.05, (now - last) / 1000) : 0.016;
    last = now;

    bootT = Math.min(1.0, bootT + dt / BOOT_SECONDS);
    easeToward(Math.min(1.0, dt * 4.0));

    var lt = levelFor(stateName, t);
    level += (lt - level) * Math.min(1.0, dt * 9.0);

    parX += (parTX - parX) * Math.min(1.0, dt * 3.0);
    parY += (parTY - parY) * Math.min(1.0, dt * 3.0);

    setUniforms(t);
    gl.drawArrays(gl.TRIANGLES, 0, 3);

    raf = requestAnimationFrame(frame);
  }

  function start() {
    if (raf) return;
    last = 0;
    raf = requestAnimationFrame(frame);
  }
  function stop() {
    if (raf) cancelAnimationFrame(raf);
    raf = 0;
  }

  document.addEventListener("visibilitychange", function () {
    if (document.hidden) stop();
    else if (!reduceMotion) start();
  });

  canvas.addEventListener("webglcontextlost", function (e) {
    e.preventDefault();
    stop();
  });

  if (reduceMotion) {
    renderOnce();
  } else {
    start();
  }
})();
