//! Media control — play/pause/next/previous/stop the active media player.
//! Ported from `src/main/tools/media.ts`. Per-OS mechanism: AppleScript to the
//! running player on macOS, `playerctl` on Linux, and the Windows media
//! virtual-keys via `keybd_event`.

use tokio::process::Command;

/// Control playback. `action` is one of play/pause/playpause/next/previous/stop
/// (with `prev`→`previous` and `toggle`→`playpause` aliases). Returns
