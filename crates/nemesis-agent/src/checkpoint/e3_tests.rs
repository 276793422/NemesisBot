//! E3（devtool-upgrade 阶段 5）：消息级回退的 checkpoint 侧测试。
//!
//! - `latest_tree_hex` / `tree_hex_before`：redo 前向恢复目标 + 陈旧性守卫
//!   基线的读法（git 形态下按 turn 序取最后一个带 tree 的；JSON 形态恒
//!   None——无 tree 可言，redo 文件步诚实跳过）。
//! - `restore_to_tree`：绕过 JSON 索引按指定影子 tree 恢复（truncate_from
//!   清掉索引后影子对象仍有效——v1 不 gc 的设计红利）；JSON 形态诚实报错。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

/// 建一个带真 git 仓的临时 workspace（同 d2_tests 夹具语义）。
fn e3_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

fn modify(rel: &str) -> FileChange {
    FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    }
}

/// git 形态：turn 边界 tree 随 begin 落库；latest = 最新 turn 的 tree，
/// tree_hex_before = 上一 turn 的 tree；restore_to_tree(旧 tree) 把文件
/// 打回旧值——redo 反向的文件原语。
#[tokio::test]
async fn git_mode_tree_queries_and_restore_to_tree() {
    let (_d, root) = e3_git_root();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");
    let t1 = store.latest_tree_hex().expect("turn 1 有 tree");

    std::fs::write(root.join("code.txt"), "v2").unwrap();
    store.snapshot(&modify("code.txt")).await;
    store.begin(2, "turn 2");
    let t2 = store.latest_tree_hex().expect("turn 2 有 tree");
    assert_ne!(t1, t2, "两次 begin 工作区不同 → tree 不同");

    // 守卫基线读法：turn < 2 的最新 tree 就是 t1。
    assert_eq!(store.tree_hex_before(2).as_deref(), Some(t1.as_str()));
    // truncate_from(2) 之后索引里剩的最新 tree 也是 t1（tree_hex_before
    // 与 truncate 后的 latest 语义对齐——redo 守卫的成立前提）。
    store.truncate_from(2);
    assert_eq!(store.latest_tree_hex().as_deref(), Some(t1.as_str()));

    // 绕过索引恢复：索引已无 turn 2，影子对象仍在 → 打回 v1。
    let (written, _deleted) = store.restore_to_tree(&t1).unwrap();
    assert!(written.contains(&"code.txt".to_string()), "{written:?}");
    assert_eq!(
        std::fs::read_to_string(root.join("code.txt")).unwrap(),
        "v1"
    );
}

/// JSON 形态：tree 查询恒 None；restore_to_tree 诚实报错（E3 redo 文件步
/// 据此跳过并注记，不静默装成功）。
#[tokio::test]
async fn json_mode_tree_queries_none_and_restore_errors() {
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    store.begin(1, "turn 1");

    assert_eq!(store.latest_tree_hex(), None);
    assert_eq!(store.tree_hex_before(2), None);
    let err = store
        .restore_to_tree("4b825dc642cb6eb9a060e54bf8d69288fbee4904")
        .unwrap_err();
    assert!(err.contains("影子库不可用"), "{err}");
}
