use super::*;

async fn snapshot_modify(store: &CheckpointStore, rel: &str, body: &str) {
    tokio::fs::write(store.root.join(rel), body).await.unwrap();
    let change = FileChange {
        path: rel.to_string(),
        kind: FileChangeKind::Modify,
    };
    store.snapshot(&change).await;
}

#[tokio::test]
async fn restore_reverts_modified_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(0, "edit the file");
    snapshot_modify(&store, "a.txt", "original").await;
    // Simulate the tool modifying it.
    tokio::fs::write(root.join("a.txt"), "CHANGED")
        .await
        .unwrap();

    let (written, deleted) = store.restore_code(0).await;
    assert_eq!(written, vec!["a.txt".to_string()]);
    assert!(deleted.is_empty());
    let restored = tokio::fs::read_to_string(root.join("a.txt")).await.unwrap();
    assert_eq!(restored, "original");
}

#[tokio::test]
async fn restore_deletes_created_file() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(0, "create the file");
    // Create kind → snapshot records None (file did not exist).
    let change = FileChange {
        path: "new.txt".to_string(),
        kind: FileChangeKind::Create,
    };
    store.snapshot(&change).await;
    // Simulate the tool creating it.
    tokio::fs::write(root.join("new.txt"), "fresh")
        .await
        .unwrap();
    assert!(root.join("new.txt").exists());

    let (_, deleted) = store.restore_code(0).await;
    assert_eq!(deleted, vec!["new.txt".to_string()]);
    assert!(!root.join("new.txt").exists());
}

#[tokio::test]
async fn per_turn_dedup_keeps_turn_start_content() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(0, "two edits same file");
    snapshot_modify(&store, "f.txt", "v1").await;
    // Second touch same turn — should NOT overwrite the v1 snapshot.
    let change2 = FileChange {
        path: "f.txt".to_string(),
        kind: FileChangeKind::Modify,
    };
    store.snapshot(&change2).await;
    tokio::fs::write(root.join("f.txt"), "v2-changed")
        .await
        .unwrap();

    store.restore_code(0).await;
    let restored = tokio::fs::read_to_string(root.join("f.txt")).await.unwrap();
    assert_eq!(restored, "v1", "dedup should keep turn-start content");
}

#[tokio::test]
async fn persistence_reloads_across_instances() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    let root = dir.path().to_path_buf();

    {
        let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
        store.begin(0, "persisted turn");
        snapshot_modify(&store, "p.txt", "orig").await;
    }
    // New instance reloads the persisted checkpoint.
    let store2 = CheckpointStore::new(Some(ckpt_dir), root.clone());
    tokio::fs::write(root.join("p.txt"), "modified")
        .await
        .unwrap();
    let (written, _) = store2.restore_code(0).await;
    assert_eq!(written, vec!["p.txt".to_string()]);
    let restored = tokio::fs::read_to_string(root.join("p.txt")).await.unwrap();
    assert_eq!(restored, "orig");
}

// ----- boundary /异常 tests -----

#[tokio::test]
async fn snapshot_before_begin_is_noop() {
    // Boundary: snapshot without begin (no active turn) must not panic.
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    let change = FileChange {
        path: "x.txt".into(),
        kind: FileChangeKind::Modify,
    };
    store.snapshot(&change).await; // must not panic
    assert!(store.list_meta().is_empty());
}

#[tokio::test]
async fn restore_nonexistent_turn_returns_empty() {
    // Boundary: rewinding a turn with no checkpoints must return empty, not panic.
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    let (w, d) = store.restore_code(99).await;
    assert!(w.is_empty() && d.is_empty());
}

#[tokio::test]
async fn path_escape_is_rejected_on_restore() {
    // Boundary: a snapshot path containing ".." must never be written/deleted
    // outside the workspace root on restore (safe_path returns None → skipped).
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());
    store.begin(0, "evil");
    let change = FileChange {
        path: "../outside.txt".into(),
        kind: FileChangeKind::Delete,
    };
    store.snapshot(&change).await;
    let (w, d) = store.restore_code(0).await;
    assert!(w.is_empty(), "escape path must not be written: {:?}", w);
    assert!(d.is_empty(), "escape path must not be deleted: {:?}", d);
    // And nothing was created outside the workspace.
    assert!(!dir.path().parent().unwrap().join("outside.txt").exists());
}

#[tokio::test]
async fn corrupted_persisted_checkpoint_is_skipped() {
    // Boundary: a malformed turn-N.json must not break loading of the others.
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    std::fs::create_dir_all(&ckpt_dir).unwrap();
    std::fs::write(ckpt_dir.join("turn-0.json"), b"NOT VALID JSON {{{").unwrap();
    let good = serde_json::to_vec(&Checkpoint {
        turn: 1,
        time: "t".into(),
        prompt: "good".into(),
        files: vec![],
    })
    .unwrap();
    std::fs::write(ckpt_dir.join("turn-1.json"), good).unwrap();

    let store = CheckpointStore::new(Some(ckpt_dir), dir.path().to_path_buf());
    let metas = store.list_meta();
    assert_eq!(
        metas.len(),
        1,
        "corrupted turn-0 must be skipped, turn-1 kept"
    );
    assert_eq!(metas[0].turn, 1);
}

#[tokio::test]
async fn empty_path_snapshot_is_ignored() {
    // Boundary: empty path must be ignored (no panic, no snapshot).
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    store.begin(0, "empty");
    let change = FileChange {
        path: String::new(),
        kind: FileChangeKind::Modify,
    };
    store.snapshot(&change).await;
    let meta = store.list_meta();
    assert_eq!(meta.len(), 1);
    assert!(
        meta[0].paths.is_empty(),
        "empty path must not be snapshotted"
    );
}

// ----- W3a branch coverage -----

/// load(): non-`.json` entries are skipped (line 90); a `.json`-named
/// DIRECTORY makes std::fs::read fail → skipped (line 93); both must not break
/// loading of the valid file.
#[tokio::test]
async fn load_skips_non_json_and_unreadable_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    std::fs::create_dir_all(&ckpt_dir).unwrap();
    std::fs::write(ckpt_dir.join("notes.txt"), b"not a checkpoint").unwrap();
    // A DIRECTORY named turn-2.json: extension matches, read fails.
    std::fs::create_dir_all(ckpt_dir.join("turn-2.json")).unwrap();
    let good = serde_json::to_vec(&Checkpoint {
        turn: 0,
        time: "t".into(),
        prompt: "good".into(),
        files: vec![],
    })
    .unwrap();
    std::fs::write(ckpt_dir.join("turn-0.json"), good).unwrap();

    let store = CheckpointStore::new(Some(ckpt_dir), dir.path().to_path_buf());
    let metas = store.list_meta();
    assert_eq!(metas.len(), 1, "only the valid turn-0 loads");
    assert_eq!(metas[0].turn, 0);
}

/// begin() twice finalizes the previous checkpoint into `done` (line 114):
/// two turns are visible in list_meta.
#[tokio::test]
async fn begin_twice_finalizes_previous_checkpoint() {
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(None, dir.path().to_path_buf());
    store.begin(0, "first");
    snapshot_modify(&store, "a.txt", "v0").await;
    store.begin(1, "second");

    let metas = store.list_meta();
    assert_eq!(metas.len(), 2, "first turn finalized into done");
    assert_eq!(metas[0].prompt, "first");
    assert_eq!(metas[1].prompt, "second");
}

/// restore_code(from_turn): checkpoints before from_turn are skipped
/// (line 173); a path already collected from an earlier checkpoint is not
/// overwritten by a later one (line 177 — earliest wins).
#[tokio::test]
async fn restore_respects_from_turn_and_earliest_wins() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(0, "old turn");
    snapshot_modify(&store, "shared.txt", "turn0-content").await;
    snapshot_modify(&store, "old-only.txt", "old").await;

    store.begin(1, "new turn");
    snapshot_modify(&store, "shared.txt", "turn1-content").await;
    snapshot_modify(&store, "new-only.txt", "new").await;

    tokio::fs::write(root.join("shared.txt"), "clobbered")
        .await
        .unwrap();

    let (written, _) = store.restore_code(1).await;
    // old-only belongs to turn 0 (< from_turn) → skipped.
    assert!(!written.contains(&"old-only.txt".to_string()));
    assert!(written.contains(&"new-only.txt".to_string()));
    // shared.txt appears in both turns; the EARLIEST (turn 1's first touch
    // within scope) is restored — turn-0 content must NOT come back here.
    let shared = tokio::fs::read_to_string(root.join("shared.txt"))
        .await
        .unwrap();
    assert_eq!(shared, "turn1-content", "earliest in-scope snapshot wins");

    // Restoring from turn 0 must pick turn-0 content for shared.txt.
    let (written0, _) = store.restore_code(0).await;
    assert!(written0.contains(&"old-only.txt".to_string()));
    let shared0 = tokio::fs::read_to_string(root.join("shared.txt"))
        .await
        .unwrap();
    assert_eq!(
        shared0, "turn0-content",
        "from_turn=0 restores turn-0 content"
    );
}

/// restore() recreates parent dirs when a snapshot path is nested and its
/// directory was removed (lines 198-200).
#[tokio::test]
async fn restore_recreates_parent_directories() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().to_path_buf();
    let store = CheckpointStore::new(None, root.clone());

    store.begin(0, "nested");
    // The snapshot helper writes via tokio::fs::write, which needs the parent
    // to exist — create it explicitly (restore() is the code under test for
    // parent recreation, not snapshot()).
    tokio::fs::create_dir_all(root.join("sub").join("dir"))
        .await
        .unwrap();
    snapshot_modify(&store, "sub/dir/f.txt", "nested-original").await;
    // Simulate the tool deleting the whole subtree.
    let _ = tokio::fs::remove_dir_all(root.join("sub")).await;

    let (written, _) = store.restore_code(0).await;
    assert_eq!(written, vec!["sub/dir/f.txt".to_string()]);
    let body = tokio::fs::read_to_string(root.join("sub").join("dir").join("f.txt"))
        .await
        .unwrap();
    assert_eq!(body, "nested-original");
}

/// truncate_from(): in-memory done/cur are trimmed; persisted turn-N.json
/// files >= from_turn are deleted from disk; unrelated names survive.
#[tokio::test]
async fn truncate_from_trims_memory_and_disk() {
    let dir = tempfile::tempdir().unwrap();
    let ckpt_dir = dir.path().join(".ck");
    let root = dir.path().to_path_buf();

    let store = CheckpointStore::new(Some(ckpt_dir.clone()), root.clone());
    store.begin(0, "keep");
    snapshot_modify(&store, "k.txt", "k").await;
    store.begin(1, "drop-cur");
    snapshot_modify(&store, "d1.txt", "d1").await;
    store.begin(2, "drop-done");
    snapshot_modify(&store, "d2.txt", "d2").await;
    // begin(2) finalized turn 1 into done; cur is turn 2.
    // Stray non-checkpoint file under the dir must survive the sweep.
    std::fs::write(ckpt_dir.join("turn-notanumber.json"), b"{}").unwrap();
    std::fs::write(ckpt_dir.join("other.json"), b"{}").unwrap();

    assert!(ckpt_dir.join("turn-1.json").exists());
    assert!(ckpt_dir.join("turn-2.json").exists());

    store.truncate_from(1);

    // Memory: only turn 0 remains.
    let metas = store.list_meta();
    assert_eq!(metas.len(), 1);
    assert_eq!(metas[0].turn, 0);

    // Disk: turn-1/turn-2 gone, turn-0 and strays kept.
    assert!(ckpt_dir.join("turn-0.json").exists());
    assert!(!ckpt_dir.join("turn-1.json").exists());
    assert!(!ckpt_dir.join("turn-2.json").exists());
    assert!(ckpt_dir.join("turn-notanumber.json").exists());
    assert!(ckpt_dir.join("other.json").exists());
}

/// persist() with an unusable dir (parent is a FILE): silent no-op, no panic;
/// in-memory state still works.
#[tokio::test]
async fn persist_with_broken_dir_is_silent_noop() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"file").unwrap();
    let bad_dir = blocker.join(".ck"); // create_dir_all will fail

    let store = CheckpointStore::new(Some(bad_dir), dir.path().to_path_buf());
    store.begin(0, "unpersistable"); // must not panic
    snapshot_modify(&store, "x.txt", "v").await;
    let metas = store.list_meta();
    assert_eq!(metas.len(), 1, "in-memory checkpoint still exists");
}

// ---------------------------------------------------------------------------
// 2026-08-30：空 turn 不落盘（begin 只开内存 turn，首个文件快照才持久化）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn empty_turn_does_not_create_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = CheckpointStore::new(
        Some(dir.path().join("cp").to_path_buf()),
        dir.path().to_path_buf(),
    );

    store.begin(0, "empty turn");
    // 无任何 snapshot → 不落盘。
    let cp_file = dir.path().join("cp").join("turn-0.json");
    assert!(
        !cp_file.exists(),
        "empty turn must not be persisted: {cp_file:?}"
    );

    // 有文件快照的 turn → 落盘。
    store
        .snapshot(&FileChange {
            path: "f.txt".into(),
            kind: FileChangeKind::Modify,
        })
        .await;
    assert!(
        cp_file.exists(),
        "turn with file changes is persisted: {cp_file:?}"
    );
}
