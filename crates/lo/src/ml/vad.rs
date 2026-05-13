//! Voice-activity detection via Silero VAD v5 (ONNX, run through `ort` 2.0).
//!
//! A faithful Rust port of `src/renderer/audio/capture-vad.ts`. The browser used
//! `@ricky0123/vad-web` (`model: 'v5'`) which wraps the same Silero v5 ONNX graph;
//! here we drive that graph directly with `ort` and re-implement the segmentation
//! state machine — including the speculative `SilenceStart` / `SpeechResume`
//! behaviour the TS relied on for low-latency speculative transcription.
//!
//! Frame contract (identical to the TS): **512-sample, 16 kHz mono f32** frames.
//! At 16 kHz, 512 samples ≈ 32 ms, so the timing constants convert to frame counts:
//!   - `positiveSpeechThreshold = 0.6`, `negativeSpeechThreshold = 0.4`
//!   - `minSpeechMs = 150`  (not separately enforced here; see note below)
//!   - `redemptionMs  = 900` → ~28 frames of sub-threshold audio ends the turn
//!   - `preSpeechPadMs = 250` → 7-frame pre-roll prepended to the utterance
