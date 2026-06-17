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
    feature = "vad-silero",
    feature = "wake-openwakeword"
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
    feature = "asr-whisper",
    feature = "tts-kokoro",
    feature = "vad-silero"
))]
pub fn fetch(
    repo: &str,
    file: &str,
    label: &str,
    progress: Progress<'_>,
) -> anyhow::Result<PathBuf> {
    use hf_hub::api::sync::ApiBuilder;

    let cache_dir = lo_core::config::paths::cache_dir();
    std::fs::create_dir_all(&cache_dir)
        .with_context(|| format!("creating model cache dir {}", cache_dir.display()))?;

    report(progress, label, None);

    let api = ApiBuilder::new()
        .with_cache_dir(cache_dir)
        .build()
        .context("building hf-hub sync api")?;

    let path = api
        .model(repo.to_string())
        .get(file)
        .with_context(|| format!("downloading {file} from HuggingFace repo {repo}"))?;

    report(progress, label, Some(100));
    Ok(path)
}

/// Download a file over HTTPS into Lo's cache (under `openwakeword/`), returning
/// its local path; cached after the first fetch. Used for the openWakeWord models,
/// which ship on a GitHub release rather than HuggingFace. Runs a one-off blocking
/// download on a tiny Tokio runtime (reqwest is async), so it is safe to call from
/// the std listen thread which has no ambient reactor.
#[cfg(feature = "wake-openwakeword")]
pub fn fetch_http(
    url: &str,
    file_name: &str,
    label: &str,
    progress: Progress<'_>,
) -> anyhow::Result<PathBuf> {
    let dir = lo_core::config::paths::cache_dir().join("openwakeword");
    std::fs::create_dir_all(&dir)
        .with_context(|| format!("creating wake-word cache dir {}", dir.display()))?;
    let dest = dir.join(file_name);
    if dest.exists() {
        return Ok(dest);
    }

    report(progress, label, None);
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .context("building runtime for wake-word download")?;
    let bytes = rt
        .block_on(async {
            let client = reqwest::Client::builder().build()?;
            let resp = client.get(url).send().await?.error_for_status()?;
            Ok::<_, anyhow::Error>(resp.bytes().await?)
        })
        .with_context(|| format!("downloading {url}"))?;

    // Write atomically so an interrupted download never leaves a half file cached.
    let tmp = dest.with_extension("part");
    std::fs::write(&tmp, &bytes).with_context(|| format!("writing {}", tmp.display()))?;
    std::fs::rename(&tmp, &dest).with_context(|| format!("finalising {}", dest.display()))?;
    report(progress, label, Some(100));
    Ok(dest)
}

/// Stub used when no ML feature is enabled — keeps the module compiling but never
/// reaches a real download.
#[cfg(not(any(
    feature = "asr-whisper",
    feature = "tts-kokoro",
    feature = "vad-silero"
)))]
pub fn fetch(
    _repo: &str,
    _file: &str,
    _label: &str,
    _progress: Progress<'_>,
) -> anyhow::Result<PathBuf> {
    anyhow::bail!("model download unavailable: built without any ML feature")
}
