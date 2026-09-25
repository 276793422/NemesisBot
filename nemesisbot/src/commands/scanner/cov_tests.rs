//! scanner wave-5 round-2 补测：直调 fn（同 tests.rs 形态，temp 配置路径、
//! 不动 env）。补 cmd_add 新增/合并两臂、cmd_remove 真删臂、cmd_clamav_info
//! 离线打印体。
//!
//! 结构性边界：process::exit(1) 的"引擎不存在/未知引擎"臂不能在测试进程
//! 内直调（exit 杀死整个测试二进制）；install/update/freshclam 走真下载与
//! 真 clamd——均不进单测。get_info 只 ping 127.0.0.1:3310（回环拒绝秒回）。
#![cfg(target_os = "windows")]

use super::*;

fn tmp_cfg(tag: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("nb-scanner-w5-{tag}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir.join("config.scanner.json")
}

fn read_cfg(path: &std::path::Path) -> ScannerFullConfig {
    serde_json::from_str(&std::fs::read_to_string(path).unwrap()).unwrap()
}

#[test]
fn w5_add_new_engine_then_readd_merges_flags() {
    let cfg_path = tmp_cfg("add");

    // 新增臂：默认值 + 三个显式旗标。
    cmd_add(
        &cfg_path,
        "clamav",
        Some("http://127.0.0.1:9/clamav.zip"),
        Some("C:/w5/clamav"),
        Some("127.0.0.1:3310"),
    )
    .expect("新增引擎 → Ok");
    let cfg = read_cfg(&cfg_path);
    let ec = parse_engine_config(cfg.engines.get("clamav").unwrap());
    assert_eq!(ec.url, "http://127.0.0.1:9/clamav.zip");
    assert_eq!(ec.clamav_path, "C:/w5/clamav");
    assert_eq!(ec.address, "127.0.0.1:3310");

    // 已存在 → 合并臂：只改 url，其余字段保留。
    cmd_add(
        &cfg_path,
        "clamav",
        Some("http://127.0.0.1:9/v2"),
        None,
        None,
    )
    .expect("重复 add → 合并臂 → Ok");
    let cfg = read_cfg(&cfg_path);
    let ec = parse_engine_config(cfg.engines.get("clamav").unwrap());
    assert_eq!(ec.url, "http://127.0.0.1:9/v2", "url 必须被覆盖");
    assert_eq!(ec.clamav_path, "C:/w5/clamav", "path 必须保留");
    assert_eq!(ec.address, "127.0.0.1:3310", "address 必须保留");

    let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
}

#[test]
fn w5_remove_existing_engine_prunes_enabled_list() {
    let cfg_path = tmp_cfg("remove");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(ClamAVEngineConfig::default()).unwrap(),
    );
    cfg.enabled.push("clamav".to_string());
    save_scanner_config(&cfg_path, &cfg).unwrap();

    cmd_remove(&cfg_path, "clamav").expect("删除已存在引擎 → Ok");

    let cfg = read_cfg(&cfg_path);
    assert!(!cfg.engines.contains_key("clamav"), "engines 必须移除");
    assert!(
        !cfg.enabled.iter().any(|n| n == "clamav"),
        "enabled 必须同步剪除"
    );
    let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
}

#[tokio::test]
async fn w5_clamav_info_prints_config_offline() {
    let cfg_path = tmp_cfg("info");
    let mut cfg = ScannerFullConfig::default();
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(ClamAVEngineConfig::default()).unwrap(),
    );
    save_scanner_config(&cfg_path, &cfg).unwrap();

    // get_info 只 ping 回环（拒绝秒回 → ready=false），全程离线、不 exit。
    cmd_clamav_info(&cfg_path).await.expect("离线 info → Ok");

    let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
}
