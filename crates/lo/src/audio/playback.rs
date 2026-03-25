//! Speech playback plumbing.
//!
//! Ports `playback.ts`: raw mono PCM (e.g. Kokoro 24 kHz) is resampled to the
//! device output rate and scheduled back-to-back for gapless speech, while a
//! spectrum tee feeds the "speaking" orb (replacing the Web Audio
//! `AnalyserNode`).
//!
//! In the browser, gaplessness came from scheduling each buffer at
//! `max(currentTime + 0.02, nextStartTime)`. Here it falls out for free: the
//! output callback pulls from a single ring at the device rate and writes
//! silence whenever the ring runs dry, so queued chunks play seamlessly and
//! underruns are silent rather than glitchy.
