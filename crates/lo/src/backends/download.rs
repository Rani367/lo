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

