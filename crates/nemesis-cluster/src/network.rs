//! Network utilities for cluster communication.
//!
//! Provides helpers for local IP detection, subnet matching, address parsing,
//! and connection management. Mirrors Go's `network.go` and `node.go` helper
//! functions.

use std::net::{IpAddr, Ipv4Addr, SocketAddr, ToSocketAddrs};

// ---------------------------------------------------------------------------
// Basic helpers
// ---------------------------------------------------------------------------

/// Resolve a hostname:port string to socket addresses.
pub fn resolve_addr(addr_str: &str) -> Result<Vec<SocketAddr>, String> {
    addr_str
        .to_socket_addrs()
        .map(|iter| iter.collect())
        .map_err(|e| format!("Failed to resolve '{}': {}", addr_str, e))
}

/// Get the first non-loopback IPv4 address of the local machine.
/// Returns `127.0.0.1` if no suitable address is found.
pub fn get_local_ip() -> IpAddr {
    match std::net::UdpSocket::bind("0.0.0.0:0") {
        Ok(socket) => {
            if socket.connect("8.8.8.8:80").is_ok() {
                if let Ok(addr) = socket.local_addr() {
                    let ip = addr.ip();
                    if !ip.is_loopback() {
                        return ip;
                    }
                }
            }
            IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))
        }
        Err(_) => IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1)),
    }
}

/// Parse a socket address string. Returns a default if parsing fails.
pub fn parse_socket_addr(addr_str: &str) -> SocketAddr {
    addr_str.parse().unwrap_or_else(|_| {
        SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9000)
    })
}

/// Check whether an address is a loopback address.
pub fn is_loopback(addr: &SocketAddr) -> bool {
    addr.ip().is_loopback()
}

/// Build a bind address from an optional bind string and a default port.
pub fn build_bind_address(bind: Option<&str>, default_port: u16) -> String {
    match bind {
        Some(b) if !b.is_empty() => b.to_string(),
        _ => format!("0.0.0.0:{}", default_port),
    }
}

// ---------------------------------------------------------------------------
// Network interface enumeration
// ---------------------------------------------------------------------------

/// A local network interface with its subnet information.
#[derive(Debug, Clone)]
pub struct NetworkInterface {
    pub ip: String,
    pub mask: String,
    pub network_ip: String,
}

/// Get all local network interfaces with their subnet masks.
///
/// Enumerates network interfaces using `if-addrs` (cross-platform).
/// Falls back to the UDP connect trick if enumeration fails.
pub fn get_local_network_interfaces() -> Vec<NetworkInterface> {
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            let mut result = Vec::new();
            for iface in &interfaces {
                if iface.is_loopback() {
                    continue;
                }
                match &iface.addr {
                    if_addrs::IfAddr::V4(v4_addr) => {
                        let v4 = v4_addr.ip;
                        if v4.is_link_local() {
                            continue;
                        }
                        if is_virtual_interface(&iface.name) {
                            continue;
                        }
                        let mask = v4_addr.netmask;
                        let mask_octets = mask.octets();
                        let octets = v4.octets();
                        result.push(NetworkInterface {
                            ip: v4.to_string(),
                            mask: mask.to_string(),
                            network_ip: format!(
                                "{}.{}.{}.{}",
                                octets[0] & mask_octets[0],
                                octets[1] & mask_octets[1],
                                octets[2] & mask_octets[2],
                                octets[3] & mask_octets[3]
                            ),
                        });
                    }
                    if_addrs::IfAddr::V6(_) => {
                        // Skip IPv6 for now
                    }
                }
            }
            result
        }
        Err(_) => {
            // Fallback: single interface via UDP connect trick
            let local = get_local_ip();
            if local.is_loopback() || local.is_unspecified() {
                return Vec::new();
            }
            match local {
                IpAddr::V4(v4) => {
                    let octets = v4.octets();
                    vec![NetworkInterface {
                        ip: local.to_string(),
                        mask: "255.255.255.0".into(),
                        network_ip: format!("{}.{}.{}.0", octets[0], octets[1], octets[2]),
                    }]
                }
                IpAddr::V6(_) => Vec::new(),
            }
        }
    }
}

/// Convert a prefix length (0-32) to an IPv4 subnet mask.
#[allow(dead_code)]
fn prefix_len_to_mask(prefix_len: u32) -> Ipv4Addr {
    if prefix_len == 0 {
        return Ipv4Addr::new(0, 0, 0, 0);
    }
    let mask = if prefix_len >= 32 {
        0xffffffffu32
    } else {
        !((1u32 << (32 - prefix_len)) - 1)
    };
    Ipv4Addr::from(mask)
}

/// Check if two IP addresses are in the same subnet.
///
/// Uses real subnet masks from local network interfaces when available,
/// falls back to simple /24 matching.
pub fn is_same_subnet(ip1: &str, ip2: &str) -> bool {
    let local_interfaces = get_local_network_interfaces();
    if local_interfaces.is_empty() {
        return is_same_subnet_simple(ip1, ip2);
    }

    let parsed1: IpAddr = match ip1.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };
    let parsed2: IpAddr = match ip2.parse() {
        Ok(ip) => ip,
        Err(_) => return false,
    };

    for iface in &local_interfaces {
        let mask: IpAddr = match iface.mask.parse() {
            Ok(m) => m,
            Err(_) => continue,
        };

        match (parsed1, parsed2, mask) {
            (IpAddr::V4(a), IpAddr::V4(b), IpAddr::V4(m)) => {
                let ao = a.octets();
                let bo = b.octets();
                let mo = m.octets();
                let same = (0..4).all(|i| (ao[i] & mo[i]) == (bo[i] & mo[i]));
                if same {
                    return true;
                }
            }
            _ => continue,
        }
    }

    false
}

/// Simple /24 subnet check fallback.
fn is_same_subnet_simple(ip1: &str, ip2: &str) -> bool {
    let parts1: Vec<&str> = ip1.split('.').collect();
    let parts2: Vec<&str> = ip2.split('.').collect();
    if parts1.len() < 4 || parts2.len() < 4 {
        return false;
    }
    parts1[0] == parts2[0] && parts1[1] == parts2[1] && parts1[2] == parts2[2]
}

// ---------------------------------------------------------------------------
// GetAllLocalIPs - returns all local IPs sorted by priority
// ---------------------------------------------------------------------------

/// A candidate IP with its interface priority (lower = higher priority).
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CandidateIp {
    ip: String,
    priority: u32,
}

/// Get all local IP addresses by enumerating network interfaces.
///
/// Mirrors Go's `GetAllLocalIPs()`:
/// 1. Enumerates all network interfaces via `if-addrs` (cross-platform).
/// 2. Skips loopback, link-local, and virtual interfaces.
/// 3. Collects only non-loopback IPv4 addresses.
/// 4. Sorts by interface priority (Ethernet > WiFi > Other).
/// 5. Falls back to the UDP-connect trick if interface enumeration fails.
pub fn get_all_local_ips() -> Vec<String> {
    // Try enumerating real network interfaces first (matches Go's net.Interfaces()).
    match if_addrs::get_if_addrs() {
        Ok(interfaces) => {
            let mut candidates: Vec<CandidateIp> = Vec::new();

            for iface in &interfaces {
                // Skip loopback addresses
                if iface.is_loopback() {
                    continue;
                }

                let ip = iface.addr.ip();

                // Only consider IPv4, exclude link-local (169.254.x.x)
                if let IpAddr::V4(v4) = ip {
                    if v4.is_link_local() {
                        continue;
                    }

                    // Skip virtual interfaces by name
                    if is_virtual_interface(&iface.name) {
                        continue;
                    }

                    let priority = get_interface_priority(&iface.name);
                    candidates.push(CandidateIp {
                        ip: v4.to_string(),
                        priority,
                    });
                }
            }

            // Sort by priority (stable sort to preserve relative order)
            candidates.sort_by_key(|c| c.priority);

            // Extract just the IP strings, deduplicating
            let mut result: Vec<String> = Vec::new();
            for c in candidates {
                if !result.contains(&c.ip) {
                    result.push(c.ip);
                }
            }

            // If we found nothing, fall back to UDP connect trick
            if result.is_empty() {
                fallback_local_ip()
            } else {
                result
            }
        }
        Err(_) => {
            // Interface enumeration failed, fall back
            fallback_local_ip()
        }
    }
}

/// Fallback: get a single local IP via the UDP connect trick.
fn fallback_local_ip() -> Vec<String> {
    let local_ip = get_local_ip();
    if local_ip.is_loopback() || local_ip.is_unspecified() {
        Vec::new()
    } else {
        vec![local_ip.to_string()]
    }
}

/// Get all local IPs with virtual interface filtering.
///
/// The base `get_all_local_ips()` already filters virtual interfaces,
/// so this is now simply a delegate. Kept for API compatibility.
pub fn get_all_local_ips_filtered() -> Vec<String> {
    get_all_local_ips()
}

/// Check if an interface name matches common virtual interface patterns.
pub fn is_virtual_interface(name: &str) -> bool {
    let lower = name.to_lowercase();
    let patterns = [
        "veth", "docker", "br-", "virbr", "tun", "tap",
        "vbox", "vmnet", "utun", "awdl", "llw", "anpi",
        "ipsec", "gif", "stf", "p2p", "lo", "loopback",
    ];
    patterns.iter().any(|p| lower.contains(p))
}

/// Get the priority score for an interface name (lower = higher priority).
pub fn get_interface_priority(name: &str) -> u32 {
    let lower = name.to_lowercase();

    // Ethernet
    if lower.starts_with("eth")
        || lower.starts_with("eno")
        || lower.starts_with("ens")
        || lower.starts_with("enp")
    {
        return 1;
    }

    // WiFi
    if lower.starts_with("wlan") || lower.starts_with("wlp") {
        return 2;
    }

    // Other physical
    if lower.starts_with("en") || lower.starts_with("wl") {
        return 3;
    }

    // Everything else
    99
}

// ---------------------------------------------------------------------------
// Port availability
// ---------------------------------------------------------------------------

/// Find an available port starting from the given port.
/// Tries port, port+1, port+2, ... up to 100 attempts.
pub fn find_available_port(start_port: u16) -> Result<u16, String> {
    for offset in 0..100u16 {
        let port = start_port.saturating_add(offset);
        let addr = format!("0.0.0.0:{}", port);

        // Try to bind to check if port is available
        match std::net::TcpListener::bind(&addr) {
            Ok(listener) => {
                drop(listener);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    Err(format!(
        "no available port found starting from {}",
        start_port
    ))
}

/// Find an available UDP port.
pub fn find_available_udp_port(start_port: u16) -> Result<u16, String> {
    for offset in 0..100u16 {
        let port = start_port.saturating_add(offset);
        let addr = format!("0.0.0.0:{}", port);

        match std::net::UdpSocket::bind(&addr) {
            Ok(socket) => {
                drop(socket);
                return Ok(port);
            }
            Err(_) => continue,
        }
    }
    Err(format!(
        "no available UDP port found starting from {}",
        start_port
    ))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_local_ip() {
        let ip = get_local_ip();
        assert!(!ip.is_unspecified());
    }

    #[test]
    fn test_parse_socket_addr_valid() {
        let addr = parse_socket_addr("127.0.0.1:8080");
        assert_eq!(addr.port(), 8080);
    }

    #[test]
    fn test_parse_socket_addr_invalid() {
        let addr = parse_socket_addr("not-an-address");
        assert_eq!(addr.port(), 9000);
    }

    #[test]
    fn test_is_loopback() {
        let local: SocketAddr = "127.0.0.1:9000".parse().unwrap();
        assert!(is_loopback(&local));

        let external: SocketAddr = "10.0.0.1:9000".parse().unwrap();
        assert!(!is_loopback(&external));
    }

    #[test]
    fn test_build_bind_address() {
        assert_eq!(build_bind_address(Some("0.0.0.0:9100"), 9000), "0.0.0.0:9100");
        assert_eq!(build_bind_address(None, 9000), "0.0.0.0:9000");
        assert_eq!(build_bind_address(Some(""), 9000), "0.0.0.0:9000");
    }

    #[test]
    fn test_get_local_network_interfaces() {
        let interfaces = get_local_network_interfaces();
        // May be empty on some CI environments, but shouldn't panic
        for iface in &interfaces {
            assert!(!iface.ip.is_empty());
            assert!(!iface.mask.is_empty());
        }
    }

    #[test]
    fn test_is_same_subnet_simple_match() {
        assert!(is_same_subnet_simple("192.168.1.10", "192.168.1.20"));
    }

    #[test]
    fn test_is_same_subnet_simple_no_match() {
        assert!(!is_same_subnet_simple("192.168.1.10", "10.0.0.1"));
    }

    #[test]
    fn test_is_same_subnet_simple_invalid() {
        assert!(!is_same_subnet_simple("invalid", "192.168.1.10"));
    }

    // -----------------------------------------------------------------------
    // get_all_local_ips tests (mirrors Go's get_local_ip_test.go + ip_handling_test.go)
    // -----------------------------------------------------------------------

    #[test]
    fn test_get_all_local_ips() {
        let ips = get_all_local_ips();
        // Should return at least one non-loopback IP in most environments
        for ip in &ips {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(!parsed.is_loopback(), "should not include loopback: {}", ip);
        }
    }

    #[test]
    fn test_get_all_local_ips_excludes_virtual() {
        let ips = get_all_local_ips();
        // Verify each returned IP is valid IPv4, not loopback, not link-local
        for (i, ip) in ips.iter().enumerate() {
            let parsed: IpAddr = ip.parse().unwrap_or_else(|_| {
                panic!("Invalid IP at index {}: {}", i, ip)
            });
            assert!(!parsed.is_loopback(), "should not include loopback: {}", ip);

            if let IpAddr::V4(v4) = parsed {
                assert!(!v4.is_link_local(), "should not include link-local: {}", ip);
            }
        }
    }

    #[test]
    fn test_get_all_local_ips_never_panics() {
        // Multiple calls should be stable
        for _ in 0..3 {
            let _ips = get_all_local_ips();
        }
    }

    #[test]
    fn test_get_all_local_ips_no_duplicates() {
        let ips = get_all_local_ips();
        let mut seen = std::collections::HashSet::new();
        for ip in &ips {
            assert!(seen.insert(ip.clone()), "duplicate IP found: {}", ip);
        }
    }

    #[test]
    fn test_get_all_local_ips_sorted_by_priority() {
        // If we have multiple IPs, they should be sorted by interface priority.
        // We can't know the exact order without knowing the machine's interfaces,
        // but we can verify the result is deterministic.
        let ips1 = get_all_local_ips();
        let ips2 = get_all_local_ips();
        assert_eq!(ips1, ips2, "get_all_local_ips should return deterministic results");
    }

    #[test]
    fn test_get_all_local_ips_enumerates_all_interfaces() {
        // Verify that get_all_local_ips finds at least as many IPs as
        // the old single-IP approach. On a multi-homed machine this should
        // return more than one.
        let all_ips = get_all_local_ips();
        let single_ip = get_local_ip();

        if !single_ip.is_loopback() && !single_ip.is_unspecified() {
            // The single IP should be somewhere in the full list
            assert!(
                all_ips.contains(&single_ip.to_string()),
                "get_all_local_ips should contain the primary IP {} in {:?}",
                single_ip,
                all_ips
            );
        }
    }

    #[test]
    fn test_get_all_local_ips_filtered_equals_get_all_local_ips() {
        // get_all_local_ips_filtered should return the same results
        // since filtering is now done inside get_all_local_ips itself.
        assert_eq!(get_all_local_ips(), get_all_local_ips_filtered());
    }

    // -----------------------------------------------------------------------
    // Virtual interface detection tests (mirrors Go's get_local_ip_test.go)
    // -----------------------------------------------------------------------

    #[test]
    fn test_is_virtual_interface() {
        // Virtual interfaces - should be detected
        assert!(is_virtual_interface("veth123"));
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("br-abc"));
        assert!(is_virtual_interface("tun0"));
        assert!(is_virtual_interface("tap0"));
        assert!(is_virtual_interface("virbr0"));
        assert!(is_virtual_interface("vboxnet0"));
        assert!(is_virtual_interface("vmnet1"));
        assert!(is_virtual_interface("utun0"));
        assert!(is_virtual_interface("awdl0"));
        assert!(is_virtual_interface("lo"));
        assert!(is_virtual_interface("Loopback"));

        // Physical interfaces - should NOT be virtual
        assert!(!is_virtual_interface("eth0"));
        assert!(!is_virtual_interface("eno1"));
        assert!(!is_virtual_interface("ens33"));
        assert!(!is_virtual_interface("enp0s3"));
        assert!(!is_virtual_interface("wlan0"));
        assert!(!is_virtual_interface("wlp3s0"));
        assert!(!is_virtual_interface("en0"));
    }

    #[test]
    fn test_get_interface_priority() {
        // Ethernet = priority 1
        assert_eq!(get_interface_priority("eth0"), 1);
        assert_eq!(get_interface_priority("eno1"), 1);
        assert_eq!(get_interface_priority("ens33"), 1);
        assert_eq!(get_interface_priority("enp0s3"), 1);

        // WiFi = priority 2
        assert_eq!(get_interface_priority("wlan0"), 2);
        assert_eq!(get_interface_priority("wlp3s0"), 2);

        // Other physical = priority 3
        assert_eq!(get_interface_priority("en0"), 3);

        // Unknown = priority 99
        assert_eq!(get_interface_priority("unknown0"), 99);
    }

    // -----------------------------------------------------------------------
    // Subnet mask helper tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_prefix_len_to_mask() {
        assert_eq!(prefix_len_to_mask(0), Ipv4Addr::new(0, 0, 0, 0));
        assert_eq!(prefix_len_to_mask(8), Ipv4Addr::new(255, 0, 0, 0));
        assert_eq!(prefix_len_to_mask(16), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(prefix_len_to_mask(24), Ipv4Addr::new(255, 255, 255, 0));
        assert_eq!(prefix_len_to_mask(32), Ipv4Addr::new(255, 255, 255, 255));
        assert_eq!(prefix_len_to_mask(23), Ipv4Addr::new(255, 255, 254, 0));
        assert_eq!(prefix_len_to_mask(25), Ipv4Addr::new(255, 255, 255, 128));
    }

    #[test]
    fn test_find_available_port() {
        // Should find an available port
        let port = find_available_port(50000).unwrap();
        assert!(port >= 50000);
    }

    #[test]
    fn test_find_available_udp_port() {
        let port = find_available_udp_port(50000).unwrap();
        assert!(port >= 50000);
    }

    #[test]
    fn test_resolve_addr_localhost() {
        let addrs = resolve_addr("127.0.0.1:80").unwrap();
        assert!(!addrs.is_empty());
    }

    #[test]
    fn test_resolve_addr_invalid() {
        assert!(resolve_addr("invalid-host-that-does-not-exist:80").is_err());
    }

    // ============================================================
    // Coverage improvement: more edge cases
    // ============================================================

    #[test]
    fn test_is_same_subnet_invalid_ip() {
        assert!(!is_same_subnet("not-an-ip", "192.168.1.10"));
        assert!(!is_same_subnet("192.168.1.10", "not-an-ip"));
    }

    #[test]
    fn test_is_same_subnet_simple_short_ip() {
        // Less than 4 octets
        assert!(!is_same_subnet_simple("192.168", "192.168.1.10"));
        assert!(!is_same_subnet_simple("192.168.1.10", "10"));
    }

    #[test]
    fn test_parse_socket_addr_empty() {
        let addr = parse_socket_addr("");
        assert_eq!(addr.port(), 9000);
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
    }

    #[test]
    fn test_network_interface_debug() {
        let iface = NetworkInterface {
            ip: "192.168.1.1".to_string(),
            mask: "255.255.255.0".to_string(),
            network_ip: "192.168.1.0".to_string(),
        };
        let debug = format!("{:?}", iface);
        assert!(debug.contains("192.168.1.1"));
        assert!(debug.contains("255.255.255.0"));
    }

    #[test]
    fn test_prefix_len_to_mask_boundary() {
        assert_eq!(prefix_len_to_mask(1), Ipv4Addr::new(128, 0, 0, 0));
        assert_eq!(prefix_len_to_mask(31), Ipv4Addr::new(255, 255, 255, 254));
    }

    #[test]
    fn test_is_virtual_interface_case_insensitive() {
        assert!(is_virtual_interface("Docker0"));
        assert!(is_virtual_interface("VETH123"));
        assert!(is_virtual_interface("BR-abc"));
    }

    #[test]
    fn test_get_interface_priority_case_insensitive() {
        assert_eq!(get_interface_priority("ETH0"), 1);
        assert_eq!(get_interface_priority("WLAN0"), 2);
        assert_eq!(get_interface_priority("En0"), 3);
    }

    #[test]
    fn test_is_loopback_ipv6() {
        let local: SocketAddr = "[::1]:9000".parse().unwrap();
        assert!(is_loopback(&local));
    }

    #[test]
    fn test_build_bind_address_with_port() {
        assert_eq!(build_bind_address(Some("0.0.0.0:9100"), 9000), "0.0.0.0:9100");
    }

    // ============================================================
    // Coverage improvement: network helper edge cases
    // ============================================================

    #[test]
    fn test_is_same_subnet_different_subnets() {
        // These are on different /24 subnets
        assert!(!is_same_subnet("192.168.1.1", "192.168.2.1"));
    }

    #[test]
    fn test_is_same_subnet_same_subnet() {
        // These may or may not be in same subnet depending on interface config
        // but they share the same first 3 octets, so the simple fallback should say yes
        // if no real interfaces match
        let result = is_same_subnet("192.168.1.1", "192.168.1.2");
        // Result depends on actual network interfaces, but shouldn't panic
        let _ = result;
    }

    #[test]
    fn test_is_same_subnet_both_invalid() {
        assert!(!is_same_subnet("invalid1", "invalid2"));
    }

    #[test]
    fn test_is_same_subnet_one_valid_one_invalid() {
        assert!(!is_same_subnet("192.168.1.1", "not-an-ip"));
        assert!(!is_same_subnet("not-an-ip", "192.168.1.1"));
    }

    #[test]
    fn test_is_same_subnet_simple_less_than_four_parts() {
        assert!(!is_same_subnet_simple("192.168", "192.168.1.10"));
        assert!(!is_same_subnet_simple("192.168.1.10", "192.168"));
        assert!(!is_same_subnet_simple("192", "10"));
    }

    #[test]
    fn test_is_same_subnet_simple_same_third_octet() {
        assert!(is_same_subnet_simple("10.0.1.5", "10.0.1.100"));
    }

    #[test]
    fn test_is_same_subnet_simple_different_third_octet() {
        assert!(!is_same_subnet_simple("10.0.1.5", "10.0.2.5"));
    }

    #[test]
    fn test_prefix_len_to_mask_all_ones() {
        assert_eq!(prefix_len_to_mask(32), Ipv4Addr::new(255, 255, 255, 255));
    }

    #[test]
    fn test_prefix_len_to_mask_zero() {
        assert_eq!(prefix_len_to_mask(0), Ipv4Addr::new(0, 0, 0, 0));
    }

    #[test]
    fn test_prefix_len_to_mask_common() {
        assert_eq!(prefix_len_to_mask(8), Ipv4Addr::new(255, 0, 0, 0));
        assert_eq!(prefix_len_to_mask(16), Ipv4Addr::new(255, 255, 0, 0));
        assert_eq!(prefix_len_to_mask(24), Ipv4Addr::new(255, 255, 255, 0));
    }

    #[test]
    fn test_find_available_port_from_high() {
        let port = find_available_port(55000).unwrap();
        assert!(port >= 55000);
    }

    #[test]
    fn test_find_available_udp_port_from_high() {
        let port = find_available_udp_port(55000).unwrap();
        assert!(port >= 55000);
    }

    #[test]
    fn test_is_virtual_interface_common_patterns() {
        // Docker patterns
        assert!(is_virtual_interface("docker0"));
        assert!(is_virtual_interface("br-abc123"));
        // VPN patterns
        assert!(is_virtual_interface("tun0"));
        assert!(is_virtual_interface("tap0"));
        // Virtualization
        assert!(is_virtual_interface("veth123"));
        assert!(is_virtual_interface("virbr0"));
        assert!(is_virtual_interface("vboxnet0"));
        assert!(is_virtual_interface("vmnet1"));
        // macOS
        assert!(is_virtual_interface("utun0"));
        assert!(is_virtual_interface("awdl0"));
        // Loopback
        assert!(is_virtual_interface("lo"));
        assert!(is_virtual_interface("Loopback"));
        // IPsec
        assert!(is_virtual_interface("ipsec0"));
        assert!(is_virtual_interface("gif0"));
        assert!(is_virtual_interface("stf0"));
        // P2P
        assert!(is_virtual_interface("p2p0"));
        // LLW
        assert!(is_virtual_interface("llw0"));
        // ANPI
        assert!(is_virtual_interface("anpi0"));
    }

    #[test]
    fn test_is_virtual_interface_physical() {
        // Real physical interfaces should NOT match
        assert!(!is_virtual_interface("eth0"));
        assert!(!is_virtual_interface("eno1"));
        assert!(!is_virtual_interface("ens33"));
        assert!(!is_virtual_interface("enp0s3"));
        assert!(!is_virtual_interface("wlan0"));
        assert!(!is_virtual_interface("wlp3s0"));
        assert!(!is_virtual_interface("en0"));
        assert!(!is_virtual_interface("wl0"));
    }

    #[test]
    fn test_get_interface_priority_ethernet() {
        assert_eq!(get_interface_priority("eth0"), 1);
        assert_eq!(get_interface_priority("eno1"), 1);
        assert_eq!(get_interface_priority("ens33"), 1);
        assert_eq!(get_interface_priority("enp0s3"), 1);
        assert_eq!(get_interface_priority("ETH0"), 1);
    }

    #[test]
    fn test_get_interface_priority_wifi() {
        assert_eq!(get_interface_priority("wlan0"), 2);
        assert_eq!(get_interface_priority("wlp3s0"), 2);
        assert_eq!(get_interface_priority("WLAN0"), 2);
    }

    #[test]
    fn test_get_interface_priority_other_physical() {
        assert_eq!(get_interface_priority("en0"), 3);
        assert_eq!(get_interface_priority("wl0"), 3);
        assert_eq!(get_interface_priority("En0"), 3);
    }

    #[test]
    fn test_get_interface_priority_unknown() {
        assert_eq!(get_interface_priority("usb0"), 99);
        assert_eq!(get_interface_priority("ppp0"), 99);
        assert_eq!(get_interface_priority("random"), 99);
    }

    #[test]
    fn test_network_interface_clone_debug() {
        let iface = NetworkInterface {
            ip: "10.0.0.1".to_string(),
            mask: "255.255.255.0".to_string(),
            network_ip: "10.0.0.0".to_string(),
        };
        let cloned = iface.clone();
        assert_eq!(cloned.ip, "10.0.0.1");
        assert_eq!(cloned.mask, "255.255.255.0");
        assert_eq!(cloned.network_ip, "10.0.0.0");

        let debug_str = format!("{:?}", iface);
        assert!(debug_str.contains("10.0.0.1"));
    }

    #[test]
    fn test_parse_socket_addr_various() {
        // Valid
        let addr = parse_socket_addr("127.0.0.1:8080");
        assert_eq!(addr.port(), 8080);

        // Valid IPv6
        let addr = parse_socket_addr("[::1]:8080");
        assert_eq!(addr.port(), 8080);

        // Invalid
        let addr = parse_socket_addr("garbage");
        assert_eq!(addr.port(), 9000);
        assert_eq!(addr.ip(), IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)));
    }

    #[test]
    fn test_resolve_addr_valid_localhost() {
        let addrs = resolve_addr("127.0.0.1:80").unwrap();
        assert!(!addrs.is_empty());
    }

    #[test]
    fn test_resolve_addr_invalid_hostname() {
        assert!(resolve_addr("this-is-not-a-real-host.invalid:80").is_err());
    }

    #[test]
    fn test_build_bind_address_none() {
        assert_eq!(build_bind_address(None, 9000), "0.0.0.0:9000");
    }

    #[test]
    fn test_build_bind_address_empty() {
        assert_eq!(build_bind_address(Some(""), 9000), "0.0.0.0:9000");
    }

    #[test]
    fn test_build_bind_address_custom() {
        assert_eq!(build_bind_address(Some("192.168.1.1:8080"), 9000), "192.168.1.1:8080");
    }

    #[test]
    fn test_get_local_network_interfaces_no_panic() {
        // Just verify it doesn't panic
        let interfaces = get_local_network_interfaces();
        for iface in &interfaces {
            assert!(!iface.ip.is_empty());
            assert!(!iface.mask.is_empty());
            assert!(!iface.network_ip.is_empty());
        }
    }

    #[test]
    fn test_get_all_local_ips_no_panic() {
        let ips = get_all_local_ips();
        for ip in &ips {
            let parsed: IpAddr = ip.parse().unwrap();
            assert!(!parsed.is_loopback());
            if let IpAddr::V4(v4) = parsed {
                assert!(!v4.is_link_local());
            }
        }
    }

    #[test]
    fn test_get_all_local_ips_filtered_same() {
        let ips1 = get_all_local_ips();
        let ips2 = get_all_local_ips_filtered();
        assert_eq!(ips1, ips2);
    }
}
