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
