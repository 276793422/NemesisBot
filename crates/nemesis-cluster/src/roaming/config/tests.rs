//! roaming 段配置解析测试：六态（缺文件 / 坏 JSON / 缺段 / 旧形态兼容 /
//! 全字段 / 类型不符逐字段降级）+ 与 `roaming_enabled_in_config` 同源一致性。

use std::io::Write as _;

use super::{
    DEFAULT_RELAY_HTTP_PORT, DEFAULT_RELAY_QUIC_PORT, RelaySettings, RelayTls,
    RoamingOverlayConfig, parse_roaming_value, resolved_roaming,
};
use crate::cluster::Cluster;

/// 落临时 config.cluster.json 并返回其路径。
fn write_cfg(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("config.cluster.json");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(body.as_bytes()).expect("write");
    path
}

// ---------------------------------------------------------------------------
// 六态
// ---------------------------------------------------------------------------

#[test]
fn missing_file_all_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = resolved_roaming(&tmp.path().join("no-such-file.json"));
    assert_eq!(cfg, RoamingOverlayConfig::default());
}

#[test]
fn bad_json_all_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = write_cfg(tmp.path(), "{ not json !!!");
    assert_eq!(resolved_roaming(&path), RoamingOverlayConfig::default());
}

#[test]
fn missing_section_defaults() {
    // 集群其他键在场、roaming 段缺席 = 默认（roaming 关）。
    let cfg = parse_roaming_value(Some(&serde_json::json!({ "enabled": true, "port": 5600 })));
    assert_eq!(cfg, RoamingOverlayConfig::default());
}

#[test]
fn legacy_shape_enabled_only_is_backward_compatible() {
    // 2026-10-18 交付形态：roaming 段只有 enabled 键——缺新键 = 默认，
    // enabled 语义字节不变（回归锁）。
    let cfg = parse_roaming_value(Some(&serde_json::json!({
        "roaming": { "enabled": true }
    })));
    assert!(cfg.enabled);
    assert_eq!(cfg.relay_url, "");
    assert_eq!(cfg.relay_token, "");
    assert_eq!(cfg.relay, RelaySettings::default());
}

#[test]
fn full_shape_all_fields_honored() {
    let cfg = parse_roaming_value(Some(&serde_json::json!({
        "roaming": {
            "enabled": true,
            "relay_url": "http://vps.example.com:3340",
            "relay_token": "client-token",
            "relay": {
                "enabled": true,
                "http_port": 3341,
                "quic_port": 7843,
                "external_host": "vps.example.com",
                "tls": "manual",
                "token": "s3cret",
                "cert_path": "/etc/cert.pem",
                "key_path": "/etc/key.pem"
            }
        }
    })));
    assert!(cfg.enabled);
    assert_eq!(cfg.relay_url, "http://vps.example.com:3340");
    assert_eq!(cfg.relay_token, "client-token");
    assert!(cfg.relay.enabled);
    assert_eq!(cfg.relay.http_port, 3341);
    assert_eq!(cfg.relay.quic_port, 7843);
    assert_eq!(cfg.relay.external_host, "vps.example.com");
    assert_eq!(cfg.relay.tls, RelayTls::Manual);
    assert_eq!(cfg.relay.token, "s3cret");
    assert_eq!(cfg.relay.cert_path, "/etc/cert.pem");
    assert_eq!(cfg.relay.key_path, "/etc/key.pem");
}

#[test]
fn type_mismatches_fall_back_per_field() {
    let cfg = parse_roaming_value(Some(&serde_json::json!({
        "roaming": {
            "enabled": "true",
            "relay_url": 42,
            "relay": {
                "enabled": 1,
                "http_port": 99999,
                "quic_port": -1,
                "external_host": true,
                "tls": 123,
                "token": ["x"]
            }
        }
    })));
    // 逐字段落默认：真值形态（"true" 字符串 / 1 数值）不等价于 JSON true。
    assert!(!cfg.enabled);
    assert_eq!(cfg.relay_url, "");
    assert!(!cfg.relay.enabled);
    assert_eq!(cfg.relay.http_port, DEFAULT_RELAY_HTTP_PORT);
    assert_eq!(cfg.relay.quic_port, DEFAULT_RELAY_QUIC_PORT);
    assert_eq!(cfg.relay.external_host, "");
    assert_eq!(cfg.relay.tls, RelayTls::SelfSigned);
    assert_eq!(cfg.relay.token, "");
}

// ---------------------------------------------------------------------------
// 值域与档位解析
// ---------------------------------------------------------------------------

#[test]
fn tls_case_insensitive_and_unknown_falls_to_default() {
    let tls = |v: &str| {
        parse_roaming_value(Some(&serde_json::json!({
            "roaming": { "relay": { "tls": v } }
        })))
        .relay
        .tls
    };
    assert_eq!(tls("self-signed"), RelayTls::SelfSigned);
    assert_eq!(tls("SELF-SIGNED"), RelayTls::SelfSigned);
    assert_eq!(tls(" manual "), RelayTls::Manual);
    assert_eq!(tls("None"), RelayTls::None);
    assert_eq!(tls("acme"), RelayTls::SelfSigned, "未知档落默认");
}

#[test]
fn port_zero_is_honored_not_defaulted() {
    // 0 是合法显式值（= 不监听该 transport），必须与「缺键」区分。
    let cfg = parse_roaming_value(Some(&serde_json::json!({
        "roaming": { "relay": { "http_port": 0, "quic_port": 0 } }
    })));
    assert_eq!(cfg.relay.http_port, 0);
    assert_eq!(cfg.relay.quic_port, 0);
}

// ---------------------------------------------------------------------------
// 单一真相源：resolved_roaming().enabled ≡ roaming_enabled_in_config
// ---------------------------------------------------------------------------

#[test]
fn enabled_matches_roaming_enabled_in_config() {
    let tmp = tempfile::tempdir().expect("tempdir");

    let path = write_cfg(tmp.path(), r#"{"roaming":{"enabled":true}}"#);
    assert!(resolved_roaming(&path).enabled);
    assert!(Cluster::roaming_enabled_in_config(&path));

    let path = write_cfg(
        tmp.path(),
        r#"{"roaming":{"enabled":false,"relay":{"enabled":true}}}"#,
    );
    assert!(!resolved_roaming(&path).enabled);
    assert!(!Cluster::roaming_enabled_in_config(&path));

    let path = write_cfg(tmp.path(), "broken");
    assert!(!resolved_roaming(&path).enabled);
    assert!(!Cluster::roaming_enabled_in_config(&path));
}
