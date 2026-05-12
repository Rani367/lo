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

/// An optional progress sink: `(label, percent)`. `percent` is `None` when the
/// total size is unknown (indeterminate phase). Must be `Send + Sync` so the
/// worker thread can hold it.
pub type Progress<'a> = Option<&'a (dyn Fn(&str, Option<u8>) + Send + Sync)>;

/// Emit a progress tick if a callback is installed. Centralised so the engines
/// don't each repeat the `if let Some(cb)` dance.
#[inline]
pub fn report(progress: Progress<'_>, label: &str, pct: Option<u8>) {
    if let Some(cb) = progress {
        cb(label, pct);
    }
}

/// Fetch a single file from a HuggingFace model repo into Lo's cache dir,
/// returning its local path. Cached after the first download.
///
/// `label` is the prefix the progress callback should display (e.g. `"HEARING"`,
/// `"VOICE"`, `"VAD"`). hf-hub's sync API does not surface byte-level progress, so
/// we emit a single indeterminate tick before the (potentially long) blocking
/// download and a `100%` tick once it lands.
#[cfg(any(
