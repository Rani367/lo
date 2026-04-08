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
    /// Spawned, waiting for the model to load and `/health` to go ready.
    Loading,
    /// Running and serving.
    Ready,
    /// Crashed or failed to start (see [`ManagedServer::last_error`]).
    Error,
}

impl ServerState {
    fn from_u8(v: u8) -> Self {
        match v {
            1 => ServerState::Loading,
            2 => ServerState::Ready,
            3 => ServerState::Error,
            _ => ServerState::Idle,
        }
    }
    fn as_u8(self) -> u8 {
        match self {
            ServerState::Idle => 0,
            ServerState::Loading => 1,
            ServerState::Ready => 2,
            ServerState::Error => 3,
        }
    }
}

/// How to (re)build the spawn command from current settings/env. Re-invoked on
/// every start so an env/settings change is picked up on restart.
pub struct CommandSpec {
    /// The program to execute (`python`, the `llama-server` path, …).
    pub program: String,
    /// Its arguments, in order.
    pub args: Vec<String>,
    /// Extra environment variables layered on top of the inherited environment.
    pub envs: Vec<(String, String)>,
}

/// The recipe for a managed server: its log name, how to build its command, the
/// health URL to poll, and the readiness predicate.
pub struct ServerSpec {
    /// Short name used as a log prefix (`brain`, `llama`, …).
    pub name: String,
    /// Build the spawn command/env from current settings (re-read on each start).
    pub build: Box<dyn Fn() -> CommandSpec + Send + Sync>,
    /// The `GET` URL polled until ready.
    pub health_url: String,
    /// Given a `/health` HTTP status, is the model loaded and serving?
    pub is_ready: Box<dyn Fn(u16) -> bool + Send + Sync>,
}

/// Shared, cloneable interior state so the spawned reader tasks and the caller
/// can all observe/mutate the server's status.
struct Inner {
    state: AtomicU8,
    last_error: Mutex<Option<String>>,
    /// Set just before a deliberate kill so the exit path treats it as a clean
    /// stop, not a crash.
    intentional_stop: AtomicBool,
    /// The live child handle (`None` when idle). Guarded by a *sync* mutex; only
    /// synchronous `Child` methods are called under it.
    child: Mutex<Option<Child>>,
    spec: ServerSpec,
}

impl Inner {
    fn set_state(&self, s: ServerState) {
        self.state.store(s.as_u8(), Ordering::SeqCst);
    }
    fn state(&self) -> ServerState {
        ServerState::from_u8(self.state.load(Ordering::SeqCst))
    }
    fn set_error(&self, msg: Option<String>) {
        *self.last_error.lock().expect("last_error poisoned") = msg;
    }
    fn error(&self) -> Option<String> {
        self.last_error.lock().expect("last_error poisoned").clone()
    }

    /// Has the current child exited? Returns `Some(exit_detail)` on exit,
    /// `None` while still running (or no child). Reaps the handle on exit and
    /// updates state, distinguishing a deliberate stop from a crash.
    fn poll_exit(&self) -> Option<String> {
        let mut guard = self.child.lock().expect("child poisoned");
        let exited = match guard.as_mut() {
            Some(child) => match child.try_wait() {
                Ok(Some(status)) => Some(status_detail(&self.spec.name, Ok(status))),
                Ok(None) => None,
                Err(err) => Some(status_detail(&self.spec.name, Err(err))),
            },
            None => return None,
        };
        if let Some(detail) = exited {
            *guard = None;
            drop(guard);
            if self.intentional_stop.swap(false, Ordering::SeqCst) {
                self.set_state(ServerState::Idle);
                None
            } else {
                self.set_error(Some(detail.clone()));
                self.set_state(ServerState::Error);
                Some(detail)
            }
        } else {
            None
        }
    }
}

fn status_detail(name: &str, status: std::io::Result<std::process::ExitStatus>) -> String {
    match status {
        Ok(code) => format!(
            "{name} exited ({}).",
            code.code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".into())
        ),
        Err(err) => format!("{name} exited ({err})."),
    }
}

/// A health-checked child process. Cheap to clone-share via its `Arc<Inner>`.
pub struct ManagedServer {
    inner: Arc<Inner>,
}

impl ManagedServer {
    /// Create a managed server from its spec. Nothing is spawned until
    /// [`ensure`](Self::ensure).
    pub fn new(spec: ServerSpec) -> Self {
        Self {
            inner: Arc::new(Inner {
                state: AtomicU8::new(ServerState::Idle.as_u8()),
                last_error: Mutex::new(None),
                intentional_stop: AtomicBool::new(false),
                child: Mutex::new(None),
                spec,
            }),
        }
    }

    /// A second handle to the *same* managed server, sharing its process and
    /// lifecycle state (`Arc<Inner>`). Lets the caller keep the canonical handle
    /// while still issuing `ensure`/`stop`/`state` from elsewhere.
    pub fn handle(&self) -> ManagedServer {
        ManagedServer {
            inner: self.inner.clone(),
        }
    }

    /// Is a child handle currently present (alive, not yet reaped)?
    fn has_child(&self) -> bool {
        self.inner.child.lock().expect("child poisoned").is_some()
    }

    /// Start (if needed) and resolve once the server reports healthy.
    ///
    /// - Already `Ready` with a live child → no-op.
    /// - A live child that is *not* ready (a prior health failure) → restart
    ///   cleanly rather than spawning a second process that would clash on the
    ///   port.
    /// - Otherwise → spawn fresh.
    pub async fn ensure(&self) -> anyhow::Result<()> {
        // Reap a quietly-dead child first so `has_child` reflects reality.
        self.inner.poll_exit();

        let alive = self.has_child();
        let ready = self.inner.state() == ServerState::Ready;

        if alive && ready {
            return Ok(());
        }
        if alive {
            // Live but not ready — restart instead of double-spawning.
            self.restart().await;
            return self.ready_or_err();
        }
        self.start().await
    }

    fn ready_or_err(&self) -> anyhow::Result<()> {
        if self.inner.state() == ServerState::Ready {
            Ok(())
        } else {
            Err(anyhow::anyhow!(self.inner.error().unwrap_or_else(
                || format!("{} is not ready.", self.inner.spec.name)
            )))
        }
    }

    /// Spawn the child, wire up stdout/stderr logging, then poll `/health` until
    /// ready (or fail).
    async fn start(&self) -> anyhow::Result<()> {
        self.inner.set_error(None);
        self.inner.set_state(ServerState::Loading);
        self.inner.intentional_stop.store(false, Ordering::SeqCst);

        let CommandSpec {
            program,
            args,
            envs,
        } = (self.inner.spec.build)();

        let mut cmd = Command::new(&program);
        cmd.args(&args)
            .envs(envs)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(err) => {
                let msg = format!("{} failed to start: {err}", self.inner.spec.name);
                self.inner.set_error(Some(msg.clone()));
                self.inner.set_state(ServerState::Error);
                return Err(anyhow::anyhow!(msg));
            }
        };

        let name = self.inner.spec.name.clone();

        // Drain stdout: log each non-empty line with the server's name prefix.
        if let Some(stdout) = child.stdout.take() {
            let name = name.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        tracing::info!(target: "lo::backend", "[{name}] {}", line.trim_end());
                    }
                }
            });
        }

        // Drain stderr: log it, and flag an EADDRINUSE so `ensure` surfaces a
        // clear "port already in use" rather than a generic timeout.
        if let Some(stderr) = child.stderr.take() {
            let name = name.clone();
            let inner = self.inner.clone();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if line.contains("Address already in use") || line.contains("bind") {
                        inner.set_error(Some(format!(
                            "{name}: port already in use — a previous instance may still be running."
                        )));
                    }
                    if !line.trim().is_empty() {
                        tracing::warn!(target: "lo::backend", "[{name}] {}", line.trim_end());
                    }
                }
            });
        }

        // Hand the child to the shared state.
        *self.inner.child.lock().expect("child poisoned") = Some(child);

        // Poll /health until ready (or the child dies / we time out).
        self.wait_for_health().await
    }

    /// Poll `GET health_url` every 500ms up to 180s, ready when `is_ready(status)`.
    /// Fails fast if the child dies before becoming ready.
    async fn wait_for_health(&self) -> anyhow::Result<()> {
        let client = reqwest::Client::builder()
            .build()
            .map_err(|e| anyhow::anyhow!("failed to build health client: {e}"))?;

        let deadline = Instant::now() + HEALTH_TIMEOUT;
        loop {
            // If the child died while binding/loading, fail with its error.
            if let Some(detail) = self.inner.poll_exit() {
                return Err(anyhow::anyhow!(detail));
            }
            if !self.has_child() {
                // Reaped as an intentional stop, or never started.
                let msg = self
                    .inner
                    .error()
                    .unwrap_or_else(|| "Server exited before becoming ready.".into());
                self.inner.set_state(ServerState::Error);
                self.inner.set_error(Some(msg.clone()));
                return Err(anyhow::anyhow!(msg));
            }

            if let Ok(res) = client
                .get(&self.inner.spec.health_url)
                .timeout(HEALTH_REQUEST_TIMEOUT)
                .send()
                .await
            {
                if (self.inner.spec.is_ready)(res.status().as_u16()) {
                    self.inner.set_state(ServerState::Ready);
                    self.inner.set_error(None);
                    return Ok(());
                }
            }
