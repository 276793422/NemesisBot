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
