//! S9 覆盖率批次：checkpoint.rs 剩余未覆盖行。
//! - 200（+198-203）：restore_code 的 Some(body) 写回臂——begin →
//!   snapshot(Modify)（读原内容）→ 改写文件 → restore 写回 + Create 分支
//!   删除新建文件。
//! - 254-255：truncate_from 的持久化目录清理循环收尾（真实 dir 下
//!   turn-N.json 文件的解析/删除路径）。
//! - 262：persist 的 to_vec_pretty 失败分支结构性不可达（纯
//!   String/usize/Option 结构，见报告豁免组）。

use super::*;

fn temp_root(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "nemesis_ckpt_s9_{}_{}_{}",
        tag,
        std::process::id(),
        line!()
    ))
}

/// restore_code：Modify 快照写回原文（198-203），Create 快照删除文件。
#[tokio::test]
async fn restore_code_writes_back_and_deletes_by_snapshot_kind() {
    let root = temp_root("restore");
    let ckpt_dir = root.join(".checkpoints").join("s9.ckpt");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();
    std::fs::write(root.join("a.txt"), "original").unwrap();

    let store = CheckpointStore::new(Some(ckpt_dir), root.clone());
    store.begin(1, "turn one");
    store
        .snapshot(&FileChange {
            path: "a.txt".to_string(),
            kind: FileChangeKind::Modify,
        })
        .await;
    store
        .snapshot(&FileChange {
            path: "made_up.txt".to_string(),
            kind: FileChangeKind::Create,
        })
        .await;

    // 轮内改动：a.txt 被改、made_up.txt 被建
    std::fs::write(root.join("a.txt"), "modified").unwrap();
    std::fs::write(root.join("made_up.txt"), "created during turn").unwrap();

    let (written, deleted) = store.restore_code(1).await;
    assert_eq!(written, vec!["a.txt".to_string()]);
    assert_eq!(deleted, vec!["made_up.txt".to_string()]);
    assert_eq!(std::fs::read_to_string(root.join("a.txt")).unwrap(), "original");
    assert!(!root.join("made_up.txt").exists());
    let _ = std::fs::remove_dir_all(&root);
}

/// truncate_from：持久化目录里 turn-2.json 被删、turn-1.json 保留
/// （240-255 全路径，含解析与 remove_file）。
#[tokio::test]
async fn truncate_from_removes_persisted_turn_files() {
    let root = temp_root("trunc");
    let ckpt_dir = root.join(".checkpoints").join("s9.ckpt");
    let _ = std::fs::remove_dir_all(&root);
    std::fs::create_dir_all(&root).unwrap();

    let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
    // 新语义（2026-08-30 空 turn 不落盘）：只有产生文件快照的 turn 才落盘。
    store.begin(1, "first");
    store
        .snapshot(&FileChange {
            path: "f1.txt".into(),
            kind: FileChangeKind::Modify,
        })
        .await;
    store.begin(2, "second");
    store
        .snapshot(&FileChange {
            path: "f2.txt".into(),
            kind: FileChangeKind::Modify,
        })
        .await;
    assert!(ckpt_dir.join("turn-1.json").exists());
    assert!(ckpt_dir.join("turn-2.json").exists());

    store.truncate_from(2);
    assert!(!ckpt_dir.join("turn-2.json").exists(), "turn-2 removed from disk");
    assert!(ckpt_dir.join("turn-1.json").exists(), "turn-1 kept");
    let metas = store.list_meta();
    assert_eq!(metas.len(), 1, "only turn-1 remains in memory");
    assert_eq!(metas[0].turn, 1);
    let _ = std::fs::remove_dir_all(&root);
}
