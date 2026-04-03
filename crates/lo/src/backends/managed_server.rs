//! A long-lived, health-checked child process behind a tiny typed surface — the
//! backend-agnostic plumbing that powers every spawned local server (the MLX
//! brain and the bundled `llama-server`). It owns spawn → health-poll →
//! restart-on-failure → kill-on-exit; callers only describe *how* to start a
//! server (a [`ServerSpec`]) and *when* it is ready (`is_ready`).
//!
//! Ported from `src/main/backends/managed-server.ts`, preserving its hard-won
//! lifecycle semantics: intentional-stop bookkeeping (a deliberate kill is not a
//! crash), `Address already in use` detection on stderr, and restart-rather-than-
//! double-spawn so two instances never clash on the port.
//!
//! Concurrency note: the child handle lives behind a plain `std::sync::Mutex`,
//! and we only ever call the *synchronous* `Child` methods (`try_wait`,
//! `start_kill`, `id`) while holding it — never `.await` under the lock. Process
//! exit is observed by polling `try_wait()` inside the health loop and a short
//! background reaper, mirroring the TS poll-based lifecycle without a long-lived
//! task that would pin the handle.

use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};

/// Model loads (especially a first-run download) can be slow.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(180);
const HEALTH_POLL: Duration = Duration::from_millis(500);
const HEALTH_REQUEST_TIMEOUT: Duration = Duration::from_secs(2);

/// The lifecycle state of a managed server (mirrors the TS `ServerState`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerState {
    /// Not running.
    Idle,
