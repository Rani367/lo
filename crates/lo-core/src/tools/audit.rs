//! Audit log for gated tool invocations (ported from `src/main/tools/confirm.ts`).
//! confirm/danger tools run only when power-user mode is on; every gated
//! invocation — allowed, denied, or errored — is appended here for an
//! after-the-fact record. Best-effort: logging never breaks a turn.

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allowed,
    Denied,
    Error,
}

#[derive(Serialize)]
struct AuditLine<'a> {
    /// Unix epoch milliseconds (the TS used an ISO-8601 string; epoch ms keeps
    /// the core date-library-free and is trivially convertible).
    t: u128,
    tool: &'a str,
    args: serde_json::Value,
    decision: Decision,
    detail: String,
}

/// Append one audit line to the given path. Best-effort — never panics, never
/// propagates an error.
pub fn audit_log_to(
    path: &Path,
    tool: &str,
    args: &serde_json::Value,
    decision: Decision,
    detail: &str,
) {
    let line = AuditLine {
        t: now_millis(),
        tool,
        args: truncate_args(args),
        decision,
        detail: detail.chars().take(200).collect(),
    };
    let Ok(json) = serde_json::to_string(&line) else {
        return;
    };
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
