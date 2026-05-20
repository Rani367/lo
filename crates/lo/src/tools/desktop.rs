//! Cross-platform desktop actions: open/focus/quit an app, set the system
//! volume, capture a screenshot, and report the local date+time. Ported from
//! `src/main/tools/desktop.ts`.
//!
//! Every shell-out uses an argv array (never a shell string) so an
//! LLM-provided name can't inject. On Windows the app name is handed to
//! PowerShell through the `LO_APP` environment variable — never interpolated
//! into the command string — and AppleScript string literals are escaped.

use std::path::PathBuf;

use chrono::{Datelike, Local, Timelike};
use lo_core::config::paths;
use tokio::process::Command;

/// Open/launch an application by name.
pub async fn open_app(name: &str) -> Result<String, String> {
    let app = name.trim();
    if app.is_empty() {
        return Err("No application name given.".to_string());
    }
    if cfg!(target_os = "macos") {
        run("open", &["-a", app]).await?;
    } else if cfg!(target_os = "windows") {
        // PowerShell does NOT re-parse env-var contents for $()/backtick
        // subexpressions, so passing the name via LO_APP can't inject.
        run_with_app_env(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "Start-Process -FilePath $env:LO_APP",
            ],
            app,
        )
        .await?;
    } else {
        // Linux: try a desktop launcher, then a bare binary.
        if run("gtk-launch", &[app]).await.is_err() {
            let safe = shell_safe(app);
            let cmd = format!("command -v {safe} >/dev/null && {safe} & disown");
            run("sh", &["-c", &cmd]).await?;
        }
    }
    Ok(format!("Opened {app}."))
}

/// Set the system output volume to a 0-100 percentage.
pub async fn set_volume(percent: f64) -> Result<String, String> {
    let v = percent.round().clamp(0.0, 100.0) as i64;
    if cfg!(target_os = "macos") {
        run(
            "osascript",
