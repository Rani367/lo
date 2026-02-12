//! `run_command` validation (ported from `src/main/tools/shell.ts`). The single
//! most powerful capability, so it is Danger-tier. The always-on safety baseline,
//! regardless of the gate, is reproduced here:
//!   - argv array, never a shell string → no shell interpolation.
//!   - cwd validated to an allowed root (reuses the filesystem sandbox).
//!   - bounded timeout + output cap (constants; enforced by the bin's executor).

use crate::config::LoSettings;
use crate::tools::sandbox::{self, SandboxError};
