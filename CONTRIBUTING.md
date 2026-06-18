# Contributing to Lo

Thanks for your interest in Lo! This is a pure-native Rust desktop voice agent.
Contributions of all kinds (bug reports, fixes, features, docs) are welcome. Lo is
MIT-licensed; by contributing you agree your work is provided under the same
[license](LICENSE).

## Project layout

- **`crates/lo-core`** — pure, dependency-light logic (config, the brain agent
  loop + SSE parsing, backend selection + download-URL resolution + the RAM ladder,
  the tool registry + safety gate + SSRF guard + filesystem sandbox, TTS chunking).
  No GPU/audio/ML native dependencies, so it builds fast and is exhaustively
  unit-tested.
- **`crates/lo`** — the binary: the `winit`/`wgpu` UI, `cpal` audio, on-device ML
  (`whisper-rs`/`ort`/Kokoro), the brain HTTP client, backend supervision, and the
  tool execution bodies.

## Building and testing

Lo targets **Rust 1.85+**. The default `ml` feature builds the on-device ML stack
(whisper.cpp via `whisper-rs`), which needs a C/C++ toolchain and **CMake**.

```bash
# Fast path: core + binary with the heavy ML stack off (clean stubs in its place)
cargo test  --workspace --no-default-features
cargo build --workspace --no-default-features

# Full path: ML on (whisper/ort/Kokoro)
cargo test  -p lo
cargo build -p lo

# Lints (run both feature configurations)
cargo fmt --all --check
cargo clippy --workspace --all-targets --no-default-features -- -D warnings
cargo clippy --workspace --all-targets -- -D warnings

# Headless self-checks
cargo run -p lo -- --smoke                    # resolved model tier + subsystems
cargo run -p lo -- --turn "what time is it"   # one agent turn (needs a brain server)
```

Please make sure `cargo fmt --check`, `cargo clippy -- -D warnings`, and the test
suite all pass before opening a pull request. New behavior should come with tests;
prefer putting testable logic in `lo-core` where it can be unit-tested without the
native ML toolchain.

## Environment variables (handy for development and triage)

| Variable | Effect |
|---|---|
| `LO_LOG` | `tracing` filter, e.g. `LO_LOG=lo=debug`. |
| `LO_RAM_GB` | Force the detected RAM (overrides the model-tier ladder). |
| `LO_WHISPER_CPU=1` | Force CPU speech-to-text (skip the GPU path). |
| `LO_NO_MIC` | Launch the GUI with audio streams disabled. |
| `LO_VAD_POSITIVE` / `LO_VAD_NEGATIVE` / `LO_VAD_REDEMPTION_MS` | Tune voice-activity detection without editing settings. |
| `LO_LLM_URL` / `LO_LLM_MODEL` / `LO_LLM_KEY` | Point the brain at a custom OpenAI-compatible endpoint. |

## Coding style

- Match the surrounding code's idiom, naming, and comment density.
- Comments should describe **what the code does and why**, not its history.
- Keep `lo-core` free of GPU/audio/ML dependencies.

## Reporting security issues

Please do not file public issues for security vulnerabilities — see
[SECURITY.md](SECURITY.md) for responsible disclosure.
