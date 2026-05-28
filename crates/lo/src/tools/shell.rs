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
