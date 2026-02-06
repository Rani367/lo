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
pub enum Tier {
    Safe,
    Confirm,
    Danger,
}

/// One advertised tool: its safety tier plus the OpenAI function schema.
pub struct ToolSchema {
    pub tier: Tier,
    pub name: &'static str,
    pub description: &'static str,
    /// JSON-schema `parameters` object.
    pub parameters: serde_json::Value,
}

/// The canned refusal returned when a gated tool is called with power-user off.
pub const POWER_USER_REQUIRED: &str =
    "That action needs power-user mode, which is off. Enable powerUserMode in settings to allow it.";

/// The result of the safety gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateDecision {
    /// Run the tool.
