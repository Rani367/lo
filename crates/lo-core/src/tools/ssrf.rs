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
