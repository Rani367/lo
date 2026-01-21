//! Opt-in conversation persistence (ported from `src/main/history.ts`). The
//! rolling transcript is stored as JSON in the config dir so context survives a
//! restart. When `persist_history` is off, load/save are no-ops.

use super::paths;
use crate::types::ChatMessage;
use std::fs;
use std::path::Path;

/// Keep a bounded rolling window.
pub const MAX: usize = 24;

/// Load the persisted transcript, or `[]` when persistence is off / unreadable.
pub fn load(persist: bool) -> Vec<ChatMessage> {
    if !persist {
        return Vec::new();
    }
    load_from(paths::history_file())
}

pub fn load_from<P: AsRef<Path>>(path: P) -> Vec<ChatMessage> {
    match fs::read_to_string(path.as_ref()) {
        Ok(raw) => match serde_json::from_str::<Vec<ChatMessage>>(&raw) {
            Ok(mut msgs) => {
                if msgs.len() > MAX {
                    msgs = msgs.split_off(msgs.len() - MAX);
                }
                msgs
            }
            Err(_) => Vec::new(),
        },
        Err(_) => Vec::new(),
    }
}

