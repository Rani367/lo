# Lo — pure-native Rust rewrite

A from-scratch rewrite of **Lo / J.A.R.V.I.S** (originally an Electron + TypeScript
local voice agent) into a **single pure-native Rust desktop app** — **no webview,
no HTML/JS, no Python**.

- **UI**: `winit` + `wgpu` (the "living core" orb shader ported to WGSL) + `egui`
  for captions/chrome.
- **Audio**: `cpal` duplex capture/playback, `rubato` resampling, `rustfft` spectrum.
- **On-device ML (all Rust)**: `whisper-rs` (whisper.cpp) for ASR, **GPU-accelerated
  per platform** (Metal + CoreML on macOS by default; CUDA/Vulkan opt-in on
  Linux/Windows) with automatic CPU fallback; Kokoro TTS (`kokoro-tts`) + Silero
  VAD via the `ort` ONNX runtime (CoreML EP on macOS); a hands-free **"Hey Jarvis"
  wake word** via openWakeWord (also `ort`, no API key). All feature-gated.
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
├── .github/workflows/
│   ├── ci.yml                       # 3-OS matrix: fmt + clippy + test + build (+ 3-OS ML build)
│   └── release.yml                  # tag-triggered: build + package per OS → draft Release
└── crates/
    ├── lo-core/                     # pure-logic core (no GPU/audio/ML deps)
    │   ├── assets/shaders/orb.wgsl  # orb shader, ported from the GLSL original
    │   └── src/ {types, config/, text, brain/, backends/, tools/, shaders}
    └── lo/                          # the binary: GUI + audio + ML + glue
        ├── assets/icons|macos|linux/  # app icons, macOS Info.plist (mic), Linux .desktop
        └── src/
            ├── main.rs · events.rs          # entry + threading; RAM model pick; graceful shutdown
            ├── app/ {mod, state}            # winit ApplicationHandler + turn/epoch state machine (idle-throttled)
            ├── gui/ {mod, orb, captions}    # wgpu orb pass + egui captions/chrome
            ├── audio/ {capture, playback, resample, spectrum}  # cpal duplex + rubato + rustfft
            ├── ml/ {asr, tts, vad, wakeword, download}         # whisper-rs / kokoro / ort silero + openWakeWord
            ├── brain/                       # reqwest SSE streaming client (retry/backoff)
            ├── backends/ {managed_server, download, …}         # process supervision + downloads
            ├── tools/ {web, files, shell, system, desktop, …}  # the 22 OS-action tool bodies
            ├── worker.rs                    # tokio agent loop + TTS thread + engine prewarm
            └── listen.rs                    # capture-draining ASR/VAD/wake thread
```

`lo-core` holds everything that needs **no** GPU/audio/ML native toolchain, so it
builds in seconds and is exhaustively unit-tested; the `lo` binary layers
winit/wgpu/cpal/whisper-rs/ort on top and drives it.

## Status — release-polished across the board

| Area | State |
|---|---|
| `lo-core` — config, brain loop, backends, tools, safety | ✅ **69 tests**, clippy/fmt clean |
| Brain: RAM-tiered model pick, sampling knobs, time-grounding, retry/backoff | ✅ |
| Orb shader + idle frame throttle (~30 fps idle, full rate when active) | ✅ |
| whisper ASR — per-platform GPU (Metal/CoreML/CUDA/Vulkan) + CPU fallback | ✅ |
| Kokoro TTS (CPU) + Silero VAD (tunable, CoreML EP) | ✅ |
| **"Hey Jarvis" wake word** (openWakeWord, hands-free → VAD capture) | ✅ |
| streaming brain client + backends + `ManagedServer` + engine prewarm | ✅ |
| 22 tool bodies (web/fs/shell/desktop/clipboard/system/media/timer + `copy_file`) | ✅ |
| Graceful shutdown (thread joins), audit-log rotation, self-hearing guard | ✅ |
| Packaging (`cargo-packager`) + tag-triggered release pipeline, 3-OS ML CI | ✅ |

**Verified:** `cargo build` (default = ML: whisper.cpp + ort + Kokoro + openWakeWord)
and `--no-default-features` both link; the full test suite passes; `cargo clippy
--workspace --all-targets -- -D warnings` is clean; `cargo fmt --check` is clean;
and `lo --smoke` reports the resolved model tier and every subsystem.

Code signing / notarization is intentionally deferred ("pipeline now, sign later"):
the release pipeline produces **unsigned** installers today, with documented secret
slots (`APPLE_*`, `WINDOWS_CERT_THUMBPRINT`) to enable real signing later.

## Develop

```bash
cargo test --workspace                       # fast core + bin tests (ML off)
cargo test -p lo                             # ML-on bin tests (whisper/ort/kokoro/wake)
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo fmt --check
cargo run -p lo -- --smoke                   # headless self-check: resolved tier + subsystems
cargo run -p lo -- --turn "what time is it"  # headless single agent turn (needs a brain server)
cargo run -p lo                              # launch the app (needs a brain server + mic permission)
```

Requires **Rust 1.85+**. Building with the default `ml` feature needs a C/C++
toolchain + **CMake** (whisper.cpp via `whisper-rs`). To build without the heavy
ML stack (clean stubs in its place): `cargo build --no-default-features`.

### Activation

- **Push-to-talk** (default): hold **Space**, speak, release.
- **VAD**: `"activationMode": "vad"` — auto-segments speech; Lo won't transcribe its
  own TTS while it's speaking.
- **Wake word**: `"activationMode": "wake"` — say **"Hey Jarvis"**, then your request;
  the utterance is auto-captured by the VAD. Models download on first use.

### Configuration

Settings live in `settings.json` in the OS config dir
(`~/Library/Application Support/com.lo.assistant/` on macOS, `~/.config/com.lo.assistant/`
on Linux, `%APPDATA%\com.lo.assistant\` on Windows), merged over defaults. Models
download once into the OS cache dir.

| Key | Default | Notes |
|---|---|---|
| `model` | `auto`* | Brain model; `auto`/default → **picked by RAM** (4B→30B). Or an explicit MLX id / GGUF ref / Ollama tag. |
| `backend` | `auto` | `auto` (MLX on Apple Silicon, else llama.cpp), `mlx`, `llama`, `ollama`, `custom`. |
| `temperature`·`topP`·`topK`·`repeatPenalty`·`minP` | 0.6·0.95·40·1.1·0 | Sampling (top-k/penalty/min-p sent to local backends only). |
| `voice`·`speechRate` | `af_heart`·1.15 | Kokoro voice + speed. |
| `activationMode` | `ptt` | `ptt`, `vad`, or `wake`. |
| `vadPositiveThreshold`·`vadNegativeThreshold`·`vadRedemptionMs` | 0.6·0.4·900 | VAD sensitivity. |
| `wakeThreshold` | 0.5 | "Hey Jarvis" score needed to trigger. |
| `powerUserMode` | `false` | Allows the confirm/danger tools (write/move/copy/delete/run). |
| `allowedFsRoots` | `[]` (→ home) | Folders the filesystem tools may touch. |

\* The shipped default is auto-tiered by RAM at startup; set an explicit `model` to override.

Env overrides (handy for triage): `LO_RAM_GB` (force the tier), `LO_WHISPER_CPU=1`
(force CPU ASR), `LO_VAD_POSITIVE`/`LO_VAD_NEGATIVE`/`LO_VAD_REDEMPTION_MS`,
`LO_NO_MIC` (run the GUI with no audio), `LO_LLM_URL`/`LO_LLM_MODEL`/`LO_LLM_KEY`
(custom endpoint), `LO_LOG` (tracing filter).

### Packaging & release

```bash
cargo install cargo-packager --locked
cd crates/lo
cargo packager --release -f dmg              # macOS → unsigned .dmg (Metal+CoreML baked in)
cargo packager --release -f deb,appimage     # Linux
cargo packager --release -f wix,nsis         # Windows
```

Tagging `vX.Y.Z` runs `.github/workflows/release.yml`, which builds + packages on
macOS (arm64 + x64), Windows, and Linux and uploads the installers to a **draft**
GitHub Release. Artifacts are unsigned until the signing secrets are filled in (see
the commented slots in `crates/lo/Cargo.toml` and the workflow `env:`).

### Troubleshooting

- **macOS "can't be opened" (unsigned):** right-click the app → **Open** the first
  time (or `xattr -dr com.apple.quarantine /Applications/Lo.app`).
- **No microphone prompt / capture fails:** the `.app` bundle carries
  `NSMicrophoneUsageDescription`; a bare `cargo run` binary won't — use `LO_NO_MIC`
  to launch the GUI without audio, or run the packaged app.
- **ASR slow / GPU issues:** set `LO_WHISPER_CPU=1` to force the CPU path.
- **First launch is slow:** the brain model + ASR/TTS/VAD/wake models download once
  (progress shows in the captions); they're cached for offline use after.
