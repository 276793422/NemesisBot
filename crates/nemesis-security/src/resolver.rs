//! SSRF DNS resolver and comprehensive IP classification.
//!
//! Provides URL parsing, DNS resolution helpers, and IP address classification
//! for private, loopback, metadata, link-local, and reserved ranges.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

/// Error type for resolver operations.
#[derive(Debug, thiserror::Error)]
pub enum ResolverError {
    #[error("empty URL")]
    EmptyUrl,
    #[error("failed to parse URL: {0}")]
    ParseError(String),
    #[error("URL has no host")]
    NoHost,
    #[error("unsupported scheme: {0}")]
    UnsupportedScheme(String),
    #[error("URLs with embedded credentials are not allowed")]
    EmbeddedCredentials,
    #[error("hostname contains control characters")]
    ControlChars,
    #[error("invalid hostname")]
    InvalidHost,
    #[error("invalid port: {0}")]
    InvalidPort(String),
    #[error("DNS lookup failed for {0}: {1}")]
    DnsFailed(String, String),
    #[error("no IP addresses resolved for {0}")]
    NoAddresses(String),
}

/// Parsed URL result.
#[derive(Debug, Clone)]
pub struct ParsedUrl {
    pub scheme: String,
    pub host: String,
    pub port: Option<u16>,
    pub path: String,
}

/// Parse and validate a URL string.
pub fn parse_url(raw_url: &str) -> Result<ParsedUrl, ResolverError> {
    if raw_url.is_empty() {
        return Err(ResolverError::EmptyUrl);
    }

    let to_parse = if !raw_url.contains("://") {
        format!("http://{}", raw_url)
    } else {
        raw_url.to_string()
    };

    let parsed = url::Url::parse(&to_parse).map_err(|e| ResolverError::ParseError(e.to_string()))?;

    let host = parsed.host_str().unwrap_or("");
    if host.is_empty() {
        return Err(ResolverError::NoHost);
    }

    let scheme = parsed.scheme().to_lowercase();
    if scheme != "http" && scheme != "https" {
        return Err(ResolverError::UnsupportedScheme(scheme));
    }

    if parsed.username() != "" || parsed.password().is_some() {
        return Err(ResolverError::EmbeddedCredentials);
    }

    if contains_control_chars(host) {
        return Err(ResolverError::ControlChars);
    }

    if host.contains('@') || host == "[" || host == "]" || host == "[]" {
        return Err(ResolverError::InvalidHost);
    }

    let port = parsed.port();
    if let Some(port_val) = port {
        if port_val.to_string().parse::<u16>().is_err() {
            return Err(ResolverError::InvalidPort(port_val.to_string()));
        }
    }

    Ok(ParsedUrl {
        scheme,
        host: host.to_string(),
        port,
        path: parsed.path().to_string(),
    })
}

/// Check if an IP address is in a private range.
///
/// Covers RFC 1918, RFC 4193, RFC 5735, RFC 3927, RFC 6598, etc.
pub fn is_private_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => is_private_ipv4(v4),
        IpAddr::V6(v6) => is_private_ipv6(v6),
    }
}

fn is_private_ipv4(ip: &Ipv4Addr) -> bool {
    let octets = ip.octets();

    // RFC 1918
    if octets[0] == 10 { return true; }
    if octets[0] == 172 && octets[1] >= 16 && octets[1] <= 31 { return true; }
    if octets[0] == 192 && octets[1] == 168 { return true; }

    // RFC 5735 TEST-NET
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 2 { return true; }
    if octets[0] == 198 && octets[1] == 51 && octets[2] == 100 { return true; }
    if octets[0] == 203 && octets[1] == 0 && octets[2] == 113 { return true; }

    // RFC 6598 Carrier-grade NAT
    if octets[0] == 100 && octets[1] >= 64 && octets[1] <= 127 { return true; }

    // RFC 6890 IETF Protocol Assignments
    if octets[0] == 192 && octets[1] == 0 && octets[2] == 0 { return true; }

    // RFC 3068 6to4 Relay
    if octets[0] == 192 && octets[1] == 88 && octets[2] == 99 { return true; }

    // RFC 2544 Benchmarking
    if octets[0] == 198 && (octets[1] == 18 || octets[1] == 19) { return true; }

    false
}

fn is_private_ipv6(_ip: &Ipv6Addr) -> bool {
    // RFC 4193 Unique Local: fc00::/7
    let segments = _ip.segments();
    if (segments[0] & 0xfe00) == 0xfc00 { return true; }

    // RFC 6052 IPv4/IPv6 Translation: 64:ff9b::/96
    if segments[0] == 0x0064 && segments[1] == 0xff9b { return true; }

    false
}

/// Check if an IP address is loopback.
pub fn is_loopback_ip(ip: &IpAddr) -> bool {
    ip.is_loopback()
}

/// Check if an IP address is a cloud metadata endpoint.
pub fn is_metadata_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 169 && octets[1] == 254 && octets[2] == 169 && octets[3] == 254
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            segments[0] == 0xfd00
                && segments[1] == 0x0ec2
                && segments[2..7].iter().all(|&s| s == 0)
                && segments[7] == 0x0254
        }
    }
}

/// Check if an IP address is link-local.
pub fn is_link_local_ip(ip: &IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            octets[0] == 169 && octets[1] == 254
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            (segments[0] & 0xffc0) == 0xfe80
        }
    }
}

/// Check if an IP address is in reserved ranges.
pub fn is_reserved_ip(ip: &IpAddr) -> bool {
    if ip.is_unspecified() {
        return true;
    }

    match ip {
        IpAddr::V4(v4) => {
            let octets = v4.octets();
            // 0.0.0.0/8 Current network
            if octets[0] == 0 { return true; }
            // 224.0.0.0/4 Multicast
            if octets[0] >= 224 { return true; }
            // 255.255.255.255/32 Broadcast
            if octets[0] == 255 && octets[1] == 255 && octets[2] == 255 && octets[3] == 255 {
                return true;
            }
            false
        }
        IpAddr::V6(v6) => {
            let segments = v6.segments();
            // ff00::/8 IPv6 multicast
            (segments[0] & 0xff00) == 0xff00
        }
    }
}

fn contains_control_chars(s: &str) -> bool {
    s.chars().any(|c| c < '\x20' || c == '\x7f')
}

/// Perform DNS resolution on a hostname and return all IP addresses.
///
/// If the host is already an IP address (v4 or v6), it is returned directly
/// without performing a DNS lookup.
///
/// Mirrors Go's `ResolveHost`.
pub async fn resolve_host(host: &str) -> Result<Vec<IpAddr>, ResolverError> {
    // Strip bracket notation for IPv6 URLs: [::1] -> ::1
    let clean_host = if host.starts_with('[') && host.ends_with(']') && host.len() > 2 {
        &host[1..host.len() - 1]
    } else {
        host
    };

    // If it's already an IP, return it directly
    if let Ok(ip) = clean_host.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }

    // DNS lookup using tokio's async resolver
    let addrs = tokio::net::lookup_host(format!("{}:0", clean_host))
        .await
        .map_err(|e| ResolverError::DnsFailed(clean_host.to_string(), e.to_string()))?;

    let ips: Vec<IpAddr> = addrs.map(|sa| sa.ip()).collect();
    if ips.is_empty() {
        return Err(ResolverError::NoAddresses(clean_host.to_string()));
    }

    Ok(ips)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_url_basic() {
        let result = parse_url("https://example.com/path").unwrap();
        assert_eq!(result.scheme, "https");
        assert_eq!(result.host, "example.com");
    }

    #[test]
    fn test_parse_url_no_scheme() {
        let result = parse_url("example.com/path").unwrap();
        assert_eq!(result.scheme, "http");
    }

    #[test]
    fn test_parse_url_empty() {
        assert!(parse_url("").is_err());
    }

    #[test]
    fn test_parse_url_file_scheme() {
        assert!(parse_url("file:///etc/passwd").is_err());
    }

    #[test]
    fn test_parse_url_credentials() {
        assert!(parse_url("http://user:pass@host.com/").is_err());
    }

    #[test]
    fn test_private_ipv4() {
        assert!(is_private_ip(&"10.0.0.1".parse().unwrap()));
        assert!(is_private_ip(&"172.16.0.1".parse().unwrap()));
        assert!(is_private_ip(&"192.168.1.1".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_loopback() {
        assert!(is_loopback_ip(&"127.0.0.1".parse().unwrap()));
        assert!(is_loopback_ip(&"::1".parse().unwrap()));
        assert!(!is_loopback_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_metadata() {
        assert!(is_metadata_ip(&"169.254.169.254".parse().unwrap()));
        assert!(!is_metadata_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[test]
    fn test_link_local() {
        assert!(is_link_local_ip(&"169.254.1.1".parse().unwrap()));
        assert!(!is_link_local_ip(&"10.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_reserved() {
        assert!(is_reserved_ip(&"0.0.0.0".parse().unwrap()));
        assert!(is_reserved_ip(&"224.0.0.1".parse().unwrap()));
        assert!(is_reserved_ip(&"255.255.255.255".parse().unwrap()));
        assert!(!is_reserved_ip(&"8.8.8.8".parse().unwrap()));
    }

    #[tokio::test]
    async fn test_resolve_host_ip_address() {
        let result = resolve_host("127.0.0.1").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "127.0.0.1".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn test_resolve_host_ipv6_address() {
        let result = resolve_host("::1").await.unwrap();
        assert_eq!(result.len(), 1);
    }

    #[tokio::test]
    async fn test_resolve_host_ipv6_brackets() {
        let result = resolve_host("[::1]").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "::1".parse::<IpAddr>().unwrap());
    }

    #[tokio::test]
    async fn test_resolve_host_invalid() {
        let result = resolve_host("this-domain-definitely-does-not-exist-xyz123.invalid").await;
        assert!(result.is_err());
    }

    // --- Additional coverage tests for 95%+ ---

    #[test]
    fn test_parse_url_control_chars() {
        // Control characters should be rejected - either by the URL parser or our check
        let result = parse_url("http://host\x01name.com/");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_url_invalid_host_brackets() {
        let result = parse_url("http://[]/");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_url_with_port() {
        let result = parse_url("http://example.com:8080/path").unwrap();
        assert_eq!(result.port, Some(8080));
        assert_eq!(result.path, "/path");
    }

    #[test]
    fn test_parse_url_ftp_scheme() {
        let result = parse_url("ftp://example.com/");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("unsupported scheme"));
        }
    }

    #[test]
    fn test_parse_url_username_only() {
        let result = parse_url("http://user@host.com/");
        assert!(result.is_err());
        if let Err(e) = result {
            assert!(e.to_string().contains("credentials"));
        }
    }

    #[test]
    fn test_parse_url_password_only() {
        let result = parse_url("http://:pass@host.com/");
        assert!(result.is_err());
    }

    #[test]
    fn test_private_ipv4_rfc5735_test_net() {
        assert!(is_private_ip(&"192.0.2.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc5735_test_net_2() {
        assert!(is_private_ip(&"198.51.100.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc5735_test_net_3() {
        assert!(is_private_ip(&"203.0.113.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc6598_cgn() {
        assert!(is_private_ip(&"100.64.0.1".parse().unwrap()));
        assert!(is_private_ip(&"100.127.255.255".parse().unwrap()));
        assert!(!is_private_ip(&"100.63.255.255".parse().unwrap()));
        assert!(!is_private_ip(&"100.128.0.0".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc3068_6to4() {
        assert!(is_private_ip(&"192.88.99.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc2544_benchmark() {
        assert!(is_private_ip(&"198.18.0.1".parse().unwrap()));
        assert!(is_private_ip(&"198.19.255.255".parse().unwrap()));
        assert!(!is_private_ip(&"198.20.0.0".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_rfc6890_ietf() {
        assert!(is_private_ip(&"192.0.0.1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv4_not_private() {
        assert!(!is_private_ip(&"1.1.1.1".parse().unwrap()));
        assert!(!is_private_ip(&"8.8.8.8".parse().unwrap()));
        assert!(!is_private_ip(&"172.15.255.255".parse().unwrap()));
        assert!(!is_private_ip(&"172.32.0.0".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv6_rfc4193() {
        assert!(is_private_ip(&"fc00::1".parse().unwrap()));
        assert!(is_private_ip(&"fd12:3456::1".parse().unwrap()));
        assert!(!is_private_ip(&"fe80::1".parse().unwrap()));
    }

    #[test]
    fn test_private_ipv6_rfc6052_translation() {
        assert!(is_private_ip(&"64:ff9b::1".parse().unwrap()));
        assert!(!is_private_ip(&"64:ff9c::1".parse().unwrap()));
    }

    #[test]
    fn test_metadata_ipv6() {
        assert!(is_metadata_ip(&"fd00:ec2::254".parse().unwrap()));
        assert!(!is_metadata_ip(&"fd00:ec2::253".parse().unwrap()));
        assert!(!is_metadata_ip(&"fd00:ec3::254".parse().unwrap()));
    }

    #[test]
    fn test_link_local_ipv6() {
        assert!(is_link_local_ip(&"fe80::1".parse().unwrap()));
        assert!(!is_link_local_ip(&"fc00::1".parse().unwrap()));
    }

    #[test]
    fn test_reserved_ipv6_multicast() {
        assert!(is_reserved_ip(&"ff00::1".parse().unwrap()));
        assert!(!is_reserved_ip(&"fc00::1".parse().unwrap()));
    }

    #[test]
    fn test_reserved_ipv6_unspecified() {
        assert!(is_reserved_ip(&"::".parse().unwrap()));
    }

    #[test]
    fn test_reserved_ipv4_zero_network() {
        assert!(is_reserved_ip(&"0.0.0.1".parse().unwrap()));
        assert!(is_reserved_ip(&"0.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_reserved_ipv4_multicast() {
        assert!(is_reserved_ip(&"224.0.0.1".parse().unwrap()));
        assert!(is_reserved_ip(&"239.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_reserved_ipv4_broadcast() {
        assert!(is_reserved_ip(&"255.255.255.255".parse().unwrap()));
    }

    #[test]
    fn test_parse_url_with_path() {
        let result = parse_url("https://example.com/api/v1/data?key=value").unwrap();
        assert_eq!(result.scheme, "https");
        assert_eq!(result.host, "example.com");
    }

    #[test]
    fn test_parse_url_ipv4_host() {
        let result = parse_url("http://192.168.1.1:8080/").unwrap();
        assert_eq!(result.host, "192.168.1.1");
        assert_eq!(result.port, Some(8080));
    }

    #[tokio::test]
    async fn test_resolve_host_ipv4_address() {
        let result = resolve_host("8.8.8.8").await.unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0], "8.8.8.8".parse::<IpAddr>().unwrap());
    }

    #[test]
    fn test_contains_control_chars_normal() {
        assert!(!contains_control_chars("normal-host.com"));
    }

    #[test]
    fn test_contains_control_chars_with_null() {
        assert!(contains_control_chars("host\x00.com"));
    }

    #[test]
    fn test_contains_control_chars_with_del() {
        assert!(contains_control_chars("host\x7f.com"));
    }

    #[test]
    fn test_resolver_error_display() {
        let e = ResolverError::EmptyUrl;
        assert!(e.to_string().contains("empty URL"));

        let e = ResolverError::ParseError("bad".to_string());
        assert!(e.to_string().contains("bad"));

        let e = ResolverError::NoHost;
        assert!(e.to_string().contains("no host"));

        let e = ResolverError::UnsupportedScheme("ftp".to_string());
        assert!(e.to_string().contains("ftp"));

        let e = ResolverError::EmbeddedCredentials;
        assert!(e.to_string().contains("credentials"));

        let e = ResolverError::ControlChars;
        assert!(e.to_string().contains("control"));

        let e = ResolverError::InvalidHost;
        assert!(e.to_string().contains("invalid hostname"));

        let e = ResolverError::InvalidPort("abc".to_string());
        assert!(e.to_string().contains("abc"));
    }
}
