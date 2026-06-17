# Changelog

All notable changes to Lo are documented here. The format is based on
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Lo follows
[Semantic Versioning](https://semver.org/spec/v2.0.0.html).

## [1.0.0] — 2026-06-17

The first stable release of Lo — a fast, fully-local, pure-native Rust voice
agent. No cloud, no webview, no Python.

### Added
- **Hands-free voice** by default: a Silero VAD segmenter listens continuously and
  ends a turn when you stop talking. Lo's own speech is suppressed so it never
  transcribes itself.
- **Push-to-talk**: hold **Space** at any time as an override on top of hands-free,
  or set `activationMode: "ptt"` for push-to-talk only.
- **On-device speech-to-text** via whisper.cpp, GPU-accelerated per platform
  (Metal + CoreML on macOS by default; CUDA/Vulkan/HIP opt-in elsewhere) with an
  automatic CPU fallback.
- **On-device text-to-speech** via Kokoro, and **voice-activity detection** via
  Silero, both running through the ONNX Runtime.
- **Local LLM orchestration** behind one selector: `mlx_lm server` on Apple
  Silicon, an auto-downloaded `llama-server` elsewhere, Ollama, or a custom
  OpenAI-compatible URL — with streaming native tool calls.
- **Memory-tiered model selection**: leaving `model` on its default picks a
  Qwen3 tier (4B → 30B-A3B) sized to the machine's RAM.
- **22 OS-action tools** (web search/fetch, files, shell, system info, app/media
  control, clipboard, screenshots, timers) behind a three-tier safety gate; the
  confirm/danger tiers require `powerUserMode`, and every gated call is audit-logged.
- **The "living core" orb** rendered with `wgpu` (WGSL shader) plus `egui`
  captions, with idle frame-rate throttling.
- **Packaging** via `cargo-packager` (.dmg, .msi/.nsis, .deb/.appimage) and a
  tag-triggered GitHub Actions release pipeline.

### Security
- SSRF guard with case-insensitive IPv4-mapped-IPv6 classification (RFC 5952),
  DNS re-validation of every resolved record, and per-hop redirect checks.
- Filesystem tools are sandboxed to user-configured roots (home by default).
- `run_command` is argv-only (no shell parsing), time-limited, and output-capped.

[1.0.0]: https://github.com/Rani367/lo/releases/tag/v1.0.0
