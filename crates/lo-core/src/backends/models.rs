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
    ModelTier {
        label: "Qwen3-Coder-30B-A3B",
        min_ram_gb: 32,
        mlx: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ",
        gguf:
            "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-UD-Q4_K_XL.gguf",
    },
    ModelTier {
        label: "Qwen3-14B",
        min_ram_gb: 24,
        mlx: "mlx-community/Qwen3-14B-4bit",
        gguf: "unsloth/Qwen3-14B-GGUF:Qwen3-14B-UD-Q4_K_XL.gguf",
    },
    ModelTier {
        label: "Qwen3-8B",
        min_ram_gb: 16,
        mlx: "mlx-community/Qwen3-8B-4bit",
        gguf: "unsloth/Qwen3-8B-GGUF:Qwen3-8B-UD-Q4_K_XL.gguf",
    },
    ModelTier {
        label: "Qwen3-4B",
        min_ram_gb: 8,
        mlx: "mlx-community/Qwen3-4B-4bit",
