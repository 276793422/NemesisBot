//! fsutil 韧性删除行为用例（真文件系统，不 mock）。

use std::fs;

use super::{
    read_config_string, remove_dir_all_resilient, remove_dir_all_retry, remove_dir_all_retry_async,
    remove_dir_retry, remove_file_resilient, remove_file_retry,
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

// async 包装面（spawn_blocking 转发同步实现）——guest
// delete_dir 等异步调用点的接线用例。
#[tokio::test]
async fn async_retry_removes_nested_tree() {
    let tmp = tempfile::tempdir().unwrap();
    let tree = tmp.path().join("tree").join("inner");
    fs::create_dir_all(&tree).unwrap();
    fs::write(tree.join("leaf.txt"), b"x").unwrap();
    remove_dir_all_retry_async(&tree).await.unwrap();
    assert!(!tree.exists());
}

#[tokio::test]
async fn async_retry_treats_not_found_as_success() {
    let tmp = tempfile::tempdir().unwrap();
    let absent = tmp.path().join("never-existed");
    remove_dir_all_retry_async(&absent).await.unwrap();
}

#[test]
fn file_resilient_removes_and_reports_true() {
    let tmp = tempfile::tempdir().unwrap();
    let f = tmp.path().join("stale.tmp");
    fs::write(&f, b"x").unwrap();
    assert!(remove_file_resilient(&f));
    assert!(!f.exists());
    // 缺失文件同语义：清理类调用点 NotFound 不算失败。
    assert!(remove_file_resilient(&f));
}

// remove_dir_retry 的 rmdir 语义（2026-11-06 cluster_logs 空壳摘除护栏）：
// 非空即败是「判空到删除」窗口竞态的安全护栏——新条目落进来时诚实失败，
// 不能把新条目一起带走（remove_dir_all 没有这层保护）。
#[test]
fn dir_retry_removes_empty_dir_only() {
    let tmp = tempfile::tempdir().unwrap();
    // 空目录：删成功。
    let empty = tmp.path().join("empty-shell");
    fs::create_dir(&empty).unwrap();
    remove_dir_retry(&empty).unwrap();
    assert!(!empty.exists());
    // 非空目录：失败且内容原样（这是与 remove_dir_all 的本质差异）。
    let non_empty = tmp.path().join("non-empty");
    fs::create_dir_all(&non_empty).unwrap();
    fs::write(non_empty.join("live.txt"), b"x").unwrap();
    assert!(remove_dir_retry(&non_empty).is_err());
    assert!(non_empty.join("live.txt").exists(), "内容不得被带走");
    // 目录缺席：NotFound 视为成功（幂等）。
    remove_dir_retry(&empty).unwrap();
}

// 重试耗尽面（真 AV/索引器句柄冲突）无零依赖模拟手段：std 打开的句柄默认
// 共享 DELETE（可删），新版 std Windows 删除实现还会清只读位——「制造稳定
// 删失败」需要 windows-sys 控 share_mode/ACL，不值。耗尽路径是 10 次循环
// 收尾返回最后错误的直白逻辑，语义面由上面 3 个用例覆盖。

// Windows 记事本「带 BOM 的 UTF-8」保存的配置文件——
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

// ---- BUG-20261109-036：strip_bom 字符串级原语（BOM 非 whitespace，trim 不剥） ----

#[test]
fn strip_bom_strips_leading_bom() {
    assert_eq!(super::strip_bom("\u{feff}{\"a\":1}"), "{\"a\":1}");
    assert_eq!(super::strip_bom("\u{feff}---\n"), "---\n");
}

#[test]
fn strip_bom_passthrough_without_bom() {
    assert_eq!(super::strip_bom("{\"a\":1}"), "{\"a\":1}");
    assert_eq!(super::strip_bom(""), "");
    // BOM 只在前缀剥离——中间的 U+FEFF 不是本函数关注面。
    assert_eq!(super::strip_bom("a\u{feff}b"), "a\u{feff}b");
}
