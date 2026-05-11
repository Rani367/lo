//! Tiny wrapper over the hf-hub 0.5 **synchronous** (blocking) API.
//!
//! Every ML engine in this subsystem needs the same thing: given a HuggingFace
//! `(repo, file)`, fetch it (downloading on first use, hitting the cache after)
//! and hand back a local [`PathBuf`]. We point hf-hub at
//! [`lo_core::config::paths::cache_dir`] so all weights live under Lo's own cache
//! directory instead of the global `~/.cache/huggingface`.
//!
//! Progress is reported through the shared [`Progress`] callback type so the HUD
//! can show "HEARING 42%" / "VOICE 42%" during the first-run download.

use std::path::PathBuf;

#[cfg(any(
    feature = "asr-whisper",
    feature = "tts-kokoro",
    feature = "vad-silero"
))]
use anyhow::Context;
