//! Mono f32 sample-rate conversion built on rubato 3.0.0's asynchronous sinc
//! resampler.
//!
//! Both the capture downsample (device rate -> 16 kHz) and the playback upsample
//! (Kokoro 24 kHz / device rate) run here, on worker threads or the
//! enqueue/drain calls — NEVER inside an audio callback.
//!
//! rubato's [`Async`] resampler is "fixed input": it consumes a fixed number of
//! input frames per `process` call and emits a variable number of output frames.
//! We buffer leftover input across calls so callers can hand us arbitrary-length
//! mono chunks.

use rubato::audioadapter::Adapter;
use rubato::audioadapter_buffers::direct::SequentialSlice;
use rubato::{
