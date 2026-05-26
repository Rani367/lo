//! OS-action tool *execution* bodies — the side-effecting half of the tool
//! system. The registry, schemas, safety gate, SSRF classifier, filesystem
//! sandbox, argv validation, and audit log all live in [`lo_core::tools`] and are
//! reused verbatim here; this module only carries the bodies that need HTTP,
//! process spawning, the clipboard, screen capture, notifications, and timers.
//!
//! Ported from `src/main/tools/{web,websearch,system,desktop,media,files,shell,
//! clipboard}.ts`. The [`dispatch`] entry reproduces `registry.ts`'s
//! `dispatchTool` exactly: parse args, run the safety gate, audit non-`Safe`
//! tiers, and return a plain string the brain can phrase in the Lo voice.

mod clipboard;
mod desktop;
mod files;
mod media;
mod shell;
mod system;
mod timer;
mod web;

use lo_core::tools::audit::{self, Decision};
use lo_core::tools::{self, GateDecision};
use lo_core::LoSettings;
use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

/// Execute a requested tool by name and return a string result for the brain.
///
/// Mirrors `dispatchTool` in `registry.ts`:
/// 1. Parse `args_json` with serde_json; on parse failure return a fixed message.
/// 2. Run the safety gate; if it denies, audit `Denied` and return the canned
///    power-user refusal.
/// 3. Otherwise execute. For non-`Safe` tiers, audit `Allowed` (with the result)
///    on success or `Error` (with the message) on failure.
///
/// `announce` is the worker → UI string channel a fired timer speaks into.
pub async fn dispatch(
    name: &str,
    args_json: &str,
    settings: &LoSettings,
    announce: &UnboundedSender<String>,
) -> String {
    // 1) Parse the arguments (empty string => empty object, matching the TS).
    let args: Value = if args_json.is_empty() {
        Value::Object(Default::default())
    } else {
