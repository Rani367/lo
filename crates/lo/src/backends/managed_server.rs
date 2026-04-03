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
