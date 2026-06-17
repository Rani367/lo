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
        return Vec::new();
    }

    let sentences = split_sentences(&text);

    let mut chunks: Vec<String> = Vec::new();
    let mut buf = String::new();

    let flush = |buf: &mut String, chunks: &mut Vec<String>| {
        let t = buf.trim();
        if !t.is_empty() {
            chunks.push(t.to_string());
        }
        buf.clear();
    };

    for sentence in sentences {
        let s = sentence.trim();
        if s.is_empty() {
            continue;
        }
        if char_len(s) > max_chars {
            flush(&mut buf, &mut chunks);
            chunks.extend(hard_split(s, max_chars));
            continue;
        }
        // (buf + ' ' + s) length, trimmed.
        let candidate = if buf.is_empty() {
            s.to_string()
        } else {
            format!("{buf} {s}")
        };
        if char_len(candidate.trim()) > max_chars {
            flush(&mut buf, &mut chunks);
            buf = s.to_string();
        } else {
            buf = candidate;
        }
    }
    flush(&mut buf, &mut chunks);
    chunks
}

/// Convenience wrapper using the default `TTS_MAX_CHARS`.
pub fn chunk_for_tts_default(input: &str) -> Vec<String> {
    chunk_for_tts(input, TTS_MAX_CHARS)
}

/// Mirror the JS regex `/[^.!?]+[.!?]+(?:["')\]]+)?|\S[^.!?]*$/g`: a run up to and
/// including terminal punctuation (plus trailing closing quotes/brackets), or a
/// final non-terminated fragment.
fn split_sentences(text: &str) -> Vec<String> {
    let chars: Vec<char> = text.chars().collect();
    let mut out: Vec<String> = Vec::new();
    let mut start = 0;
    let mut i = 0;
    let n = chars.len();
    let is_term = |c: char| c == '.' || c == '!' || c == '?';
    let is_closer = |c: char| c == '"' || c == '\'' || c == ')' || c == ']';

    while i < n {
        if is_term(chars[i]) {
            // A '.' between two digits is a decimal point (e.g. "3.5"), not a
            // sentence end — splitting there makes Kokoro read it as two chunks.
            if chars[i] == '.'
                && i > 0
                && i + 1 < n
                && chars[i - 1].is_ascii_digit()
                && chars[i + 1].is_ascii_digit()
            {
                i += 1;
                continue;
            }
            // consume the run of terminal punctuation
            while i < n && is_term(chars[i]) {
                i += 1;
            }
            // optional trailing closers
            while i < n && is_closer(chars[i]) {
                i += 1;
            }
            let seg: String = chars[start..i].iter().collect();
            if !seg.trim().is_empty() {
                out.push(seg);
            }
            // skip a single separating space (the regex's `[^.!?]+` re-consumes leading ws)
            start = i;
        } else {
            i += 1;
        }
    }
    if start < n {
        let seg: String = chars[start..n].iter().collect();
        if !seg.trim().is_empty() {
            out.push(seg);
        }
    }
    if out.is_empty() {
        out.push(text.to_string());
    }
    out
}

/// Split an over-long fragment on clause/word/char boundaries.
fn hard_split(s: &str, max_chars: usize) -> Vec<String> {
    let mut out: Vec<String> = Vec::new();
    let mut rest: Vec<char> = s.chars().collect();
    while rest.len() > max_chars {
        let mut cut = last_boundary_before(&rest, max_chars);
        if cut == 0 {
            cut = max_chars; // no boundary found — hard cut
        }
        let head: String = rest[..cut].iter().collect();
        out.push(head.trim().to_string());
        rest = rest[cut..].to_vec();
        // trim leading whitespace of the remainder
        while rest.first().is_some_and(|c| c.is_whitespace()) {
            rest.remove(0);
        }
    }
    if !rest.is_empty() {
        let tail: String = rest.iter().collect();
        let t = tail.trim();
        if !t.is_empty() {
            out.push(t.to_string());
        }
    }
    out
}

/// Index (in chars) just after the last clause/whitespace boundary within
/// `limit`, or 0 if none past the halfway point — mirrors `lastBoundaryBefore`.
fn last_boundary_before(s: &[char], limit: usize) -> usize {
    let window = &s[..limit.min(s.len())];
    // Prefer clause punctuation followed by whitespace, then plain whitespace.
    let is_clause = |c: char| matches!(c, ',' | ';' | ':' | '—' | '-');
    // pass 1: clause punctuation + whitespace
    {
        let mut idx = 0usize;
        for w in 0..window.len().saturating_sub(1) {
            if is_clause(window[w]) && window[w + 1].is_whitespace() {
                idx = w + 2; // index after the punctuation + space
            }
        }
        if idx as f64 > limit as f64 * 0.5 {
            return idx;
        }
    }
    // pass 2: any whitespace
    {
        let mut idx = 0usize;
        for (w, &c) in window.iter().enumerate() {
            if c.is_whitespace() {
                idx = w + 1;
            }
        }
        if idx as f64 > limit as f64 * 0.5 {
            return idx;
        }
    }
    0
}

/// Collapse all runs of ASCII/Unicode whitespace to single spaces and trim.
fn collapse_ws(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn char_len(s: &str) -> usize {
    s.chars().count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_input_yields_no_chunks() {
        assert!(chunk_for_tts_default("").is_empty());
        assert!(chunk_for_tts_default("   \n\t ").is_empty());
    }

    #[test]
    fn short_reply_is_a_single_chunk() {
        let chunks = chunk_for_tts_default("It is 3pm.");
        assert_eq!(chunks, vec!["It is 3pm.".to_string()]);
    }

    #[test]
    fn splits_on_sentence_boundaries_under_budget() {
        let s = "First sentence here. Second one follows! Third?";
        let chunks = chunk_for_tts(s, 30);
        // Each chunk must respect the budget and preserve terminal punctuation.
        for c in &chunks {
            assert!(c.chars().count() <= 30, "chunk too long: {c:?}");
        }
        assert_eq!(chunks.join(" "), s);
    }

    #[test]
    fn over_long_sentence_is_hard_split_within_budget() {
        let long = "word ".repeat(80); // 400 chars, no sentence punctuation
        let chunks = chunk_for_tts(long.trim(), 50);
        assert!(chunks.len() > 1);
        for c in &chunks {
            assert!(
                c.chars().count() <= 50,
                "chunk too long: {} ({c:?})",
                c.len()
            );
        }
    }

    #[test]
    fn strip_directives_removes_bracketed_spans() {
        assert_eq!(
            strip_directives("[dry] Hello [warmly] there"),
            "Hello there"
        );
        // An unclosed / over-long bracket is left as-is.
        assert_eq!(strip_directives("a [b"), "a [b");
        let long = format!("x [{}] y", "z".repeat(50));
        assert_eq!(strip_directives(&long), long.replace("  ", " "));
    }

    #[test]
    fn decimal_points_do_not_split_sentences() {
        // A small budget forces one chunk per sentence; "3.5" must stay intact
        // inside its sentence rather than being split at the decimal point.
        let chunks = chunk_for_tts("It costs 3.5 dollars total. Thanks for asking.", 30);
        assert_eq!(chunks.len(), 2, "{chunks:?}");
        assert!(chunks[0].contains("3.5 dollars"), "{chunks:?}");
        assert!(chunks[1].starts_with("Thanks"), "{chunks:?}");
    }

    #[test]
    fn no_chunk_exceeds_default_budget() {
        let para = "Lo is a fast, local AI agent that runs entirely on your own machine, \
            with no cloud, no API keys, and no data leaving your computer at all, which is \
            rather the whole point of the thing if you stop and think about it for a moment.";
        for c in chunk_for_tts_default(para) {
            assert!(c.chars().count() <= TTS_MAX_CHARS, "too long: {c:?}");
        }
    }
}
