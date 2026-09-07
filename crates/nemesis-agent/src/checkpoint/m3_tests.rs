//! M3（devtool-upgrade 阶段 5）：会话级 diff 的 checkpoint 基线原语测试。
//!
//! - `base_for_path`：首个声明过该文件的条目（git 形态=tree 锚；JSON
//!   形态=content 锚；无 `paths` 的老 JSON 条目按 `files` 命中——D2 前
//!   兼容）；未声明过的路径 → None。
//! - `read_file_from_tree`：按影子 tree 读单文件内容（NotFound → None，
//!   不误报错误）。

use super::*;
use crate::r#loop::{FileChange, FileChangeKind};

fn m3_git_root() -> (tempfile::TempDir, std::path::PathBuf) {
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

/// git 形态：base_for_path 取首个声明条目的 turn+tree；read_file_from_
/// tree 读出该时刻内容；不在 tree 的路径 → Ok(None)。
#[tokio::test]
async fn git_mode_base_for_path_and_read_file_from_tree() {
    let (_d, root) = m3_git_root();
    std::fs::write(root.join("code.txt"), "v1").unwrap();

    let store = CheckpointStore::new(None, root.clone());
    store.begin(1, "turn 1");
    store.snapshot(&modify("code.txt")).await;
    std::fs::write(root.join("code.txt"), "v2").unwrap();

    store.begin(2, "turn 2");
    store.snapshot(&modify("code.txt")).await;
    std::fs::write(root.join("code.txt"), "v3").unwrap();

    let base = store.base_for_path("code.txt").expect("已声明的文件有基线");
    assert_eq!(base.turn, 1, "首个声明条目");
    let tree = base.tree.expect("git 形态带 tree");

    let content = store.read_file_from_tree(&tree, "code.txt").unwrap();
    assert_eq!(content.as_deref(), Some("v1"), "基线=pre-edit 内容");

    let missing = store.read_file_from_tree(&tree, "nope.txt").unwrap();
    assert_eq!(missing, None, "tree 里没有 → None（新增文件语义）");

    assert!(store.base_for_path("untouched.txt").is_none());
}

/// JSON 形态：base 带 content 锚（Modify 的 pre-edit 快照）；未声明路径
/// → None。
#[tokio::test]
async fn json_mode_base_for_path_content() {
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    std::fs::write(dir.path().join("a.txt"), "v1").unwrap();
    store.begin(1, "turn 1");
    store.snapshot(&modify("a.txt")).await;

    let base = store
        .base_for_path("a.txt")
        .expect("JSON 形态有 content 基线");
    assert_eq!(base.turn, 1);
    assert_eq!(base.tree, None, "JSON 形态无 tree");
    assert_eq!(
        base.content.as_deref(),
        Some("v1"),
        "Modify 的 pre-edit 快照"
    );

    assert!(store.base_for_path("b.txt").is_none());
}
