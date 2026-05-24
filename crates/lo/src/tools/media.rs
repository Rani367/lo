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
