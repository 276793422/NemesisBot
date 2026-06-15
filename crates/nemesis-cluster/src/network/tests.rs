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
    // Use IPs from different top-level ranges (10.x vs 192.168.x) so the
    // result is independent of the host's local interface masks. The legacy
    // test (192.168.1.1 vs 192.168.2.1) was environment-sensitive: if the
    // host has a /16 interface (common under WSL/Docker), the mask-based
    // check returned `true`, masking the failure. A different A-class pair
    // makes the assertion stable across environments.
    assert!(!is_same_subnet("10.0.0.1", "192.168.1.1"));
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
