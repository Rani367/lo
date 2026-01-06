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
            "macos" => "darwin",
            "windows" => "win32",
            _ => "linux",
        };
        let arch = if std::env::consts::ARCH == "aarch64" {
            "arm64"
        } else {
            "x64"
        };
        HostTarget {
            platform,
            arch,
            variant,
        }
    }
}

/// Choose the best llama.cpp release asset for this host. Pure (mirrors
/// `matchLlamaAsset`). `variant` selects the accelerator: `cpu` (default
/// baseline) prefers the build with no accelerator token; `cuda`/`vulkan`/…
/// prefer that token.
pub fn match_llama_asset(names: &[String], target: HostTarget) -> Option<String> {
    let plat_toks: &[&str] = match target.platform {
        "darwin" => &["macos"],
        "win32" => &["win"],
        _ => &["ubuntu", "linux"],
    };
    let arch_tok = if target.arch == "arm64" {
        "arm64"
    } else {
        "x64"
    };

    let zips: Vec<&String> = names
        .iter()
        .filter(|n| {
            let lc = n.to_lowercase();
            lc.ends_with(".zip") && lc.contains("bin")
        })
        .collect();

    let candidates: Vec<&String> = zips
        .into_iter()
        .filter(|n| {
            let lc = n.to_lowercase();
            plat_toks.iter().any(|p| lc.contains(p)) && lc.contains(arch_tok)
        })
        .collect();

    if candidates.is_empty() {
        return None;
    }

    let wants_accel = !target.variant.is_empty() && target.variant != "cpu";
    if wants_accel {
        let v = target.variant.to_lowercase();
        let accel: Vec<&String> = candidates
            .iter()
            .copied()
            .filter(|n| n.to_lowercase().contains(&v))
            .collect();
        if !accel.is_empty() {
            return shortest(&accel);
        }
    }

    // Baseline: prefer an explicit `cpu` build (Windows), else one with no accel token.
    let explicit_cpu: Vec<&String> = candidates
        .iter()
        .copied()
        .filter(|n| n.to_lowercase().contains("cpu"))
        .collect();
    if !explicit_cpu.is_empty() {
        return shortest(&explicit_cpu);
    }
    let no_accel: Vec<&String> = candidates
        .iter()
        .copied()
        .filter(|n| {
            let lc = n.to_lowercase();
            !ACCEL_TOKENS.iter().any(|t| lc.contains(t))
        })
        .collect();
    shortest(if no_accel.is_empty() {
        &candidates
    } else {
        &no_accel
    })
}

fn shortest(list: &[&String]) -> Option<String> {
    list.iter().min_by_key(|s| s.len()).map(|s| s.to_string())
