//! Model catalog + hardware-tiered recommendation (ported from
//! `src/main/backends/models.ts`). Each tier pairs an MLX weight (Apple-Silicon
//! fast path) with a GGUF reference (the bundled llama.cpp path), so one logical
//! "model" resolves to the right artifact for whichever backend is active.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelTier {
    /// Short human label for the HUD/setup UI.
    pub label: &'static str,
    /// Minimum unified/system RAM (GB) this tier wants for a comfortable fit.
    pub min_ram_gb: u32,
    /// MLX weight id (Apple Silicon).
    pub mlx: &'static str,
    /// GGUF reference `owner/repo:file.gguf` (llama.cpp / Ollama path).
    pub gguf: &'static str,
}

/// Capability ladder, largest first.
pub const MODEL_TIERS: &[ModelTier] = &[
