//! `run_command` validation. The single most powerful capability, so it is
//! Danger-tier. The always-on safety baseline, regardless of the gate, is
//! enforced here:
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
/// confined to an allowed root (defaulting to the first root). This is the
/// resolution done before the command is spawned.
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
}

/// Cap captured output to `MAX_OUTPUT` chars, appending a truncation marker.
pub fn truncate_output(s: &str) -> String {
    if s.chars().count() > MAX_OUTPUT {
        let head: String = s.chars().take(MAX_OUTPUT).collect();
        format!("{head}\n… (truncated)")
    } else {
        s.to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn rooted(root: &std::path::Path) -> LoSettings {
        LoSettings {
            allowed_fs_roots: vec![root.to_string_lossy().into_owned()],
            ..Default::default()
        }
    }

    #[test]
    fn empty_command_is_rejected() {
        let s = LoSettings::default();
        assert!(matches!(
            prepare(&s, "  ", &[], None),
            Err(ShellError::EmptyCommand)
        ));
    }

    #[test]
    fn cwd_defaults_to_first_root() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let s = rooted(&root);
        let plan = prepare(&s, "git", &["status".to_string()], None).unwrap();
        assert_eq!(plan.program, "git");
        assert_eq!(plan.args, vec!["status".to_string()]);
        assert_eq!(plan.cwd, root);
    }

    #[test]
    fn cwd_outside_roots_is_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let s = rooted(&root);
        let outside = tempfile::tempdir().unwrap();
        let outside_path = outside.path().to_string_lossy().into_owned();
        let err = prepare(&s, "ls", &[], Some(&outside_path));
        assert!(matches!(err, Err(ShellError::Sandbox(_))), "got {err:?}");
    }

    #[test]
    fn argv_is_preserved_verbatim_no_shell_parsing() {
        let dir = tempfile::tempdir().unwrap();
        let root = fs::canonicalize(dir.path()).unwrap();
        let s = rooted(&root);
        // A would-be injection stays a single literal argument.
        let plan = prepare(&s, "echo", &["; rm -rf /".to_string()], None).unwrap();
        assert_eq!(plan.args, vec!["; rm -rf /".to_string()]);
    }

    #[test]
    fn output_is_truncated() {
        let big = "a".repeat(MAX_OUTPUT + 100);
        let out = truncate_output(&big);
        assert!(out.ends_with("… (truncated)"));
    }
}
