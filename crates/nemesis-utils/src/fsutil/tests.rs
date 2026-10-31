//! fsutil 韧性删除行为用例（真文件系统，不 mock）。

use std::fs;

use super::{
    read_config_string, remove_dir_all_resilient, remove_dir_all_retry, remove_file_retry,
};

#[test]
fn retry_removes_nested_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("tree").join("inner");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("leaf.txt"), b"x").unwrap();
    remove_dir_all_retry(&tree).unwrap();
    assert!(!tree.exists());
}

#[test]
fn retry_treats_not_found_as_success() {
    let tmp = tempfile::tempdir().unwrap();
    let absent = tmp.path().join("never-existed");
    remove_dir_all_retry(&absent).unwrap();
    remove_dir_all_resilient(&absent);
    let absent_file = tmp.path().join("never-existed.txt");
    remove_file_retry(&absent_file).unwrap();
}

#[test]
fn file_retry_removes_single_file() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("stale.tmp");
    fs::write(&f, b"x").unwrap();
    remove_file_retry(&f).unwrap();
    assert!(!f.exists());
}

#[test]
fn resilient_removes_tree_and_reports_true() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().join("staging");
    fs::create_dir_all(dir.join("a").join("b")).unwrap();
    fs::write(dir.join("a").join("b").join("f"), b"x").unwrap();
    assert!(remove_dir_all_resilient(&dir));
}

// 重试耗尽面（真 AV/索引器句柄冲突）无零依赖模拟手段：std 打开的句柄默认
// 共享 DELETE（可删），新版 std Windows 删除实现还会清只读位——「制造稳定
// 删失败」需要 windows-sys 控 share_mode/ACL，不值。耗尽路径是 10 次循环
// 收尾返回最后错误的直白逻辑，语义面由上面 3 个用例覆盖。

// BUG-20261031-260：Windows 记事本「带 BOM 的 UTF-8」保存的配置文件——
// serde_json/toml 都拒 BOM（expected value at line 1 column 1），peers.toml
// 更会走「解析失败→全新序列化覆盖写」冲掉用户手工 peers 表。read_config_string
// 剥前导 BOM；非 BOM 文件字节不变。
#[test]
fn read_config_string_strips_leading_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("bom.json");
    fs::write(&f, "\u{feff}{\"a\":1}").unwrap();
    let s = read_config_string(&f).unwrap();
    assert_eq!(s, "{\"a\":1}");
    // 剥完必须可解析（serde_json 对未剥版本报错，此处顺带钉住根因）。
    let raw = fs::read_to_string(&f).unwrap();
    assert!(serde_json::from_str::<serde_json::Value>(&raw).is_err());
    assert!(serde_json::from_str::<serde_json::Value>(&s).is_ok());
}

#[test]
fn read_config_string_passthrough_without_bom() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("plain.json");
    fs::write(&f, "{\"a\":1}").unwrap();
    assert_eq!(read_config_string(&f).unwrap(), "{\"a\":1}");
}

#[test]
fn read_config_string_errors_on_missing_file() {
    let tmp = tempfile::tempdir().unwrap();
    let err = read_config_string(tmp.path().join("nope.json")).unwrap_err();
    assert_eq!(err.kind(), std::io::ErrorKind::NotFound);
}
