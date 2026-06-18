//! Countdown timer — start a timer that, on completion, both speaks (over the
//! `announce` channel) and raises a desktop notification.
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
    };
    let human = format_duration(secs);
    let what = label
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty());

    let what_for_task = what.clone();
    let human_for_task = human.clone();
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        let msg = match &what_for_task {
            Some(w) => format!("Your timer for {w} is complete."),
            None => format!("Your {human_for_task} timer is complete."),
        };
        // Best-effort desktop notification (may be unavailable / unsupported).
        let _ = Notification::new().summary("Lo").body(&msg).show();
        // Speak it when the UI is idle.
        let _ = announce.send(format!("[crisply] {msg}"));
    });

    match what {
        Some(w) => format!("Timer set for {human} ({w})."),
        None => format!("Timer set for {human}."),
    }
}

/// Human-friendly duration, e.g. `45 seconds`, `3 minutes`, `2 minutes 5 seconds`.
fn format_duration(secs: u64) -> String {
    if secs < 60 {
        return format!("{secs} second{}", plural(secs));
    }
    if secs % 60 == 0 {
        let m = (secs as f64 / 60.0).round() as u64;
        return format!("{m} minute{}", plural(m));
    }
    let m = secs / 60;
    let s = secs % 60;
    format!("{m} minute{} {s} seconds", plural(m))
}

fn plural(n: u64) -> &'static str {
    if n == 1 {
        ""
    } else {
        "s"
    }
}

#[cfg(test)]
mod tests {
    use super::format_duration;

    #[test]
    fn reads_naturally_across_boundaries() {
        assert_eq!(format_duration(1), "1 second");
        assert_eq!(format_duration(45), "45 seconds");
        assert_eq!(format_duration(60), "1 minute");
        assert_eq!(format_duration(120), "2 minutes");
        assert_eq!(format_duration(125), "2 minutes 5 seconds");
        assert_eq!(format_duration(3600), "60 minutes");
    }
}
