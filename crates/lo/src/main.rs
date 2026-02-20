//! Lo — pure-native Rust voice agent. Entry point + threading model.
//!
//! The winit event loop owns the main thread (it is intentionally NOT async). A
//! multi-thread tokio runtime hosts the worker (brain loop, backends, tools,
//! downloads). On-device ML (whisper ASR, Silero VAD) runs on a dedicated
//! "listen" std thread that owns the !Send models; Kokoro TTS runs on its own
//! std thread (spawned by the worker). cpal audio streams (also !Send) live on
//! the main thread inside the `App`.
//!
//! Bridges (replacing Electron IPC, see `events`):
//!   - UI/listen → worker: `mpsc::UnboundedSender<UiCommand>`
//!   - worker/ML → UI: `EventLoopProxy<AppEvent>` → `ApplicationHandler::user_event`
//!   - barge-in epoch: a `watch::channel<u64>` the UI bumps and the worker/TTS
