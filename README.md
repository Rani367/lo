# Lo — pure-native Rust rewrite

A from-scratch rewrite of **Lo / J.A.R.V.I.S** (originally an Electron + TypeScript
local voice agent) into a **single pure-native Rust desktop app** — **no webview,
no HTML/JS, no Python**.

- **UI**: `winit` + `wgpu` (the "living core" orb shader ported to WGSL) + `egui`
  for captions/chrome.
- **Audio**: `cpal` duplex capture/playback, `rubato` resampling, `rustfft` spectrum.
- **On-device ML (all Rust)**: `whisper-rs` (whisper.cpp) for ASR — one path that
  replaces *both* the old MLX Parakeet Python sidecar and the transformers.js
  Whisper; Kokoro TTS (`kokoro-tts`) + Silero VAD via the `ort` ONNX runtime;
  Porcupine wake word behind a trait (vendored later). All feature-gated.
- **Brain (LLM)**: the orchestration model is preserved — Rust spawns/queries
  OpenAI-compatible servers (`mlx_lm server` on Apple Silicon, an auto-downloaded
  `llama-server` elsewhere, Ollama, or a custom URL) behind one selector, streaming
  native tool-calls.

The full design and phased roadmap live in the master plan:
`~/.claude/plans/completely-rewrite-this-project-quiet-river.md`.

## Workspace layout

```
lo/
├── Cargo.toml                       # workspace
├── assets/icons|macos/              # app icons + macOS Info.plist (mic permission)
├── .github/workflows/ci.yml         # 3-OS matrix: fmt + clippy + test + build (+ macOS ML build)
└── crates/
    ├── lo-core/                     # pure-logic core (no GPU/audio/ML deps)
    │   ├── assets/shaders/orb.wgsl  # orb shader, ported from the GLSL original
    │   └── src/ {types, config/, text, brain/, backends/, tools/, shaders}
    └── lo/                          # the binary: GUI + audio + ML + glue
        └── src/
            ├── main.rs · events.rs          # entry + threading; the worker↔UI message contract
            ├── app/ {mod, state}            # winit ApplicationHandler + the turn/epoch state machine
            ├── gui/ {mod, orb, captions}    # wgpu orb pass + egui captions/chrome
            ├── audio/ {capture, playback, resample, spectrum}  # cpal duplex + rubato + rustfft
            ├── ml/ {asr, tts, vad, wakeword, download}         # whisper-rs / kokoro / ort silero
            ├── brain/                       # reqwest SSE streaming client
            ├── backends/ {managed_server, download, …}         # process supervision + downloads
            ├── tools/ {web, files, shell, system, desktop, …}  # the 21 OS-action tool bodies
            ├── worker.rs                    # tokio agent loop + TTS thread
            └── listen.rs                    # capture-draining ASR/VAD/wake thread
```

`lo-core` holds everything that needs **no** GPU/audio/ML native toolchain, so it
builds in seconds and is exhaustively unit-tested; the `lo` binary layers
winit/wgpu/cpal/whisper-rs/ort on top and drives it.

## Status — the full app builds, runs its self-check, and is green

| Area | State |
|---|---|
| `lo-core` — config, brain loop, backends, tools, safety | ✅ **64 tests**, clippy/fmt clean |
| Orb shader (GLSL → WGSL) | ✅ ported + naga-validated |
| `lo` binary — winit/wgpu window + orb + state machine | ✅ builds |
| cpal audio (capture/playback/resample/spectrum) | ✅ builds + tests (**13** bin tests) |
| whisper-rs ASR + Kokoro TTS + Silero VAD (+ wake-word trait) | ✅ builds (feature-gated) |
| streaming brain client + backends + `ManagedServer` + downloads | ✅ builds |
| 21 tool bodies (web/fs/shell/desktop/clipboard/system/media/timer) | ✅ builds |
| Packaging config + 3-OS CI | ✅ scaffolded |

**Verified:** `cargo build` (default = ML: whisper.cpp + ort + Kokoro) and
`--no-default-features` both link; **77 tests pass**; `cargo clippy --workspace
--all-targets -- -D warnings` is clean; `cargo fmt --check` is clean; and
`lo --smoke` initializes every subsystem headlessly.

Remaining toward a shipped 1.0 (later phases): a live end-to-end voice turn on
each OS, wake-word vendoring (Porcupine), packaging validation + notarization, and
orb/caption fidelity polish.

## Develop

```bash
cargo test --workspace                       # 77 tests
cargo clippy --workspace --all-targets -- -D warnings
cargo fmt --check
cargo run -p lo -- --smoke                   # headless subsystem self-check (no window)
cargo run -p lo                              # launch the app (needs a brain server + mic permission)
```

Requires **Rust 1.85+**. Building with the default `ml` feature needs a C/C++
toolchain + **CMake** (whisper.cpp via `whisper-rs`). To build without the heavy
ML stack (clean stubs in its place): `cargo build --no-default-features`.

### Configuration

Settings live in `settings.json` in the OS config dir
(`~/Library/Application Support/com.lo.assistant/` on macOS), merged over defaults,
with `LO_*` env overrides. E.g. to use an Ollama model instead of the default
MLX/llama path:

```json
{ "backend": "ollama", "model": "<your-ollama-tag>" }
```
