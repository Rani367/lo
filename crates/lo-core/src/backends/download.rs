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
}

/// Resolve a HuggingFace GGUF download URL from a GGUF reference. Accepts
/// `owner/repo:path/to/file.gguf` or `owner/repo/file.gguf`. Returns `""` when
/// the input isn't a GGUF reference (mirrors `resolveGgufUrl`).
pub fn resolve_gguf_url(reference: &str) -> String {
    let id = reference.trim();
    if !id.ends_with(".gguf") {
        return String::new();
    }
    if let Some(colon) = id.find(':') {
        if colon > 0 {
            let repo = &id[..colon];
            let file = &id[colon + 1..];
            return format!("https://huggingface.co/{repo}/resolve/main/{file}?download=true");
        }
    }
    let parts: Vec<&str> = id.split('/').collect();
    if parts.len() >= 3 {
        let repo = parts[..2].join("/");
        let file = parts[2..].join("/");
        return format!("https://huggingface.co/{repo}/resolve/main/{file}?download=true");
    }
    String::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names() -> Vec<String> {
        // A representative slice of real ggml-org/llama.cpp release asset names.
        [
            "llama-b1234-bin-macos-arm64.zip",
            "llama-b1234-bin-macos-x64.zip",
            "llama-b1234-bin-ubuntu-x64.zip",
            "llama-b1234-bin-ubuntu-vulkan-x64.zip",
            "llama-b1234-bin-win-cpu-x64.zip",
            "llama-b1234-bin-win-cuda-x64.zip",
            "llama-b1234-bin-win-vulkan-x64.zip",
            "cudart-llama-bin-win-cuda-x64.zip", // a dependency archive, not the engine
            "llama-b1234-bin-win-arm64.zip",
            "llama-b1234.tar.gz", // not a zip
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }

    #[test]
    fn picks_macos_arm64_baseline() {
        let got = match_llama_asset(
            &names(),
            HostTarget {
                platform: "darwin",
                arch: "arm64",
                variant: "cpu",
            },
        );
        assert_eq!(got.as_deref(), Some("llama-b1234-bin-macos-arm64.zip"));
    }

    #[test]
    fn picks_explicit_cpu_on_windows() {
        let got = match_llama_asset(
            &names(),
            HostTarget {
                platform: "win32",
                arch: "x64",
                variant: "cpu",
            },
        );
        assert_eq!(got.as_deref(), Some("llama-b1234-bin-win-cpu-x64.zip"));
    }

    #[test]
    fn picks_accelerated_build_when_requested() {
        let got = match_llama_asset(
            &names(),
            HostTarget {
                platform: "linux",
                arch: "x64",
                variant: "vulkan",
            },
        );
        assert_eq!(
            got.as_deref(),
            Some("llama-b1234-bin-ubuntu-vulkan-x64.zip")
        );
        let got = match_llama_asset(
            &names(),
            HostTarget {
                platform: "win32",
                arch: "x64",
                variant: "cuda",
            },
        );
        // shortest cuda build (the engine zip, not the longer cudart dependency)
        assert_eq!(got.as_deref(), Some("llama-b1234-bin-win-cuda-x64.zip"));
    }

    #[test]
    fn linux_baseline_avoids_accel_builds() {
        let got = match_llama_asset(
            &names(),
            HostTarget {
                platform: "linux",
                arch: "x64",
                variant: "cpu",
            },
        );
        assert_eq!(got.as_deref(), Some("llama-b1234-bin-ubuntu-x64.zip"));
    }

    #[test]
