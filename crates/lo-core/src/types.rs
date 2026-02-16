//! Shared types — the contract that, in the Electron app, crossed the main↔renderer
//! IPC boundary (`src/shared/types.ts`). In the single-process Rust app these are
//! plain values passed over channels (see the eventual `app::events`), but the
//! shapes are preserved 1:1 so `settings.json` and the chat protocol stay
//! byte-compatible with the TypeScript build.

use serde::{Deserialize, Serialize};

/// The renderer/UI state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LoState {
    Boot,
    Idle,
    Listening,
