//! First-run asset *resolution* for the managed llama.cpp backend (ported from
//! the pure logic of `src/main/backends/download.ts`): choosing the right
//! `llama-server` release asset for this host, and turning a GGUF reference into
//! a HuggingFace download URL.
//!
//! The actual streaming download + zip extraction (which need `reqwest`/`zip`)
//! lands in the `lo` binary crate; these pure functions are unit-tested here so
//! the asset matrix is provably correct on every platform/arch/variant.

const ACCEL_TOKENS: &[&str] = &[
    "cuda", "vulkan", "hip", "rocm", "sycl", "kompute", "musa", "adreno",
];

/// The repo that ships prebuilt `llama-server` binaries.
pub const LLAMA_REPO: &str = "ggml-org/llama.cpp";

/// Host descriptor for asset matching. `platform`/`arch` use the Node spellings
/// (`darwin`/`win32`/`linux`, `arm64`/`x64`) so the matrix mirrors the TS tests.
#[derive(Debug, Clone, Copy)]
pub struct HostTarget<'a> {
    pub platform: &'a str,
    pub arch: &'a str,
    pub variant: &'a str,
}

impl HostTarget<'_> {
    /// The current host, mapped from `std::env::consts` to the Node spellings.
    pub fn current(variant: &str) -> HostTarget<'_> {
        let platform = match std::env::consts::OS {
