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
}

impl PlaybackState {
    /// Fresh, idle state.
    pub fn new() -> Self {
        Self {
            flush: AtomicBool::new(false),
            played: AtomicU64::new(0),
            queued: AtomicU64::new(0),
        }
    }

    /// True while audio is queued or the flush barrier is still draining.
    pub fn is_playing(&self) -> bool {
        self.queued.load(Ordering::Relaxed) > 0 && !self.flush.load(Ordering::Relaxed)
    }
}

impl Default for PlaybackState {
    fn default() -> Self {
        Self::new()
    }
}

/// Producer/consumer halves of the playback path.
///
/// `pcm_*` carries device-rate mono f32 to the output callback; `tee_*` carries
/// a copy of what the callback emitted, for the spectrum analyser.
pub struct PlaybackRings {
    /// Device-rate mono f32, written by `enqueue_pcm`, read by the output cb.
    pub pcm_prod: Producer<f32>,
    pub pcm_cons: Consumer<f32>,
    /// Tee of emitted output samples, read by the spectrum analyser.
    pub tee_prod: Producer<f32>,
    pub tee_cons: Consumer<f32>,
    /// Shared play/flush state.
    pub state: Arc<PlaybackState>,
}

impl PlaybackRings {
    /// Allocate the playback rings (capacities in samples).
    pub fn new(pcm_capacity: usize, tee_capacity: usize) -> Self {
        let (pcm_prod, pcm_cons) = RingBuffer::<f32>::new(pcm_capacity);
        let (tee_prod, tee_cons) = RingBuffer::<f32>::new(tee_capacity);
        Self {
            pcm_prod,
            pcm_cons,
            tee_prod,
            tee_cons,
            state: Arc::new(PlaybackState::new()),
        }
    }
}

/// RT-safe output-callback body for **one** output channel-frame block.
///
/// Fills `out` (already deinterleaved to a per-channel write helper by the
/// caller via `channels`) by popping mono samples from `pcm_cons`, duplicating
/// each mono sample across `channels`, writing silence on underrun, and teeing
/// the emitted mono stream into `tee_prod`. Honours the flush barrier for
/// instant barge-in (drops queued audio, emits silence).
///
/// `out` length must be `frames * channels`. Pure ring/atomic work — no alloc,
/// no locks, no DSP.
pub fn fill_output(
    out: &mut [f32],
    channels: usize,
    pcm_cons: &mut Consumer<f32>,
    tee_prod: &mut Producer<f32>,
    state: &PlaybackState,
) {
    let channels = channels.max(1);
    let frames = out.len() / channels;

    // Barge-in: drop everything queued and emit silence this block.
    if state.flush.load(Ordering::Relaxed) {
        let drop_n = pcm_cons.slots();
        if drop_n > 0 {
            if let Ok(chunk) = pcm_cons.read_chunk(drop_n) {
                chunk.commit_all();
            }
        }
        state.queued.store(0, Ordering::Relaxed);
        for s in out.iter_mut() {
            *s = 0.0;
        }
        // Ring is now empty; lift the barrier so subsequent enqueues play.
        state.flush.store(false, Ordering::Relaxed);
        return;
    }

    let avail = pcm_cons.slots();
    let take = avail.min(frames);
    let mut emitted = 0usize;

    if take > 0 {
        if let Ok(chunk) = pcm_cons.read_chunk(take) {
            let (a, b) = chunk.as_slices();
            let mut src = a.iter().chain(b.iter());
            for frame in 0..take {
                let mono = *src.next().unwrap_or(&0.0);
                let base = frame * channels;
                for c in 0..channels {
                    out[base + c] = mono;
                }
            }
            chunk.commit_all();
            emitted = take;
        }
    }

    // Underrun → silence for the remaining frames (gapless: queued chunks just
    // resume on the next callback).
    if emitted < frames {
        let base = emitted * channels;
