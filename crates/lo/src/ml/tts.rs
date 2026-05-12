//! On-device text-to-speech via Kokoro (the `kokoro-tts` 0.3.3 crate, ONNX under
//! the hood).
//!
//! Ports `src/renderer/ml/tts.ts`: load the ~82M Kokoro model once, then turn one
//! chunk of text into 24 kHz mono f32 PCM per call. The caller chunks prose with
//! [`lo_core::text::chunk_for_tts`] and feeds one chunk per [`Tts::synth`].
//!
//! The kokoro-tts crate expects two local files in the `thewh1teagle/kokoro-onnx`
//! release format: the ONNX graph (`kokoro-v1.0.onnx`) and a single combined
//! voice-style binary (`voices-v1.0.bin`). We fetch both from the HF mirror
//! `leonelhs/kokoro-thewh1teagle`. Voice selection and speed are both expressed
//! through the crate's `Voice` enum (each v1.0 variant carries an `f32` speed).
//!
//! Feature-gated behind `tts-kokoro`; with the feature off, [`load_tts`] returns a
