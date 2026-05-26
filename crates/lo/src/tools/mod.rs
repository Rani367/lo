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
