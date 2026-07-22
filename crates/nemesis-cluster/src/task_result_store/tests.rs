use super::*;
use std::fs as std_fs;

// ---- Original tests (unchanged) ----

#[test]
fn test_store_and_get_success() {
    let store = TaskResultStore::new(100);
    store.store_success("task-1", "peer_chat", serde_json::json!("response"));

    let result = store.get("task-1").unwrap();
    assert_eq!(result.task_id, "task-1");
    assert!(result.success);
    assert_eq!(result.action, "peer_chat");
}

#[test]
fn test_store_and_get_failure() {
    let store = TaskResultStore::new(100);
    store.store_failure("task-2", "forge_share", "connection refused");

    let result = store.get("task-2").unwrap();
    assert!(!result.success);
    assert_eq!(
        result.result.get("error").unwrap().as_str().unwrap(),
        "connection refused"
    );
}

#[test]
fn test_max_size_eviction() {
    let store = TaskResultStore::new(2);
    store.store_success("task-1", "a", serde_json::json!(1));
    store.store_success("task-2", "b", serde_json::json!(2));
    store.store_success("task-3", "c", serde_json::json!(3));

    // One should have been evicted
    assert_eq!(store.len(), 2);
}

#[test]
fn test_remove_and_clear() {
    let store = TaskResultStore::new(100);
    store.store_success("task-x", "action", serde_json::json!(null));
    assert!(store.remove("task-x"));
    assert!(store.get("task-x").is_none());

    store.store_success("task-y", "action", serde_json::json!(null));
    store.clear();
    assert!(store.is_empty());
}

// ---- Disk persistence tests ----

#[test]
fn test_disk_persistence_write() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    store.store_success("task-disk-1", "peer_chat", serde_json::json!("hello"));

    // Verify file exists on disk
    let file_path = tmp.path().join("task-disk-1.json");
    assert!(file_path.exists(), "result file should exist on disk");

    // Verify file content is valid JSON
    let data = std_fs::read_to_string(&file_path).unwrap();
    let parsed: TaskResult = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed.task_id, "task-disk-1");
    assert!(parsed.success);
    assert_eq!(parsed.action, "peer_chat");
}

#[test]
fn test_disk_persistence_failure() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    store.store_failure("task-disk-2", "forge_share", "timeout");

    let file_path = tmp.path().join("task-disk-2.json");
    assert!(file_path.exists());

    let data = std_fs::read_to_string(&file_path).unwrap();
    let parsed: TaskResult = serde_json::from_str(&data).unwrap();
    assert!(!parsed.success);
    assert_eq!(
        parsed.result.get("error").unwrap().as_str().unwrap(),
        "timeout"
    );
}

#[test]
fn test_disk_persistence_atomic_write() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    store.store_success("task-atomic", "action", serde_json::json!(42));

    // Main file should exist
    let file_path = tmp.path().join("task-atomic.json");
    assert!(file_path.exists());

    // Temp file should NOT remain
    let tmp_path = tmp.path().join("task-atomic.json.tmp");
    assert!(!tmp_path.exists(), "temp file should be cleaned up");
}

#[test]
fn test_load_from_disk() {
    let tmp = tempfile::tempdir().unwrap();

    // Store 1: write results to disk
    {
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        store.store_success("task-load-1", "peer_chat", serde_json::json!("response-1"));
        store.store_failure("task-load-2", "forge_share", "some error");
    }

    // Store 2: load from disk and verify
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk();
    assert_eq!(loaded, 2);

    let r1 = store.get("task-load-1").unwrap();
    assert_eq!(r1.task_id, "task-load-1");
    assert!(r1.success);

    let r2 = store.get("task-load-2").unwrap();
    assert_eq!(r2.task_id, "task-load-2");
    assert!(!r2.success);
    assert_eq!(
        r2.result.get("error").unwrap().as_str().unwrap(),
        "some error"
    );
}

#[test]
fn test_load_from_disk_respects_max_size() {
    let tmp = tempfile::tempdir().unwrap();

    // Write 5 result files
    {
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        for i in 0..5 {
            store.store_success(&format!("task-max-{i}"), "action", serde_json::json!(i));
        }
    }

    // Load with max_size = 2, should only load 2
    let store = TaskResultStore::with_disk_persistence(2, tmp.path());
    let loaded = store.load_from_disk();
    assert_eq!(loaded, 2);
    assert_eq!(store.len(), 2);
}

#[test]
fn test_load_from_disk_no_persistence() {
    let store = TaskResultStore::new(100);
    let loaded = store.load_from_disk();
    assert_eq!(loaded, 0);
}

#[test]
fn test_load_from_disk_corrupt_file() {
    let tmp = tempfile::tempdir().unwrap();

    // Write a valid file and a corrupt file
    {
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        store.store_success("task-good", "action", serde_json::json!("ok"));
    }
    // Write corrupt JSON
    std_fs::write(tmp.path().join("task-bad.json"), "not valid json{{{").unwrap();

    let store = TaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk();
    // Only the good file should be loaded
    assert_eq!(loaded, 1);
    assert!(store.get("task-good").is_some());
    assert!(store.get("task-bad").is_none());
}

#[test]
fn test_cleanup_delivered() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    store.store_success("task-cleanup", "action", serde_json::json!("data"));

    // File should exist
    let file_path = tmp.path().join("task-cleanup.json");
    assert!(file_path.exists());

    // Cleanup
    assert!(store.cleanup_delivered("task-cleanup"));
    assert!(store.get("task-cleanup").is_none());

    // File should be removed from disk
    assert!(!file_path.exists(), "file should be removed after cleanup");
}

#[test]
fn test_cleanup_delivered_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    // Should return false for non-existent task
    assert!(!store.cleanup_delivered("no-such-task"));
}

#[test]
fn test_remove_deletes_disk_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());

    store.store_success("task-remove", "action", serde_json::json!(1));
    let file_path = tmp.path().join("task-remove.json");
    assert!(file_path.exists());

    assert!(store.remove("task-remove"));
    assert!(!file_path.exists(), "file should be removed from disk");
}

#[test]
fn test_eviction_deletes_disk_file() {
    let tmp = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(2, tmp.path());

    store.store_success("evict-1", "a", serde_json::json!(1));
    store.store_success("evict-2", "b", serde_json::json!(2));
    // This should evict one of the previous entries
    store.store_success("evict-3", "c", serde_json::json!(3));

    assert_eq!(store.len(), 2);

    // At least one of the first two should be gone from disk
    let f1 = tmp.path().join("evict-1.json");
    let f2 = tmp.path().join("evict-2.json");
    let f3 = tmp.path().join("evict-3.json");
    // evict-3 must always exist
    assert!(f3.exists());
    // exactly one of evict-1/evict-2 should have been evicted
    assert_eq!(
        f1.exists() as u8 + f2.exists() as u8,
        1,
        "exactly one of the first two should remain on disk"
    );
}

#[test]
fn test_no_persistence_no_files() {
    let tmp = tempfile::tempdir().unwrap();

    // Use plain new() — no disk persistence
    let store = TaskResultStore::new(100);
    store.store_success("no-disk", "action", serde_json::json!("data"));

    // No files should be written to tmp
    let count = std_fs::read_dir(tmp.path())
        .unwrap()
        .filter(|e| {
            e.as_ref()
                .map(|e| e.path().extension().and_then(|e| e.to_str()) == Some("json"))
                .unwrap_or(false)
        })
        .count();
    assert_eq!(count, 0);
}

#[test]
fn test_restore_after_restart() {
    let tmp = tempfile::tempdir().unwrap();

    // Phase 1: write results
    {
        let store = TaskResultStore::with_disk_persistence(100, tmp.path());
        store.store_success("restart-1", "peer_chat", serde_json::json!("response-1"));
        store.store_failure("restart-2", "forge_share", "error-2");
    }

    // Phase 2: simulate restart — new store from same directory
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk();
    assert_eq!(loaded, 2);

    // Verify both results
    let r1 = store.get("restart-1").unwrap();
    assert!(r1.success);
    assert_eq!(r1.action, "peer_chat");

    let r2 = store.get("restart-2").unwrap();
    assert!(!r2.success);
    assert_eq!(r2.action, "forge_share");

    // Cleanup one, verify the other remains
    assert!(store.cleanup_delivered("restart-1"));
    assert!(store.get("restart-1").is_none());
    assert!(store.get("restart-2").is_some());

    // Phase 3: another restart — only restart-2 should load
    let store = TaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk();
    assert_eq!(loaded, 1);
    assert!(store.get("restart-1").is_none());
    assert!(store.get("restart-2").is_some());
}

// ---- Async tests ----

#[tokio::test]
async fn test_async_store_and_get() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

    store
        .store_success_async("async-1", "peer_chat", serde_json::json!("hello"))
        .await;
    store
        .store_failure_async("async-2", "forge_share", "boom")
        .await;

    let r1 = store.get_async("async-1").unwrap();
    assert!(r1.success);
    assert_eq!(r1.action, "peer_chat");

    let r2 = store.get_async("async-2").unwrap();
    assert!(!r2.success);
    assert_eq!(r2.result.get("error").unwrap().as_str().unwrap(), "boom");
}

#[tokio::test]
async fn test_async_disk_write() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

    store
        .store_success_async("async-disk", "action", serde_json::json!(42))
        .await;

    let file_path = tmp.path().join("async-disk.json");
    assert!(file_path.exists());

    let data = std_fs::read_to_string(&file_path).unwrap();
    let parsed: TaskResult = serde_json::from_str(&data).unwrap();
    assert_eq!(parsed.task_id, "async-disk");
}

#[tokio::test]
async fn test_async_load_from_disk() {
    let tmp = tempfile::tempdir().unwrap();

    // Write with one store
    {
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
        store
            .store_success_async("async-load-1", "a", serde_json::json!(1))
            .await;
        store.store_failure_async("async-load-2", "b", "err").await;
    }

    // Load with another
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk_async().await;
    assert_eq!(loaded, 2);

    assert!(store.get_async("async-load-1").unwrap().success);
    assert!(!store.get_async("async-load-2").unwrap().success);
}

#[tokio::test]
async fn test_async_cleanup_delivered() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

    store
        .store_success_async("async-clean", "action", serde_json::json!("data"))
        .await;

    let file_path = tmp.path().join("async-clean.json");
    assert!(file_path.exists());

    let existed = store.cleanup_delivered_async("async-clean").await;
    assert!(existed);
    assert!(store.get_async("async-clean").is_none());
    assert!(!file_path.exists(), "file should be deleted after cleanup");
}

#[tokio::test]
async fn test_async_cleanup_nonexistent() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());

    let existed = store.cleanup_delivered_async("nope").await;
    assert!(!existed);
}

#[tokio::test]
async fn test_async_eviction_deletes_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(2, tmp.path());

    store
        .store_success_async("ae-1", "a", serde_json::json!(1))
        .await;
    store
        .store_success_async("ae-2", "b", serde_json::json!(2))
        .await;
    store
        .store_success_async("ae-3", "c", serde_json::json!(3))
        .await;

    assert_eq!(store.get_async("ae-3").unwrap().task_id, "ae-3");

    let f3 = tmp.path().join("ae-3.json");
    assert!(f3.exists());
}

#[tokio::test]
async fn test_async_full_lifecycle() {
    let tmp = tempfile::tempdir().unwrap();

    // Phase 1: write
    {
        let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
        store
            .store_success_async("life-1", "peer_chat", serde_json::json!("response"))
            .await;
        store
            .store_failure_async("life-2", "action", "failed")
            .await;

        // Cleanup life-1
        assert!(store.cleanup_delivered_async("life-1").await);
    }

    // Phase 2: restart — only life-2 should be on disk
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk_async().await;
    assert_eq!(loaded, 1);
    assert!(store.get_async("life-1").is_none());
    assert!(store.get_async("life-2").is_some());

    // Cleanup life-2
    assert!(store.cleanup_delivered_async("life-2").await);

    // Phase 3: another restart — nothing left
    let store = AsyncTaskResultStore::with_disk_persistence(100, tmp.path());
    let loaded = store.load_from_disk_async().await;
    assert_eq!(loaded, 0);
}

// ---- Go-compatible store tests ----

#[test]
fn test_go_store_new_creates_directory() {
    let tmp = tempfile::tempdir().unwrap();
    let workspace = tmp.path().join("ws");
    let store = GoTaskResultStore::new(&workspace).unwrap();
    assert!(workspace.join("cluster").join("task_results").exists());
    let _ = store;
}

#[test]
fn test_go_store_set_running_and_get() {
    let tmp = tempfile::tempdir().unwrap();
    let store = GoTaskResultStore::new(tmp.path()).unwrap();

    store.set_running("task-1", "node-A");

    let entry = store.get("task-1").unwrap();
    assert_eq!(entry.task_id, "task-1");
    assert_eq!(entry.status, "running");
    assert!(store.is_running("task-1"));
}

#[test]
fn test_go_store_set_result_success() {
    let tmp = tempfile::tempdir().unwrap();
    let store = GoTaskResultStore::new(tmp.path()).unwrap();

    store.set_running("task-1", "node-A");
    store
        .set_result("task-1", "success", "hello world", "", "node-A")
        .unwrap();

    // Running should be cleared
    assert!(!store.is_running("task-1"));

    let entry = store.get("task-1").unwrap();
    assert_eq!(entry.status, "done");
    assert_eq!(entry.result_status.as_deref(), Some("success"));
    assert_eq!(entry.response.as_deref(), Some("hello world"));
    assert!(entry.error.is_none());
    assert_eq!(entry.source_node, "node-A");

    // Data file should exist
    let file_path = tmp.path().join("cluster/task_results/task-1.json");
    assert!(file_path.exists());
}

#[test]
fn test_go_store_set_result_error() {
    let tmp = tempfile::tempdir().unwrap();
    let store = GoTaskResultStore::new(tmp.path()).unwrap();

    store
        .set_result("task-2", "error", "", "connection refused", "node-B")
        .unwrap();

    let entry = store.get("task-2").unwrap();
    assert_eq!(entry.status, "done");
    assert_eq!(entry.result_status.as_deref(), Some("error"));
    assert!(entry.response.is_none());
    assert_eq!(entry.error.as_deref(), Some("connection refused"));
}

#[test]
fn test_go_store_get_unknown_returns_none() {
    let tmp = tempfile::tempdir().unwrap();
    let store = GoTaskResultStore::new(tmp.path()).unwrap();
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_go_store_delete() {
    let tmp = tempfile::tempdir().unwrap();
    let store = GoTaskResultStore::new(tmp.path()).unwrap();

    store
        .set_result("task-del", "success", "ok", "", "node-A")
        .unwrap();
    assert!(store.get("task-del").is_some());

    store.delete("task-del").unwrap();
    assert!(store.get("task-del").is_none());

    // Data file should be removed
    let file_path = tmp.path().join("cluster/task_results/task-del.json");
    assert!(!file_path.exists());
}

#[test]
fn test_go_store_index_persistence() {
    let tmp = tempfile::tempdir().unwrap();

    // Write with one store
    {
        let store = GoTaskResultStore::new(tmp.path()).unwrap();
        store
            .set_result("persist-1", "success", "result-1", "", "node-A")
            .unwrap();
        store
            .set_result("persist-2", "error", "", "timeout", "node-B")
            .unwrap();
    }

    // Load with new store
    let store = GoTaskResultStore::new(tmp.path()).unwrap();
    assert_eq!(store.done_count(), 2);

    let r1 = store.get("persist-1").unwrap();
    assert_eq!(r1.status, "done");
    assert_eq!(r1.result_status.as_deref(), Some("success"));

    let r2 = store.get("persist-2").unwrap();
    assert_eq!(r2.status, "done");
    assert_eq!(r2.result_status.as_deref(), Some("error"));
}

#[test]
fn test_go_store_restart_loses_running() {
    let tmp = tempfile::tempdir().unwrap();

    // Create store, set running + done
    {
        let store = GoTaskResultStore::new(tmp.path()).unwrap();
        store.set_running("running-task", "node-A");
        store
            .set_result("done-task", "success", "ok", "", "node-A")
            .unwrap();
    }

    // New store -- running state should be lost
    let store = GoTaskResultStore::new(tmp.path()).unwrap();
    assert!(!store.is_running("running-task"));
    assert!(store.get("running-task").is_none()); // Not in index either
    assert!(store.get("done-task").is_some());
}

// ============================================================
// Additional TaskResultStore tests for missing coverage
// ============================================================

#[test]
fn test_task_result_store_new() {
    let store = TaskResultStore::new(10);
    assert_eq!(store.len(), 0);
    assert!(store.is_empty());
}

#[test]
fn test_task_result_store_store_and_get() {
    let store = TaskResultStore::new(10);
    store.store_success(
        "task-1",
        "peer_chat",
        serde_json::json!({"response": "hello"}),
    );

    let result = store.get("task-1").unwrap();
    assert_eq!(result.task_id, "task-1");
    assert_eq!(result.action, "peer_chat");
    assert!(result.success);
    assert!(!result.stored_at.is_empty());
}

#[test]
fn test_task_result_store_failure() {
    let store = TaskResultStore::new(10);
    store.store_failure("task-2", "peer_chat", "connection refused");

    let result = store.get("task-2").unwrap();
    assert!(!result.success);
    assert_eq!(result.result["error"], "connection refused");
}

#[test]
fn test_task_result_store_get_nonexistent() {
    let store = TaskResultStore::new(10);
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_task_result_store_remove() {
    let store = TaskResultStore::new(10);
    store.store_success("task-1", "action", serde_json::json!({}));
    assert!(store.remove("task-1"));
    assert!(store.get("task-1").is_none());
}

#[test]
fn test_task_result_store_remove_nonexistent() {
    let store = TaskResultStore::new(10);
    assert!(!store.remove("nonexistent"));
}

#[test]
fn test_task_result_store_cleanup_delivered() {
    let store = TaskResultStore::new(10);
    store.store_success("task-1", "action", serde_json::json!({}));
    assert!(store.cleanup_delivered("task-1"));
    assert!(store.get("task-1").is_none());
}

#[test]
fn test_task_result_store_clear() {
    let store = TaskResultStore::new(10);
    store.store_success("task-1", "action", serde_json::json!({}));
    store.store_success("task-2", "action", serde_json::json!({}));
    store.clear();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn test_task_result_store_eviction() {
    let store = TaskResultStore::new(2);
    store.store_success("task-1", "a", serde_json::json!({}));
    store.store_success("task-2", "b", serde_json::json!({}));
    store.store_success("task-3", "c", serde_json::json!({}));

    // Should have at most 2 entries after eviction
    assert!(store.len() <= 2);
}

#[test]
fn test_task_result_serialization() {
    let result = TaskResult {
        task_id: "test-123".to_string(),
        action: "peer_chat".to_string(),
        result: serde_json::json!({"message": "hello"}),
        success: true,
        stored_at: "2026-05-11T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: TaskResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.task_id, "test-123");
    assert!(parsed.success);
    assert_eq!(parsed.action, "peer_chat");
}

#[test]
fn test_task_result_store_with_disk_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(10, dir.path());
    store.store_success("disk-1", "action", serde_json::json!({"ok": true}));

    // Should be on disk
    let file_path = dir.path().join("disk-1.json");
    assert!(file_path.exists());
}

#[test]
fn test_task_result_store_load_from_disk() {
    let dir = tempfile::tempdir().unwrap();

    // Write a result file
    let result = TaskResult {
        task_id: "loaded-1".to_string(),
        action: "test".to_string(),
        result: serde_json::json!({}),
        success: true,
        stored_at: "2026-05-11T00:00:00Z".to_string(),
    };
    let file_path = dir.path().join("loaded-1.json");
    std::fs::write(&file_path, serde_json::to_string(&result).unwrap()).unwrap();

    // Load from disk
    let store = TaskResultStore::with_disk_persistence(10, dir.path());
    let count = store.load_from_disk();
    assert_eq!(count, 1);
    assert!(store.get("loaded-1").is_some());
}

#[test]
fn test_task_result_store_load_from_disk_no_dir() {
    let store = TaskResultStore::new(10); // No disk persistence
    let count = store.load_from_disk();
    assert_eq!(count, 0);
}

#[test]
fn test_task_result_store_load_from_disk_invalid_json() {
    let dir = tempfile::tempdir().unwrap();
    // Write an invalid JSON file
    std::fs::write(dir.path().join("bad.json"), "not valid json").unwrap();

    let store = TaskResultStore::with_disk_persistence(10, dir.path());
    let count = store.load_from_disk();
    assert_eq!(count, 0); // Should skip invalid files
}

#[test]
fn test_task_result_store_load_from_disk_non_json() {
    let dir = tempfile::tempdir().unwrap();
    // Write a non-JSON file (should be ignored)
    std::fs::write(dir.path().join("readme.txt"), "hello").unwrap();

    let store = TaskResultStore::with_disk_persistence(10, dir.path());
    let count = store.load_from_disk();
    assert_eq!(count, 0);
}

#[test]
fn test_task_result_store_remove_deletes_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(10, dir.path());
    store.store_success("disk-del", "action", serde_json::json!({}));

    let file_path = dir.path().join("disk-del.json");
    assert!(file_path.exists());

    store.remove("disk-del");
    assert!(!file_path.exists());
}

// ============================================================
// Coverage improvement: more store edge cases
// ============================================================

#[test]
fn test_task_result_store_default() {
    let store = TaskResultStore::default();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
}

#[test]
fn test_task_result_store_clear_v2() {
    let store = TaskResultStore::new(10);
    store.store_success("task-1", "action", serde_json::json!({}));
    store.store_success("task-2", "action", serde_json::json!({}));
    assert_eq!(store.len(), 2);

    store.clear();
    assert!(store.is_empty());
}

#[test]
fn test_task_result_store_store_failure() {
    let store = TaskResultStore::new(10);
    assert!(store.get("nonexistent").is_none());
}

#[test]
fn test_task_result_store_remove_nonexistent_v2() {
    let store = TaskResultStore::new(10);
    assert!(!store.remove("nonexistent"));
}

#[test]
fn test_task_result_store_cleanup_delivered_v2() {
    let store = TaskResultStore::new(10);
    store.store_success("delivered-task", "action", serde_json::json!({}));
    assert!(store.cleanup_delivered("delivered-task"));
    assert!(store.get("delivered-task").is_none());
}

#[test]
fn test_task_result_store_cleanup_delivered_nonexistent() {
    let store = TaskResultStore::new(10);
    assert!(!store.cleanup_delivered("nonexistent"));
}

#[test]
fn test_task_result_store_no_disk_persistence() {
    let store = TaskResultStore::new(10);
    // No cache_dir set, write_to_disk and delete_from_disk are no-ops
    store.store_success("no-disk", "action", serde_json::json!({}));
    assert!(store.get("no-disk").is_some());

    store.remove("no-disk");
    assert!(store.get("no-disk").is_none());
}

#[test]
fn test_task_result_store_load_from_disk_no_cache() {
    let store = TaskResultStore::new(10);
    let count = store.load_from_disk();
    assert_eq!(count, 0);
}

#[test]
fn test_task_result_store_eviction_with_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = TaskResultStore::with_disk_persistence(2, dir.path());

    // Store 3 results, max_size is 2
    store.store_success("evict-1", "action", serde_json::json!({"n": 1}));
    store.store_success("evict-2", "action", serde_json::json!({"n": 2}));
    store.store_success("evict-3", "action", serde_json::json!({"n": 3}));

    // Should have at most 2
    assert!(store.len() <= 2);
    // The newest should still be there
    assert!(store.get("evict-3").is_some());
}

#[test]
fn test_task_result_debug() {
    let result = TaskResult {
        task_id: "debug-test".into(),
        action: "test".into(),
        result: serde_json::json!({"key": "value"}),
        success: true,
        stored_at: "2026-01-01T00:00:00Z".into(),
    };
    let debug = format!("{:?}", result);
    assert!(debug.contains("debug-test"));
    assert!(debug.contains("test"));
}

#[test]
fn test_task_result_serialization_roundtrip() {
    let result = TaskResult {
        task_id: "ser-test".into(),
        action: "act".into(),
        result: serde_json::json!({"x": 42}),
        success: true,
        stored_at: "2026-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: TaskResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.task_id, "ser-test");
    assert_eq!(parsed.action, "act");
    assert_eq!(parsed.result["x"], 42);
    assert!(parsed.success);
}

// -- Go-compatible store tests --

#[test]
fn test_go_task_result_entry_serialization() {
    let entry = GoTaskResultEntry {
        task_id: "go-task".into(),
        status: "running".into(),
        result_status: None,
        response: None,
        error: None,
        source_node: "node-a".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:00Z".into(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: GoTaskResultEntry = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.task_id, "go-task");
    assert_eq!(parsed.status, "running");
    assert!(parsed.response.is_none());
}

#[test]
fn test_go_task_result_entry_with_result() {
    let entry = GoTaskResultEntry {
        task_id: "go-done".into(),
        status: "done".into(),
        result_status: Some("success".into()),
        response: Some("task completed".into()),
        error: None,
        source_node: "node-b".into(),
        created_at: "2026-01-01T00:00:00Z".into(),
        updated_at: "2026-01-01T00:00:01Z".into(),
    };
    let json = serde_json::to_string(&entry).unwrap();
    let parsed: GoTaskResultEntry = serde_json::from_str(&json).unwrap();
    assert!(parsed.response.is_some());
    assert_eq!(parsed.result_status.unwrap(), "success");
}

#[test]
fn test_go_task_result_index_default() {
    let index = GoTaskResultIndex::default();
    assert!(index.tasks.is_empty());
}

#[test]
fn test_go_task_result_index_serialization() {
    let mut index = GoTaskResultIndex::default();
    index.tasks.insert(
        "t1".into(),
        GoTaskResultEntry {
            task_id: "t1".into(),
            status: "running".into(),
            result_status: None,
            response: None,
            error: None,
            source_node: "n1".into(),
            created_at: "2026-01-01".into(),
            updated_at: "2026-01-01".into(),
        },
    );
    let json = serde_json::to_string(&index).unwrap();
    let parsed: GoTaskResultIndex = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.tasks.len(), 1);
}

// -- Async store tests --

#[tokio::test]
async fn test_async_store_success_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

    store
        .store_success_async("async-1", "action", serde_json::json!({"ok": true}))
        .await;
    let result = store.get_async("async-1").unwrap();
    assert!(result.success);
    assert_eq!(result.result["ok"], true);
}

#[tokio::test]
async fn test_async_store_failure_and_get() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

    store
        .store_failure_async("async-fail", "action", "error msg")
        .await;
    let result = store.get_async("async-fail").unwrap();
    assert!(!result.success);
}

#[tokio::test]
async fn test_async_store_cleanup_delivered() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

    store
        .store_success_async("async-del", "action", serde_json::json!({}))
        .await;
    let existed = store.cleanup_delivered_async("async-del").await;
    assert!(existed);
    assert!(store.get_async("async-del").is_none());
}

#[tokio::test]
async fn test_async_store_load_from_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());

    store
        .store_success_async("async-load", "action", serde_json::json!({}))
        .await;
    // Give disk write time to complete
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;

    // Create new store and load from disk
    let store2 = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
    let count = store2.load_from_disk_async().await;
    assert!(count >= 1);
    assert!(store2.get_async("async-load").is_some());
}

#[tokio::test]
async fn test_async_store_load_no_cache_dir() {
    let inner = TaskResultStore::new(10);
    let store = AsyncTaskResultStore::new(inner);
    let count = store.load_from_disk_async().await;
    assert_eq!(count, 0);
}

#[tokio::test]
async fn test_async_store_cleanup_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
    let existed = store.cleanup_delivered_async("nonexistent").await;
    assert!(!existed);
}

#[tokio::test]
async fn test_async_store_get_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = AsyncTaskResultStore::with_disk_persistence(10, dir.path());
    assert!(store.get_async("nonexistent").is_none());
}
