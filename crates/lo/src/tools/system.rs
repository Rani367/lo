//! System telemetry tool — read-only host info: overview, cpu, memory, disk,
//! battery, network, or all. Ported from `src/main/tools/system.ts`, but backed
//! by `sysinfo` + `starship-battery` instead of per-OS shell-outs.
//!
//! The kind → section mapping reproduces the TS `want()` predicate:
//!   - `overview` includes the host line plus cpu/memory/disk/battery (NOT network).
//!   - `all` includes every section.
//!   - a specific kind includes only that section.

use lo_core::tools::sandbox;
use lo_core::LoSettings;
use starship_battery::units::ratio::percent;
use starship_battery::{Manager, State};
use sysinfo::{Disks, Networks, System};

/// Render the requested telemetry as a single speakable line.
pub async fn system_info(kind: &str, settings: &LoSettings) -> String {
    let kind = match kind {
        "cpu" | "memory" | "disk" | "battery" | "network" | "all" => kind,
        _ => "overview",
    };
    // `want(k)`: all matches everything; overview matches everything but network;
    // otherwise only the exact kind.
    let want = |k: &str| kind == "all" || kind == k || (kind == "overview" && k != "network");

    let mut parts: Vec<String> = Vec::new();

    if kind == "overview" || kind == "all" {
        parts.push(host_line());
    }
    if want("cpu") {
        parts.push(cpu_line());
    }
    if want("memory") {
        parts.push(memory_line());
    }
    if want("disk") {
        parts.push(disk_line(settings));
    }
    if want("battery") {
        parts.push(battery_line());
    }
    if kind == "network" || kind == "all" {
        parts.push(network_line());
    }

    parts
        .into_iter()
        .filter(|p| !p.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

/// `Host {name} — {os} {release} ({arch}), up {uptime}.`
