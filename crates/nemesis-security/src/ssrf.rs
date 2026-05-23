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
mod tests;
