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
            &["-e", &format!("set volume output volume {v}")],
        )
        .await?;
    } else if cfg!(target_os = "linux") {
        if run(
            "pactl",
            &["set-sink-volume", "@DEFAULT_SINK@", &format!("{v}%")],
        )
        .await
        .is_err()
        {
            run("amixer", &["-q", "sset", "Master", &format!("{v}%")]).await?;
        }
    } else if cfg!(target_os = "windows") {
        // No built-in absolute-volume CLI: drive to a known floor with the
        // volume-down media key (~2%/press), then step up to the target.
        let ups = (v as f64 / 2.0).round() as i64;
        let ps = format!(
            "Add-Type -MemberDefinition '[DllImport(\"user32.dll\")] public static extern void keybd_event(byte b, byte s, uint f, int e);' \
             -Name K -Namespace W; \
             for($i=0;$i -lt 50;$i++){{[W.K]::keybd_event(0xAE,0,0,0)}}; \
             for($i=0;$i -lt {ups};$i++){{[W.K]::keybd_event(0xAF,0,0,0)}};"
        );
        run("powershell", &["-NoProfile", "-Command", &ps]).await?;
    }
    Ok(format!("Volume set to {v} percent."))
}

/// Bring a running application to the foreground.
pub async fn focus_app(name: &str) -> Result<String, String> {
    let app = name.trim();
    if app.is_empty() {
        return Err("No application name given.".to_string());
    }
    if cfg!(target_os = "macos") {
        run(
            "osascript",
            &[
                "-e",
                &format!(
                    "tell application \"{}\" to activate",
                    escape_applescript(app)
                ),
            ],
        )
        .await?;
    } else if cfg!(target_os = "windows") {
        run_with_app_env(
            "powershell",
            &[
                "-NoProfile",
                "-Command",
                "(New-Object -ComObject WScript.Shell).AppActivate($env:LO_APP)",
            ],
            app,
        )
        .await?;
    } else {
        if run("wmctrl", &["-a", app]).await.is_err() {
            run("xdotool", &["search", "--name", app, "windowactivate"]).await?;
        }
    }
    Ok(format!("Brought {app} to the front."))
}

/// Quit a running application gracefully.
pub async fn quit_app(name: &str) -> Result<String, String> {
    let app = name.trim();
    if app.is_empty() {
        return Err("No application name given.".to_string());
    }
    if cfg!(target_os = "macos") {
        run(
            "osascript",
            &[
                "-e",
                &format!("tell application \"{}\" to quit", escape_applescript(app)),
            ],
        )
        .await?;
    } else if cfg!(target_os = "windows") {
        let image = if app.to_lowercase().ends_with(".exe") {
            app.to_string()
        } else {
            format!("{app}.exe")
        };
        run("taskkill", &["/IM", &image]).await?;
    } else {
        run("pkill", &["-f", app]).await?;
    }
    Ok(format!("Closed {app}."))
}

/// Capture a screenshot of the screen(s) and save it to `~/Pictures`.
///
/// Uses `xcap` to capture every monitor and save the first as a PNG, which is
/// portable across macOS/Windows/Linux without per-OS screenshot binaries.
pub async fn take_screenshot() -> Result<String, String> {
    let dir = pictures_dir();
    let file = dir.join(format!("lo-{}.png", stamp()));

    // `xcap` is blocking + does its own platform work; keep it off the async
    // executor with `spawn_blocking`.
    let file_for_task = file.clone();
    let result = tokio::task::spawn_blocking(move || -> Result<(), String> {
        if let Some(parent) = file_for_task.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("could not create the Pictures folder: {e}"))?;
        }
        let monitors =
            xcap::Monitor::all().map_err(|e| format!("could not enumerate displays: {e}"))?;
        let monitor = monitors
            .into_iter()
            .next()
            .ok_or_else(|| "no display was found to capture.".to_string())?;
        let image = monitor
            .capture_image()
            .map_err(|e| format!("the screen capture failed: {e}"))?;
        image
            .save(&file_for_task)
            .map_err(|e| format!("could not save the screenshot: {e}"))?;
        Ok(())
    })
    .await
    .map_err(|e| format!("the screenshot task failed: {e}"))?;

    result?;
    Ok(format!("Screenshot saved to {}.", file.display()))
}

/// Report the current local date and time, e.g.
/// `It is 3:05 pm on Saturday, June 14, 2026.`
pub fn get_datetime() -> String {
    let now = Local::now();
    // 12-hour clock with no leading zero on the hour (matches `hour: 'numeric'`).
    let mut hour12 = now.hour() % 12;
    if hour12 == 0 {
        hour12 = 12;
