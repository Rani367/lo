//! Single source of truth for the option lists (ported from `src/shared/options.ts`).

/// Kokoro voices (warm American first — Lo's default voice).
pub const VOICES: &[&str] = &[
    "af_heart", // American female — Lo's default
    "af_bella",
    "am_michael", // American male
    "am_adam",
    "bf_emma",
    "bf_isabella",
    "bm_george",
    "bm_lewis",
];

/// Brain capability ladder (MLX ids). On non-Apple-Silicon these map to the
/// matching GGUF via `backends::models`; users may also type a GGUF ref.
pub const MODELS: &[&str] = &[
    "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ",
    "mlx-community/Qwen3-14B-4bit",
    "mlx-community/Qwen3-8B-4bit",
    "mlx-community/Qwen3-4B-4bit",
];

pub const ASR_MODELS: &[&str] = &[
    "mlx-community/parakeet-tdt-0.6b-v3",
    "mlx-community/whisper-large-v3-turbo",
];

/// Selectable engines. `auto` suits almost everyone.
pub const BACKENDS: &[&str] = &["auto", "mlx", "llama", "ollama", "custom"];
