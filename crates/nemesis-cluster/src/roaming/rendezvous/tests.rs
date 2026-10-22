//! rendezvous 客户端半边单测：注册表地址推导（显式/默认/IPv6/坏 relay_url）
//! + 拨号地址组装（relay_url 兜底）。

use super::{RendezvousMember, member_dial_address, resolve_registry_url};

// ---------------------------------------------------------------------------
// resolve_registry_url
// ---------------------------------------------------------------------------

#[test]
fn explicit_rendezvous_url_wins() {
    assert_eq!(
        resolve_registry_url(
            " http://reg.example.com:9443 ",
            "http://relay.example.com:3340"
        )
        .expect("explicit"),
        "http://reg.example.com:9443",
        "显式 rendezvous_url 优先（trim 后原样）"
    );
}

#[test]
fn derive_from_relay_http() {
    assert_eq!(
        resolve_registry_url("", "http://vps.example.com:3340").expect("derive"),
        "http://vps.example.com:3341",
        "默认推导 = relay host + 3341"
    );
}

#[test]
fn derive_from_relay_https_and_no_port() {
    assert_eq!(
        resolve_registry_url("", "https://vps.example.com:8443").expect("derive https"),
        "https://vps.example.com:3341",
        "scheme 保留（https relay → https registry）"
    );
    assert_eq!(
        resolve_registry_url("", "http://vps.example.com").expect("derive no port"),
        "http://vps.example.com:3341",
        "无端口 relay_url 同样可推导"
    );
}

#[test]
fn derive_ipv6_host_keeps_brackets() {
    assert_eq!(
        resolve_registry_url("", "http://[2001:db8::1]:3340").expect("derive ipv6"),
        "http://[2001:db8::1]:3341",
        "IPv6 字面量方括号保留"
    );
}

#[test]
fn derive_rejects_bad_relay_url() {
    assert!(
        resolve_registry_url("", "vps.example.com:3340").is_err(),
        "缺 scheme 必须报错"
    );
    assert!(
        resolve_registry_url("", "ftp://vps.example.com").is_err(),
        "非 http(s) scheme 必须报错"
    );
}

// ---------------------------------------------------------------------------
// member_dial_address
// ---------------------------------------------------------------------------

fn member(iroh_hex: &str, relay_url: &str) -> RendezvousMember {
    RendezvousMember {
        node_id: "node-b".to_string(),
        name: String::new(),
        role: String::new(),
        category: String::new(),
        node_type: String::new(),
        rpc_port: 21949,
        tags: Vec::new(),
        professions: Vec::new(),
        tier: None,
        iroh_id: iroh_hex.to_string(),
        build: String::new(),
        relay_url: relay_url.to_string(),
        features: Vec::new(),
        last_seen_ms: 0,
    }
}

#[test]
fn dial_address_uses_member_relay_url() {
    let m = member(&"aa".repeat(32), "http://vps.example.com:3340");
    assert_eq!(
        member_dial_address(&m, "http://fallback:3340"),
        format!("iroh:{}@http://vps.example.com:3340", "aa".repeat(32)),
        "成员自报 relay_url 优先"
    );
}

#[test]
fn dial_address_falls_back_to_local_relay_url() {
    let m = member(&"bb".repeat(32), "");
    assert_eq!(
        member_dial_address(&m, "http://fallback.example.com:3340"),
        format!("iroh:{}@http://fallback.example.com:3340", "bb".repeat(32)),
        "成员未自报 = 本方 relay_url 兜底（同中继部署下两者一致）"
    );
}
