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
        gguf: "unsloth/Qwen3-4B-Instruct-2507-GGUF:Qwen3-4B-Instruct-2507-UD-Q4_K_XL.gguf",
    },
];

/// Total system memory in GB. `LO_RAM_GB` overrides for testing; otherwise the
/// caller passes the OS-reported value (the bin crate uses `sysinfo`).
pub fn total_ram_gb(detected_bytes: u64) -> f64 {
    if let Ok(v) = std::env::var("LO_RAM_GB") {
        if let Ok(n) = v.trim().parse::<f64>() {
            if n > 0.0 {
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

/// Map a model id to its local GGUF filename (the llama backend's `ggufFileFor`).
pub fn gguf_file_for(model_id: &str) -> String {
    let leaf = model_id
        .split('/')
        .rfind(|s| !s.is_empty())
        .unwrap_or("model");
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
