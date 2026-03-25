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
//!
//! RT-safety: the cpal output callback only pops from [`PlaybackRings::pcm_cons`]
//! into the device buffer, tees a copy into [`PlaybackRings::tee_prod`], and
//! reads/clears atomics. No resampling or FFT happens in the callback.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

use rtrb::{Consumer, Producer, RingBuffer};

/// Shared, lock-free state describing whether speech is currently playing and
/// whether a barge-in flush has been requested.
pub struct PlaybackState {
    /// Set by `stop_playback`; the output callback drains the ring + emits
    /// silence while it is set, then clears it once the ring is empty.
    pub flush: AtomicBool,
    /// Monotonic count of samples the output callback has actually emitted as
    /// audio (not silence). Combined with `queued` it tells `is_playing` whether
    /// audio is in flight without locks.
    pub played: AtomicU64,
    /// Number of audio samples currently sitting in the playback ring, kept in
    /// sync by the enqueue path and the output callback.
    pub queued: AtomicU64,
