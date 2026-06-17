//! Model catalog + hardware-tiered recommendation. Each tier pairs an MLX weight
//! (Apple-Silicon fast path), a GGUF reference (the bundled llama.cpp path), and an
//! Ollama tag, so one logical "model" resolves to the right artifact for whichever
//! backend is active. The recommended tier scales with system RAM.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ModelTier {
    /// Short human label for the HUD/setup UI.
    pub label: &'static str,
    /// Minimum unified/system RAM (GB) this tier wants for a comfortable fit.
    pub min_ram_gb: u32,
    /// MLX weight id (Apple Silicon).
    pub mlx: &'static str,
    /// GGUF reference `owner/repo:file.gguf` (llama.cpp path).
    pub gguf: &'static str,
    /// Ollama model tag for this tier (the `ollama pull` name).
    pub ollama: &'static str,
}

/// Capability ladder, largest first.
pub const MODEL_TIERS: &[ModelTier] = &[
    ModelTier {
        label: "Qwen3-Coder-30B-A3B",
        min_ram_gb: 32,
        mlx: "mlx-community/Qwen3-Coder-30B-A3B-Instruct-4bit-DWQ",
        gguf:
            "unsloth/Qwen3-Coder-30B-A3B-Instruct-GGUF:Qwen3-Coder-30B-A3B-Instruct-UD-Q4_K_XL.gguf",
        ollama: "qwen3-coder:30b",
    },
    ModelTier {
        label: "Qwen3-14B",
        min_ram_gb: 24,
        mlx: "mlx-community/Qwen3-14B-4bit",
        gguf: "unsloth/Qwen3-14B-GGUF:Qwen3-14B-UD-Q4_K_XL.gguf",
        ollama: "qwen3:14b",
    },
    ModelTier {
        label: "Qwen3-8B",
        min_ram_gb: 16,
        mlx: "mlx-community/Qwen3-8B-4bit",
        gguf: "unsloth/Qwen3-8B-GGUF:Qwen3-8B-UD-Q4_K_XL.gguf",
        ollama: "qwen3:8b",
    },
    ModelTier {
        label: "Qwen3-4B",
        min_ram_gb: 8,
        mlx: "mlx-community/Qwen3-4B-4bit",
        gguf: "unsloth/Qwen3-4B-Instruct-2507-GGUF:Qwen3-4B-Instruct-2507-UD-Q4_K_XL.gguf",
        ollama: "qwen3:4b",
    },
];

/// Total system memory in GB. `LO_RAM_GB` overrides for testing; otherwise the
/// caller passes the OS-reported value (the bin crate uses `sysinfo`).
pub fn total_ram_gb(detected_bytes: u64) -> f64 {
    if let Ok(v) = std::env::var("LO_RAM_GB") {
        if let Ok(n) = v.trim().parse::<f64>() {
            // Reject non-finite values (e.g. `Infinity`) so they can't force the
            // largest tier.
            if n > 0.0 && n.is_finite() {
                return n;
            }
        }
    }
    detected_bytes as f64 / 1e9
}

/// Recommend the largest tier that fits in roughly 80% of available RAM, falling
/// back to the smallest tier on very low-memory machines.
pub fn recommend_tier(ram_gb: f64) -> ModelTier {
    let usable = ram_gb * 0.8;
    MODEL_TIERS
        .iter()
        .copied()
        .find(|t| (t.min_ram_gb as f64) <= usable)
        .unwrap_or_else(|| *MODEL_TIERS.last().expect("non-empty ladder"))
}

/// Find the catalog tier a model id belongs to (by MLX id or GGUF ref), if any.
pub fn tier_for_model(model_id: &str) -> Option<ModelTier> {
    MODEL_TIERS
        .iter()
        .copied()
        .find(|t| t.mlx == model_id || t.gguf == model_id)
}

/// Resolve a GGUF reference for a logical model id. If the id already looks like
/// a GGUF ref it's returned as-is; a known MLX id maps to that tier's GGUF;
/// otherwise an empty string (the caller errors clearly).
pub fn gguf_ref_for_model(model_id: &str) -> String {
    let id = model_id.trim();
    if id.contains(".gguf") {
        return id.to_string();
    }
    tier_for_model(id)
        .map(|t| t.gguf.to_string())
        .unwrap_or_default()
}

/// Map a model id to the local GGUF filename it downloads to. A HuggingFace GGUF
/// reference uses `owner/repo:file.gguf`, so the filename is the part after the
/// last colon (a bare `:` is illegal in filenames on Windows); a plain MLX id maps
/// to its last path segment with a `.gguf` suffix.
pub fn gguf_file_for(model_id: &str) -> String {
    let leaf = model_id
        .rsplit('/')
        .find(|s| !s.is_empty())
        .unwrap_or("model");
    // `repo:file.gguf` → keep only `file.gguf`.
    let leaf = leaf.rsplit(':').next().unwrap_or(leaf);
    if leaf.ends_with(".gguf") {
        leaf.to_string()
    } else {
        format!("{leaf}.gguf")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ram_ladder_uses_80_percent_headroom() {
        // 32 GB * 0.8 = 25.6 → only the 24 GB tier fits, not the 32 GB one.
        assert_eq!(recommend_tier(32.0).label, "Qwen3-14B");
        // 40 GB * 0.8 = 32 → the top tier fits exactly.
        assert_eq!(recommend_tier(40.0).label, "Qwen3-Coder-30B-A3B");
        // 20 GB * 0.8 = 16 → the 8 GB-min (16 GB) tier fits exactly.
        assert_eq!(recommend_tier(20.0).label, "Qwen3-8B");
        // 16 GB * 0.8 = 12.8 → only the 4B tier (8 GB min) fits, after headroom.
        assert_eq!(recommend_tier(16.0).label, "Qwen3-4B");
        // Tiny box → smallest tier (never None).
        assert_eq!(recommend_tier(4.0).label, "Qwen3-4B");
    }

    #[test]
    fn gguf_ref_resolution() {
        // Already a GGUF ref → unchanged.
        let r = "unsloth/Qwen3-8B-GGUF:Qwen3-8B-UD-Q4_K_XL.gguf";
        assert_eq!(gguf_ref_for_model(r), r);
        // MLX id → mapped to the tier's GGUF.
        assert_eq!(
            gguf_ref_for_model("mlx-community/Qwen3-8B-4bit"),
            "unsloth/Qwen3-8B-GGUF:Qwen3-8B-UD-Q4_K_XL.gguf"
        );
        // Unknown → empty.
        assert_eq!(gguf_ref_for_model("nobody/unknown"), "");
    }

    #[test]
    fn gguf_filename_from_id() {
        assert_eq!(
            gguf_file_for("mlx-community/Qwen3-8B-4bit"),
            "Qwen3-8B-4bit.gguf"
        );
        assert_eq!(gguf_file_for("owner/repo/weights.gguf"), "weights.gguf");
        // Catalog refs use the HuggingFace `repo:file.gguf` syntax; the local
        // filename must be just the file part (a `:` is illegal on Windows).
        assert_eq!(
            gguf_file_for("unsloth/Qwen3-8B-GGUF:Qwen3-8B-UD-Q4_K_XL.gguf"),
            "Qwen3-8B-UD-Q4_K_XL.gguf"
        );
        // Every shipped catalog tier resolves to a colon-free filename.
        for tier in MODEL_TIERS {
            let name = gguf_file_for(tier.gguf);
            assert!(!name.contains(':'), "{} has a colon: {name}", tier.label);
            assert!(name.ends_with(".gguf"), "{} not .gguf: {name}", tier.label);
        }
    }
}
