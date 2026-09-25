//! wave6（2026-09-25）：`parse_ipv4_octets` / `select_advertised_lan_ip`
//! 纯函数补测。此前套件只喂过合法 v4 输入——5 段超长点分串（5-octet →
//! None 哨兵臂）与其余形态从未触达。

use super::{parse_ipv4_octets, select_advertised_lan_ip};

#[test]
fn w6_parse_ipv4_octets_five_octets_is_none() {
    // 第 5 段存在 → `it.next().is_some()` 哨兵 → None。
    assert_eq!(parse_ipv4_octets("10.0.0.1.9"), None);
}

#[test]
fn w6_parse_ipv4_octets_short_and_garbage_shapes_are_none() {
    assert_eq!(parse_ipv4_octets("10.0.0"), None); // 3 段
    assert_eq!(parse_ipv4_octets("10.0"), None); // 2 段
    assert_eq!(parse_ipv4_octets("10"), None); // 1 段
    assert_eq!(parse_ipv4_octets(""), None); // 空
    assert_eq!(parse_ipv4_octets("a.b.c.d"), None); // 非数字
    assert_eq!(parse_ipv4_octets("300.1.2.3"), None); // u8 溢出
}

#[test]
fn w6_parse_ipv4_octets_valid_with_trim() {
    assert_eq!(parse_ipv4_octets("192.168.1.7"), Some((192, 168, 1, 7)));
    assert_eq!(parse_ipv4_octets("  10.1.2.3  "), Some((10, 1, 2, 3))); // trim 生效
    assert_eq!(parse_ipv4_octets("0.0.0.0"), Some((0, 0, 0, 0)));
}

#[test]
fn w6_select_advertised_lan_ip_prefers_peer_subnet_over_first_guess() {
    // 本机双网卡：首猜 10.0.0.5 与 peer 同段——同 /24 优先命中。
    let ips = vec!["192.168.50.2".to_string(), "10.0.0.5".to_string()];
    let peers = vec!["10.0.0.77:9100".to_string()];
    assert_eq!(
        select_advertised_lan_ip(&ips, &peers).as_deref(),
        Some("10.0.0.5")
    );
}

#[test]
fn w6_select_advertised_lan_ip_falls_back_to_first_non_loopback() {
    // 无 peer（或 peer 全回环/非 v4）→ 首个非回环本机 IP。
    let ips = vec!["127.0.0.1".to_string(), "192.168.1.10".to_string()];
    assert_eq!(
        select_advertised_lan_ip(&ips, &[]).as_deref(),
        Some("192.168.1.10")
    );
    // peer 非 v4 形态（解析失败）不参与同段匹配，走回落。
    let peers = vec!["not-an-ip:1234".to_string(), "1.2.3.4.5:80".to_string()];
    assert_eq!(
        select_advertised_lan_ip(&ips, &peers).as_deref(),
        Some("192.168.1.10")
    );
}

#[test]
fn w6_select_advertised_lan_ip_all_loopback_returns_none() {
    let ips = vec!["127.0.0.1".to_string(), "127.9.9.9".to_string()];
    assert_eq!(select_advertised_lan_ip(&ips, &[]), None);
}
