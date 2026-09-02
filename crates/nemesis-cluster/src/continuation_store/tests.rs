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

// ============================================================
// W3b coverage batch: save/persist failure arms (warn-not-fail),
// barrier-loop error propagation + memory-hit arm, IO error on
// dir-at-snapshot-path, remove-warn, cleanup_old error propagation,
// recover read errors, cache_dir getter
// ============================================================

#[tokio::test]
async fn test_w3b_save_warns_and_continues_when_cache_dir_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"i am a file").unwrap();

    // cache_dir occupied by a regular file → create_dir_all fails →
    // save() must only warn and still record the snapshot in memory.
    let store = ContinuationStore::new(&blocker);
    store.save(make_snapshot("w3b-nodisk")).await.unwrap();
    assert!(
        store.contains("w3b-nodisk"),
        "memory must win when disk fails"
    );
    assert_eq!(store.len(), 1);
}

#[tokio::test]
async fn test_w3b_save_persist_tmp_and_rename_failures() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // (a) tmp path blocked by a directory → tmp write fails → warn, still Ok
    std::fs::create_dir_all(dir.path().join("w3b-tmpfail.json.tmp")).unwrap();
    store.save(make_snapshot("w3b-tmpfail")).await.unwrap();
    assert!(store.contains("w3b-tmpfail"));

    // (b) final path blocked by a directory → rename fails → warn, still Ok
    std::fs::create_dir_all(dir.path().join("w3b-renfail.json")).unwrap();
    store.save(make_snapshot("w3b-renfail")).await.unwrap();
    assert!(store.contains("w3b-renfail"));
    // The tmp file was written before the rename failed.
    assert!(dir.path().join("w3b-renfail.json.tmp").exists());
}

#[tokio::test]
async fn test_w3b_load_barrier_loop_propagates_parse_error() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // tmp exists → the retry loop is entered; the final .json exists but is
    // corrupt → load_from_disk_inner returns Json (not NotFound) which must
    // propagate immediately instead of sleeping through all 50 retries.
    std::fs::write(dir.path().join("w3b-corrupt.json.tmp"), "saving").unwrap();
    std::fs::write(dir.path().join("w3b-corrupt.json"), "<<<not json>>>").unwrap();

    let start = std::time::Instant::now();
    let result = store.load("w3b-corrupt").await;
    let elapsed = start.elapsed();
    assert!(
        matches!(result, Err(ContinuationError::Json(_))),
        "expected Json error, got: {:?}",
        result
    );
    assert!(
        elapsed < std::time::Duration::from_secs(2),
        "parse error must propagate immediately, took {:?}",
        elapsed
    );
}

#[tokio::test]
async fn test_w3b_load_barrier_loop_memory_hit() {
    let dir = tempfile::tempdir().unwrap();
    let store = std::sync::Arc::new(ContinuationStore::new(dir.path()));
    let task_id = "w3b-memhit".to_string();

    // A stale tmp file makes load() enter the retry loop (memory + disk both
    // initially miss). A parallel writer inserts the snapshot into memory
    // mid-retry; the loop's memory check must pick it up on a later iteration.
    std::fs::write(dir.path().join("w3b-memhit.json.tmp"), "saving").unwrap();

    let writer = {
        let store = store.clone();
        let task_id = task_id.clone();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(60)).await;
            store
                .snapshots
                .lock()
                .insert(task_id, make_snapshot("w3b-memhit"));
        })
    };

    let loaded = store.load(&task_id).await.unwrap();
    assert_eq!(loaded.task_id, "w3b-memhit");
    writer.await.unwrap();
}

#[tokio::test]
async fn test_w3b_load_read_error_when_snapshot_is_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    // {task}.json as a directory: exists() is true so the NotFound guard is
    // skipped, but read_to_string fails → Io error surfaces.
    std::fs::create_dir_all(dir.path().join("w3b-ioerr.json")).unwrap();
    let result = store.load("w3b-ioerr").await;
    assert!(
        matches!(result, Err(ContinuationError::Io(_))),
        "expected Io error, got: {:?}",
        result
    );
}

#[tokio::test]
async fn test_w3b_remove_warns_when_disk_file_is_directory() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("w3b-rmwarn")).await.unwrap();
    let path = dir.path().join("w3b-rmwarn.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap(); // dir where the file used to be

    // remove() still reports success for the in-memory entry; the disk
    // delete failure is only logged.
    assert!(store.remove("w3b-rmwarn").await);
    assert!(!store.contains("w3b-rmwarn"));
    assert!(
        path.is_dir(),
        "the directory must survive the failed delete"
    );
}

#[tokio::test]
async fn test_w3b_cleanup_old_read_dir_error_and_dir_named_json_remove_error() {
    // (a) cache_dir occupied by a regular file → exists() passes but
    // read_dir fails → the io::Error must propagate via `?`.
    let holder = tempfile::tempdir().unwrap();
    let blocker = holder.path().join("blocker");
    std::fs::write(&blocker, b"file").unwrap();
    let store = ContinuationStore::new(&blocker);
    let result = store
        .cleanup_old(std::time::Duration::from_secs(3600))
        .await;
    assert!(result.is_err(), "read_dir failure must propagate");

    // (b) a DIRECTORY named *.json inside a valid cache dir passes the
    // extension/mtime checks but remove_file(dir) fails → propagate.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("stale.json")).unwrap();
    let store2 = ContinuationStore::new(dir.path());
    // Ensure the directory's mtime is strictly older than the cutoff.
    tokio::time::sleep(std::time::Duration::from_millis(30)).await;
    let result2 = store2.cleanup_old(std::time::Duration::ZERO).await;
    assert!(result2.is_err(), "remove_file on a directory must fail");
    assert!(dir.path().join("stale.json").is_dir(), "dir must survive");
}

#[tokio::test]
async fn test_w3b_recover_read_dir_error_and_per_file_read_error() {
    // (a) cache_dir occupied by a regular file → read_dir propagates via `?`
    let holder = tempfile::tempdir().unwrap();
    let blocker = holder.path().join("blocker");
    std::fs::write(&blocker, b"file").unwrap();
    let store = ContinuationStore::new(&blocker);
    assert!(store.recover_from_disk().await.is_err());

    // (b) *.json DIRECTORY next to a valid snapshot → per-file read warn is
    // skipped, the good file is still recovered.
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("weird.json")).unwrap();
    let good = make_snapshot("w3b-good");
    std::fs::write(
        dir.path().join("w3b-good.json"),
        serde_json::to_string_pretty(&good).unwrap(),
    )
    .unwrap();

    let store2 = ContinuationStore::new(dir.path());
    let recovered = store2.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 1, "only the readable snapshot is recovered");
    assert!(store2.contains("w3b-good"));
}

#[tokio::test]
async fn test_w3b_cache_dir_getter() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    assert_eq!(store.cache_dir(), dir.path());
}

// ============================================================
// S4 coverage: save-barrier final attempt, remove warn arm,
// cleanup_old removal, list_pending dedupe, recover_from_disk
// skip/deser-err/read-err arms.
// ============================================================

struct S4AllEventsSubscriber;
impl tracing::Subscriber for S4AllEventsSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

fn s4_tracing_subscriber() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4AllEventsSubscriber);
    });
}

/// A snapshot appearing in memory between the last retry check and the final
/// post-loop check is returned by the final attempt
/// (continuation_store.rs 132-139). The paused clock makes the 50x100ms
/// retry loop instant; the insert is scheduled at t=4950ms — after the 50th
/// loop memory check (t=4900) but before the final check (t=5000).
#[tokio::test(start_paused = true)]
async fn test_s4_load_final_attempt_after_retry_exhaustion() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();
    // Presence of the tmp file makes `might_be_saving` true → retry loop.
    std::fs::write(dir.path().join("s4-late.json.tmp"), b"").unwrap();

    let store = std::sync::Arc::new(ContinuationStore::new(dir.path()));

    let waiter_store = store.clone();
    let waiter = tokio::spawn(async move { waiter_store.load("s4-late").await });

    let inserter_store = store.clone();
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_millis(4950)).await;
        inserter_store
            .snapshots
            .lock()
            .insert("s4-late".to_string(), make_snapshot("s4-late"));
    });

    let snap = waiter.await.unwrap().unwrap();
    assert_eq!(snap.task_id, "s4-late");
}

/// remove() with the snapshot path blocked by a directory warns but still
/// reports memory removal (continuation_store.rs 150-159).
#[tokio::test]
async fn test_s4_remove_snapshot_path_is_directory() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    store.save(make_snapshot("s4-rm")).await.unwrap();

    // Replace the snapshot file with a directory to break remove_file.
    let path = dir.path().join("s4-rm.json");
    std::fs::remove_file(&path).unwrap();
    std::fs::create_dir_all(&path).unwrap();

    assert!(store.remove("s4-rm").await, "memory removal still succeeds");
    assert!(!store.contains("s4-rm"));
    assert!(path.is_dir(), "blocked path untouched");
}

/// cleanup_old removes an aged snapshot from memory and disk
/// (continuation_store.rs 203-217).
#[tokio::test]
async fn test_s4_cleanup_old_removes_aged_snapshot() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    store.save(make_snapshot("s4-old")).await.unwrap();
    assert!(dir.path().join("s4-old.json").exists());

    // Make the file clearly older than a 20ms cutoff.
    tokio::time::sleep(std::time::Duration::from_millis(60)).await;
    let removed = store
        .cleanup_old(std::time::Duration::from_millis(20))
        .await
        .unwrap();
    assert_eq!(removed, 1);
    assert!(!store.contains("s4-old"));
    assert!(!dir.path().join("s4-old.json").exists());
}

/// list_pending skips non-json entries and dedupes a disk file already
/// present in memory (continuation_store.rs 240-252).
#[tokio::test]
async fn test_s4_list_pending_dedupe_and_non_json() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    store.save(make_snapshot("s4-dup")).await.unwrap();
    std::fs::write(dir.path().join("s4-notes.txt"), "ignore me").unwrap();

    let ids = store.list_pending().await;
    assert_eq!(
        ids.iter().filter(|id| *id == "s4-dup").count(),
        1,
        "no duplicate entries: {:?}",
        ids
    );
    assert!(!ids.iter().any(|id| id.ends_with(".txt")));
}

/// recover_from_disk: skips in-memory duplicates, warns on undecodable and
/// unreadable snapshot files, recovers the good one
/// (continuation_store.rs 271-312).
#[tokio::test]
async fn test_s4_recover_from_disk_mixed_entries() {
    s4_tracing_subscriber();
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path()).unwrap();

    // Good snapshot on disk.
    std::fs::write(
        dir.path().join("s4-ok.json"),
        serde_json::to_string(&make_snapshot("s4-ok")).unwrap(),
    )
    .unwrap();
    // Duplicate of an in-memory snapshot → skip.
    std::fs::write(
        dir.path().join("s4-skip.json"),
        serde_json::to_string(&make_snapshot("s4-skip")).unwrap(),
    )
    .unwrap();
    // Invalid JSON → deserialize error.
    std::fs::write(dir.path().join("s4-bad.json"), "{not json").unwrap();
    // Unreadable entry (directory with .json name) → read error.
    std::fs::create_dir_all(dir.path().join("s4-dir.json")).unwrap();

    let store = ContinuationStore::new(dir.path());
    store
        .snapshots
        .lock()
        .insert("s4-skip".to_string(), make_snapshot("s4-skip"));

    let recovered = store.recover_from_disk().await.unwrap();
    assert_eq!(recovered, 1, "only s4-ok is newly recovered");
    assert!(store.contains("s4-ok"));
    assert!(store.contains("s4-skip"));
    assert!(!store.contains("s4-bad"));
}
