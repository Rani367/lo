//! Opt-in conversation persistence (ported from `src/main/history.ts`). The
//! rolling transcript is stored as JSON in the config dir so context survives a
//! restart. When `persist_history` is off, load/save are no-ops.

use super::paths;
use crate::types::ChatMessage;
use std::fs;
use std::path::Path;

/// Keep a bounded rolling window.
pub const MAX: usize = 24;

