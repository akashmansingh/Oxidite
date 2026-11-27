//! Origin & Reverse-Proxy (`tailscale serve`) header validation.
//!
//! Security invariants:
//! - Loopback origins (`http://127.0.0.1:*`, `http://localhost:*`, `http://[::1]:*`) are permitted.
//! - Tailscale origins are ONLY permitted when matching an EXACT entry in
//!   `OXIDITE_ALLOWED_ORIGINS` (comma-separated, e.g. `https://victus.tail1234.ts.net`).
//! - Wildcard origins such as `*.ts.net` or `*` are rejected on parse and never honored.
//! - `X-Forwarded-Host` and `X-Forwarded-Proto` are ONLY trusted when the direct TCP peer
//!   (`ConnectInfo<SocketAddr>`) is a loopback address (`127.0.0.1` or `::1`).

use std::collections::HashSet;
use std::net::SocketAddr;

use axum::http::{HeaderMap, header};

#[derive(Debug, Clone)]
pub struct OriginPolicy {
    allowed_exact: HashSet<String>,
}

impl OriginPolicy {
    /// Parses `OXIDITE_ALLOWED_ORIGINS` from environment (comma-separated exact origins).
    pub fn from_env() -> Self {
        let raw = std::env::var("OXIDITE_ALLOWED_ORIGINS").unwrap_or_default();
        Self::from_csv(&raw)
    }

    /// Parses comma-separated exact origins, ignoring empty items and refusing wildcards.
    pub fn from_csv(csv: &str) -> Self {
        let mut allowed_exact = HashSet::new();
        for item in csv.split(',') {
            let trimmed = item.trim().trim_end_matches('/');
            if trimmed.is_empty() || trimmed.contains('*') {
                continue;
            }
            if trimmed.starts_with("https://") || trimmed.starts_with("http://") {
                allowed_exact.insert(trimmed.to_ascii_lowercase());
            }
        }
        Self { allowed_exact }
    }

    /// Validates the `Origin` header on WebSocket upgrades and state-changing REST requests.
    ///
    /// Returns `true` when:
    /// - `Origin` is absent (non-browser CLI/curl client, assuming token auth still applies), OR
    /// - `Origin` is a loopback origin, OR
    /// - `Origin` matches an exact entry in `OXIDITE_ALLOWED_ORIGINS`.
    pub fn is_origin_allowed(&self, headers: &HeaderMap) -> bool {
        let Some(val) = headers.get(header::ORIGIN) else {
            return true;
        };
        let Ok(origin_str) = val.to_str() else {
            return false;
        };
        self.is_origin_str_allowed(origin_str)
    }

    pub fn is_origin_str_allowed(&self, origin: &str) -> bool {
        let normalized = origin.trim().trim_end_matches('/').to_ascii_lowercase();
        if is_loopback_origin(&normalized) {
            return true;
        }
        self.allowed_exact.contains(&normalized)
    }

    /// Resolves the external base URL (e.g. for mobile pairing QR / enrollment links).
    ///
    /// Only honors `X-Forwarded-Host` and `X-Forwarded-Proto` when `peer` is loopback
    /// (`tailscale serve` forwarding to `127.0.0.1:3030`) AND the resulting origin is
    /// either loopback or in `OXIDITE_ALLOWED_ORIGINS`.
    pub fn external_base_url(
        &self,
        peer: SocketAddr,
        headers: &HeaderMap,
        fallback_host: &str,
    ) -> String {
        if peer.ip().is_loopback() {
            if let Some(fwd_host) = header_str(headers, "x-forwarded-host") {
                let proto = header_str(headers, "x-forwarded-proto")
                    .map(|p| p.to_ascii_lowercase())
                    .filter(|p| p == "https" || p == "http")
                    .unwrap_or_else(|| "https".to_string());
                let candidate = format!("{proto}://{}", fwd_host.trim().trim_end_matches('/'));
                if self.is_origin_str_allowed(&candidate) {
                    return candidate;
                }
            }
        }

        if let Some(host) = headers.get(header::HOST).and_then(|v| v.to_str().ok()) {
            let candidate = format!("http://{}", host.trim());
            if is_loopback_origin(&candidate.to_ascii_lowercase()) {
                return candidate;
            }
        }

        format!("http://{fallback_host}")
    }
}

fn header_str<'a>(headers: &'a HeaderMap, name: &str) -> Option<&'a str> {
    headers
        .get(name)
        .and_then(|v| v.to_str().ok())
        .map(|s| s.split(',').next().unwrap_or(s).trim())
        .filter(|s| !s.is_empty())
}

fn is_loopback_origin(normalized_lower: &str) -> bool {
    let Some(rest) = normalized_lower
        .strip_prefix("http://")
        .or_else(|| normalized_lower.strip_prefix("https://"))
    else {
        return false;
    };

    // Strip optional port
    let host = if rest.starts_with('[') {
        match rest.find(']') {
            Some(end) => &rest[..=end],
            None => return false,
        }
    } else {
        rest.split(':').next().unwrap_or(rest)
    };

    matches!(host, "127.0.0.1" | "localhost" | "[::1]")
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::http::HeaderValue;
    use std::net::{IpAddr, Ipv4Addr};

    fn loopback_peer() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::LOCALHOST), 54321)
    }

    fn remote_peer() -> SocketAddr {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(192, 168, 1, 50)), 54321)
    }

    #[test]
    fn allows_exact_configured_tailscale_origin_and_blocks_wildcards_or_other_hosts() {
        let policy = OriginPolicy::from_csv("https://victus.tail999.ts.net, https://*.ts.net");
        assert!(policy.is_origin_str_allowed("https://victus.tail999.ts.net"));
        assert!(policy.is_origin_str_allowed("http://127.0.0.1:3030"));
        assert!(policy.is_origin_str_allowed("http://localhost:5173"));
        assert!(policy.is_origin_str_allowed("http://[::1]:3030"));

        // Arbitrary or spoofed .ts.net domains MUST be blocked
        assert!(!policy.is_origin_str_allowed("https://evil.tail999.ts.net"));
        assert!(!policy.is_origin_str_allowed("https://evil.ts.net"));
        assert!(!policy.is_origin_str_allowed("https://attacker.com"));
    }

    #[test]
    fn trusts_forwarded_headers_only_from_loopback_peer() {
        let policy = OriginPolicy::from_csv("https://victus.tail999.ts.net");
        let mut headers = HeaderMap::new();
        headers.insert(
            "x-forwarded-host",
            HeaderValue::from_static("victus.tail999.ts.net"),
        );
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        assert_eq!(
            policy.external_base_url(loopback_peer(), &headers, "127.0.0.1:3030"),
            "https://victus.tail999.ts.net"
        );
        // Non-loopback peer spoofing X-Forwarded-Host must NOT be trusted
        assert_eq!(
            policy.external_base_url(remote_peer(), &headers, "127.0.0.1:3030"),
            "http://127.0.0.1:3030"
        );
    }

    #[test]
    fn ignores_forwarded_host_not_in_allowlist() {
        let policy = OriginPolicy::from_csv("https://victus.tail999.ts.net");
        let mut headers = HeaderMap::new();
        headers.insert("x-forwarded-host", HeaderValue::from_static("evil.ts.net"));
        headers.insert("x-forwarded-proto", HeaderValue::from_static("https"));

        assert_eq!(
            policy.external_base_url(loopback_peer(), &headers, "127.0.0.1:3030"),
            "http://127.0.0.1:3030"
        );
    }
}
