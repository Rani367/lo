//! SSRF guard (ported byte-for-byte from `isPrivateIp` / `assertPublicHost` in
//! `src/main/tools/web.ts`). The DNS resolution + redirect-following transport
//! lives in the `lo` binary; this module owns the IP/host classification, which
//! is the security-critical, exhaustively-tested part.

/// True if `ip` is a private/loopback/link-local/ULA/CGNAT/multicast/unspecified
/// address that must never be fetched. Operates on the string form (incl.
/// IPv4-mapped IPv6) to match the original logic exactly.
pub fn is_private_ip(ip: &str) -> bool {
    // IPv4-mapped IPv6, e.g. ::ffff:169.254.169.254
    let v4 = ip.strip_prefix("::ffff:").unwrap_or(ip);

    if let Some((a, b)) = parse_v4_first_two(v4) {
        if a == 10 || a == 127 || a == 0 {
            return true;
        }
        if a == 169 && b == 254 {
            return true; // link-local
        }
        if a == 172 && (16..=31).contains(&b) {
            return true;
        }
        if a == 192 && b == 168 {
            return true;
        }
        if a == 100 && (64..=127).contains(&b) {
            return true; // CGNAT
        }
        if a >= 224 {
            return true; // multicast (224.0.0.0/4) + reserved/broadcast
        }
        return false;
    }

    let lc = ip.to_lowercase();
    if lc == "::1" || lc == "::" {
        return true; // loopback / unspecified
    }
    if lc.starts_with("fe80") || lc.starts_with("fc") || lc.starts_with("fd") {
        return true; // link-local / ULA
    }
    if lc.starts_with("ff") {
        return true; // multicast (ff00::/8)
    }
    false
}

/// Parse a dotted-quad IPv4 (exactly four all-digit octets, matching
/// `/^\d+\.\d+\.\d+\.\d+$/`) and return its first two octets.
fn parse_v4_first_two(s: &str) -> Option<(u16, u16)> {
    let parts: Vec<&str> = s.split('.').collect();
    if parts.len() != 4 {
        return None;
    }
    if !parts
        .iter()
        .all(|p| !p.is_empty() && p.bytes().all(|c| c.is_ascii_digit()))
    {
        return None;
    }
    // JS `Number("999")` allows >255; mirror that (we only branch on ranges).
    let a = parts[0].parse::<u32>().ok()? as u16;
    let b = parts[1].parse::<u32>().ok()? as u16;
    Some((a, b))
}

/// Reason a host literal is rejected before any DNS lookup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostReject {
    PrivateLiteral,
    Localhost,
    DotLocal,
}

/// The pre-DNS literal check from `assertPublicHost`: reject a private-IP
/// literal, `localhost`, or any `*.local` host. `None` means "passes the literal
/// check; resolve + re-validate every record next".
pub fn reject_literal_host(hostname: &str) -> Option<HostReject> {
    let literal = hostname.trim_start_matches('[').trim_end_matches(']');
    if is_private_ip(literal) {
        return Some(HostReject::PrivateLiteral);
    }
    if hostname == "localhost" {
        return Some(HostReject::Localhost);
    }
    if hostname.ends_with(".local") {
        return Some(HostReject::DotLocal);
    }
    None
