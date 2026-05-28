//! `run_command` — run an arbitrary executable. The most powerful (and most
//! dangerous) capability, so it is Danger-tier (gated unless power-user mode).
//! Ported from `src/main/tools/shell.ts`.
//!
//! The validation (non-empty command, argv list, cwd confined to an allowed
