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
    addr_str
        .parse()
        .unwrap_or_else(|_| SocketAddr::new(IpAddr::V4(Ipv4Addr::new(0, 0, 0, 0)), 9000))
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
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
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
/// Checks whether two IPs are in the same subnet **according to the local
/// network interfaces' masks**.
///
/// Note: this is NOT a pure function of `(ip1, ip2)` — the result depends on
/// what network interfaces the host has. If the host has a /16 interface
/// (common under WSL/Docker), `192.168.1.1` and `192.168.2.1` will be
/// considered the same subnet. Callers needing deterministic behavior should
/// use `is_same_subnet_simple` directly with an explicit `/24` assumption.
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

    // Patterns safe for substring matching (long enough to avoid false positives)
    let contains_patterns = [
        "docker", "br-", "virbr", "vbox", "vmnet", "awdl", "anpi", "ipsec", "loopback",
    ];
    if contains_patterns.iter().any(|p| lower.contains(p)) {
        return true;
    }

    // Short patterns: use starts_with to reduce false positives.
    // "veth" matches Linux container veth (veth0, veth1abc) but NOT
    // Windows Hyper-V "vethernet" which is a real virtual switch.
    let starts_with_patterns = ["tun", "tap", "utun", "llw", "gif", "stf", "p2p"];
    if starts_with_patterns.iter().any(|p| lower.starts_with(p)) {
        return true;
    }

    // Linux veth interfaces: "veth" followed by digits/hex (e.g. veth0, veth2f3a)
    // but NOT "vethernet" (Windows Hyper-V virtual switch)
    if lower.starts_with("veth") && !lower.starts_with("vethernet") {
        return true;
    }

    // Linux loopback: exact name "lo" or "lo"+digit (lo0, lo1)
    if lower == "lo" || lower.starts_with("lo0") {
        return true;
    }

    false
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
mod tests;
