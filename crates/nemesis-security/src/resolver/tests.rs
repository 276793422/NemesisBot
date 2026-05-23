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
