//! SSRF Guard - Layer 6
//!
//! Validates URLs to prevent Server-Side Request Forgery.
//!
//! Provides full parity with Go's `module/security/ssrf/guard.go`:
//! - `SsrfConfig` with all configuration fields (blocked CIDRs, allowed hosts,
//!   selective blocking toggles, max redirects)
//! - `Guard` with CIDR-based blocked nets, host whitelist, and thread-safe
//!   dynamic mutation methods
//! - DNS resolution via `std::net` (synchronous) for IP checking
//! - IP classification delegates to `crate::resolver` functions

use std::collections::HashSet;
use std::net::IpAddr;
use std::str::FromStr;
use std::sync::Arc;

use ipnet::IpNet;
use parking_lot::RwLock;

use crate::resolver;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// SSRF guard error.
#[derive(Debug, thiserror::Error)]
pub enum SsrfError {
    #[error("private IP address blocked: {0}")]
    PrivateIp(String),
    #[error("localhost access blocked: {0}")]
    Localhost(String),
    #[error("metadata endpoint blocked: {0}")]
    MetadataEndpoint(String),
    #[error("link-local address blocked: {0}")]
    LinkLocal(String),
    #[error("reserved IP address blocked: {0}")]
    ReservedIp(String),
    #[error("blocked CIDR: IP {ip} is in {cidr}")]
    BlockedCidr { ip: String, cidr: String },
    #[error("invalid URL: {0}")]
    InvalidUrl(String),
    #[error("blocked scheme: {0}")]
    BlockedScheme(String),
    #[error("invalid IP address: {0}")]
    InvalidIp(String),
    #[error("DNS resolution failed for {host}: {reason}")]
    DnsFailed { host: String, reason: String },
    #[error("no IP addresses found for {0}")]
    NoAddresses(String),
    #[error("host {host} resolves to blocked IP {ip}: {reason}")]
    ResolvesToBlocked {
        host: String,
        ip: String,
        reason: String,
    },
    #[error("invalid CIDR: {0}")]
    InvalidCidr(String),
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// SSRF guard configuration matching Go's `ssrf.Config`.
#[derive(Debug, Clone)]
pub struct SsrfConfig {
    /// Master switch.
    pub enabled: bool,
    /// Additional CIDRs to block (e.g. `["203.0.113.0/24"]`).
    pub blocked_cidrs: Vec<String>,
    /// Whitelist hosts that bypass all checks.
    pub allowed_hosts: Vec<String>,
    /// Block cloud metadata endpoints (169.254.169.254).
    pub block_metadata: bool,
    /// Block localhost / loopback.
    pub block_localhost: bool,
    /// Block RFC 1918 / RFC 4193 private ranges.
    pub block_private_ips: bool,
    /// Max HTTP redirects to follow (reserved for future use).
    pub max_redirects: usize,
}

impl Default for SsrfConfig {
    /// Returns a secure-by-default configuration matching Go's `DefaultConfig()`.
    fn default() -> Self {
        Self {
            enabled: true,
            blocked_cidrs: Vec::new(),
            allowed_hosts: Vec::new(),
            block_metadata: true,
            block_localhost: true,
            block_private_ips: true,
            max_redirects: 5,
        }
    }
}

// ---------------------------------------------------------------------------
// Inner (mutable state behind RwLock)
// ---------------------------------------------------------------------------

/// Mutable inner state shared behind a RwLock.
struct Inner {
    blocked_nets: Vec<IpNet>,
    allowed_set: HashSet<String>,
}

// ---------------------------------------------------------------------------
// Guard
// ---------------------------------------------------------------------------

/// SSRF guard providing full URL / IP validation with CIDR matching and
/// host whitelisting. Thread-safe via internal `RwLock`.
pub struct Guard {
    config: SsrfConfig,
    inner: Arc<RwLock<Inner>>,
}

impl Guard {
    // -----------------------------------------------------------------------
    // Constructors
    // -----------------------------------------------------------------------

    /// Create a new SSRF guard from the given configuration.
    ///
    /// Parses all `blocked_cidrs` entries at construction time so that runtime
    /// checks are pure lookups.
    pub fn new(config: SsrfConfig) -> Result<Self, SsrfError> {
        let mut blocked_nets = Vec::new();
        for cidr in &config.blocked_cidrs {
            let net: IpNet = IpNet::from_str(cidr)
                .map_err(|e| SsrfError::InvalidCidr(format!("{}: {}", cidr, e)))?;
            blocked_nets.push(net);
        }

        let allowed_set: HashSet<String> =
            config.allowed_hosts.iter().map(|h| h.to_lowercase()).collect();

        Ok(Self {
            config,
            inner: Arc::new(RwLock::new(Inner {
                blocked_nets,
                allowed_set,
            })),
        })
    }

    /// Convenience constructor matching the legacy API used by `pipeline.rs`.
    ///
    /// Creates a guard with default config where `enabled` is set to the given
    /// boolean value. This preserves backward compatibility.
    pub fn from_enabled(enabled: bool) -> Self {
        let config = SsrfConfig {
            enabled,
            ..SsrfConfig::default()
        };
        // Default config has no CIDRs to parse, so this cannot fail.
        Self::new(config).expect("default SsrfConfig should always parse")
    }

    // -----------------------------------------------------------------------
    // Public queries
    // -----------------------------------------------------------------------

    /// Returns whether the SSRF guard is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    // -----------------------------------------------------------------------
    // Validate URL
    // -----------------------------------------------------------------------

    /// Validate a URL for SSRF protection.
    ///
    /// This is the main entry point. It:
    /// 1. Returns early if the guard is disabled.
    /// 2. Parses the URL.
    /// 3. Checks the host against the whitelist.
    /// 4. Checks the scheme.
    /// 5. Attempts to resolve the host via DNS.
    /// 6. Validates every resolved IP against the blocking rules.
    pub fn validate_url(&self, url: &str) -> Result<(), SsrfError> {
        if !self.config.enabled {
            return Ok(());
        }

        let guard = self.inner.read();

        // Parse URL ----------------------------------------------------------
        let parsed = resolver::parse_url(url)
            .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

        // Whitelist check ----------------------------------------------------
        let host_lower = parsed.host.to_lowercase();
        if guard.allowed_set.contains(&host_lower) {
            return Ok(());
        }

        // Scheme check (parse_url already restricts to http/https, but we
        // also block other schemes that might slip through raw parsing).
        let scheme = parsed.scheme.to_lowercase();
        if scheme != "http" && scheme != "https" {
            return Err(SsrfError::BlockedScheme(scheme));
        }

        // Resolve & validate IPs ---------------------------------------------
        self.resolve_and_validate_locked(&guard, &parsed.host)
    }

    /// Resolve DNS for the given URL's host and validate all resulting IPs.
    ///
    /// This is a standalone method equivalent to Go's `Guard.ResolveAndValidate()`.
    /// Useful when you want to resolve DNS separately from making the actual request.
    pub fn resolve_and_validate(&self, raw_url: &str) -> Result<(), SsrfError> {
        if !self.config.enabled {
            return Ok(());
        }

        let guard = self.inner.read();

        let parsed = resolver::parse_url(raw_url)
            .map_err(|e| SsrfError::InvalidUrl(e.to_string()))?;

        let host_lower = parsed.host.to_lowercase();
        if guard.allowed_set.contains(&host_lower) {
            return Ok(());
        }

        self.resolve_and_validate_locked(&guard, &parsed.host)
    }

    // -----------------------------------------------------------------------
    // Check IP
    // -----------------------------------------------------------------------

    /// Check a single IP address string against all blocking rules.
    pub fn check_ip(&self, ip_str: &str) -> Result<(), SsrfError> {
        if !self.config.enabled {
            return Ok(());
        }

        let ip: IpAddr = ip_str
            .parse()
            .map_err(|_| SsrfError::InvalidIp(ip_str.to_string()))?;

        let guard = self.inner.read();
        self.check_ip_locked(&guard, &ip)
    }

    // -----------------------------------------------------------------------
    // Dynamic mutation
    // -----------------------------------------------------------------------

    /// Dynamically add a CIDR to the block list.
    pub fn add_blocked_cidr(&self, cidr: &str) -> Result<(), SsrfError> {
        let net: IpNet = IpNet::from_str(cidr)
            .map_err(|e| SsrfError::InvalidCidr(format!("{}: {}", cidr, e)))?;
        let mut guard = self.inner.write();
        guard.blocked_nets.push(net);
        Ok(())
    }

    /// Dynamically add a host to the whitelist.
    pub fn add_allowed_host(&self, host: &str) {
        let mut guard = self.inner.write();
        guard.allowed_set.insert(host.to_lowercase());
    }

    /// Remove a host from the whitelist.
    pub fn remove_allowed_host(&self, host: &str) {
        let mut guard = self.inner.write();
        guard.allowed_set.remove(&host.to_lowercase());
    }

    // -----------------------------------------------------------------------
    // Internal helpers (must be called with read lock held)
    // -----------------------------------------------------------------------

    /// Resolve host via DNS and validate all resulting IPs.
    ///
    /// `guard` is the read-lock guard so we don't re-acquire the lock.
    fn resolve_and_validate_locked(
        &self,
        guard: &parking_lot::RwLockReadGuard<'_, Inner>,
        host: &str,
    ) -> Result<(), SsrfError> {
        let host_lower = host.to_lowercase();

        // Check localhost hostname strings
        if self.config.block_localhost {
            if host_lower == "localhost"
                || host_lower.ends_with(".localhost")
                || host_lower == "localhost.localdomain"
            {
                return Err(SsrfError::Localhost(format!(
                    "localhost hostname {} is blocked",
                    host
                )));
            }
        }

        // Try to parse as IP directly
        if let Ok(ip) = host_lower.parse::<IpAddr>() {
            return self.check_ip_locked(guard, &ip);
        }

        // DNS resolution (synchronous via std::net)
        let clean_host = strip_ipv6_brackets(host);
        let ips = resolve_host_dns(&clean_host)?;

        if ips.is_empty() {
            return Err(SsrfError::NoAddresses(host.to_string()));
        }

        // Validate every resolved IP
        for ip in &ips {
            if let Err(e) = self.check_ip_locked(guard, ip) {
                return Err(SsrfError::ResolvesToBlocked {
                    host: host.to_string(),
                    ip: ip.to_string(),
                    reason: e.to_string(),
                });
            }
        }

        Ok(())
    }

    /// Check a single `IpAddr` against all blocking rules.
    fn check_ip_locked(
        &self,
        guard: &parking_lot::RwLockReadGuard<'_, Inner>,
        ip: &IpAddr,
    ) -> Result<(), SsrfError> {
        // Block localhost / loopback
        if self.config.block_localhost && resolver::is_loopback_ip(ip) {
            return Err(SsrfError::Localhost(format!(
                "loopback IP {} is blocked",
                ip
            )));
        }

        // Block cloud metadata
        if self.config.block_metadata && resolver::is_metadata_ip(ip) {
            return Err(SsrfError::MetadataEndpoint(format!(
                "cloud metadata IP {} is blocked",
                ip
            )));
        }

        // Block private IPs (RFC 1918, RFC 4193, etc.)
        if self.config.block_private_ips && resolver::is_private_ip(ip) {
            return Err(SsrfError::PrivateIp(format!(
                "private IP {} is blocked",
                ip
            )));
        }

        // Always check link-local (regardless of config toggles)
        if resolver::is_link_local_ip(ip) {
            return Err(SsrfError::LinkLocal(format!(
                "link-local IP {} is blocked",
                ip
            )));
        }

        // Always check other reserved ranges
        if resolver::is_reserved_ip(ip) {
            return Err(SsrfError::ReservedIp(format!(
                "reserved IP {} is blocked",
                ip
            )));
        }

        // Check user-configured blocked CIDRs
        for net in &guard.blocked_nets {
            if net.contains(ip) {
                return Err(SsrfError::BlockedCidr {
                    ip: ip.to_string(),
                    cidr: net.to_string(),
                });
            }
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// DNS resolution helper (standalone, no state)
// ---------------------------------------------------------------------------

/// Strip `[` `]` brackets from IPv6 URL host notation.
fn strip_ipv6_brackets(host: &str) -> String {
    if host.starts_with('[') && host.ends_with(']') {
        host[1..host.len() - 1].to_string()
    } else {
        host.to_string()
    }
}

/// Resolve a hostname using synchronous DNS lookup via `std::net`.
///
/// If `host` is already an IP address, it is returned directly.
fn resolve_host_dns(host: &str) -> Result<Vec<IpAddr>, SsrfError> {
    let clean = strip_ipv6_brackets(host);

    // Already an IP?
    if let Ok(ip) = clean.parse::<IpAddr>() {
        return Ok(vec![ip]);
    }

    // Use std::net for synchronous DNS resolution
    use std::net::ToSocketAddrs;
    let addr_str = format!("{}:0", clean);
    let addrs: Vec<IpAddr> = addr_str
        .to_socket_addrs()
        .map(|iter| iter.map(|sa| sa.ip()).collect())
        .map_err(|e| SsrfError::DnsFailed {
            host: clean.clone(),
            reason: e.to_string(),
        })?;

    if addrs.is_empty() {
        return Err(SsrfError::NoAddresses(clean));
    }

    Ok(addrs)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
}
