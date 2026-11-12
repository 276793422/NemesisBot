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

// ===========================================================================
// cov-w6：吃剩的离线可达臂——save 的父目录补建、enable 对已带安装态
// 引擎的跳过臂、check 后写盘失败（只读文件 → rename 拒绝）的 warn 臂。
// ===========================================================================

/// save_scanner_config：目标父目录整链不存在 → create_dir_all 补建。
#[test]
fn w6_save_bootstraps_missing_parent_dirs() {
    let dir = std::env::temp_dir().join(format!("nb-scanner-w6-save-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let cfg_path = dir.join("a").join("b").join("config.scanner.json");
    save_scanner_config(&cfg_path, &ScannerFullConfig::default()).expect("补建父目录后落盘");
    assert!(cfg_path.exists());
    let _ = std::fs::remove_dir_all(&dir);
}

/// enable：引擎 state.install_status 已有值 → 465 判空跳过重写，
/// enabled 名单照常追加。
#[test]
fn w6_enable_keeps_existing_install_status() {
    let cfg_path = tmp_cfg("enable-keep");
    let mut ec = ClamAVEngineConfig::default();
    ec.state.install_status = "failed".to_string();
    let mut cfg = ScannerFullConfig::default();
    cfg.engines
        .insert("clamav".to_string(), serde_json::to_value(ec).unwrap());
    save_scanner_config(&cfg_path, &cfg).unwrap();

    cmd_enable(&cfg_path, "clamav").expect("enable → Ok");

    let cfg = read_cfg(&cfg_path);
    assert!(cfg.enabled.iter().any(|n| n == "clamav"));
    let ec = parse_engine_config(cfg.engines.get("clamav").unwrap());
    assert_eq!(ec.state.install_status, "failed", "已有安装态不得被改写");
    let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
}

/// check：引擎状态变化后写盘失败（目标文件只读 → 原子 rename 拒绝）→
/// warn 臂（不阻断命令）。
#[tokio::test]
async fn w6_check_save_failure_warns_without_failing() {
    let cfg_path = tmp_cfg("check-readonly");
    let mut cfg = ScannerFullConfig::default();
    // enabled + 无安装路径 + 系统无 clamav → pending 状态回写 → changed。
    cfg.enabled.push("clamav".to_string());
    cfg.engines.insert(
        "clamav".to_string(),
        serde_json::to_value(ClamAVEngineConfig::default()).unwrap(),
    );
    save_scanner_config(&cfg_path, &cfg).unwrap();

    // 只读属性 → load 可读、原子写 rename 失败。
    let meta = std::fs::metadata(&cfg_path).unwrap();
    let mut perms = meta.permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(&cfg_path, perms).unwrap();

    cmd_check(&cfg_path).expect("check 主流程不因写盘失败而 Err");

    // 还原只读，便于目录清理。
    let meta = std::fs::metadata(&cfg_path).unwrap();
    let mut perms = meta.permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(&cfg_path, perms).unwrap();
    let _ = std::fs::remove_dir_all(cfg_path.parent().unwrap());
}
