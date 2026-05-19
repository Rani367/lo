//! Clipboard tools — read and write the system clipboard via `arboard`. Ported
//! from `src/main/tools/clipboard.ts` (which used Electron's built-in clipboard).

use arboard::Clipboard;

/// 64 KB read cap, matching the TS `MAX_CLIP`.
const MAX_CLIP: usize = 64 * 1024;

/// Read the clipboard's text. Empty/non-text clipboards return a friendly note;
/// long text is truncated. Never errors (returns the note instead).
pub fn read_clipboard() -> Result<String, String> {
    let mut clipboard = match Clipboard::new() {
        Ok(c) => c,
        // No clipboard available — treat as empty rather than failing the turn.
        Err(_) => return Ok("The clipboard is empty (or holds non-text content).".to_string()),
    };
    let text = clipboard.get_text().unwrap_or_default();
    if text.is_empty() {
        return Ok("The clipboard is empty (or holds non-text content).".to_string());
    }
