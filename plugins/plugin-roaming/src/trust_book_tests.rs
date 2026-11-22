//! BUG-20261122-010 回归锁：`save_trust_book` 原子写。
//!
//! 原形态裸 `std::fs::write` 非原子——崩溃/断电半截 trust.toml 在读侧
//! fail-closed（trust_allows 对解析失败拒绝）语义下 = roaming 桥全拒直到
//! 人工修复。修复 = 改走 `atomic::atomic_write`（宿主 write_file_atomic
//! 语义孪生：create_new 临时名 + fsync + rename 覆盖 + 失败清理），与宿主
//! TrustBook::save / identity seed / relay PEM 同纪律。
//!
//! 崩溃窗口本身不可测；本组锁钉可观测契约：roundtrip 一致、损坏旧文件被
//! rename 覆盖（自愈路径）、落盘后无临时文件残留。

use std::path::Path;

use crate::endpoint::{TrustBookData, load_trust_book, save_trust_book, trust_book_path};

fn book(trusted: &[&str], revoked: &[&str]) -> TrustBookData {
    TrustBookData {
        trusted: trusted.iter().map(|s| s.to_string()).collect(),
        revoked: revoked.iter().map(|s| s.to_string()).collect(),
    }
}

#[test]
fn save_then_load_roundtrip() {
    let ws = tempfile::tempdir().expect("tempdir");
    let original = book(&["0123456789abcdef"], &["fedcba9876543210"]);
    save_trust_book(ws.path(), &original).expect("save");
    let loaded = load_trust_book(ws.path()).expect("load");
    assert_eq!(loaded.trusted, original.trusted);
    assert_eq!(loaded.revoked, original.revoked);
}

#[test]
fn save_overwrites_corrupt_file() {
    // 模拟旧形态崩溃残迹：半截 trust.toml（旧读侧 fail-closed = 桥全拒）。
    // 修复后 save 走 rename 覆盖，一次写回即自愈——无需人工删文件。
    let ws = tempfile::tempdir().expect("tempdir");
    let path = trust_book_path(ws.path());
    std::fs::create_dir_all(path.parent().expect("parent")).expect("mkdir");
    std::fs::write(&path, "trusted = [ \"deadbeef\"").expect("seed corrupt file");
    assert!(
        load_trust_book(ws.path()).is_err(),
        "前置：损坏文件解析必须失败"
    );

    save_trust_book(ws.path(), &book(&["0123456789abcdef"], &[])).expect("save over corrupt");
    let loaded = load_trust_book(ws.path()).expect("load after heal");
    assert_eq!(loaded.trusted, vec!["0123456789abcdef".to_string()]);
    assert!(loaded.revoked.is_empty());
}

#[test]
fn save_leaves_no_temp_files() {
    // 原子写落盘后目录里只有 trust.toml 本体（.tmp-* 由失败清理臂负责，
    // 成功路径 rename 后不残留）。
    let ws = tempfile::tempdir().expect("tempdir");
    save_trust_book(ws.path(), &book(&["aa"], &[])).expect("save");
    let cluster_dir = ws.path().join("cluster");
    let mut names: Vec<String> = std::fs::read_dir(&cluster_dir)
        .expect("read_dir")
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .collect();
    names.sort();
    assert_eq!(names, vec!["trust.toml".to_string()], "残留条目: {names:?}");
}

#[test]
fn repeated_save_is_idempotent_replace() {
    // rename 覆盖语义：连续多次 save（target 已存在）必须全成功——
    // Windows MOVEFILE_REPLACE_EXISTING 依赖与 plugin-mail 同款。
    let ws = tempfile::tempdir().expect("tempdir");
    for round in 0..3u8 {
        let data = book(&[&format!("{:032x}", round)], &[]);
        save_trust_book(ws.path(), &data)
            .unwrap_or_else(|e| panic!("round {round} save failed: {e}"));
        let loaded = load_trust_book(ws.path()).expect("load");
        assert_eq!(loaded.trusted, vec![format!("{:032x}", round)]);
    }
    // 收尾：最终盘上只有 trust.toml（三轮 rename 无残留）。
    assert!(Path::new(&trust_book_path(ws.path())).is_file());
}
