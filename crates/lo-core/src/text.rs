//! Text helpers shared by the brain and the TTS pipeline (ported from
//! `src/shared/text.ts`). Kokoro synthesizes one chunk at a time, so replies are
//! split into short, sentence-aligned chunks that synthesize and play back as a
//! pipeline for low-latency, gapless speech.

/// Keep chunks short for snappy, gapless playback.
pub const TTS_MAX_CHARS: usize = 190;

/// Remove `[direction]` tags (1–40 chars between brackets) so on-screen captions
