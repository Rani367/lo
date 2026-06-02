//! Countdown timer — start a timer that, on completion, both speaks (over the
//! `announce` channel) and raises a desktop notification. Ported from
//! `setTimer`/`formatDuration` in `src/main/tools/desktop.ts`.
//!
//! The confirmation string is returned immediately; the firing happens in a
//! detached tokio task so the agent turn doesn't block on the countdown.

use std::time::Duration;

use notify_rust::Notification;
use tokio::sync::mpsc::UnboundedSender;

/// Start a countdown of `seconds` (clamped to ≥1) with an optional `label`.
/// Spawns a task that, when the timer fires, sends a `[crisply] …` line on
/// `announce` and shows a notification. Returns the confirmation immediately.
pub fn set_timer(seconds: f64, label: Option<String>, announce: UnboundedSender<String>) -> String {
    let secs = if seconds.is_finite() {
        (seconds.round() as i64).max(1) as u64
    } else {
        1
