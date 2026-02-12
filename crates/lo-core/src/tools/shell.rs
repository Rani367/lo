//! `run_command` validation (ported from `src/main/tools/shell.ts`). The single
//! most powerful capability, so it is Danger-tier. The always-on safety baseline,
//! regardless of the gate, is reproduced here:
//!   - argv array, never a shell string → no shell interpolation.
//!   - cwd validated to an allowed root (reuses the filesystem sandbox).
//!   - bounded timeout + output cap (constants; enforced by the bin's executor).

use crate::config::LoSettings;
use crate::tools::sandbox::{self, SandboxError};
use std::path::PathBuf;

pub const TIMEOUT_MS: u64 = 60_000;
pub const MAX_OUTPUT: usize = 16 * 1024;

#[derive(Debug, thiserror::Error)]
pub enum ShellError {
    #[error("No command was given.")]
    EmptyCommand,
    #[error(transparent)]
    Sandbox(#[from] SandboxError),
}

/// A validated, ready-to-spawn command (argv only — never a shell string).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunPlan {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
}

/// Validate a `run_command` request: non-empty executable, argv list, and a cwd
/// confined to an allowed root (defaulting to the first root). Mirrors the
/// resolution `runCommand` does before `execFile`.
pub fn prepare(
    settings: &LoSettings,
    command: &str,
    args: &[String],
    cwd: Option<&str>,
) -> Result<RunPlan, ShellError> {
    let program = command.trim();
    if program.is_empty() {
        return Err(ShellError::EmptyCommand);
    }
    let dir = match cwd {
        Some(c) if !c.trim().is_empty() => sandbox::resolve_in_roots(settings, c)?,
        _ => sandbox::allowed_roots(settings)
            .into_iter()
            .next()
            .unwrap_or_else(crate::config::paths::home_dir),
    };
    Ok(RunPlan {
        program: program.to_string(),
        args: args.to_vec(),
        cwd: dir,
    })
