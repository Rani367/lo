//! First-run asset acquisition for the managed llama.cpp backend: the
//! `llama-server` binary (resolved from `ggml-org/llama.cpp` GitHub releases and
//! extracted) and the GGUF weights (resolved from HuggingFace). Downloads stream
//! to disk with progress and land atomically (`.part` → rename) so a crashed
//! download never leaves a half-file that looks complete.
//!
//! Ported from `src/main/backends/download.ts`. The pure resolution logic
//! (`match_llama_asset` / `resolve_gguf_url` / the host matrix) is reused from
//! `lo_core::backends::download`; this module owns only the network + extraction.

use std::path::{Path, PathBuf};

use anyhow::{anyhow, Context};
use futures_util::StreamExt;
use lo_core::backends::download::{match_llama_asset, resolve_gguf_url, HostTarget, LLAMA_REPO};
use lo_core::backends::models::gguf_ref_for_model;
use serde::Deserialize;
use tokio::io::AsyncWriteExt;

/// Progress callback: `(label, pct)` where `pct` is `None` for an indeterminate
/// phase. Matches the `Progress` alias in the TS.
pub type Progress<'a> = Option<&'a (dyn Fn(&str, Option<u8>) + Send + Sync)>;

fn report(progress: Progress, label: &str, pct: Option<u8>) {
    if let Some(cb) = progress {
        cb(label, pct);
    }
}

/// One asset in a GitHub release.
#[derive(Debug, Deserialize)]
struct ReleaseAsset {
    name: String,
    browser_download_url: String,
}

#[derive(Debug, Deserialize)]
struct ReleaseBody {
    #[serde(default)]
    assets: Vec<ReleaseAsset>,
}

fn env_trimmed(key: &str) -> Option<String> {
    std::env::var(key)
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
}

/// A tiny shared HTTP client for the download/API requests.
fn http_client() -> anyhow::Result<reqwest::Client> {
    reqwest::Client::builder()
        .user_agent("Lo")
        .build()
        .context("failed to build download HTTP client")
}

/// Stream a URL to `dest` atomically (`.part` → rename), reporting 0–100% when a
/// content-length is known. Mirrors `downloadFile`.
pub async fn download_file(
    url: &str,
    dest: &Path,
    label: &str,
    progress: Progress<'_>,
) -> anyhow::Result<()> {
    if let Some(parent) = dest.parent() {
        tokio::fs::create_dir_all(parent)
            .await
            .with_context(|| format!("creating {}", parent.display()))?;
    }
    let part = with_extension_suffix(dest, "part");

    let result = download_file_inner(url, &part, label, progress).await;
    match result {
        Ok(()) => {
            tokio::fs::rename(&part, dest)
                .await
                .with_context(|| format!("renaming {} → {}", part.display(), dest.display()))?;
            Ok(())
        }
        Err(err) => {
            // Never leave a half-written `.part` to be mistaken for complete.
            let _ = tokio::fs::remove_file(&part).await;
            Err(err)
        }
    }
}

async fn download_file_inner(
    url: &str,
    part: &Path,
    label: &str,
    progress: Progress<'_>,
) -> anyhow::Result<()> {
    let client = http_client()?;
    let res = client
        .get(url)
        .send()
        .await
        .with_context(|| format!("requesting {url}"))?;
    if !res.status().is_success() {
        return Err(anyhow!(
            "Download failed ({}) for {url}",
            res.status().as_u16()
        ));
    }

    let total = res.content_length().unwrap_or(0);
    let mut received: u64 = 0;
    let mut last_pct: i32 = -1;

    let mut out = tokio::fs::File::create(part)
        .await
        .with_context(|| format!("creating {}", part.display()))?;

    let mut stream = res.bytes_stream();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("error reading download stream")?;
        out.write_all(&chunk)
            .await
            .with_context(|| format!("writing {}", part.display()))?;
        received += chunk.len() as u64;
        if total > 0 {
            let pct = ((received as f64 / total as f64) * 100.0).round() as i32;
            if pct != last_pct {
                last_pct = pct;
                report(progress, label, Some(pct.clamp(0, 100) as u8));
            }
        } else {
            report(progress, label, None);
        }
    }
    out.flush().await.ok();
    Ok(())
}

/* ---------------- llama-server binary ---------------- */

/// Resolve the download URL + asset name for the llama-server release for this
/// host (mirrors `resolveLlamaAssetUrl`). Honors `LO_LLAMA_RELEASE_URL`,
/// `LO_LLAMA_RELEASE_TAG`, and `LO_LLAMA_VARIANT`.
async fn resolve_llama_asset_url() -> anyhow::Result<(String, String)> {
    if let Some(explicit) = env_trimmed("LO_LLAMA_RELEASE_URL") {
        // Derive a leaf name from the URL path.
        let name = explicit
            .rsplit('/')
            .next()
            .filter(|s| !s.is_empty())
            .unwrap_or("llama-release.zip")
            .split('?')
            .next()
            .unwrap_or("llama-release.zip")
            .to_string();
        return Ok((explicit, name));
    }

    let tag = env_trimmed("LO_LLAMA_RELEASE_TAG");
    let api = match &tag {
        Some(t) => format!("https://api.github.com/repos/{LLAMA_REPO}/releases/tags/{t}"),
        None => format!("https://api.github.com/repos/{LLAMA_REPO}/releases/latest"),
    };

    let client = http_client()?;
    let res = client
        .get(&api)
        .header(reqwest::header::ACCEPT, "application/vnd.github+json")
        .send()
        .await
        .with_context(|| format!("querying {api}"))?;
    if !res.status().is_success() {
        return Err(anyhow!(
            "Could not query llama.cpp releases ({}).",
            res.status().as_u16()
        ));
    }
    let body: ReleaseBody = res
        .json()
        .await
        .context("parsing llama.cpp releases JSON")?;

    let variant = env_trimmed("LO_LLAMA_VARIANT").unwrap_or_else(|| "cpu".to_string());
    let names: Vec<String> = body.assets.iter().map(|a| a.name.clone()).collect();
    let name = match_llama_asset(&names, HostTarget::current(&variant));

    let asset = name
        .as_ref()
        .and_then(|n| body.assets.iter().find(|a| &a.name == n));

    match asset {
        Some(a) => Ok((a.browser_download_url.clone(), a.name.clone())),
        None => Err(anyhow!(
            "No matching llama-server build for {}/{}. Set LO_LLAMA_BIN to an existing binary, or install Ollama.",
            std::env::consts::OS,
            std::env::consts::ARCH
        )),
    }
}

/// Ensure the `llama-server` binary exists at `dest`, downloading + extracting if
/// missing. Co-locates sibling shared libraries and swaps the whole directory
/// into place atomically (mirrors `ensureLlamaBinary`).
pub async fn ensure_llama_binary(dest: &Path, progress: Progress<'_>) -> anyhow::Result<()> {
    if dest.exists() {
        return Ok(());
    }
    report(progress, "ENGINE", None);

    let (url, name) = resolve_llama_asset_url().await?;

    let tmp = unique_tmp_dir("lo-llama-").await?;
    let cleanup_tmp = tmp.clone();
    let result = ensure_llama_binary_inner(&url, &name, dest, &tmp, progress).await;
    let _ = tokio::fs::remove_dir_all(&cleanup_tmp).await;
    result
}

async fn ensure_llama_binary_inner(
    url: &str,
    name: &str,
    dest: &Path,
    tmp: &Path,
    progress: Progress<'_>,
) -> anyhow::Result<()> {
    let archive_name = if name.to_lowercase().ends_with(".zip") {
        name.to_string()
    } else {
        format!("{name}.zip")
    };
    let archive = tmp.join(&archive_name);
    download_file(url, &archive, "ENGINE", progress).await?;

    let unpack = tmp.join("unpacked");
    let exe = exe_name();

    // Extraction is blocking (zip + std::fs) — run it off the async runtime.
    let archive_b = archive.clone();
    let unpack_b = unpack.clone();
    let exe_b = exe.to_string();
    let found: PathBuf =
        tokio::task::spawn_blocking(move || extract_and_find(&archive_b, &unpack_b, &exe_b))
            .await
            .context("zip extraction task panicked")??;

    // Co-locate the binary with its sibling shared libraries, then swap the whole
    // directory into place atomically — a crash mid-copy can't leave a partial
    // engine that `dest.exists()` would later treat as complete.
    let dest_dir = dest
        .parent()
        .ok_or_else(|| anyhow!("destination {} has no parent dir", dest.display()))?
        .to_path_buf();
    let staging = with_extension_suffix(&dest_dir, "staging");

    let _ = tokio::fs::remove_dir_all(&staging).await;
    tokio::fs::create_dir_all(&staging)
        .await
        .with_context(|| format!("creating {}", staging.display()))?;

    let src_dir = found
        .parent()
        .ok_or_else(|| anyhow!("extracted binary has no parent dir"))?
        .to_path_buf();

    // Copy every sibling file next to the found binary.
    let mut entries = tokio::fs::read_dir(&src_dir)
        .await
        .with_context(|| format!("reading {}", src_dir.display()))?;
    while let Some(entry) = entries.next_entry().await? {
        let ft = entry.file_type().await?;
        if ft.is_file() {
            let from = entry.path();
            let to = staging.join(entry.file_name());
            tokio::fs::copy(&from, &to)
                .await
                .with_context(|| format!("copying {} → {}", from.display(), to.display()))?;
        }
    }

    chmod_executable(&staging.join(exe)).await;

    let _ = tokio::fs::remove_dir_all(&dest_dir).await;
    tokio::fs::rename(&staging, &dest_dir)
        .await
        .with_context(|| format!("renaming {} → {}", staging.display(), dest_dir.display()))?;
    Ok(())
}

/// Extract `archive` into `unpack` and return the path of the first `exe`-named
/// file inside it. Synchronous (runs under `spawn_blocking`).
fn extract_and_find(archive: &Path, unpack: &Path, exe: &str) -> anyhow::Result<PathBuf> {
    use std::fs::File;
    use std::io;
    use zip::ZipArchive;

    std::fs::create_dir_all(unpack).with_context(|| format!("creating {}", unpack.display()))?;

