// config_loader.rs 覆盖率补充测试（load_app_config 解析失败/读取失败
// WARN 宽容臂 + save 双写路径的父目录创建）。
//
// 豁免：35/188（`if let Some(parent)` 块收尾 `}` 区域伪行——成功臂随
// roundtrip 执行，失败语义由 create_dir_all 的 `?` 直接上抛，无独立臂）。

use super::*;
use std::path::PathBuf;

fn temp_ws(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!(
        "nemesis-cluster-clcov-{}-{name}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// 解析失败（坏 JSON）→ WARN + 默认配置（lenient，不阻断装配）。
#[test]
fn load_app_config_parse_failure_warns_and_defaults() {
    let ws = temp_ws("parse-fail");
    let path = nemesis_path::resolve_cluster_config_path_in_workspace(&ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{not valid json").unwrap();

    let cfg = load_app_config(&ws);
    assert!(!cfg.enabled, "默认态 cluster 关");
    assert_eq!(cfg.rpc_port, 21949);
}

/// 读取失败（配置位是目录）→ WARN + 默认配置。
#[test]
fn load_app_config_unreadable_warns_and_defaults() {
    let ws = temp_ws("unreadable");
    let path = nemesis_path::resolve_cluster_config_path_in_workspace(&ws);
    std::fs::create_dir_all(&path).unwrap(); // 目录占住配置位

    let cfg = load_app_config(&ws);
    assert!(!cfg.enabled);
}

/// save/load round-trip：typed 全量覆盖写 + 回读逐字段一致（含 token /
/// node_name 防删键字段）。
#[test]
fn save_app_config_roundtrip_preserves_fields() {
    let ws = temp_ws("roundtrip");
    let mut cfg = AppConfig::default();
    cfg.enabled = true;
    cfg.port = 11999;
    cfg.rpc_port = 21999;
    cfg.broadcast_interval = 45;
    cfg.llm_timeout_secs = 3600;
    cfg.token = "cov-secret-token".into();
    cfg.node_name = "CovNode".into();

    save_app_config(&ws, &cfg).expect("save ok");
    let loaded = load_app_config(&ws);
    assert!(loaded.enabled);
    assert_eq!(loaded.port, 11999);
    assert_eq!(loaded.rpc_port, 21999);
    assert_eq!(loaded.broadcast_interval, 45);
    assert_eq!(loaded.llm_timeout_secs, 3600);
    assert_eq!(loaded.token, "cov-secret-token");
    assert_eq!(loaded.node_name, "CovNode");
}
