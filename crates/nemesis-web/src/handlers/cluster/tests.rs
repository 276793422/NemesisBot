use super::*;
use std::time::Duration;

#[test]
fn format_duration_zero() {
    assert_eq!(format_duration(Duration::from_secs(0)), "0s");
}

#[test]
fn format_duration_seconds_only() {
    assert_eq!(format_duration(Duration::from_secs(45)), "45s");
}

#[test]
fn format_duration_minutes() {
    assert_eq!(format_duration(Duration::from_secs(125)), "2m 5s");
}

#[test]
fn format_duration_hours() {
    assert_eq!(format_duration(Duration::from_secs(3725)), "1h 2m");
}

#[test]
fn format_duration_days() {
    assert_eq!(format_duration(Duration::from_secs(90061)), "1d 1h");
}

#[test]
fn format_ago_branches() {
    assert_eq!(format_ago(Duration::from_secs(5)), "5s ago");
    assert_eq!(format_ago(Duration::from_secs(120)), "2m ago");
    assert_eq!(format_ago(Duration::from_secs(7200)), "2h ago");
    assert_eq!(format_ago(Duration::from_secs(86400 * 3)), "3d ago");
}

#[test]
fn format_bytes_units() {
    assert_eq!(format_bytes(512), "512B");
    assert_eq!(format_bytes(2048), "2KB");
    assert_eq!(format_bytes(1024 * 1024 * 5), "5.0MB");
}

#[test]
fn truncate_str_no_truncation_when_short_enough() {
    assert_eq!(truncate_str("hello", 10), "hello");
    assert_eq!(truncate_str("hello", 5), "hello");
}

#[test]
fn truncate_str_ascii_truncates_with_ellipsis() {
    assert_eq!(truncate_str("hello world", 5), "hello...");
}

#[test]
fn truncate_str_utf8_lands_on_char_boundary() {
    // Each emoji is 4 bytes; max_len=6 must not split a multi-byte char.
    let out = truncate_str("🎯🎯🎯🎯", 6);
    assert!(out.ends_with("..."));
    // Should contain exactly one full emoji before the ellipsis (4 bytes ≤ 6).
    assert_eq!(out, "🎯...");
}

#[test]
fn test_broadcast_flag_passes_on_any_platform() {
    // Binding 0.0.0.0:0 always succeeds; setting SO_BROADCAST is universally
    // supported. This verifies the success-shape of the diagnostic helper.
    let r = test_broadcast_flag();
    assert_eq!(r["name"], "broadcast_flag");
    assert_eq!(r["pass"], true);
}

// --- iroh 跨网漫游开关面（feature=cluster-roaming；config 闸 + enable/disable） ---

#[cfg(feature = "cluster-roaming")]
#[test]
fn test_roaming_config_write_and_gate_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    let cfg_path = cluster_config_path(ws.to_str().unwrap());
    // 初始：无 config = 关
    assert!(!Cluster::roaming_enabled_in_config(&cfg_path));
    // 写 true → 闸读回 true
    update_cluster_config_roaming(ws.to_str().unwrap(), true).unwrap();
    assert!(Cluster::roaming_enabled_in_config(&cfg_path));
    // 写 false → 闸读回 false（且文件其余键保留）
    update_cluster_config_roaming(ws.to_str().unwrap(), false).unwrap();
    assert!(!Cluster::roaming_enabled_in_config(&cfg_path));
    let raw = std::fs::read_to_string(&cfg_path).unwrap();
    let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
    assert_eq!(v["roaming"]["enabled"], serde_json::json!(false));
}

#[cfg(feature = "cluster-roaming")]
#[test]
fn test_roaming_status_payload_default_disabled() {
    let dir = tempfile::tempdir().unwrap();
    let payload = roaming_status_payload(dir.path().to_str().unwrap(), None)
        .unwrap()
        .unwrap();
    assert_eq!(payload["available"], serde_json::json!(true));
    assert_eq!(payload["enabled"], serde_json::json!(false));
    assert_eq!(payload["active"], serde_json::json!(false));
    assert_eq!(payload["requires_restart"], serde_json::json!(false));
    assert_eq!(payload["key_present"], serde_json::json!(false));
    assert!(payload["node_id"].is_null());
    assert_eq!(payload["trust"]["enabled"], serde_json::json!(false));
}

#[cfg(feature = "cluster-roaming")]
#[test]
fn test_roaming_status_payload_requires_restart_consistency() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    update_cluster_config_roaming(ws, true).unwrap();
    // config 开了但运行态未知（None→false）：requires_restart=true
    let p = roaming_status_payload(ws, None).unwrap().unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(true));
    // 运行态跟上（active=true）：requires_restart=false
    let p = roaming_status_payload(ws, Some(true)).unwrap().unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    assert_eq!(p["active"], serde_json::json!(true));
}

#[cfg(feature = "cluster-roaming")]
#[test]
fn test_roaming_set_enabled_creates_key_once() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path();
    // enable：缺 key 顺手生成
    let p = roaming_set_enabled_impl(ws.to_str().unwrap(), true)
        .unwrap()
        .unwrap();
    assert_eq!(p["key_created"], serde_json::json!(true));
    let node1 = p["node_id"].as_str().unwrap();
    assert_eq!(node1.len(), 64);
    assert!(roaming_key_path(ws).exists());
    // config 闸同步为 true
    assert!(Cluster::roaming_enabled_in_config(&cluster_config_path(
        ws.to_str().unwrap()
    )));
    // 二次 enable：key 不重建、身份稳定
    let p2 = roaming_set_enabled_impl(ws.to_str().unwrap(), true)
        .unwrap()
        .unwrap();
    assert_eq!(p2["key_created"], serde_json::json!(false));
    assert_eq!(p2["node_id"].as_str().unwrap(), node1);
    // disable：只落闸，requires_restart=true
    let p3 = roaming_set_enabled_impl(ws.to_str().unwrap(), false)
        .unwrap()
        .unwrap();
    assert_eq!(p3["enabled"], serde_json::json!(false));
    assert_eq!(p3["requires_restart"], serde_json::json!(true));
    assert!(!Cluster::roaming_enabled_in_config(&cluster_config_path(
        ws.to_str().unwrap()
    )));
}

#[cfg(not(feature = "cluster-roaming"))]
#[test]
fn test_roaming_status_payload_feature_off_degrades() {
    let dir = tempfile::tempdir().unwrap();
    let payload = roaming_status_payload(dir.path().to_str().unwrap(), None)
        .unwrap()
        .unwrap();
    assert_eq!(payload["available"], serde_json::json!(false));
    assert_eq!(payload["enabled"], serde_json::json!(false));
    assert!(payload["reason"].as_str().unwrap().contains("feature"));
}

// --- QUIC 自建中继管理面（feature=roaming-relay） ---

#[cfg(feature = "cluster-roaming")]
#[test]
fn test_roaming_enabled_write_preserves_relay_siblings() {
    // 回归锁：roaming.enabled 单键写曾整段替换 roaming 对象，把
    // relay_url/relay_token/relay 静默抹掉（2026-10-21 relay WSAPI 面上线前修复）。
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = cluster_config_path(ws);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{
  "enabled": true,
  "token": "cluster-tok",
  "roaming": {
    "enabled": true,
    "relay_url": "http://vps.example.com:3340",
    "relay_token": "client-tok",
    "relay": { "enabled": true, "http_port": 3340, "tls": "manual" }
  }
}"#,
    )
    .unwrap();

    update_cluster_config_roaming(ws, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    // 单键翻转生效
    assert_eq!(v["roaming"]["enabled"], serde_json::json!(false));
    // 段内兄弟键 + 顶层兄弟键全保留
    assert_eq!(
        v["roaming"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(v["roaming"]["relay_token"], serde_json::json!("client-tok"));
    assert_eq!(v["roaming"]["relay"]["tls"], serde_json::json!("manual"));
    assert_eq!(v["roaming"]["relay"]["enabled"], serde_json::json!(true));
    assert_eq!(v["enabled"], serde_json::json!(true));
    assert_eq!(v["token"], serde_json::json!("cluster-tok"));
}

#[cfg(feature = "roaming-relay")]
#[test]
fn test_relay_config_save_merge_and_clear_semantics() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = cluster_config_path(ws);
    std::fs::create_dir_all(cfg_path.parent().unwrap()).unwrap();
    std::fs::write(
        &cfg_path,
        r#"{
  "enabled": true,
  "token": "cluster-tok",
  "roaming": { "enabled": true, "relay_url": "http://vps.example.com:3340" }
}"#,
    )
    .unwrap();

    // 1. 合并写：relay 段缺席 → 创建；roaming/顶层兄弟键保留；token trim
    let p = relay_config_save_impl(
        ws,
        &serde_json::json!({ "http_port": 4000, "tls": "manual", "token": "  s3cret  " }),
    )
    .unwrap()
    .unwrap();
    assert_eq!(p["saved"], serde_json::json!(true));
    assert_eq!(p["requires_restart"], serde_json::json!(true));
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["roaming"]["relay"]["http_port"], serde_json::json!(4000));
    assert_eq!(v["roaming"]["relay"]["tls"], serde_json::json!("manual"));
    assert_eq!(v["roaming"]["relay"]["token"], serde_json::json!("s3cret"));
    assert_eq!(
        v["roaming"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(v["roaming"]["enabled"], serde_json::json!(true));
    assert_eq!(v["token"], serde_json::json!("cluster-tok"));
    // 回显载荷 token 永不出现（token_set bool only）
    let echo = serde_json::to_string(&p).unwrap();
    assert!(!echo.contains("s3cret"));
    assert_eq!(p["relay"]["token_set"], serde_json::json!(true));

    // 2. 空串 = 清除该键（其余键保留）
    relay_config_save_impl(ws, &serde_json::json!({ "token": "" })).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert!(v["roaming"]["relay"].get("token").is_none());
    assert_eq!(v["roaming"]["relay"]["tls"], serde_json::json!("manual"));

    // 3. 非法值 loud 拒（写面是当次意志，不静默纠偏）
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "http_port": 0 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "quic_port": 70000 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "tls": "bogus" })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "unknown_key": 1 })).is_err());
    assert!(relay_config_save_impl(ws, &serde_json::json!({ "token": 42 })).is_err());
    // 拒绝后配置未被半写（仍是步骤 2 形态）
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["roaming"]["relay"]["http_port"], serde_json::json!(4000));
    assert!(v["roaming"]["relay"].get("token").is_none());
}

#[cfg(feature = "roaming-relay")]
#[test]
fn test_relay_status_payload_shape() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let p = relay_status_payload(ws, false, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(p["available"], serde_json::json!(true));
    assert_eq!(p["enabled"], serde_json::json!(false));
    assert_eq!(p["active"], serde_json::json!(false));
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    // 默认档：self-signed + 默认端口 + 证书待生成
    assert_eq!(p["relay"]["http_port"], serde_json::json!(3340));
    assert_eq!(p["relay"]["quic_port"], serde_json::json!(7842));
    assert_eq!(p["relay"]["tls"], serde_json::json!("self-signed"));
    assert_eq!(p["relay"]["token_set"], serde_json::json!(false));
    assert_eq!(p["relay"]["cert_state"], serde_json::json!("pending"));
    assert_eq!(p["client"]["relay_url"], serde_json::json!(""));
    assert_eq!(p["client"]["relay_token_set"], serde_json::json!(false));
    assert!(p["http_addr"].is_null());
    assert!(p["quic_addr"].is_null());

    // config 闸开而未跑 → requires_restart
    relay_config_save_impl(ws, &serde_json::json!({ "enabled": true })).unwrap();
    let p = relay_status_payload(ws, false, None, None)
        .unwrap()
        .unwrap();
    assert_eq!(p["enabled"], serde_json::json!(true));
    assert_eq!(p["requires_restart"], serde_json::json!(true));

    // 运行态跟上 → requires_restart=false + 地址面回显
    let p = relay_status_payload(
        ws,
        true,
        Some("127.0.0.1:3340".parse().unwrap()),
        Some("127.0.0.1:7842".parse().unwrap()),
    )
    .unwrap()
    .unwrap();
    assert_eq!(p["requires_restart"], serde_json::json!(false));
    assert_eq!(p["http_addr"], serde_json::json!("127.0.0.1:3340"));
    assert_eq!(p["quic_addr"], serde_json::json!("127.0.0.1:7842"));
}

#[cfg(feature = "roaming-relay")]
#[test]
fn test_relay_config_save_client_face() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_str().unwrap();
    let cfg_path = cluster_config_path(ws);
    // 无文件起步：客户端两键写进 roaming 段直挂（非 relay 子段）
    let p = relay_config_save_impl(
        ws,
        &serde_json::json!({
            "relay_url": " http://vps.example.com:3340 ",
            "relay_token": " c-tok "
        }),
    )
    .unwrap()
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(
        v["roaming"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(v["roaming"]["relay_token"], serde_json::json!("c-tok"));
    // 回显面：relay_url 原样、relay_token 只有 bool
    assert_eq!(
        p["client"]["relay_url"],
        serde_json::json!("http://vps.example.com:3340")
    );
    assert_eq!(p["client"]["relay_token_set"], serde_json::json!(true));
    assert!(!serde_json::to_string(&p).unwrap().contains("c-tok"));

    // 空串清除：relay_url 键移除（回落 n0 公共中继），relay_token 不动
    relay_config_save_impl(ws, &serde_json::json!({ "relay_url": "" })).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert!(v["roaming"].get("relay_url").is_none());
    assert_eq!(v["roaming"]["relay_token"], serde_json::json!("c-tok"));
    // 服务端子段与客户端键互不侵扰：同一次保存两边都可写
    relay_config_save_impl(
        ws,
        &serde_json::json!({ "http_port": 4000, "relay_url": "http://r2.example.com:3340" }),
    )
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["roaming"]["relay"]["http_port"], serde_json::json!(4000));
    assert_eq!(
        v["roaming"]["relay_url"],
        serde_json::json!("http://r2.example.com:3340")
    );
}
