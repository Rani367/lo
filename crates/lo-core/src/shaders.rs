//! Embedded GPU shader sources. Kept in `lo-core` so they're version-controlled
//! next to the (tested) host-side uniform contract; the `lo` binary crate feeds
//! [`ORB_WGSL`] to wgpu.

/// The "living core" orb shader (vertex + fragment). See the header comment in
/// the `.wgsl` file for the uniform-buffer layout the host struct must mirror.
pub const ORB_WGSL: &str = include_str!("../assets/shaders/orb.wgsl");

#[cfg(test)]
mod tests {
    use super::ORB_WGSL;

    /// The orb WGSL must parse and pass naga validation (catches uniform
    /// misalignment, type mismatch, and missing entry points without a GPU).
    #[test]
    fn orb_wgsl_parses_and_validates() {
        let module = naga::front::wgsl::parse_str(ORB_WGSL).expect("orb.wgsl should parse as WGSL");
        let mut validator = naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        );
        validator
            .validate(&module)
            .expect("orb.wgsl should pass naga validation");
    }

    #[test]
    fn declares_both_entry_points() {
        let module = naga::front::wgsl::parse_str(ORB_WGSL).unwrap();
        let stages: Vec<_> = module
            .entry_points
            .iter()
            .map(|e| e.name.as_str())
            .collect();
        assert!(stages.contains(&"vs_main"), "missing vertex entry point");
        assert!(stages.contains(&"fs_main"), "missing fragment entry point");
    }
}
