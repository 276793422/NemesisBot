//! quic 配置解析测试：四态（缺文件 / 坏 JSON / 缺段 / 全字段 / 类型不符
//! 逐字段降级）+ 旧位置首读迁移（一次性 / 幂等 / 无旧形态不生成 / 旧键
//! 保留 / 未知键保真）。

use std::io::Write as _;

use super::{QuicConfig, ensure_quic_config, parse_quic_value, resolved_quic};
use crate::roaming::config::{RelaySettings, RelayTls};

/// 落临时文件并返回其路径。
fn write_file(dir: &std::path::Path, name: &str, body: &str) -> std::path::PathBuf {
    let path = dir.join(name);
    let mut f = std::fs::File::create(&path).expect("create");
    f.write_all(body.as_bytes()).expect("write");
    path
}

// ---------------------------------------------------------------------------
// 解析四态
// ---------------------------------------------------------------------------

#[test]
fn missing_file_all_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cfg = resolved_quic(&tmp.path().join("no-such-file.json"));
    assert_eq!(cfg, QuicConfig::default());
}

#[test]
fn bad_json_all_defaults() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let path = write_file(tmp.path(), "config.quic.json", "{ not json !!!");
    assert_eq!(resolved_quic(&path), QuicConfig::default());
}

#[test]
fn missing_sections_defaults() {
    let cfg = parse_quic_value(Some(&serde_json::json!({ "unrelated": true })));
    assert_eq!(cfg, QuicConfig::default());
}

#[test]
fn full_shape_both_segments_honored() {
    let cfg = parse_quic_value(Some(&serde_json::json!({
        "relay": {
            "enabled": true,
            "http_port": 3341,
            "quic_port": 7843,
            "registry_port": 3342,
            "external_host": "vps.example.com",
            "tls": "manual",
            "token": "s3cret",
            "cert_path": "/etc/cert.pem",
            "key_path": "/etc/key.pem"
        },
        "client": {
            "relay_url": "http://vps.example.com:3340",
            "relay_token": "client-token",
            "rendezvous_url": "http://vps.example.com:3341"
        }
    })));
    assert!(cfg.relay.enabled);
    assert_eq!(cfg.relay.http_port, 3341);
    assert_eq!(cfg.relay.quic_port, 7843);
    assert_eq!(cfg.relay.registry_port, 3342);
    assert_eq!(cfg.relay.external_host, "vps.example.com");
    assert_eq!(cfg.relay.tls, RelayTls::Manual);
    assert_eq!(cfg.relay.token, "s3cret");
    assert_eq!(cfg.relay.cert_path, "/etc/cert.pem");
    assert_eq!(cfg.relay.key_path, "/etc/key.pem");
    assert_eq!(cfg.client.relay_url, "http://vps.example.com:3340");
    assert_eq!(cfg.client.relay_token, "client-token");
    assert_eq!(cfg.client.rendezvous_url, "http://vps.example.com:3341");
}

#[test]
fn type_mismatches_fall_back_per_field() {
    let cfg = parse_quic_value(Some(&serde_json::json!({
        "relay": { "enabled": 1, "http_port": 99999, "tls": 123 },
        "client": { "relay_url": 42, "relay_token": true }
    })));
    // 逐字段落默认：真值形态（1 数值）不等价于 JSON true。
    assert!(!cfg.relay.enabled);
    assert_eq!(
        cfg.relay.http_port,
        crate::roaming::config::DEFAULT_RELAY_HTTP_PORT
    );
    assert_eq!(cfg.relay.tls, RelayTls::SelfSigned);
    assert_eq!(cfg.client.relay_url, "");
    assert_eq!(cfg.client.relay_token, "");
}

#[test]
fn port_zero_is_honored_not_defaulted() {
    // 0 是合法显式值（= 不监听），必须与「缺键」区分。
    let cfg = parse_quic_value(Some(&serde_json::json!({
        "relay": { "http_port": 0, "quic_port": 0, "registry_port": 0 }
    })));
    assert_eq!(cfg.relay.http_port, 0);
    assert_eq!(cfg.relay.quic_port, 0);
    assert_eq!(cfg.relay.registry_port, 0);
}

// ---------------------------------------------------------------------------
// 旧位置首读迁移（ensure_quic_config）
// ---------------------------------------------------------------------------

/// 旧形态 cluster.json（roaming.relay 段 + 客户端三键全带）。
const LEGACY_BODY: &str = r#"{
  "enabled": true,
  "port": 5600,
  "token": "cluster-token",
  "roaming": {
    "enabled": true,
    "relay_url": "http://vps.example.com:3340",
    "relay_token": "legacy-client-token",
    "rendezvous_url": "",
    "relay": {
      "enabled": true,
      "http_port": 3350,
      "quic_port": 7843,
      "registry_port": 3351,
      "external_host": "vps.example.com",
      "tls": "none",
      "token": "legacy-relay-token"
    }
  }
}"#;

#[test]
fn migrate_moves_legacy_shape_to_quic_json() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cluster_path = write_file(tmp.path(), "config.cluster.json", LEGACY_BODY);
    let quic_path = tmp.path().join("config.quic.json");
    let before = std::fs::read_to_string(&cluster_path).expect("read cluster");

    assert!(ensure_quic_config(&cluster_path, &quic_path), "应发生迁移");

    // 解析读回与旧值一致（tls "none" 字符串形态原样保留）。
    let cfg = resolved_quic(&quic_path);
    assert!(cfg.relay.enabled);
    assert_eq!(cfg.relay.http_port, 3350);
    assert_eq!(cfg.relay.quic_port, 7843);
    assert_eq!(cfg.relay.registry_port, 3351);
    assert_eq!(cfg.relay.external_host, "vps.example.com");
    assert_eq!(cfg.relay.tls, RelayTls::None);
    assert_eq!(cfg.relay.token, "legacy-relay-token");
    assert_eq!(cfg.client.relay_url, "http://vps.example.com:3340");
    assert_eq!(cfg.client.relay_token, "legacy-client-token");
    assert_eq!(cfg.client.rendezvous_url, "");

    // 旧键保留不删：cluster.json 字节不动。
    let after = std::fs::read_to_string(&cluster_path).expect("read cluster");
    assert_eq!(before, after, "迁移不碰 cluster.json");
}

#[test]
fn migrate_is_idempotent_once_quic_json_exists() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cluster_path = write_file(tmp.path(), "config.cluster.json", LEGACY_BODY);
    let quic_path = write_file(
        tmp.path(),
        "config.quic.json",
        r#"{"relay":{"token":"user-edited"}}"#,
    );

    assert!(
        !ensure_quic_config(&cluster_path, &quic_path),
        "幂等：已在不覆盖"
    );
    // 用户文件原样。
    let content = std::fs::read_to_string(&quic_path).expect("read quic");
    assert!(content.contains("user-edited"));
}

#[test]
fn migrate_skips_when_no_legacy_shape() {
    let tmp = tempfile::tempdir().expect("tempdir");
    // 场景 A：无 roaming 段；场景 B：roaming 只有 enabled（叠加层开关留守，
    // 不构成迁移触发）；场景 C：cluster.json 缺席。三者都不生成 quic.json。
    for body in [
        r#"{"enabled":true,"port":5600}"#,
        r#"{"roaming":{"enabled":true}}"#,
    ] {
        let cluster_path = write_file(tmp.path(), "config.cluster.json", body);
        let quic_path = tmp.path().join("config.quic.json");
        assert!(!ensure_quic_config(&cluster_path, &quic_path));
        assert!(!quic_path.exists(), "无旧形态不应生成文件");
        let _ = std::fs::remove_file(&cluster_path);
    }
    let quic_path = tmp.path().join("config.quic.json");
    assert!(!ensure_quic_config(
        &tmp.path().join("no-such.json"),
        &quic_path
    ));
    assert!(!quic_path.exists());
}

#[test]
fn migrate_bad_cluster_json_is_noop() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cluster_path = write_file(tmp.path(), "config.cluster.json", "{ broken");
    let quic_path = tmp.path().join("config.quic.json");
    assert!(!ensure_quic_config(&cluster_path, &quic_path));
    assert!(!quic_path.exists());
}

#[test]
fn migrate_client_keys_only_still_migrates() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cluster_path = write_file(
        tmp.path(),
        "config.cluster.json",
        r#"{"roaming":{"enabled":true,"relay_url":"http://vps:3340","relay_token":"tk"}}"#,
    );
    let quic_path = tmp.path().join("config.quic.json");
    assert!(ensure_quic_config(&cluster_path, &quic_path));
    let cfg = resolved_quic(&quic_path);
    assert_eq!(
        cfg.relay,
        RelaySettings::default(),
        "无 relay 段 = 服务端默认"
    );
    assert_eq!(cfg.client.relay_url, "http://vps:3340");
    assert_eq!(cfg.client.relay_token, "tk");
}

#[test]
fn migrate_preserves_unknown_relay_keys() {
    let tmp = tempfile::tempdir().expect("tempdir");
    let cluster_path = write_file(
        tmp.path(),
        "config.cluster.json",
        r#"{"roaming":{"relay":{"enabled":true,"future_key":42}}}"#,
    );
    let quic_path = tmp.path().join("config.quic.json");
    assert!(ensure_quic_config(&cluster_path, &quic_path));
    let raw = std::fs::read_to_string(&quic_path).expect("read quic");
    let v: serde_json::Value = serde_json::from_str(&raw).expect("parse");
    assert_eq!(v["relay"]["future_key"], 42, "未知键 Value 级保真");
}
