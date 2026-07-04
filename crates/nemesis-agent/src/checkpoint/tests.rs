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
