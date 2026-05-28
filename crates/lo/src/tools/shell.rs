//! `run_command` — run an arbitrary executable. The most powerful (and most
//! dangerous) capability, so it is Danger-tier (gated unless power-user mode).
//! Ported from `src/main/tools/shell.ts`.
//!
//! The validation (non-empty command, argv list, cwd confined to an allowed
//! root) is reused from [`lo_core::tools::shell::prepare`]; this body only
//! executes the resulting plan with an argv array (never a shell string), a
//! bounded timeout, and a capped, captured output.

use std::process::Stdio;
use std::time::Duration;

use lo_core::tools::shell::{self, truncate_output, TIMEOUT_MS};
use lo_core::LoSettings;
use tokio::process::Command;
use tokio::time::timeout;

/// Run `command` with `args` in `cwd` (defaulting to the first allowed root).
/// Returns combined stdout/stderr (truncated) on success, or a
/// `Command failed (…)` line on a non-zero exit, matching `runCommand`.
pub async fn run_command(
    settings: &LoSettings,
    command: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<String, String> {
    // Validation + cwd resolution lives in lo_core (tested). Its errors carry
    // good wording; surface them as the tool error.
    let plan = shell::prepare(settings, command, args, cwd).map_err(|e| e.to_string())?;

    let mut child_cmd = Command::new(&plan.program);
    child_cmd
        .args(&plan.args)
        .current_dir(&plan.cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    let spawn = child_cmd.output();
    let output = match timeout(Duration::from_millis(TIMEOUT_MS), spawn).await {
        Ok(Ok(o)) => o,
        Ok(Err(e)) => {
            // Spawn/exec failure (e.g. command not found) — report like `execFile`.
            return Ok(format!(
                "Command failed (error): {}",
                truncate_output(&e.to_string())
            ));
        }
        Err(_) => {
            return Ok(format!(
                "Command failed (timeout): no result within {} seconds.",
                TIMEOUT_MS / 1000
            ));
        }
    };

    let out = String::from_utf8_lossy(&output.stdout).trim().to_string();
    let err_out = String::from_utf8_lossy(&output.stderr).trim().to_string();

    if !output.status.success() {
        let code = output
            .status
            .code()
            .map(|c| c.to_string())
            .unwrap_or_else(|| "error".to_string());
        let detail = if !err_out.is_empty() {
            err_out
        } else if !out.is_empty() {
            out
        } else {
            format!("exited with status {}", output.status)
        };
        return Ok(format!(
            "Command failed ({code}): {}",
            truncate_output(&detail)
        ));
    }

    let body = [out, err_out]
        .into_iter()
        .filter(|s| !s.is_empty())
        .collect::<Vec<_>>()
        .join("\n");
    Ok(if body.is_empty() {
        "Command completed with no output.".to_string()
    } else {
        truncate_output(&body)
    })
}
