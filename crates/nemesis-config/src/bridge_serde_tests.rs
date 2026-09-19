//! `bridge` 配置段 serde 测试（goal：节点显示名 + 反向桥，一期批次一）。
//!
//! 语义锚点：`bridge` 缺省 = None（零行为变化）；`bridge` 段内每字段
//! `#[serde(default)]` 全可省；空 token = 接入门不开放（fail-closed）；
//! 客户端 `enabled` 缺省 false（不连中继）。

use crate::{BridgeClientConfig, BridgeConfig, BridgeServerConfig};

/// 反序列化辅助：直接 serde 解析 Config（不经 env/文件系统）。
fn parse(json: &str) -> crate::Config {
    serde_json::from_str(json).expect("配置 JSON 应合法")
}

#[test]
fn bridge_absent_means_none() {
    let cfg = parse("{}");
    assert!(cfg.bridge.is_none(), "bridge 段缺省 = None（零行为变化）");
}

#[test]
fn bridge_empty_object_gives_all_defaults() {
    let cfg = parse(r#"{"bridge": {}}"#);
    let bridge = cfg.bridge.expect("bridge 段在场");
    // 服务端：空 token = 接入门不开放（fail-closed）。
    assert_eq!(bridge.server.token, "");
    // 客户端：默认关（零行为变化）。
    assert!(!bridge.client.enabled);
    assert_eq!(bridge.client.relay_url, "");
    assert_eq!(bridge.client.token, "");
    assert_eq!(bridge.client.access_token, "");
}

#[test]
fn bridge_server_token_only() {
    let cfg = parse(r#"{"bridge": {"server": {"token": "pair-key-1"}}}"#);
    let bridge = cfg.bridge.unwrap();
    assert_eq!(bridge.server.token, "pair-key-1");
    // client 段省略 → 全默认。
    assert_eq!(bridge.client, BridgeClientConfig::default());
}

#[test]
fn bridge_client_full_fields() {
    let cfg = parse(
        r#"{"bridge": {"client": {"enabled": true, "relay_url": "ws://vps:60600",
             "token": "pair-key-2", "access_token": "panel-pass"}}}"#,
    );
    let bridge = cfg.bridge.unwrap();
    // server 段省略 → 全默认。
    assert_eq!(bridge.server, BridgeServerConfig::default());
    assert!(bridge.client.enabled);
    assert_eq!(bridge.client.relay_url, "ws://vps:60600");
    assert_eq!(bridge.client.token, "pair-key-2");
    assert_eq!(bridge.client.access_token, "panel-pass");
}

#[test]
fn bridge_serde_round_trip_lossless() {
    let original = BridgeConfig {
        server: BridgeServerConfig {
            token: "tok-🔐".to_string(),
        },
        client: BridgeClientConfig {
            enabled: true,
            relay_url: "ws://中继.example.com:60600".to_string(),
            token: "tok-🔐".to_string(),
            access_token: "密码".to_string(),
        },
    };
    let cfg = format!(
        r#"{{"bridge": {}}}"#,
        serde_json::to_string(&original).expect("序列化")
    );
    let parsed = parse(&cfg);
    assert_eq!(parsed.bridge.as_ref(), Some(&original), "round-trip 应无损");
}
