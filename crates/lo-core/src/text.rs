//! Text helpers shared by the brain and the TTS pipeline (ported from
//! `src/shared/text.ts`). Kokoro synthesizes one chunk at a time, so replies are
//! split into short, sentence-aligned chunks that synthesize and play back as a
//! pipeline for low-latency, gapless speech.

/// Keep chunks short for snappy, gapless playback.
pub const TTS_MAX_CHARS: usize = 190;

/// Remove `[direction]` tags (1–40 chars between brackets) so on-screen captions
/// read clean, then collapse runs of whitespace.
pub fn strip_directives(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let chars: Vec<char> = text.chars().collect();
    let mut i = 0;
    while i < chars.len() {
        if chars[i] == '[' {
            // Look for a closing ']' within 1..=40 chars, with no nested '['.
            if let Some(close) = find_directive_close(&chars, i) {
                i = close + 1; // skip the whole [..] span
                continue;
            }
        }
        out.push(chars[i]);
        i += 1;
    }
    collapse_ws(&out)
}

/// Returns the index of the matching `]` for a directive opened at `open`, if it
/// lies within 1..=40 inner chars and contains no `[`/`]` in between.
fn find_directive_close(chars: &[char], open: usize) -> Option<usize> {
    let mut j = open + 1;
    let mut inner = 0;
    while j < chars.len() && inner <= 40 {
        match chars[j] {
            ']' => {
                return if inner >= 1 { Some(j) } else { None };
            }
            '[' => return None,
            _ => {
                inner += 1;
                j += 1;
            }
        }
    }
    None
}

/// Split a reply into ≤`max_chars` chunks, preferring sentence boundaries, then
/// clause boundaries, then word boundaries, then a hard cut as a last resort.
/// Bracketed directions are kept inline (they count toward the budget but steer
/// voice, matching the TS behavior).
pub fn chunk_for_tts(input: &str, max_chars: usize) -> Vec<String> {
    let text = collapse_ws(input);
    if text.is_empty() {
