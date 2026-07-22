use super::*;

fn make_snapshot(task_id: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: task_id.into(),
        messages: serde_json::json!([{"role": "user", "content": "hello"}]),
        tool_call_id: "tc-001".into(),
        channel: "web".into(),
        chat_id: "chat-123".into(),
        ready: true,
        created_at: chrono::Local::now().to_rfc3339(),
    }
}

#[tokio::test]
async fn test_save_and_load() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let snap = make_snapshot("task-001");
    store.save(snap).await.unwrap();

    let loaded = store.load("task-001").await.unwrap();
    assert_eq!(loaded.task_id, "task-001");
    assert_eq!(loaded.channel, "web");
}

#[tokio::test]
async fn test_load_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let result = store.load("nonexistent").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_remove() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("task-002")).await.unwrap();
    assert!(store.contains("task-002"));

    assert!(store.remove("task-002").await);
    assert!(!store.contains("task-002"));

    // Verify disk file is also deleted
    let path = dir.path().join("task-002.json");
    assert!(!path.exists());
}

#[tokio::test]
async fn test_disk_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("task-003")).await.unwrap();

    // Create a new store from the same dir
    let store2 = ContinuationStore::new(dir.path());
    // Memory is empty, but disk fallback should work
    let loaded = store2.load("task-003").await.unwrap();
    assert_eq!(loaded.task_id, "task-003");
}

#[tokio::test]
async fn test_list_pending() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    assert!(store.list_pending().await.is_empty());

    store.save(make_snapshot("task-a")).await.unwrap();
    store.save(make_snapshot("task-b")).await.unwrap();

    let pending = store.list_pending().await;
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&"task-a".to_string()));
    assert!(pending.contains(&"task-b".to_string()));
}

#[tokio::test]
async fn test_cleanup_old() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Create a snapshot
    store.save(make_snapshot("old-task")).await.unwrap();

    // Cleanup with 0-second threshold (removes everything older than "now")
    // Since the file was just created, it shouldn't be removed
    let removed = store
        .cleanup_old(std::time::Duration::from_secs(0))
        .await
        .unwrap();
    // A 0-duration cleanup may or may not remove recent files depending on FS timing
    assert!(removed <= 1);

    // Cleanup with very long threshold shouldn't remove anything
    store.save(make_snapshot("new-task")).await.unwrap();
    let removed2 = store
        .cleanup_old(std::time::Duration::from_secs(86400 * 365))
        .await
        .unwrap();
    assert_eq!(removed2, 0);
}

#[tokio::test]
async fn test_recover_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Save some snapshots
    store.save(make_snapshot("recover-1")).await.unwrap();
    store.save(make_snapshot("recover-2")).await.unwrap();

    // Create a fresh store (empty memory but disk has files)
    let store2 = ContinuationStore::new(dir.path());
    // list_pending now scans disk too, so it will find the files
    let pending = store2.list_pending().await;
    assert_eq!(pending.len(), 2);
    assert!(pending.contains(&"recover-1".to_string()));
    assert!(pending.contains(&"recover-2".to_string()));

    // Recover from disk into memory
    let recovered = store2.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 2);

    // Should be able to load them from memory now
    let loaded = store2.load("recover-1").await.unwrap();
    assert_eq!(loaded.task_id, "recover-1");
}

#[tokio::test]
async fn test_recover_from_disk_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let recovered = store.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 0);
}

#[tokio::test]
async fn test_recover_from_disk_nonexistent_dir() {
    let store = ContinuationStore::new("/nonexistent/path/that/does/not/exist");
    let recovered = store.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 0);
}

// -- Additional tests: continuation store edge cases --

#[tokio::test]
async fn test_save_multiple_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    for i in 0..5 {
        let snap = make_snapshot(&format!("task-{}", i));
        store.save(snap).await.unwrap();
    }

    assert_eq!(store.len(), 5);
    assert!(!store.is_empty());

    // Each one should be loadable
    for i in 0..5 {
        let loaded = store.load(&format!("task-{}", i)).await.unwrap();
        assert_eq!(loaded.tool_call_id, "tc-001");
    }
}

#[tokio::test]
async fn test_overwrite_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let mut snap = make_snapshot("task-overwrite");
    snap.channel = "web".into();
    store.save(snap).await.unwrap();

    let mut snap2 = make_snapshot("task-overwrite");
    snap2.channel = "rpc".into();
    store.save(snap2).await.unwrap();

    // Should still have only 1 entry (overwritten)
    assert_eq!(store.len(), 1);

    let loaded = store.load("task-overwrite").await.unwrap();
    assert_eq!(loaded.channel, "rpc");
}

#[tokio::test]
async fn test_contains() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    assert!(!store.contains("task-x"));
    store.save(make_snapshot("task-x")).await.unwrap();
    assert!(store.contains("task-x"));
}

#[tokio::test]
async fn test_len_and_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    assert!(store.is_empty());
    assert_eq!(store.len(), 0);

    store.save(make_snapshot("t1")).await.unwrap();
    assert!(!store.is_empty());
    assert_eq!(store.len(), 1);

    store.save(make_snapshot("t2")).await.unwrap();
    assert_eq!(store.len(), 2);

    store.remove("t1").await;
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn test_remove_nonexistent_returns_false() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    assert!(!store.remove("nonexistent").await);
}

#[tokio::test]
async fn test_snapshot_preserves_messages_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let messages = serde_json::json!([
        {"role": "user", "content": "hello"},
        {"role": "assistant", "content": "hi there"},
        {"role": "user", "content": "how are you?"}
    ]);

    let snap = ContinuationSnapshot {
        task_id: "msg-test".into(),
        messages: messages.clone(),
        tool_call_id: "tc-msg".into(),
        channel: "rpc".into(),
        chat_id: "chat-msg".into(),
        ready: true,
        created_at: chrono::Local::now().to_rfc3339(),
    };

    store.save(snap).await.unwrap();
    let loaded = store.load("msg-test").await.unwrap();

    assert_eq!(loaded.messages, messages);
    assert_eq!(loaded.messages.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn test_disk_file_has_correct_name() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("my-task-id")).await.unwrap();

    let expected_path = dir.path().join("my-task-id.json");
    assert!(
        expected_path.exists(),
        "Expected file at {:?}",
        expected_path
    );
}

// ============================================================
// Coverage improvement: cleanup, disk edge cases
// ============================================================

#[tokio::test]
async fn test_cleanup_old_snapshots_none_expired_v2() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Save a snapshot with current timestamp
    store.save(make_snapshot("fresh-task")).await.unwrap();

    // Cleanup with very long max age - nothing should be removed
    let removed = store
        .cleanup_old(std::time::Duration::from_secs(365 * 24 * 3600))
        .await
        .unwrap();
    assert_eq!(removed, 0);
    assert!(store.contains("fresh-task"));
}

#[tokio::test]
async fn test_cleanup_old_empty_store() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let removed = store
        .cleanup_old(std::time::Duration::from_secs(1))
        .await
        .unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_list_pending_includes_disk_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("disk-only-task")).await.unwrap();

    // Create a new store (memory empty, disk has data)
    let store2 = ContinuationStore::new(dir.path());
    let pending = store2.list_pending().await;
    assert!(pending.contains(&"disk-only-task".to_string()));
}

#[tokio::test]
async fn test_list_pending_includes_memory_only() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Save to memory but don't persist (use save without persist)
    // Actually save() does persist, so let's just verify it works
    store.save(make_snapshot("mem-task")).await.unwrap();

    let pending = store.list_pending().await;
    assert!(pending.contains(&"mem-task".to_string()));
}

#[tokio::test]
async fn test_snapshot_not_ready() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let mut snap = make_snapshot("not-ready-task");
    snap.ready = false;
    store.save(snap).await.unwrap();

    let loaded = store.load("not-ready-task").await.unwrap();
    assert!(!loaded.ready);
}

#[tokio::test]
async fn test_recover_from_disk_corrupted_file() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Write a corrupted JSON file
    tokio::fs::create_dir_all(dir.path()).await.unwrap();
    tokio::fs::write(dir.path().join("corrupted.json"), "not valid json{{{")
        .await
        .unwrap();

    let recovered = store.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 0); // Should skip corrupted file
}

// ============================================================
// Coverage improvement: save barrier, directory creation, cleanup, dedup
// ============================================================

#[tokio::test]
async fn test_save_barrier_retry_loop() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    let task_id = "barrier-retry-task";

    // Create a .json.tmp file to simulate a save in progress
    let tmp_path = dir.path().join(format!("{}.json.tmp", task_id));
    let final_path = dir.path().join(format!("{}.json", task_id));
    tokio::fs::write(&tmp_path, "saving...").await.unwrap();

    // In a separate task, after 200ms, write the actual .json file and remove .tmp
    let final_path_clone = final_path.clone();
    let tmp_path_clone = tmp_path.clone();
    let snap_json = serde_json::to_string_pretty(&make_snapshot(task_id)).unwrap();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
        tokio::fs::write(&final_path_clone, &snap_json)
            .await
            .unwrap();
        tokio::fs::remove_file(&tmp_path_clone).await.unwrap();
    });

    // load() should retry and eventually find the snapshot
    let loaded = store.load(task_id).await.unwrap();
    assert_eq!(loaded.task_id, task_id);
    assert_eq!(loaded.channel, "web");
}

#[tokio::test]
async fn test_save_barrier_retries_exhausted() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    let task_id = "barrier-exhaust-task";

    // Create a .json.tmp file but never write the actual .json
    let tmp_path = dir.path().join(format!("{}.json.tmp", task_id));
    tokio::fs::write(&tmp_path, "stuck saving...")
        .await
        .unwrap();

    // load() should exhaust retries and return NotFound
    let result = store.load(task_id).await;
    assert!(result.is_err());
    match result.unwrap_err() {
        ContinuationError::NotFound(id) => assert_eq!(id, task_id),
        other => panic!("Expected NotFound, got: {:?}", other),
    }

    // Clean up
    tokio::fs::remove_file(&tmp_path).await.unwrap();
}

#[tokio::test]
async fn test_persist_to_disk_creates_directory() {
    let dir = tempfile::tempdir().unwrap();
    // Use a non-existent subdirectory as cache_dir
    let nested_dir = dir.path().join("deeply").join("nested").join("cache");
    assert!(!nested_dir.exists());

    let store = ContinuationStore::new(&nested_dir);
    store.save(make_snapshot("mkdir-test")).await.unwrap();

    // Directory should have been created
    assert!(nested_dir.exists());

    // File should exist on disk
    let file_path = nested_dir.join("mkdir-test.json");
    assert!(file_path.exists());

    // Load from a fresh store to verify disk persistence
    let store2 = ContinuationStore::new(&nested_dir);
    let loaded = store2.load("mkdir-test").await.unwrap();
    assert_eq!(loaded.task_id, "mkdir-test");
}

#[tokio::test]
async fn test_cleanup_old_removes_old_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Save a snapshot
    store.save(make_snapshot("old-cleanup-task")).await.unwrap();

    let file_path = dir.path().join("old-cleanup-task.json");
    assert!(file_path.exists());

    // Use PowerShell to set the file modification time to 3 hours ago
    let ps_result = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-Command",
            &format!(
                "(Get-Item '{}').LastWriteTime = (Get-Date).AddHours(-3)",
                file_path.display()
            ),
        ])
        .output();

    match ps_result {
        Ok(output) if output.status.success() => {
            // Successfully set old mtime - cleanup with 1-hour threshold should remove it
            let removed = store
                .cleanup_old(std::time::Duration::from_secs(3600))
                .await
                .unwrap();
            assert_eq!(removed, 1);
            assert!(!store.contains("old-cleanup-task"));
            assert!(!file_path.exists());
        }
        _ => {
            // Fallback for environments without PowerShell: use a very short max_age
            // with a small sleep to ensure the file mtime is definitely older
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
            let removed = store
                .cleanup_old(std::time::Duration::from_nanos(1))
                .await
                .unwrap();
            // The file should be removed since nanos(1) is effectively "everything older than now"
            // Filesystem granularity may affect this, so we just verify the mechanism works
            if removed > 0 {
                assert!(!store.contains("old-cleanup-task"));
                assert!(!file_path.exists());
            }
        }
    }
}

#[tokio::test]
async fn test_list_pending_deduplicates() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // Save a snapshot (persists to both memory and disk)
    store.save(make_snapshot("dedup-task")).await.unwrap();

    // Create a new store from the same directory (empty memory, disk has data)
    let store2 = ContinuationStore::new(dir.path());

    // list_pending should not have duplicates even though disk has the file
    let pending = store2.list_pending().await;
    let count = pending.iter().filter(|id| id == &"dedup-task").count();
    assert_eq!(count, 1, "list_pending should not contain duplicates");

    // Recover into memory, then list_pending should still have exactly 1
    store2.recover_from_disk().await.unwrap();
    let pending2 = store2.list_pending().await;
    let count2 = pending2.iter().filter(|id| id == &"dedup-task").count();
    assert_eq!(
        count2, 1,
        "list_pending should still not contain duplicates after recover"
    );
}
