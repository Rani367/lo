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

