//! Filesystem sandbox (ported from `resolveInRoots`/`allowedRoots` in
//! `src/main/tools/files.ts`). Every path the model supplies is expanded,
//! absolutized, lexically normalized, then realpath'd (longest existing prefix)
//! and verified to live inside an allowed root before any I/O happens — so a
//! mistaken or adversarial path (incl. one tunnelling through a symlink) can't
//! escape the sandbox.

use crate::config::{paths, LoSettings};
use std::path::{Component, Path, PathBuf};

/// Don't stuff a giant file into the model context.
pub const MAX_READ_BYTES: u64 = 256 * 1024;
pub const MAX_LIST: usize = 200;
pub const MAX_MATCHES: usize = 100;
pub const MAX_SEARCH_DEPTH: usize = 6;

#[derive(Debug, thiserror::Error)]
pub enum SandboxError {
    #[error("No path was given.")]
    Empty,
    #[error("That path is outside the allowed folders ({roots}). Adjust them in Settings.")]
    OutsideRoots { roots: String },
}

/// Expand a leading `~` to the home directory.
pub fn expand_home(p: &str) -> PathBuf {
    if p == "~" {
        return paths::home_dir();
    }
    if let Some(rest) = p.strip_prefix("~/").or_else(|| p.strip_prefix("~\\")) {
        return paths::home_dir().join(rest);
    }
    PathBuf::from(p)
}

/// The directories the filesystem tools may touch. Empty setting => home dir.
pub fn allowed_roots(settings: &LoSettings) -> Vec<PathBuf> {
    let list: Vec<String> = if settings.allowed_fs_roots.is_empty() {
        vec![paths::home_dir().to_string_lossy().into_owned()]
    } else {
        settings.allowed_fs_roots.clone()
    };
    list.iter().map(|r| absolutize(&expand_home(r))).collect()
}

/// Resolve a user-supplied path to an absolute, real path inside an allowed root,
/// or error. Mirrors `resolveInRoots`.
pub fn resolve_in_roots(settings: &LoSettings, input: &str) -> Result<PathBuf, SandboxError> {
    let p = input.trim();
    if p.is_empty() {
        return Err(SandboxError::Empty);
    }
    let real = realpath_best_effort(&absolutize(&expand_home(p)));
    let roots: Vec<PathBuf> = allowed_roots(settings)
        .iter()
        .map(|r| realpath_best_effort(r))
        .collect();
    let ok = roots
        .iter()
        .any(|root| &real == root || real.starts_with(root));
    if !ok {
        let roots_str = roots
            .iter()
            .map(|r| r.display().to_string())
            .collect::<Vec<_>>()
            .join(", ");
        return Err(SandboxError::OutsideRoots { roots: roots_str });
    }
    Ok(real)
}

/// Absolutize (join the current dir if relative) and lexically normalize `.`/`..`
/// so a relative or `..`-laden path can't lexically escape before the realpath
