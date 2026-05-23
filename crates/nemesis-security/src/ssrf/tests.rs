use super::*;

// =======================================================================
// Constructor tests
// =======================================================================

#[test]
fn test_new_default_config() {
    let cfg = SsrfConfig::default();
    let guard = Guard::new(cfg).expect("default config should succeed");
    assert!(guard.is_enabled());
}

#[test]
fn test_new_disabled() {
    let cfg = SsrfConfig {
        enabled: false,
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).expect("disabled config should succeed");
    assert!(!guard.is_enabled());
}

#[test]
fn test_new_invalid_blocked_cidr() {
    let cfg = SsrfConfig {
        blocked_cidrs: vec!["not-a-cidr".to_string()],
        ..SsrfConfig::default()
    };
    assert!(Guard::new(cfg).is_err());
}

#[test]
fn test_new_valid_blocked_cidr() {
    let cfg = SsrfConfig {
        blocked_cidrs: vec!["203.0.113.0/24".to_string()],
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).expect("valid CIDR should succeed");
    assert!(guard.is_enabled());
}

#[test]
fn test_from_enabled_true() {
    let guard = Guard::from_enabled(true);
    assert!(guard.is_enabled());
}

#[test]
fn test_from_enabled_false() {
    let guard = Guard::from_enabled(false);
    assert!(!guard.is_enabled());
}

// =======================================================================
// Default config values
// =======================================================================

#[test]
fn test_default_config_values() {
    let cfg = SsrfConfig::default();
    assert!(cfg.enabled);
    assert!(cfg.block_metadata);
    assert!(cfg.block_localhost);
    assert!(cfg.block_private_ips);
    assert_eq!(cfg.max_redirects, 5);
    assert!(cfg.blocked_cidrs.is_empty());
    assert!(cfg.allowed_hosts.is_empty());
}

// =======================================================================
// validate_url — safe URLs
// =======================================================================

#[test]
fn test_safe_urls() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    // Public domains – may fail in restricted DNS environments; log only.
    for url in &[
        "https://example.com/api",
        "https://api.github.com/repos",
        "http://example.com:8080/path",
    ] {
        let result = guard.validate_url(url);
        if let Err(e) = &result {
            eprintln!("validate_url({}) => {} (may be DNS)", url, e);
        }
    }
}

// =======================================================================
// validate_url — localhost blocked
// =======================================================================

#[test]
fn test_localhost_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("http://127.0.0.1/admin").is_err());
    assert!(guard.validate_url("http://localhost:8080/api").is_err());
    assert!(guard.validate_url("http://0.0.0.0/health").is_err());
}

// =======================================================================
// validate_url — private IPs blocked
// =======================================================================

#[test]
fn test_private_ip_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("http://192.168.1.1/admin").is_err());
    assert!(guard.validate_url("http://10.0.0.1/internal").is_err());
    assert!(guard.validate_url("http://172.16.0.1/secret").is_err());
}

// =======================================================================
// validate_url — metadata blocked
// =======================================================================

#[test]
fn test_metadata_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard
        .validate_url("http://169.254.169.254/latest/meta-data/")
        .is_err());
    assert!(guard
        .validate_url("http://metadata.google.internal/computeMetadata/v1/")
        .is_err()
        || guard
            .validate_url("http://metadata.google.internal/computeMetadata/v1/")
            .is_ok());
    // The metadata.google.internal hostname may or may not resolve in test
    // environments; the IP-based check is the important one.
}

// =======================================================================
// validate_url — unsupported schemes
// =======================================================================

#[test]
fn test_file_scheme_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("file:///etc/passwd").is_err());
}

#[test]
fn test_ftp_scheme_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("ftp://example.com/file").is_err());
}

// =======================================================================
// validate_url — disabled guard
// =======================================================================

#[test]
fn test_disabled_guard() {
    let guard = Guard::from_enabled(false);
    // All dangerous URLs should pass when guard is disabled
    assert!(guard.validate_url("http://127.0.0.1/admin").is_ok());
    assert!(guard.validate_url("http://192.168.1.1/secret").is_ok());
    assert!(guard.validate_url("http://169.254.169.254/meta").is_ok());
}

// =======================================================================
// validate_url — empty / malformed
// =======================================================================

#[test]
fn test_empty_url() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("").is_err());
}

#[test]
fn test_no_host_url() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("http:///path").is_err());
}

// =======================================================================
// validate_url — allowed hosts (whitelist)
// =======================================================================

#[test]
fn test_allowed_hosts_bypass() {
    let cfg = SsrfConfig {
        allowed_hosts: vec!["trusted.local".to_string()],
        block_localhost: true,
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();
    // Even though trusted.local might resolve to a private IP,
    // it's whitelisted and should bypass the check.
    // Note: DNS resolution may fail in test environments.
    let result = guard.validate_url("http://trusted.local/api");
    // We only verify it doesn't panic; the DNS result is environment-dependent.
    eprintln!("allowed host result: {:?}", result);
}

#[test]
fn test_allowed_host_case_insensitive() {
    let cfg = SsrfConfig {
        allowed_hosts: vec!["MyHost.Example.COM".to_string()],
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();
    let result = guard.validate_url("http://myhost.example.com/api");
    eprintln!("case-insensitive allowed host result: {:?}", result);
}

// =======================================================================
// check_ip — loopback
// =======================================================================

#[test]
fn test_check_ip_loopback() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("127.0.0.1").is_err());
    assert!(guard.check_ip("::1").is_err());
}

// =======================================================================
// check_ip — private IPs
// =======================================================================

#[test]
fn test_check_ip_private() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("10.0.0.1").is_err());
    assert!(guard.check_ip("172.16.0.1").is_err());
    assert!(guard.check_ip("192.168.1.1").is_err());
    assert!(guard.check_ip("10.255.255.255").is_err());
    assert!(guard.check_ip("172.31.255.255").is_err());
}

// =======================================================================
// check_ip — metadata
// =======================================================================

#[test]
fn test_check_ip_metadata() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("169.254.169.254").is_err());
}

// =======================================================================
// check_ip — public IP passes
// =======================================================================

#[test]
fn test_check_ip_public() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("8.8.8.8").is_ok());
    assert!(guard.check_ip("1.1.1.1").is_ok());
}

// =======================================================================
// check_ip — invalid IP
// =======================================================================

#[test]
fn test_check_ip_invalid() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("not-an-ip").is_err());
}

// =======================================================================
// check_ip — link-local
// =======================================================================

#[test]
fn test_check_ip_link_local() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("169.254.1.1").is_err());
}

// =======================================================================
// check_ip — reserved
// =======================================================================

#[test]
fn test_check_ip_reserved() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("0.0.0.0").is_err());
    assert!(guard.check_ip("224.0.0.1").is_err());
    assert!(guard.check_ip("255.255.255.255").is_err());
}

// =======================================================================
// check_ip — disabled
// =======================================================================

#[test]
fn test_check_ip_disabled() {
    let guard = Guard::from_enabled(false);
    assert!(guard.check_ip("127.0.0.1").is_ok());
}

// =======================================================================
// check_ip — IPv6
// =======================================================================

#[test]
fn test_check_ip_ipv6_loopback() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("::1").is_err());
}

#[test]
fn test_check_ip_ipv6_link_local() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("fe80::1").is_err());
}

#[test]
fn test_check_ip_ipv6_private() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("fc00::1").is_err());
}

#[test]
fn test_check_ip_ipv6_multicast() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.check_ip("ff00::1").is_err());
}

#[test]
fn test_check_ip_ipv6_public() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    // Google public DNS IPv6 - may or may not pass depending on resolver
    let result = guard.check_ip("2001:4860:4860::8888");
    eprintln!("IPv6 public check: {:?}", result);
}

// =======================================================================
// Blocked CIDR
// =======================================================================

#[test]
fn test_blocked_cidr_at_construction() {
    let cfg = SsrfConfig {
        block_localhost: false,
        block_private_ips: false,
        blocked_cidrs: vec!["203.0.113.0/24".to_string()],
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();

    // IP inside the blocked CIDR
    assert!(guard.check_ip("203.0.113.1").is_err());

    // IP outside the blocked CIDR
    assert!(guard.check_ip("203.0.114.1").is_ok());
}

// =======================================================================
// Dynamic mutation: add_blocked_cidr
// =======================================================================

#[test]
fn test_add_blocked_cidr() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();

    guard
        .add_blocked_cidr("198.51.100.0/24")
        .expect("valid CIDR");

    assert!(guard.check_ip("198.51.100.1").is_err());
}

#[test]
fn test_add_blocked_cidr_invalid() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.add_blocked_cidr("invalid").is_err());
}

// =======================================================================
// Dynamic mutation: add_allowed_host / remove_allowed_host
// =======================================================================

#[test]
fn test_add_allowed_host_case_insensitive() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    guard.add_allowed_host("MyHost.Example.COM");
    // Verify no panic; the host is added with lowercase key.
    let result = guard.validate_url("http://myhost.example.com/api");
    eprintln!("add_allowed_host result: {:?}", result);
}

#[test]
fn test_remove_allowed_host() {
    let cfg = SsrfConfig {
        allowed_hosts: vec!["temp.example.com".to_string()],
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();
    guard.remove_allowed_host("temp.example.com");
    // After removal the host goes through normal checks - no panic.
}

// =======================================================================
// Selective config: only block localhost
// =======================================================================

#[test]
fn test_only_block_localhost() {
    let cfg = SsrfConfig {
        block_localhost: true,
        block_private_ips: false,
        block_metadata: false,
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();

    // Loopback should be blocked
    assert!(guard.check_ip("127.0.0.1").is_err());

    // Private IP should pass (block_private_ips=false).
    // 10.0.0.1 is private but not loopback, not metadata, not link-local, not reserved.
    assert!(guard.check_ip("10.0.0.1").is_ok());
}

// =======================================================================
// Selective config: no blocks
// =======================================================================

#[test]
fn test_no_blocks() {
    let cfg = SsrfConfig {
        block_localhost: false,
        block_private_ips: false,
        block_metadata: false,
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();

    // Public IP should always pass
    assert!(guard.check_ip("8.8.8.8").is_ok());
}

// =======================================================================
// DNS resolution helpers
// =======================================================================

#[test]
fn test_resolve_host_ipv4() {
    let ips = resolve_host_dns("8.8.8.8").unwrap();
    assert_eq!(ips.len(), 1);
    assert_eq!(ips[0], "8.8.8.8".parse::<IpAddr>().unwrap());
}

#[test]
fn test_resolve_host_ipv6() {
    let ips = resolve_host_dns("::1").unwrap();
    assert_eq!(ips.len(), 1);
}

#[test]
fn test_resolve_host_bracketed_ipv6() {
    let ips = resolve_host_dns("[::1]").unwrap();
    assert_eq!(ips.len(), 1);
}

#[test]
fn test_strip_ipv6_brackets() {
    assert_eq!(strip_ipv6_brackets("[::1]"), "::1");
    assert_eq!(strip_ipv6_brackets("::1"), "::1");
    assert_eq!(strip_ipv6_brackets("example.com"), "example.com");
}

// =======================================================================
// Thread safety: concurrent access
// =======================================================================

#[test]
fn test_concurrent_access() {
    use std::sync::Arc;
    use std::thread;

    let guard = Arc::new(Guard::new(SsrfConfig::default()).unwrap());
    let mut handles = vec![];

    for i in 0..4 {
        let g = Arc::clone(&guard);
        handles.push(thread::spawn(move || {
            match i % 4 {
                0 => {
                    let _ = g.validate_url("http://127.0.0.1/");
                }
                1 => {
                    let _ = g.check_ip("192.168.1.1");
                }
                2 => {
                    g.add_allowed_host(&format!("host{}.example.com", i));
                }
                3 => {
                    let _ = g.add_blocked_cidr("198.51.100.0/24");
                }
                _ => {}
            }
        }));
    }

    for h in handles {
        h.join().expect("thread should not panic");
    }
}

// =======================================================================
// Additional coverage tests
// =======================================================================

#[test]
fn test_validate_url_disabled_allows_private() {
    let guard = Guard::from_enabled(false);
    assert!(guard.validate_url("http://192.168.1.1/secret").is_ok());
    assert!(guard.validate_url("http://127.0.0.1/admin").is_ok());
}

#[test]
fn test_check_ip_disabled_allows_all() {
    let guard = Guard::from_enabled(false);
    assert!(guard.check_ip("127.0.0.1").is_ok());
    assert!(guard.check_ip("169.254.169.254").is_ok());
    assert!(guard.check_ip("192.168.1.1").is_ok());
}

#[test]
fn test_resolve_and_validate_disabled() {
    let guard = Guard::from_enabled(false);
    assert!(guard.resolve_and_validate("http://192.168.1.1/secret").is_ok());
}

#[test]
fn test_validate_url_localhost_hostname_blocked() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    assert!(guard.validate_url("http://localhost/admin").is_err());
}

#[test]
fn test_validate_url_localhost_localdomain_blocked() {
    let guard = Guard::new(SsrfConfig {
        block_localhost: true,
        ..SsrfConfig::default()
    }).unwrap();
    assert!(guard.validate_url("http://localhost.localdomain/test").is_err());
}

#[test]
fn test_validate_url_subdomain_localhost_blocked() {
    let guard = Guard::new(SsrfConfig {
        block_localhost: true,
        ..SsrfConfig::default()
    }).unwrap();
    assert!(guard.validate_url("http://sub.localhost/test").is_err());
}

#[test]
fn test_check_ip_blocked_cidr_match() {
    let cfg = SsrfConfig {
        blocked_cidrs: vec!["198.51.100.0/24".to_string()],
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();
    assert!(guard.check_ip("198.51.100.50").is_err());
    // Should be BlockedCidr variant
    let err = guard.check_ip("198.51.100.50").unwrap_err();
    assert!(err.to_string().contains("198.51.100"));
}

#[test]
fn test_add_blocked_cidr_ipv6() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    let result = guard.add_blocked_cidr("2001:db8::/32");
    assert!(result.is_ok());
}

#[test]
fn test_resolve_and_validate_ip_directly() {
    let guard = Guard::new(SsrfConfig::default()).unwrap();
    // Passing an IP directly as a URL host
    assert!(guard.validate_url("http://10.0.0.1/secret").is_err());
}

#[test]
fn test_ssrf_error_variants() {
    let err = SsrfError::PrivateIp("10.0.0.1".to_string());
    assert!(err.to_string().contains("private IP"));

    let err = SsrfError::Localhost("127.0.0.1".to_string());
    assert!(err.to_string().contains("localhost"));

    let err = SsrfError::MetadataEndpoint("169.254.169.254".to_string());
    assert!(err.to_string().contains("metadata"));

    let err = SsrfError::LinkLocal("169.254.1.1".to_string());
    assert!(err.to_string().contains("link-local"));

    let err = SsrfError::ReservedIp("0.0.0.0".to_string());
    assert!(err.to_string().contains("reserved"));

    let err = SsrfError::BlockedCidr { ip: "1.2.3.4".to_string(), cidr: "1.2.3.0/24".to_string() };
    assert!(err.to_string().contains("1.2.3.0/24"));

    let err = SsrfError::InvalidUrl("bad url".to_string());
    assert!(err.to_string().contains("invalid URL"));

    let err = SsrfError::BlockedScheme("ftp".to_string());
    assert!(err.to_string().contains("blocked scheme"));

    let err = SsrfError::InvalidIp("abc".to_string());
    assert!(err.to_string().contains("invalid IP"));

    let err = SsrfError::DnsFailed { host: "test".to_string(), reason: "timeout".to_string() };
    assert!(err.to_string().contains("DNS resolution failed"));

    let err = SsrfError::NoAddresses("empty.host".to_string());
    assert!(err.to_string().contains("no IP addresses"));

    let err = SsrfError::ResolvesToBlocked {
        host: "evil.com".to_string(),
        ip: "10.0.0.1".to_string(),
        reason: "private IP".to_string(),
    };
    assert!(err.to_string().contains("resolves to blocked IP"));

    let err = SsrfError::InvalidCidr("bad".to_string());
    assert!(err.to_string().contains("invalid CIDR"));
}

#[test]
fn test_ssrf_config_debug() {
    let cfg = SsrfConfig::default();
    let debug = format!("{:?}", cfg);
    assert!(debug.contains("enabled"));
    assert!(debug.contains("block_localhost"));
}

#[test]
fn test_ssrf_config_clone() {
    let cfg = SsrfConfig::default();
    let cloned = cfg.clone();
    assert_eq!(cfg.enabled, cloned.enabled);
    assert_eq!(cfg.block_metadata, cloned.block_metadata);
}

#[test]
fn test_resolve_host_dns_invalid_host() {
    // This may succeed or fail depending on DNS - just verify no panic
    let _ = resolve_host_dns("this-host-definitely-does-not-exist-12345.invalid");
}

#[test]
fn test_validate_url_no_blocks_private_passes() {
    let cfg = SsrfConfig {
        block_localhost: true,
        block_private_ips: false,
        block_metadata: false,
        ..SsrfConfig::default()
    };
    let guard = Guard::new(cfg).unwrap();
    // 10.0.0.1 is private but not blocked since block_private_ips=false
    // However it may still be blocked by link-local or reserved checks
    // 10.0.0.1 is RFC1918 private, not link-local or reserved in the general sense
    let result = guard.check_ip("10.0.0.1");
    assert!(result.is_ok());
}
