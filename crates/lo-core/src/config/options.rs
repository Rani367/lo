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
