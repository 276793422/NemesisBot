//! roaming 段配置解析测试（2026-10-23 收缩后：段内只剩 `enabled` 一个键）：
//! 缺文件 / 坏 JSON / 缺段 / enabled 六态类型语义 / 残留旧键忽略 / 与
//! `roaming_enabled_in_config` 同源一致性。QUIC 键（relay_url/relay_token/
//! rendezvous_url/relay 段）的解析测试在 [`super::super::quic_config::tests`]
//!（迁移后新家）。

use std::io::Write as _;

use super::{RoamingOverlayConfig, parse_roaming_value, resolved_roaming};
use crate::cluster::Cluster;

/// 落临时 config.cluster.json 并返回其路径。
fn write_cfg(dir: &std::path::Path, body: &str) -> std::path::PathBuf {
    let path = dir.join("config.cluster.json");
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(body.as_bytes()).expect("write");
    path
}

// ---------------------------------------------------------------------------
// 缺文件 / 坏 JSON / 缺段
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

// ---------------------------------------------------------------------------
// enabled 键类型语义（严格 JSON bool）
// ---------------------------------------------------------------------------

#[test]
fn enabled_shape_honored() {
    // 收缩后段内唯一键：显式 true / 显式 false / 缺键。
    let cfg = parse_roaming_value(Some(&serde_json::json!({ "roaming": { "enabled": true } })));
    assert!(cfg.enabled);
    let cfg = parse_roaming_value(Some(
        &serde_json::json!({ "roaming": { "enabled": false } }),
    ));
    assert!(!cfg.enabled);
    let cfg = parse_roaming_value(Some(&serde_json::json!({ "roaming": {} })));
    assert!(!cfg.enabled, "缺键 = 默认关");
}

#[test]
fn enabled_type_mismatch_falls_to_false() {
    // 真值形态（"true" 字符串 / 1 数值）不等价于 JSON true。
    let cfg = parse_roaming_value(Some(
        &serde_json::json!({ "roaming": { "enabled": "true" } }),
    ));
    assert!(!cfg.enabled);
    let cfg = parse_roaming_value(Some(&serde_json::json!({ "roaming": { "enabled": 1 } })));
    assert!(!cfg.enabled);
}

#[test]
fn legacy_relay_keys_are_ignored_after_decoupling() {
    // 2026-10-23 解耦回归锁：cluster.json roaming 段残留旧 QUIC 键
    //（首读迁移只拷贝不删除）= 无害遗留，解析忽略、enabled 语义不变。
    let cfg = parse_roaming_value(Some(&serde_json::json!({
        "roaming": {
            "enabled": true,
            "relay_url": "http://vps.example.com:3340",
            "relay_token": "client-tok",
            "rendezvous_url": "http://vps.example.com:3341",
            "relay": { "enabled": true, "http_port": 3340, "token": "s3cret" }
        }
    })));
    assert!(cfg.enabled);
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
