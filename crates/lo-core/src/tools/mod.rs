//! Tool registry + safety gate (ported from `src/main/tools/registry.ts`).
//!
//! Each tool carries a safety `tier`:
//!   - `Safe`    — read-only or trivially reversible; runs immediately.
//!   - `Confirm` — a visible, reversible side effect; gated unless power-user mode.
//!   - `Danger`  — destructive / irreversible / arbitrary code; gated unless power-user.
//!
//! The gate is enforced HERE, before a tool runs — the model is never trusted to
//! police itself, and every gated invocation is audit-logged. The JSON schemas
//! are the exact ones advertised to the brain via the API `tools[]` parameter.
//!
//! Tool *execution* bodies (which need HTTP, process spawning, clipboard, etc.)
//! live in the `lo` binary crate; the registry, the schemas, the gate, the SSRF
//! guard, the filesystem sandbox, the argv validation, and the audit log are all
//! here and unit-tested.

pub mod audit;
pub mod sandbox;
pub mod shell;
pub mod ssrf;

use serde::Serialize;
use std::sync::LazyLock;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
