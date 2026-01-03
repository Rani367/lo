//! First-run asset *resolution* for the managed llama.cpp backend (ported from
//! the pure logic of `src/main/backends/download.ts`): choosing the right
//! `llama-server` release asset for this host, and turning a GGUF reference into
//! a HuggingFace download URL.
//!
//! The actual streaming download + zip extraction (which need `reqwest`/`zip`)
