//! Media control — play/pause/next/previous/stop the active media player.
//! Ported from `src/main/tools/media.ts`. Per-OS mechanism: AppleScript to the
//! running player on macOS, `playerctl` on Linux, and the Windows media
//! virtual-keys via `keybd_event`.

use tokio::process::Command;

/// Control playback. `action` is one of play/pause/playpause/next/previous/stop
/// (with `prev`→`previous` and `toggle`→`playpause` aliases). Returns
/// `Err(message)` for the caller to wrap.
pub async fn media_control(action: &str) -> Result<String, String> {
    let a = normalize(action);

    if cfg!(target_os = "macos") {
        let cmd = match a {
            "playpause" => "playpause",
            "play" => "play",
            "pause" => "pause",
            "next" => "next track",
            "previous" => "previous track",
            "stop" => "stop",
            _ => "playpause",
        };
        let script = format!(
            "if application \"Spotify\" is running then tell application \"Spotify\" to {cmd}\n\
             else if application \"Music\" is running then tell application \"Music\" to {cmd}\n\
             else error \"No media player is running.\" \nend if"
        );
        run("osascript", &["-e", &script]).await?;
    } else if cfg!(target_os = "linux") {
        let sub = match a {
            "playpause" => "play-pause",
            "play" => "play",
            "pause" => "pause",
            "next" => "next",
            "previous" => "previous",
            "stop" => "stop",
            _ => "play-pause",
        };
        run("playerctl", &[sub]).await?;
    } else if cfg!(target_os = "windows") {
        let vk: u32 = match a {
            "next" => 0xB0,
            "previous" => 0xB1,
            "stop" => 0xB2,
            // play / pause / playpause / toggle all map to the play/pause key.
            _ => 0xB3,
        };
        let ps = format!(
            "Add-Type -MemberDefinition '[DllImport(\"user32.dll\")] public static extern void keybd_event(byte b, byte s, uint f, int e);' \
             -Name K -Namespace W; [W.K]::keybd_event({vk},0,0,0);"
        );
        run("powershell", &["-NoProfile", "-Command", &ps]).await?;
    } else {
        return Err("Media control is not supported on this platform.".to_string());
    }

    Ok(format!("{}.", label(a)))
}

/// `prev`→`previous`, `toggle`→`playpause`; everything else passes through.
fn normalize(a: &str) -> &str {
    match a {
        "prev" => "previous",
        "toggle" => "playpause",
        other => other,
    }
}

/// Human label for the confirmation string.
fn label(a: &str) -> &'static str {
    match a {
        "play" => "Playing",
        "pause" => "Paused",
        "playpause" => "Toggled playback",
        "next" => "Skipped ahead",
        "previous" => "Skipped back",
        "stop" => "Stopped",
