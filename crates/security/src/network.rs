/// Network security utilities — SSRF protection and internal URL detection.
use std::net::{IpAddr, ToSocketAddrs};
use std::sync::RwLock;

use once_cell::sync::Lazy;
use regex::Regex;

/// A CIDR block with pre-computed mask bits, matching Python's
/// `ipaddress.IPv4Network` / `IPv6Network` semantics for "address ∈ network".
#[derive(Debug, Clone, Copy)]
enum CidrBlock {
    V4 { base: u32, prefix: u8 },
    V6 { base: u128, prefix: u8 },
}

impl CidrBlock {
    fn parse(cidr: &str) -> Option<Self> {
        let (addr_part, prefix_part) = match cidr.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (cidr, None),
        };
        let addr: IpAddr = addr_part.parse().ok()?;
        match addr {
            IpAddr::V4(v4) => {
                let prefix: u8 = prefix_part.unwrap_or("32").parse().ok()?;
                if prefix > 32 {
                    return None;
                }
                let mask = mask_u32(prefix);
                let base = u32::from(v4) & mask;
                Some(CidrBlock::V4 { base, prefix })
            }
            IpAddr::V6(v6) => {
                let prefix: u8 = prefix_part.unwrap_or("128").parse().ok()?;
                if prefix > 128 {
                    return None;
                }
                let mask = mask_u128(prefix);
                let base = u128::from(v6) & mask;
                Some(CidrBlock::V6 { base, prefix })
            }
        }
    }

    fn contains(&self, addr: IpAddr) -> bool {
        match (*self, addr) {
            (CidrBlock::V4 { base, prefix }, IpAddr::V4(v4)) => {
                let mask = mask_u32(prefix);
                (u32::from(v4) & mask) == base
            }
            (CidrBlock::V6 { base, prefix }, IpAddr::V6(v6)) => {
                let mask = mask_u128(prefix);
                (u128::from(v6) & mask) == base
            }
            _ => false,
        }
    }
}

fn mask_u32(prefix: u8) -> u32 {
    if prefix == 0 {
        0
    } else {
        (!0u32) << (32 - prefix)
    }
}

fn mask_u128(prefix: u8) -> u128 {
    if prefix == 0 {
        0
    } else {
        (!0u128) << (128 - prefix)
    }
}

static BLOCKED_NETWORKS: Lazy<Vec<CidrBlock>> = Lazy::new(|| {
    [
        "0.0.0.0/8",
        "10.0.0.0/8",
        "100.64.0.0/10", // carrier-grade NAT
        "127.0.0.0/8",
        "169.254.0.0/16", // link-local / cloud metadata
        "172.16.0.0/12",
        "192.168.0.0/16",
        "::1/128",
        "fc00::/7",  // unique local
        "fe80::/10", // link-local v6
    ]
    .iter()
    .map(|s| CidrBlock::parse(s).expect("valid static CIDR"))
    .collect()
});

static URL_RE: Lazy<Regex> = Lazy::new(|| Regex::new(r#"(?i)https?://[^\s"'`;|<>]+"#).unwrap());

static ALLOWED_NETWORKS: Lazy<RwLock<Vec<CidrBlock>>> = Lazy::new(|| RwLock::new(Vec::new()));

/// Allow specific CIDR ranges to bypass SSRF blocking
/// (e.g. Tailscale's `100.64.0.0/10`).
pub fn configure_ssrf_whitelist(cidrs: &[String]) {
    let nets: Vec<CidrBlock> = cidrs
        .iter()
        .filter_map(|c| CidrBlock::parse(c.as_str()))
        .collect();
    if let Ok(mut guard) = ALLOWED_NETWORKS.write() {
        *guard = nets;
    }
}

/// Clear any previously-configured SSRF whitelist entries.
pub fn clear_ssrf_whitelist() {
    if let Ok(mut guard) = ALLOWED_NETWORKS.write() {
        guard.clear();
    }
}

fn is_private(addr: IpAddr) -> bool {
    if let Ok(guard) = ALLOWED_NETWORKS.read() {
        if !guard.is_empty() && guard.iter().any(|n| n.contains(addr)) {
            return false;
        }
    }
    BLOCKED_NETWORKS.iter().any(|n| n.contains(addr))
}

/// Parse out the scheme / host / optional port of a URL, mirroring the
/// subset of `urllib.parse.urlparse` used by the Python module.
///
/// Returns (scheme, netloc, hostname). `hostname` strips surrounding
/// brackets for IPv6 literals and the trailing port when present.
fn parse_url(url: &str) -> Option<(String, String, String)> {
    let scheme_end = url.find("://")?;
    let scheme = url[..scheme_end].to_lowercase();
    let rest = &url[scheme_end + 3..];
    let netloc_end = rest
        .find(|c: char| c == '/' || c == '?' || c == '#')
        .unwrap_or(rest.len());
    let netloc = &rest[..netloc_end];
    if netloc.is_empty() {
        return None;
    }
    let host_with_port = match netloc.rfind('@') {
        Some(idx) => &netloc[idx + 1..],
        None => netloc,
    };
    let hostname = if let Some(stripped) = host_with_port.strip_prefix('[') {
        let close = stripped.find(']')?;
        stripped[..close].to_string()
    } else {
        host_with_port
            .rsplit_once(':')
            .map(|(h, _port)| h.to_string())
            .unwrap_or_else(|| host_with_port.to_string())
    };
    Some((scheme, netloc.to_string(), hostname))
}

fn resolve_host(hostname: &str) -> Result<Vec<IpAddr>, std::io::Error> {
    let iter = (hostname, 0u16).to_socket_addrs()?;
    Ok(iter.map(|sa| sa.ip()).collect())
}

/// Validate a URL is safe to fetch: scheme, hostname, and resolved IPs.
///
/// Returns `(ok, error_message)`. When `ok` is `true`, `error_message` is empty.
pub fn validate_url_target(url: &str) -> (bool, String) {
    let Some((scheme, netloc, hostname)) = parse_url(url) else {
        return (false, "Missing domain".into());
    };
    if scheme != "http" && scheme != "https" {
        return (
            false,
            format!(
                "Only http/https allowed, got '{}'",
                if scheme.is_empty() { "none" } else { &scheme }
            ),
        );
    }
    if netloc.is_empty() {
        return (false, "Missing domain".into());
    }
    if hostname.is_empty() {
        return (false, "Missing hostname".into());
    }

    let infos = match resolve_host(&hostname) {
        Ok(v) => v,
        Err(_) => {
            return (false, format!("Cannot resolve hostname: {hostname}"));
        }
    };

    for addr in infos {
        if is_private(addr) {
            return (
                false,
                format!("Blocked: {hostname} resolves to private/internal address {addr}"),
            );
        }
    }

    (true, String::new())
}

/// Validate an already-fetched URL (e.g. after redirect). Only checks the
/// IP; skips DNS if the URL is malformed or has no hostname.
pub fn validate_resolved_url(url: &str) -> (bool, String) {
    let Some((_scheme, _netloc, hostname)) = parse_url(url) else {
        return (true, String::new());
    };
    if hostname.is_empty() {
        return (true, String::new());
    }

    if let Ok(ip) = hostname.parse::<IpAddr>() {
        if is_private(ip) {
            return (false, format!("Redirect target is a private address: {ip}"));
        }
        return (true, String::new());
    }
    let infos = match resolve_host(&hostname) {
        Ok(v) => v,
        Err(_) => return (true, String::new()),
    };
    for addr in infos {
        if is_private(addr) {
            return (
                false,
                format!("Redirect target {hostname} resolves to private address {addr}"),
            );
        }
    }
    (true, String::new())
}

/// Return `true` if the command string contains a URL targeting an
/// internal/private address.
pub fn contains_internal_url(command: &str) -> bool {
    for m in URL_RE.find_iter(command) {
        let url = m.as_str();
        if !validate_url_target(url).0 {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn blocked_ipv4_ranges() {
        assert!(is_private("127.0.0.1".parse().unwrap()));
        assert!(is_private("10.0.0.1".parse().unwrap()));
        assert!(is_private("169.254.169.254".parse().unwrap()));
        assert!(is_private("192.168.1.1".parse().unwrap()));
        assert!(is_private("100.64.0.1".parse().unwrap()));
        assert!(!is_private("8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn whitelist_allows_tailscale() {
        configure_ssrf_whitelist(&["100.64.0.0/10".into()]);
        assert!(!is_private("100.64.0.1".parse().unwrap()));
        clear_ssrf_whitelist();
        assert!(is_private("100.64.0.1".parse().unwrap()));
    }

    #[test]
    fn parse_url_basic() {
        assert_eq!(
            parse_url("https://example.com:8080/path"),
            Some((
                "https".into(),
                "example.com:8080".into(),
                "example.com".into()
            ))
        );
        assert_eq!(
            parse_url("http://user:pass@host/p"),
            Some(("http".into(), "user:pass@host".into(), "host".into()))
        );
        assert_eq!(
            parse_url("http://[::1]:8080/"),
            Some(("http".into(), "[::1]:8080".into(), "::1".into()))
        );
    }

    #[test]
    fn validate_url_scheme_guard() {
        let (ok, err) = validate_url_target("ftp://example.com");
        assert!(!ok);
        assert!(err.contains("Only http/https"));
    }

    #[test]
    fn validate_literal_private_ip() {
        let (ok, err) = validate_url_target("http://127.0.0.1/path");
        assert!(!ok);
        assert!(err.contains("private/internal"));
    }

    #[test]
    fn contains_internal_detects_private_url() {
        let cmd = "curl http://169.254.169.254/latest/meta-data && echo done";
        assert!(contains_internal_url(cmd));
        assert!(!contains_internal_url("curl https://example.com"));
    }
}
