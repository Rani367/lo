//! Microphone capture plumbing.
//!
//! Ports the *mic-tap* side of the original `capture-vad.ts`: per-block RMS is
//! smoothed into a 0..1 "listening" level with the exact EMA the renderer used
//! (`level = level*0.6 + rms*0.4`, exposed as `min(1, level*4)`), and 16 kHz
//! mono f32 samples are made available for the ASR/VAD frontend.
//!
//! RT-safety: the cpal input callback only pushes raw device-rate mono f32 into
//! [`CaptureRings::raw_prod`] and updates the level atomic. The resample from
//! device rate to 16 kHz runs on a worker thread that drains the raw ring and
