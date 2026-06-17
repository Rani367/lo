//! Audit log for gated tool invocations (ported from `src/main/tools/confirm.ts`).
//! confirm/danger tools run only when power-user mode is on; every gated
//! invocation — allowed, denied, or errored — is appended here for an
//! after-the-fact record. Best-effort: logging never breaks a turn.

use serde::Serialize;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Roll the audit log over once it grows past this (≈5 MB). Keeps a single `.1`
/// backup so the on-disk footprint stays bounded without losing recent history.
pub const MAX_AUDIT_BYTES: u64 = 5 * 1024 * 1024;

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
    rotate_if_needed(path);
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{json}");
    }
}

/// Roll `path` → `path.1` (overwriting any previous `.1`) once it exceeds
/// [`MAX_AUDIT_BYTES`], so the live log never grows without bound. Best-effort.
fn rotate_if_needed(path: &Path) {
    let too_big = std::fs::metadata(path)
        .map(|m| m.len() >= MAX_AUDIT_BYTES)
        .unwrap_or(false);
    if too_big {
        let mut backup = path.as_os_str().to_owned();
        backup.push(".1");
        let _ = std::fs::rename(path, backup);
    }
}

/// Append to the default audit-log location.
pub fn audit_log(tool: &str, args: &serde_json::Value, decision: Decision, detail: &str) {
    audit_log_to(
        &crate::config::paths::audit_file(),
        tool,
        args,
        decision,
        detail,
    );
}

/// Truncate an args blob to ~500 chars of JSON, matching the TS `truncate`.
fn truncate_args(args: &serde_json::Value) -> serde_json::Value {
    let s = args.to_string();
    if s.len() > 500 {
        serde_json::Value::String(format!("{}…", &s[..500]))
    } else {
        args.clone()
    }
}

fn now_millis() -> u128 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn appends_one_json_line_per_call() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lo-audit.log");
        let args = serde_json::json!({ "command": "rm", "args": ["-rf", "/tmp/x"] });
        audit_log_to(&path, "run_command", &args, Decision::Denied, "");
        audit_log_to(&path, "run_command", &args, Decision::Allowed, "ok");

        let body = std::fs::read_to_string(&path).unwrap();
        let lines: Vec<&str> = body.lines().collect();
        assert_eq!(lines.len(), 2);
        let v: serde_json::Value = serde_json::from_str(lines[0]).unwrap();
        assert_eq!(v["tool"], "run_command");
        assert_eq!(v["decision"], "denied");
        assert!(v["t"].is_number());
    }

    #[test]
    fn rotates_when_oversized() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lo-audit.log");
        // Pre-fill past the rotation threshold.
        std::fs::write(&path, vec![b'x'; (MAX_AUDIT_BYTES + 1) as usize]).unwrap();
        let args = serde_json::json!({});
        audit_log_to(&path, "write_file", &args, Decision::Allowed, "");
        // The oversized log moved to `.1`; the live log holds just the new line.
        let mut backup = path.clone().into_os_string();
        backup.push(".1");
        assert!(std::path::Path::new(&backup).exists(), "backup not created");
        let body = std::fs::read_to_string(&path).unwrap();
        assert_eq!(body.lines().count(), 1);
    }

    #[test]
    fn truncates_large_args() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("lo-audit.log");
        let big = serde_json::json!({ "content": "x".repeat(2000) });
        audit_log_to(&path, "write_file", &big, Decision::Allowed, "");
        let body = std::fs::read_to_string(&path).unwrap();
        let v: serde_json::Value = serde_json::from_str(body.lines().next().unwrap()).unwrap();
        // args got coerced to a truncated string ending in the ellipsis.
        assert!(v["args"].as_str().unwrap().ends_with('…'));
    }
}
