//! P4 交付门（delivery_gate.rs）测试。
//!
//! - JSON 形态（无 .git）→ None（诚实降级，不造证据）。
//! - 空 task_id → None。
//! - git 形态 happy path：JSON 形状（branch/task_id/node/patch_path +
//!   capture 元数据）+ patch 落盘留底内容一致。
//! - 坏 base tree hex → None（warn 不 panic，不阻断回调主链路）。

use super::*;
use nemesis_agent::checkpoint::CheckpointStore;
use std::sync::Arc;

/// 带真 git 仓的临时 workspace（同 nemesis-agent p4_tests 夹具语义）。
fn git_root() -> (tempfile::TempDir, std::path::PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().join("ws");
    std::fs::create_dir_all(&root).unwrap();
    git2::Repository::init(&root).unwrap();
    (dir, root)
}

#[test]
fn json_backend_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(CheckpointStore::new(None, dir.path().to_path_buf()));
    assert_eq!(
        store.backend(),
        nemesis_agent::checkpoint::CheckpointBackend::Json
    );
    let out = capture_task_delivery_with_store(&store, "json-task", None, "node-b");
    assert!(out.is_none(), "JSON 形态无影子库 → None（诚实降级）");
}

#[test]
fn empty_task_id_returns_none() {
    let dir = tempfile::tempdir().unwrap();
    let store = Arc::new(CheckpointStore::new(None, dir.path().to_path_buf()));
    assert!(capture_task_delivery_with_store(&store, "", None, "node-b").is_none());
}

#[test]
fn happy_path_json_shape_and_patch_file() {
    let (_d, root) = git_root();
    std::fs::write(root.join("a.txt"), "v1\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root.clone()));
    store.begin(1, "任务开始");
    let base = store.current_tree_hex().expect("git 形态有实时 tree");

    // 任务执行：改旧文件 + 增新文件。
    std::fs::write(root.join("a.txt"), "v2\nmore\n").unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    std::fs::write(root.join("src").join("new.rs"), "fn main() {}\n").unwrap();

    let out = capture_task_delivery_with_store(&store, "t-happy", Some(&base), "node-b")
        .expect("git 形态成功捕获");
    let obj = out.as_object().expect("delivery 是 JSON object");

    // 上下文字段。
    assert_eq!(obj["branch"], "run-t-happy");
    assert_eq!(obj["task_id"], "t-happy");
    assert_eq!(obj["node"], "node-b");
    assert_eq!(obj["patch_path"], "logs/deliveries/t-happy.patch");

    // capture 元数据（DeliveryBranchCapture 序列化字段）。
    assert_eq!(obj["ref_name"], "refs/heads/nemesis/run-t-happy");
    assert_eq!(obj["base_tree"], base.as_str());
    assert_eq!(obj["files_changed"], 2, "1 改 1 增");
    assert_eq!(obj["patch_truncated"], false);
    let patch = obj["patch"].as_str().expect("patch 是字符串");
    assert!(patch.contains("a.txt"));
    assert!(patch.contains("src/new.rs"));

    // patch 落盘留底与 wire JSON 一致（正斜杠相对路径）。
    let on_disk =
        std::fs::read_to_string(root.join("logs").join("deliveries").join("t-happy.patch"))
            .expect("patch 已落盘");
    assert_eq!(on_disk, patch, "盘上留底 = wire 证据");

    // commit 可在影子库解到同一 tree。
    let shadow = nemesis_path::logs_dir_in_workspace(&root).join("checkpoints.git");
    let repo = git2::Repository::open_ext(
        &shadow,
        git2::RepositoryOpenFlags::NO_SEARCH,
        std::path::Path::new("/"),
    )
    .unwrap();
    let commit = repo
        .find_commit(git2::Oid::from_str(obj["commit"].as_str().unwrap()).unwrap())
        .unwrap();
    assert_eq!(commit.tree_id().to_string(), obj["tree"]);
}

#[test]
fn bad_base_tree_hex_is_none_not_panic() {
    let (_d, root) = git_root();
    std::fs::write(root.join("f.txt"), "x\n").unwrap();
    let store = Arc::new(CheckpointStore::new(None, root));
    // 坏 hex（非 40 位）：capture 层响亮失败 → 交付门兜成 None + warn。
    let out = capture_task_delivery_with_store(&store, "t-badbase", Some("deadbeef"), "node-b");
    assert!(out.is_none(), "坏基线 = None（不造证据不炸链路）");
}
