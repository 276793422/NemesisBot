//! W3（2026-10-25 加固）：`cron_parent_session_exists`
//! 单测——磁盘双臂（sessions json ∨ session_logs jsonl）任一在场即存在；
//! 双 miss = 已删除（触发 pause 的判据）；文件名消毒与写入方同源。
//!
//! 独立测试文件（仓库纪律：生产文件只留声明行）。纯磁盘函数——临时 home
//! 下造文件即测，无进程单例纠缠。

use super::cron_parent_session_exists;
use std::path::PathBuf;

/// 临时 home（workspace 骨架自建），返回 (guard, home)。
fn temp_home(tag: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path().join(format!(".nemesisbot_{tag}"));
    std::fs::create_dir_all(home.join("workspace")).unwrap();
    (dir, home)
}

fn safe(key: &str) -> String {
    nemesis_utils::sanitize::sanitize_path_segment(key)
}

#[test]
fn exists_when_sessions_json_present() {
    let (_g, home) = temp_home("json");
    let key = "agent:main:session:abc";
    let json = crate::common::sessions_dir(&home).join(format!("{}.json", safe(key)));
    std::fs::create_dir_all(json.parent().unwrap()).unwrap();
    std::fs::write(&json, "{}").unwrap();
    assert!(cron_parent_session_exists(&home, key));
}

#[test]
fn exists_when_chat_log_jsonl_present() {
    let (_g, home) = temp_home("jsonl");
    let key = "agent:main:session:def";
    let log_dir = nemesis_path::resolve_session_logs_dir_in_workspace(&home.join("workspace"));
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(log_dir.join(format!("{}.jsonl", safe(key))), "").unwrap();
    assert!(cron_parent_session_exists(&home, key));
}

#[test]
fn missing_when_both_files_absent() {
    let (_g, home) = temp_home("gone");
    assert!(!cron_parent_session_exists(
        &home,
        "agent:main:session:deleted"
    ));
}

#[test]
fn sanitization_matches_writers() {
    // 复合键含 `/`：消毒与 chat_log 写入方同源——含 `/` 的键落消毒后的
    // 平面名（不建嵌套目录）；两臂都用同一 safe 名。
    let (_g, home) = temp_home("sanitize");
    let key = "node-x/agent:main:session:p1";
    let json = crate::common::sessions_dir(&home).join(format!("{}.json", safe(key)));
    std::fs::create_dir_all(json.parent().unwrap()).unwrap();
    std::fs::write(&json, "{}").unwrap();
    assert!(cron_parent_session_exists(&home, key), "消毒名命中 json 臂");
    // 不同键消毒名不同 → 不跨键误判存在。
    assert!(!cron_parent_session_exists(
        &home,
        "node-x/agent:main:session:p2"
    ));
}
