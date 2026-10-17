//! fsutil 韧性删除行为用例（真文件系统，不 mock）。

use std::fs;

use super::{remove_dir_all_resilient, remove_dir_all_retry};

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
