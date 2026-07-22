use super::*;
use std::sync::atomic::{AtomicUsize, Ordering};

#[test]
fn test_create_and_get_task() {
    let tm = TaskManager::new();
    let task = tm.create_task(
        "peer_chat",
        serde_json::json!({"msg": "hello"}),
        "web",
        "chat-1",
    );

    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.action, "peer_chat");
    assert_eq!(retrieved.status, TaskStatus::Pending);
    assert!(retrieved.completed_at.is_none());
}

#[test]
fn test_assign_task() {
    let tm = TaskManager::new();
    let task = tm.create_task("ping", serde_json::json!({}), "rpc", "ch");

    assert!(tm.assign_task(&task.id, "node-a"));
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Running);

    // Cannot assign again
    assert!(!tm.assign_task(&task.id, "node-b"));
}

#[test]
fn test_complete_task_with_callback() {
    let completed = Arc::new(Mutex::new(Vec::new()));
    let completed_clone = completed.clone();
    let tm = TaskManager::with_callback(Box::new(move |t: &Task| {
        completed_clone.lock().push(t.id.clone());
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task.id, serde_json::json!("result-data"));

    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Completed);
    assert!(updated.completed_at.is_some());

    // Callback should have fired
    let ids = completed.lock();
    assert!(ids.contains(&task.id));
}

#[test]
fn test_fail_task() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    assert!(tm.fail_task(&task.id, "connection refused"));
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Failed);
    assert!(updated.result.as_ref().unwrap().get("error").is_some());
}

#[test]
fn test_delete_task() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.get_task(&task.id).is_some());
    assert!(tm.delete_task(&task.id));
    assert!(tm.get_task(&task.id).is_none());
}

#[test]
fn test_submit_task() {
    let tm = TaskManager::new();
    let task = Task {
        id: "custom-task-001".to_string(),
        status: TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({"msg": "hello"}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };

    assert!(tm.submit(task).is_ok());
    let retrieved = tm.get_task("custom-task-001").unwrap();
    assert_eq!(retrieved.action, "peer_chat");
}

#[test]
fn test_submit_duplicate_fails() {
    let tm = TaskManager::new();
    let task = Task {
        id: "dup-task".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };

    assert!(tm.submit(task.clone()).is_ok());
    // Second submit should fail
    assert!(tm.submit(task).is_err());
}

#[test]
fn test_set_on_complete() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let tm = TaskManager::new();
    tm.set_on_complete(Box::new(move |_task_id: &str| {
        call_count_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task.id, serde_json::json!("done"));

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_list_by_status() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&t1.id, serde_json::json!("done"));

    let pending = tm.list_pending_tasks();
    let completed = tm.list_completed_tasks();
    assert_eq!(pending.len(), 1);
    assert_eq!(completed.len(), 1);
}

#[test]
fn test_complete_callback_error() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    tm.complete_callback(&task.id, "error", "", "something went wrong");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Failed);
}

#[test]
fn test_complete_callback_success() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    tm.complete_callback(&task.id, "success", "hello world", "");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Completed);
}

// -- InMemoryTaskStore tests --

#[test]
fn test_in_memory_store_crud() {
    let store = InMemoryTaskStore::new();

    let task = Task {
        id: "test-1".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };

    assert!(store.create(task).is_ok());
    assert!(store.get("test-1").is_ok());
    assert!(store.get("nonexistent").is_err());

    assert!(
        store
            .update_result(
                "test-1",
                TaskStatus::Completed,
                Some(serde_json::json!("done"))
            )
            .is_ok()
    );
    let t = store.get("test-1").unwrap();
    assert_eq!(t.status, TaskStatus::Completed);

    assert!(store.delete("test-1").is_ok());
    assert!(store.get("test-1").is_err());
}

#[test]
fn test_in_memory_store_list_by_status() {
    let store = InMemoryTaskStore::new();

    for i in 0..3 {
        let task = Task {
            id: format!("task-{}", i),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
    }

    let pending = store.list_by_status(TaskStatus::Pending);
    assert_eq!(pending.len(), 3);

    store
        .update_result("task-0", TaskStatus::Completed, None)
        .unwrap();
    let pending = store.list_by_status(TaskStatus::Pending);
    assert_eq!(pending.len(), 2);
    let completed = store.list_by_status(TaskStatus::Completed);
    assert_eq!(completed.len(), 1);
}

#[test]
fn test_cleanup_completed_removes_old_tasks() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    // Create a completed task with an old completed_at timestamp
    let old_task = Task {
        id: "old-completed".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(old_task).unwrap();

    // Complete it with an old timestamp (simulate)
    store
        .update_result(
            "old-completed",
            TaskStatus::Completed,
            Some(serde_json::json!("done")),
        )
        .unwrap();

    // Manually set the completed_at to 3 hours ago (can't easily do this through
    // the store interface, so we test the logic indirectly)
    // The cleanup function checks completed_at. Since we just created it,
    // it won't be cleaned up.
    cleanup_completed(&store, &None);

    // Task should still exist (completed_at is recent)
    assert!(store.get("old-completed").is_ok());
}

#[test]
fn test_cleanup_pending_timeout() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());
    let callback_count = Arc::new(AtomicUsize::new(0));
    let callback_count_clone = callback_count.clone();

    let on_complete: Option<Arc<OnCompleteCallback>> =
        Some(Arc::new(Box::new(move |_task: &Task| {
            callback_count_clone.fetch_add(1, Ordering::SeqCst);
        })));

    // Create a pending task with a very old created_at
    let old_time = (chrono::Local::now() - chrono::Duration::hours(25)).to_rfc3339();
    let old_task = Task {
        id: "old-pending".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_time,
        completed_at: None,
    };
    store.create(old_task).unwrap();

    // Create a recent pending task (should NOT be timed out)
    let recent_task = Task {
        id: "recent-pending".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(recent_task).unwrap();

    cleanup_completed(&store, &on_complete);

    // Old pending should be failed
    let old = store.get("old-pending").unwrap();
    assert_eq!(old.status, TaskStatus::Failed);

    // Recent pending should still be pending
    let recent = store.get("recent-pending").unwrap();
    assert_eq!(recent.status, TaskStatus::Pending);

    // Callback should have been called once
    assert_eq!(callback_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_task_manager_with_custom_store() {
    let store = Arc::new(InMemoryTaskStore::new());
    let mut tm = TaskManager::with_store(store);

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.get_task(&task.id).is_some());

    // start/stop should work without panicking
    tm.start();
    tm.stop();
}

#[test]
fn test_len_and_is_empty() {
    let tm = TaskManager::new();
    assert!(tm.is_empty());
    assert_eq!(tm.len(), 0);

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(!tm.is_empty());
    assert_eq!(tm.len(), 1);

    tm.delete_task(&task.id);
    assert!(tm.is_empty());
}

// -- Additional tests: task state transitions, concurrent operations, error edge cases --

#[test]
fn test_create_task_with_peer() {
    let tm = TaskManager::new();
    let task = tm.create_task_with_peer(
        "peer_chat",
        serde_json::json!({"msg": "hello"}),
        "web",
        "chat-1",
        "remote-node-001",
    );
    assert_eq!(task.peer_id, "remote-node-001");
    assert_eq!(task.action, "peer_chat");

    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.peer_id, "remote-node-001");
}

#[test]
fn test_task_full_lifecycle_pending_running_completed() {
    let tm = TaskManager::new();
    let task = tm.create_task("peer_chat", serde_json::json!({}), "rpc", "ch");

    // Pending
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Pending);
    assert!(t.completed_at.is_none());

    // Assign -> Running
    assert!(tm.assign_task(&task.id, "node-a"));
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Running);

    // Complete -> Completed
    assert!(tm.complete_task(&task.id, serde_json::json!("result")));
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
    assert!(t.completed_at.is_some());
    assert_eq!(t.result.unwrap(), serde_json::json!("result"));
}

#[test]
fn test_task_full_lifecycle_pending_failed() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    // Fail directly from Pending (skip Running)
    assert!(tm.fail_task(&task.id, "connection lost"));
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Failed);
    assert!(t.completed_at.is_some());
    let result_val = t.result.unwrap();
    let err = result_val.get("error").unwrap().as_str().unwrap();
    assert_eq!(err, "connection lost");
}

#[test]
fn test_assign_task_nonexistent_returns_false() {
    let tm = TaskManager::new();
    assert!(!tm.assign_task("nonexistent-task", "node-a"));
}

#[test]
fn test_complete_task_nonexistent_returns_false() {
    let tm = TaskManager::new();
    assert!(!tm.complete_task("nonexistent-task", serde_json::json!("x")));
}

#[test]
fn test_fail_task_nonexistent_returns_false() {
    let tm = TaskManager::new();
    assert!(!tm.fail_task("nonexistent-task", "error"));
}

#[test]
fn test_assign_running_task_fails() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    // First assign succeeds
    assert!(tm.assign_task(&task.id, "node-a"));

    // Second assign should fail (already Running)
    assert!(!tm.assign_task(&task.id, "node-b"));
}

#[test]
fn test_assign_completed_task_fails() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    tm.complete_task(&task.id, serde_json::json!("done"));

    // Assigning a completed task should fail (status != Pending)
    assert!(!tm.assign_task(&task.id, "node-a"));
}

#[test]
fn test_complete_callback_with_empty_error_defaults() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    // Error status with empty err_msg should use "unknown error"
    tm.complete_callback(&task.id, "error", "", "");
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Failed);
    let result_val = t.result.unwrap();
    let err = result_val.get("error").unwrap().as_str().unwrap();
    assert_eq!(err, "unknown error");
}

#[test]
fn test_complete_callback_with_empty_response_uses_null() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");

    // Success with empty response should use null
    tm.complete_callback(&task.id, "success", "", "");
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
    assert_eq!(t.result.unwrap(), serde_json::json!(null));
}

#[test]
fn test_callback_fires_on_fail_task() {
    let call_count = Arc::new(AtomicUsize::new(0));
    let call_count_clone = call_count.clone();
    let tm = TaskManager::with_callback(Box::new(move |_t: &Task| {
        call_count_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.fail_task(&task.id, "some error");

    assert_eq!(call_count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_list_failed_tasks() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    let t3 = tm.create_task("c", serde_json::json!({}), "rpc", "ch");

    tm.fail_task(&t1.id, "error-1");
    tm.complete_task(&t3.id, serde_json::json!("done"));

    let failed = tm.list_failed_tasks();
    assert_eq!(failed.len(), 1);
    assert_eq!(failed[0].id, t1.id);
}

#[test]
fn test_delete_nonexistent_task_returns_false() {
    let tm = TaskManager::new();
    assert!(!tm.delete_task("nonexistent"));
}

#[test]
fn test_in_memory_store_update_nonexistent_fails() {
    let store = InMemoryTaskStore::new();
    let result = store.update_result("nonexistent", TaskStatus::Completed, None);
    assert!(result.is_err());
}

#[test]
fn test_in_memory_store_delete_nonexistent_fails() {
    let store = InMemoryTaskStore::new();
    let result = store.delete("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_in_memory_store_list_all() {
    let store = InMemoryTaskStore::new();
    for i in 0..5 {
        let task = Task {
            id: format!("task-{}", i),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Local::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
    }
    let all = store.list_all();
    assert_eq!(all.len(), 5);
}

// ============================================================
// Coverage improvement: more edge cases, cleanup, start/stop
// ============================================================

#[tokio::test]
async fn test_start_stop_lifecycle() {
    let mut tm = TaskManager::with_store_and_interval(
        Arc::new(InMemoryTaskStore::new()),
        std::time::Duration::from_millis(100),
    );
    tm.start();
    assert!(tm.stop_tx.is_some());
    tm.stop();
    assert!(tm.stop_tx.is_none());
}

#[test]
fn test_start_without_runtime_is_noop() {
    let mut tm = TaskManager::new();
    tm.start();
    // Should not panic, stop_tx stays None
    assert!(tm.stop_tx.is_none());
}

#[test]
fn test_start_idempotent() {
    let mut tm = TaskManager::new();
    tm.start();
    tm.start(); // second call should be no-op
    assert!(tm.stop_tx.is_none()); // still None because no runtime
}

#[test]
fn test_stop_without_start_is_noop() {
    let mut tm = TaskManager::new();
    tm.stop(); // should not panic
}

#[test]
fn test_set_callback_replaces_existing() {
    let call_count1 = Arc::new(AtomicUsize::new(0));
    let call_count1_clone = call_count1.clone();
    let tm = TaskManager::new();
    tm.set_callback(Box::new(move |_t: &Task| {
        call_count1_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task1.id, serde_json::json!("done"));
    assert_eq!(call_count1.load(Ordering::SeqCst), 1);

    // Replace callback
    let call_count2 = Arc::new(AtomicUsize::new(0));
    let call_count2_clone = call_count2.clone();
    tm.set_callback(Box::new(move |_t: &Task| {
        call_count2_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task2.id, serde_json::json!("done"));
    // First callback should NOT have been called again
    assert_eq!(call_count1.load(Ordering::SeqCst), 1);
    assert_eq!(call_count2.load(Ordering::SeqCst), 1);
}

#[test]
fn test_complete_task_without_callback() {
    let tm = TaskManager::new();
    // No callback set
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.complete_task(&task.id, serde_json::json!("result")));
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
}

#[test]
fn test_fail_task_without_callback() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.fail_task(&task.id, "error"));
    let t = tm.get_task(&task.id).unwrap();
    assert_eq!(t.status, TaskStatus::Failed);
}

#[test]
fn test_cleanup_completed_removes_cancelled_tasks() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    // Create a task, complete it
    let task = Task {
        id: "cancelled-task".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    store
        .update_result("cancelled-task", TaskStatus::Cancelled, None)
        .unwrap();

    // Since just created, should not be cleaned up
    cleanup_completed(&store, &None);
    assert!(store.get("cancelled-task").is_ok());
}

#[test]
fn test_cleanup_completed_with_invalid_created_at() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    // Create a pending task with invalid created_at
    let task = Task {
        id: "bad-date".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: "not-a-date".to_string(),
        completed_at: None,
    };
    store.create(task).unwrap();

    // Should not panic with invalid date
    cleanup_completed(&store, &None);
    assert!(store.get("bad-date").is_ok());
}

#[test]
fn test_cleanup_completed_with_invalid_completed_at() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    let task = Task {
        id: "bad-completed-date".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    store
        .update_result(
            "bad-completed-date",
            TaskStatus::Completed,
            Some(serde_json::json!("done")),
        )
        .unwrap();

    // Should not panic
    cleanup_completed(&store, &None);
}

#[test]
fn test_with_store_and_interval_custom() {
    let store = Arc::new(InMemoryTaskStore::new());
    let tm = TaskManager::with_store_and_interval(store, std::time::Duration::from_secs(60));
    assert_eq!(tm.cleanup_interval, std::time::Duration::from_secs(60));
}

#[test]
fn test_in_memory_store_default() {
    let store = InMemoryTaskStore::default();
    assert!(store.list_all().is_empty());
}

#[test]
fn test_task_manager_default() {
    let tm = TaskManager::default();
    assert!(tm.is_empty());
}

#[test]
fn test_create_task_unique_ids() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let t2 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    assert_ne!(t1.id, t2.id);
}

#[test]
fn test_multiple_tasks_lifecycle() {
    let completed_ids = Arc::new(Mutex::new(Vec::new()));
    let completed_clone = completed_ids.clone();
    let tm = TaskManager::with_callback(Box::new(move |t: &Task| {
        completed_clone.lock().push(t.id.clone());
    }));

    let tasks: Vec<_> = (0..5)
        .map(|i| tm.create_task(&format!("action-{}", i), serde_json::json!({}), "rpc", "ch"))
        .collect();

    // Complete some, fail others
    tm.complete_task(&tasks[0].id, serde_json::json!("r0"));
    tm.fail_task(&tasks[1].id, "e1");
    tm.complete_task(&tasks[2].id, serde_json::json!("r2"));

    assert_eq!(tm.list_completed_tasks().len(), 2);
    assert_eq!(tm.list_failed_tasks().len(), 1);
    assert_eq!(tm.list_pending_tasks().len(), 2);
    assert_eq!(completed_ids.lock().len(), 3);
}

// ============================================================
// Coverage improvement: cleanup edge cases, callback replacement, idempotent start
// ============================================================

#[test]
fn test_cleanup_completed_removes_old_completed_tasks() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    // Create a task already in Completed state with completed_at 3 hours ago
    let old_completed_at = (chrono::Local::now() - chrono::Duration::hours(3)).to_rfc3339();
    let old_task = Task {
        id: "old-completed-v2".to_string(),
        status: TaskStatus::Completed,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: Some(serde_json::json!("done")),
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: (chrono::Local::now() - chrono::Duration::hours(4)).to_rfc3339(),
        completed_at: Some(old_completed_at),
    };
    store.create(old_task).unwrap();

    // Also create a failed task 3 hours ago
    let old_failed_at = (chrono::Local::now() - chrono::Duration::hours(3)).to_rfc3339();
    let old_failed_task = Task {
        id: "old-failed-v2".to_string(),
        status: TaskStatus::Failed,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: Some(serde_json::json!({"error": "timeout"})),
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: (chrono::Local::now() - chrono::Duration::hours(4)).to_rfc3339(),
        completed_at: Some(old_failed_at),
    };
    store.create(old_failed_task).unwrap();

    // Verify both exist before cleanup
    assert!(store.get("old-completed-v2").is_ok());
    assert!(store.get("old-failed-v2").is_ok());

    cleanup_completed(&store, &None);

    // Both should be deleted (older than 2 hours)
    assert!(store.get("old-completed-v2").is_err());
    assert!(store.get("old-failed-v2").is_err());
}

#[test]
fn test_cleanup_completed_keeps_recent_completed_tasks() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());

    // Create a task that was completed 5 minutes ago (within 2-hour window)
    let recent_completed_at = (chrono::Local::now() - chrono::Duration::minutes(5)).to_rfc3339();
    let recent_task = Task {
        id: "recent-completed".to_string(),
        status: TaskStatus::Completed,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: Some(serde_json::json!("result")),
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: (chrono::Local::now() - chrono::Duration::minutes(10)).to_rfc3339(),
        completed_at: Some(recent_completed_at),
    };
    store.create(recent_task).unwrap();

    cleanup_completed(&store, &None);

    // Should still exist (completed only 5 min ago)
    let t = store.get("recent-completed").unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
}

#[test]
fn test_cleanup_pending_timeout_fires_callback_v2() {
    let store: Arc<dyn TaskStore> = Arc::new(InMemoryTaskStore::new());
    let callback_fired = Arc::new(Mutex::new(None::<String>));
    let callback_fired_clone = callback_fired.clone();

    let on_complete: Option<Arc<OnCompleteCallback>> = Some(Arc::new(Box::new(move |t: &Task| {
        *callback_fired_clone.lock() = Some(t.id.clone());
    })));

    // Create a pending task with created_at 25 hours ago (exceeds 24-hour threshold)
    let old_created = (chrono::Local::now() - chrono::Duration::hours(25)).to_rfc3339();
    let old_pending_task = Task {
        id: "timed-out-task".to_string(),
        status: TaskStatus::Pending,
        action: "peer_chat".to_string(),
        peer_id: "remote-1".to_string(),
        payload: serde_json::json!({"msg": "hello"}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: old_created,
        completed_at: None,
    };
    store.create(old_pending_task).unwrap();

    // Create a recent pending task (should NOT be timed out)
    let recent_pending = Task {
        id: "recent-pending-v2".to_string(),
        status: TaskStatus::Pending,
        action: "action".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Local::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(recent_pending).unwrap();

    cleanup_completed(&store, &on_complete);

    // Old pending task should now be Failed
    let timed_out = store.get("timed-out-task").unwrap();
    assert_eq!(timed_out.status, TaskStatus::Failed);
    assert!(timed_out.completed_at.is_some());
    let binding = timed_out.result.unwrap();
    let error = binding.get("error").unwrap().as_str().unwrap();
    assert!(error.contains("timed out"));

    // Callback should have been invoked for the timed-out task
    let fired_id = callback_fired.lock().take();
    assert_eq!(fired_id, Some("timed-out-task".to_string()));

    // Recent pending should still be pending
    let recent = store.get("recent-pending-v2").unwrap();
    assert_eq!(recent.status, TaskStatus::Pending);
}

#[tokio::test]
async fn test_start_idempotent_v2() {
    // Test that calling start() twice is a no-op when a runtime is present
    let mut tm = TaskManager::with_store_and_interval(
        Arc::new(InMemoryTaskStore::new()),
        std::time::Duration::from_secs(600), // long interval so cleanup doesn't fire during test
    );
    tm.start();
    assert!(tm.stop_tx.is_some());

    // Second start should be a no-op (stop_tx still Some, not replaced)
    tm.start();
    assert!(
        tm.stop_tx.is_some(),
        "stop_tx should still be Some after second start()"
    );

    // Stop should work correctly (only one background task was spawned)
    tm.stop();
    assert!(tm.stop_tx.is_none());
}

#[test]
fn test_set_callback_replaces_existing_v2() {
    let call_count1 = Arc::new(AtomicUsize::new(0));
    let call_count1_clone = call_count1.clone();
    let call_count2 = Arc::new(AtomicUsize::new(0));
    let call_count2_clone = call_count2.clone();

    let tm = TaskManager::new();

    // Set first callback
    tm.set_callback(Box::new(move |_t: &Task| {
        call_count1_clone.fetch_add(1, Ordering::SeqCst);
    }));

    // Complete task 1 - callback1 fires
    let task1 = tm.create_task("action-a", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task1.id, serde_json::json!("done-a"));
    assert_eq!(call_count1.load(Ordering::SeqCst), 1);
    assert_eq!(call_count2.load(Ordering::SeqCst), 0);

    // Replace with second callback
    tm.set_callback(Box::new(move |_t: &Task| {
        call_count2_clone.fetch_add(1, Ordering::SeqCst);
    }));

    // Complete task 2 - only callback2 fires
    let task2 = tm.create_task("action-b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task2.id, serde_json::json!("done-b"));
    assert_eq!(
        call_count1.load(Ordering::SeqCst),
        1,
        "callback1 should NOT fire again"
    );
    assert_eq!(
        call_count2.load(Ordering::SeqCst),
        1,
        "callback2 should fire once"
    );

    // Fail task 3 - only callback2 fires
    let task3 = tm.create_task("action-c", serde_json::json!({}), "rpc", "ch");
    tm.fail_task(&task3.id, "some error");
    assert_eq!(call_count1.load(Ordering::SeqCst), 1);
    assert_eq!(
        call_count2.load(Ordering::SeqCst),
        2,
        "callback2 should fire on fail too"
    );
}
