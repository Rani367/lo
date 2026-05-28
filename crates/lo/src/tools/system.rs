//! System telemetry tool — read-only host info: overview, cpu, memory, disk,
//! battery, network, or all. Ported from `src/main/tools/system.ts`, but backed
//! by `sysinfo` + `starship-battery` instead of per-OS shell-outs.
//!
//! The kind → section mapping reproduces the TS `want()` predicate:
//!   - `overview` includes the host line plus cpu/memory/disk/battery (NOT network).
//!   - `all` includes every section.
//!   - a specific kind includes only that section.
