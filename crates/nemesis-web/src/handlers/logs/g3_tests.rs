//! G3（spill 状态卡，2026-08-28）：logs.spill_status / logs.spill_cleanup。
//!
//! 夹具：tempdir 下手工构造 spill 树（set_modified 制造过期/新鲜文件，
//! 与 nemesis-agent spill/tests.rs 同法）。签名收 spill 根路径后无需 ctx。

use super::*;

/// 造一个 spill 文件并把 mtime 拨回 `age_secs` 前（std File::set_modified）。
fn make_spill_file(
    root: &std::path::Path,
    session: &str,
    name: &str,
    age_secs: u64,
    content: &str,
) {
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(past).unwrap();
}

#[test]
fn spill_status_reports_tree_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    // 2026-08-31 spill 迁回 workspace：根 = workspace/logs/spill（U4 设计位）
    let root = dir.path().join("workspace").join("logs").join("spill");
    make_spill_file(&root, "s1", "newer.txt", 24 * 3600, "0123456789"); // 10 B
    make_spill_file(&root, "s2", "older.txt", 24 * 3600, "ab"); // 2 B

    let out = LogsHandler.spill_status(&root).unwrap().unwrap();
    assert_eq!(out["files"], 2, "out = {}", out);
    assert_eq!(out["bytes"], 12);
    assert!(out["oldest"].is_string());
    // 测试进程未装 live ConfigStore → 回退默认保留期 7 天。
    assert_eq!(out["retention_days"], 7);
    assert_eq!(out["threshold_chars"], 65_536);
    assert!(out["root"].as_str().unwrap().ends_with("spill"));
}

#[test]
fn spill_cleanup_deletes_expired_and_returns_fresh_status() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace").join("logs").join("spill");
    make_spill_file(&root, "s1", "old.txt", 10 * 24 * 3600, "old!"); // 4 B, 过期
    make_spill_file(&root, "s1", "fresh.txt", 24 * 3600, "ok"); // 2 B, 保留

    let out = LogsHandler.spill_cleanup(&root).unwrap().unwrap();
    assert_eq!(out["deleted"], 1, "out = {}", out);
    assert_eq!(out["retention_days"], 7);
    assert_eq!(out["files"], 1, "fresh file remains");
    assert_eq!(out["bytes"], 2);
    assert!(!root.join("s1").join("old.txt").exists());
    assert!(root.join("s1").join("fresh.txt").exists());
}

#[test]
fn spill_status_missing_tree_reports_zero() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("workspace").join("logs").join("spill");

    let out = LogsHandler.spill_status(&root).unwrap().unwrap();
    assert_eq!(out["files"], 0);
    assert_eq!(out["bytes"], 0);
    assert!(out["oldest"].is_null());
}
