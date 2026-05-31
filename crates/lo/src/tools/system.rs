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
fn host_line() -> String {
    let host = System::host_name().unwrap_or_else(|| "unknown".to_string());
    let os = System::name().unwrap_or_else(|| std::env::consts::OS.to_string());
    let release = System::os_version().unwrap_or_default();
    let arch = System::cpu_arch();
    format!(
        "Host {host} — {os} {release} ({arch}), up {}.",
        uptime(System::uptime())
    )
}

/// `CPU: {brand} × {count}[, load {one:.2}].`
fn cpu_line() -> String {
    let mut sys = System::new();
    sys.refresh_cpu_all();
    let cpus = sys.cpus();
    let model = cpus
        .first()
        .map(|c| {
            let brand = c.brand().trim();
            if brand.is_empty() {
                c.name().to_string()
            } else {
                brand.to_string()
            }
        })
        .unwrap_or_else(|| "unknown".to_string());
    let load = System::load_average().one;
    let load_part = if load > 0.0 {
        format!(", load {load:.2}")
    } else {
        String::new()
    };
    format!("CPU: {model} × {}{load_part}.", cpus.len())
}

/// `Memory: {used} used of {total} ({free} free).`
fn memory_line() -> String {
    let mut sys = System::new();
    sys.refresh_memory();
    let total = sys.total_memory();
    let free = sys.free_memory();
    let used = total.saturating_sub(free);
    format!(
        "Memory: {} used of {} ({} free).",
        gb(used),
        gb(total),
        gb(free)
    )
}

/// `Disk ({root}): {used} used of {total} ({free} free).` for the first allowed
/// root's filesystem. Empty string if the root can't be matched to a disk.
fn disk_line(settings: &LoSettings) -> String {
    let target = sandbox::allowed_roots(settings)
        .into_iter()
        .next()
        .unwrap_or_else(lo_core::config::paths::home_dir);

    let disks = Disks::new_with_refreshed_list();
    // Pick the disk whose mount point is the longest prefix of `target` (i.e. the
    // filesystem the root actually lives on), matching `statfs(target)`.
    let best = disks
        .list()
        .iter()
        .filter(|d| target.starts_with(d.mount_point()))
        .max_by_key(|d| d.mount_point().as_os_str().len());

    match best {
