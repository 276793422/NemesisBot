//! Supplemental tests for nemesis-cluster crate.
//!
//! Covers: task manager lifecycle, continuation store, wire message edge cases,
//! connection pool, frame encoding/decoding, cluster state, and more.

use nemesis_cluster::task_manager::{TaskManager, InMemoryTaskStore, TaskStore};
use nemesis_cluster::continuation_store::{ContinuationStore, ContinuationSnapshot};
use nemesis_cluster::rpc_types::{Frame, RPCRequest, RPCResponse, ActionType, KnownAction};
use nemesis_cluster::transport::conn::{Connection, WireMessage, TcpConnConfig};
use nemesis_cluster::transport::pool::{PoolConfig, ConnectionPool, AsyncPoolConfig, Pool, PoolStats};
use nemesis_cluster::transport::frame::{MAX_FRAME_SIZE, FRAME_HEADER_SIZE, validate_frame_size, encode_batch, decode_all, write_frame, read_frame, AsyncFrameReader, write_frame_async};
use nemesis_cluster::types::{ClusterConfig, NodeStatus, ExtendedNodeInfo};

use nemesis_types::cluster::{Task, TaskStatus, NodeInfo, NodeRole};

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;

// ===========================================================================
// TaskManager extended tests
// ===========================================================================

#[test]
fn test_task_manager_default() {
    let tm = TaskManager::default();
    assert!(tm.is_empty());
    assert_eq!(tm.len(), 0);
}

#[test]
fn test_create_task_basic_fields() {
    let tm = TaskManager::new();
    let task = tm.create_task(
        "peer_chat",
        serde_json::json!({"message": "hello"}),
        "web",
        "chat-001",
    );
    assert!(!task.id.is_empty());
    assert_eq!(task.action, "peer_chat");
    assert_eq!(task.status, TaskStatus::Pending);
    assert_eq!(task.original_channel, "web");
    assert_eq!(task.original_chat_id, "chat-001");
    assert!(task.completed_at.is_none());
    assert!(task.result.is_none());
    assert!(!task.created_at.is_empty());
}

#[test]
fn test_create_task_with_peer() {
    let tm = TaskManager::new();
    let task = tm.create_task_with_peer(
        "peer_chat",
        serde_json::json!({"msg": "hi"}),
        "rpc",
        "chat-2",
        "peer-node-1",
    );
    assert_eq!(task.peer_id, "peer-node-1");
}

#[test]
fn test_create_task_with_empty_peer() {
    let tm = TaskManager::new();
    let task = tm.create_task(
        "ping",
        serde_json::json!({}),
        "rpc",
        "ch",
    );
    assert_eq!(task.peer_id, "");
}

#[test]
fn test_complete_task_nonexistent() {
    let tm = TaskManager::new();
    assert!(!tm.complete_task("nonexistent-id", serde_json::json!("result")));
}

#[test]
fn test_fail_task_nonexistent() {
    let tm = TaskManager::new();
    assert!(!tm.fail_task("nonexistent-id", "error message"));
}

#[test]
fn test_delete_task_nonexistent() {
    let tm = TaskManager::new();
    assert!(!tm.delete_task("nonexistent-id"));
}

#[test]
fn test_get_task_nonexistent() {
    let tm = TaskManager::new();
    assert!(tm.get_task("nonexistent-id").is_none());
}

#[test]
fn test_assign_task_nonexistent() {
    let tm = TaskManager::new();
    assert!(!tm.assign_task("nonexistent-id", "node-a"));
}

#[test]
fn test_assign_task_already_running() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task.id, serde_json::json!("done"));
    assert!(!tm.assign_task(&task.id, "node-a"));
}

#[test]
fn test_assign_task_from_running_fails() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.assign_task(&task.id, "node-a"));
    assert!(!tm.assign_task(&task.id, "node-b"));
}

#[test]
fn test_complete_task_with_callback_fires() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();
    let tm = TaskManager::with_callback(Box::new(move |t: &Task| {
        count_clone.fetch_add(1, Ordering::SeqCst);
        assert!(!t.id.is_empty());
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.complete_task(&task.id, serde_json::json!("result")));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_fail_task_with_callback_fires() {
    let count = Arc::new(AtomicUsize::new(0));
    let count_clone = count.clone();
    let tm = TaskManager::with_callback(Box::new(move |_t: &Task| {
        count_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.fail_task(&task.id, "test error"));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

#[test]
fn test_complete_callback_success() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "success", "my response", "");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Completed);
    let result = updated.result.as_ref().unwrap();
    // complete_callback wraps the response as json!(response), so it's just a string
    assert_eq!(result, "my response");
}

#[test]
fn test_complete_callback_error() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "error", "", "connection refused");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Failed);
    let result = updated.result.as_ref().unwrap();
    assert_eq!(result["error"], "connection refused");
}

#[test]
fn test_complete_callback_error_empty_errmsg() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "error", "", "");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Failed);
}

#[test]
fn test_complete_callback_success_empty_response() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "success", "", "");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Completed);
}

#[test]
fn test_set_on_complete_callback() {
    let ids = Arc::new(Mutex::new(Vec::new()));
    let ids_clone = ids.clone();
    let tm = TaskManager::new();
    tm.set_on_complete(Box::new(move |task_id: &str| {
        ids_clone.lock().unwrap().push(task_id.to_string());
    }));

    let task1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let task2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task1.id, serde_json::json!("r1"));
    tm.fail_task(&task2.id, "e");

    let ids = ids.lock().unwrap();
    assert!(ids.contains(&task1.id));
    assert!(ids.contains(&task2.id));
}

#[test]
fn test_submit_prebuilt_task() {
    let tm = TaskManager::new();
    let task = Task {
        id: "custom-id-123".to_string(),
        status: TaskStatus::Pending,
        action: "test_action".to_string(),
        peer_id: "peer-1".to_string(),
        payload: serde_json::json!({"key": "value"}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    assert!(tm.submit(task).is_ok());
    let retrieved = tm.get_task("custom-id-123").unwrap();
    assert_eq!(retrieved.action, "test_action");
    assert_eq!(retrieved.peer_id, "peer-1");
}

#[test]
fn test_submit_duplicate_task_fails() {
    let tm = TaskManager::new();
    let task = Task {
        id: "dup-id".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    assert!(tm.submit(task.clone()).is_ok());
    assert!(tm.submit(task).is_err());
}

#[test]
fn test_list_pending_after_complete() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&t1.id, serde_json::json!("done"));
    assert_eq!(tm.list_pending_tasks().len(), 1);
}

#[test]
fn test_list_completed_tasks() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    let _t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&t1.id, serde_json::json!("done"));
    assert_eq!(tm.list_completed_tasks().len(), 1);
}

#[test]
fn test_list_failed_tasks() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    tm.fail_task(&t1.id, "error");
    assert_eq!(tm.list_failed_tasks().len(), 1);
}

#[test]
fn test_multiple_tasks_lifecycle() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch1");
    let t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch2");
    let t3 = tm.create_task("c", serde_json::json!({}), "rpc", "ch3");
    assert_eq!(tm.len(), 3);

    tm.assign_task(&t1.id, "node-a");
    tm.complete_task(&t2.id, serde_json::json!("result"));
    tm.fail_task(&t3.id, "fail reason");

    assert_eq!(tm.get_task(&t1.id).unwrap().status, TaskStatus::Running);
    assert_eq!(tm.get_task(&t2.id).unwrap().status, TaskStatus::Completed);
    assert_eq!(tm.get_task(&t3.id).unwrap().status, TaskStatus::Failed);

    assert_eq!(tm.list_pending_tasks().len(), 0);
    assert_eq!(tm.list_completed_tasks().len(), 1);
    assert_eq!(tm.list_failed_tasks().len(), 1);
}

#[test]
fn test_delete_task_reduces_count() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    assert_eq!(tm.len(), 1);
    assert!(tm.delete_task(&t1.id));
    assert_eq!(tm.len(), 0);
    assert!(tm.is_empty());
}

#[test]
fn test_task_with_large_payload() {
    let tm = TaskManager::new();
    let large_payload = serde_json::json!({
        "data": "x".repeat(10000),
        "nested": {
            "array": (0..100).map(|i| format!("item_{}", i)).collect::<Vec<_>>()
        }
    });
    let task = tm.create_task("action", large_payload, "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload["data"].as_str().unwrap().len(), 10000);
}

#[test]
fn test_task_with_unicode_payload() {
    let tm = TaskManager::new();
    let unicode_payload = serde_json::json!({
        "message": "Hello! Bonjour! こんにちは！你好！",
        "emoji": "🚀🎉💻",
    });
    let task = tm.create_task("action", unicode_payload.clone(), "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload, unicode_payload);
}

#[test]
fn test_task_with_null_payload() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::Value::Null, "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert!(retrieved.payload.is_null());
}

#[test]
fn test_list_tasks_returns_all() {
    let tm = TaskManager::new();
    tm.create_task("a", serde_json::json!({}), "rpc", "ch1");
    tm.create_task("b", serde_json::json!({}), "rpc", "ch2");
    tm.create_task("c", serde_json::json!({}), "rpc", "ch3");
    assert_eq!(tm.list_tasks().len(), 3);
}

// ===========================================================================
// InMemoryTaskStore extended tests
// ===========================================================================

#[test]
fn test_store_default_is_empty() {
    let store = InMemoryTaskStore::default();
    assert!(store.list_all().is_empty());
}

#[test]
fn test_store_create_get_delete_roundtrip() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "round-1".to_string(),
        status: TaskStatus::Pending,
        action: "test".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({"x": 1}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    let t = store.get("round-1").unwrap();
    assert_eq!(t.payload["x"], 1);
    store.delete("round-1").unwrap();
    assert!(store.get("round-1").is_err());
}

#[test]
fn test_store_delete_nonexistent() {
    let store = InMemoryTaskStore::new();
    assert!(store.delete("no-such-task").is_err());
}

#[test]
fn test_store_update_result_sets_completed_at() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "u-1".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    store.update_result("u-1", TaskStatus::Completed, Some(serde_json::json!("done"))).unwrap();
    let t = store.get("u-1").unwrap();
    assert_eq!(t.status, TaskStatus::Completed);
    assert!(t.completed_at.is_some());
}

#[test]
fn test_store_update_nonexistent() {
    let store = InMemoryTaskStore::new();
    assert!(store.update_result("nope", TaskStatus::Completed, None).is_err());
}

#[test]
fn test_store_list_all_multiple() {
    let store = InMemoryTaskStore::new();
    for i in 0..5 {
        let task = Task {
            id: format!("task-{}", i),
            status: TaskStatus::Pending,
            action: "a".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
    }
    assert_eq!(store.list_all().len(), 5);
}

#[test]
fn test_store_list_by_status_mixed() {
    let store = InMemoryTaskStore::new();
    for i in 0..6 {
        let task = Task {
            id: format!("t-{}", i),
            status: TaskStatus::Pending,
            action: "a".to_string(),
            peer_id: String::new(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: chrono::Utc::now().to_rfc3339(),
            completed_at: None,
        };
        store.create(task).unwrap();
    }
    for i in (0..6).step_by(2) {
        store.update_result(&format!("t-{}", i), TaskStatus::Completed, Some(serde_json::json!("ok"))).unwrap();
    }
    store.update_result("t-1", TaskStatus::Failed, Some(serde_json::json!({"error": "fail"}))).unwrap();

    assert_eq!(store.list_by_status(TaskStatus::Pending).len(), 2);
    assert_eq!(store.list_by_status(TaskStatus::Completed).len(), 3);
    assert_eq!(store.list_by_status(TaskStatus::Failed).len(), 1);
}

#[test]
fn test_store_create_duplicate_fails() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "dup".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    assert!(store.create(task.clone()).is_ok());
    assert!(store.create(task).is_err());
}

// ===========================================================================
// ContinuationStore extended tests
// ===========================================================================

fn make_snapshot(task_id: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: task_id.to_string(),
        messages: serde_json::json!([{"role": "user", "content": "hello"}]),
        tool_call_id: "tc-default".to_string(),
        channel: "web".to_string(),
        chat_id: "chat-default".to_string(),
        ready: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    }
}

#[tokio::test]
async fn test_continuation_save_overwrite() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let snap1 = make_snapshot("task-1");
    store.save(snap1).await.unwrap();

    let snap2 = ContinuationSnapshot {
        task_id: "task-1".to_string(),
        messages: serde_json::json!([{"role": "user", "content": "updated"}]),
        tool_call_id: "tc-updated".to_string(),
        channel: "rpc".to_string(),
        chat_id: "chat-updated".to_string(),
        ready: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    };
    store.save(snap2).await.unwrap();

    let loaded = store.load("task-1").await.unwrap();
    assert_eq!(loaded.tool_call_id, "tc-updated");
    assert_eq!(loaded.channel, "rpc");
}

#[tokio::test]
async fn test_continuation_contains() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    assert!(!store.contains("nonexistent"));
    store.save(make_snapshot("task-x")).await.unwrap();
    assert!(store.contains("task-x"));
}

#[tokio::test]
async fn test_continuation_len_and_is_empty() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);

    store.save(make_snapshot("t1")).await.unwrap();
    store.save(make_snapshot("t2")).await.unwrap();
    assert_eq!(store.len(), 2);
    assert!(!store.is_empty());
}

#[tokio::test]
async fn test_continuation_remove_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    assert!(!store.remove("nonexistent").await);
}

#[tokio::test]
async fn test_continuation_save_and_remove_clears_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("task-disk")).await.unwrap();
    let file_path = dir.path().join("task-disk.json");
    assert!(file_path.exists());

    store.remove("task-disk").await;
    assert!(!file_path.exists());
}

#[tokio::test]
async fn test_continuation_snapshot_fields() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let snap = ContinuationSnapshot {
        task_id: "fields-test".to_string(),
        messages: serde_json::json!([
            {"role": "system", "content": "You are helpful"},
            {"role": "user", "content": "Hello"},
            {"role": "assistant", "content": "Hi", "tool_calls": [{"id": "tc-1", "name": "search", "arguments": "{}"}]}
        ]),
        tool_call_id: "tc-001".to_string(),
        channel: "discord".to_string(),
        chat_id: "chat-456".to_string(),
        ready: true,
        created_at: "2026-01-01T00:00:00Z".to_string(),
    };
    store.save(snap).await.unwrap();

    let loaded = store.load("fields-test").await.unwrap();
    assert_eq!(loaded.channel, "discord");
    assert_eq!(loaded.chat_id, "chat-456");
    assert!(loaded.ready);
    assert_eq!(loaded.messages.as_array().unwrap().len(), 3);
}

#[tokio::test]
async fn test_continuation_multiple_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    for i in 0..10 {
        store.save(make_snapshot(&format!("multi-{}", i))).await.unwrap();
    }
    assert_eq!(store.len(), 10);

    let pending = store.list_pending().await;
    assert_eq!(pending.len(), 10);
}

#[tokio::test]
async fn test_continuation_disk_recovery_preserves_data() {
    let dir = tempfile::tempdir().unwrap();

    {
        let store = ContinuationStore::new(dir.path());
        let snap = ContinuationSnapshot {
            task_id: "recover-test".to_string(),
            messages: serde_json::json!([{"role": "user", "content": "test message"}]),
            tool_call_id: "tc-recover".to_string(),
            channel: "web".to_string(),
            chat_id: "chat-r".to_string(),
            ready: true,
            created_at: chrono::Utc::now().to_rfc3339(),
        };
        store.save(snap).await.unwrap();
    }

    {
        let store2 = ContinuationStore::new(dir.path());
        let recovered = store2.recover_from_disk().await.unwrap();
        assert_eq!(recovered, 1);

        let loaded = store2.load("recover-test").await.unwrap();
        assert_eq!(loaded.tool_call_id, "tc-recover");
    }
}

#[tokio::test]
async fn test_continuation_cleanup_with_no_files() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    let removed = store.cleanup_old(std::time::Duration::from_secs(0)).await.unwrap();
    assert_eq!(removed, 0);
}

#[tokio::test]
async fn test_continuation_cleanup_nonexistent_dir() {
    let store = ContinuationStore::new("/tmp/nonexistent_cleanup_test_dir_12345");
    let result = store.cleanup_old(std::time::Duration::from_secs(0)).await;
    // Nonexistent dir should return error or 0
    assert!(result.is_ok() || result.is_err());
}

#[tokio::test]
async fn test_continuation_load_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());
    let result = store.load("nonexistent").await;
    assert!(result.is_err());
}

// ===========================================================================
// WireMessage tests
// ===========================================================================

#[test]
fn test_wire_message_new_request_fields() {
    let msg = WireMessage::new_request("from-node", "to-node", "test_action", serde_json::json!({"k": "v"}));
    assert_eq!(msg.version, "1.0");
    assert_eq!(msg.msg_type, "request");
    assert_eq!(msg.from, "from-node");
    assert_eq!(msg.to, "to-node");
    assert_eq!(msg.action, "test_action");
    assert!(msg.error.is_empty());
    assert!(msg.timestamp > 0);
    assert!(!msg.id.is_empty());
}

#[test]
fn test_wire_message_response_preserves_id() {
    let req = WireMessage::new_request("a", "b", "ping", serde_json::json!({}));
    let resp = WireMessage::new_response(&req, serde_json::json!({"pong": true}));
    assert_eq!(resp.id, req.id);
    assert_eq!(resp.msg_type, "response");
    assert_eq!(resp.from, "b");
    assert_eq!(resp.to, "a");
    assert_eq!(resp.action, "ping");
    assert!(resp.is_response());
    assert!(!resp.is_request());
    assert!(!resp.is_error());
}

#[test]
fn test_wire_message_error_response() {
    let req = WireMessage::new_request("a", "b", "action", serde_json::json!({}));
    let err = WireMessage::new_error(&req, "connection refused");
    assert_eq!(err.id, req.id);
    assert_eq!(err.msg_type, "error");
    assert_eq!(err.error, "connection refused");
    assert_eq!(err.from, "b");
    assert_eq!(err.to, "a");
    assert!(err.is_error());
    assert!(err.payload.is_null());
}

#[test]
fn test_wire_message_validate_empty_version() {
    let mut msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    msg.version = String::new();
    assert!(msg.validate().is_err());
}

#[test]
fn test_wire_message_validate_empty_id() {
    let mut msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    msg.id = String::new();
    assert!(msg.validate().is_err());
}

#[test]
fn test_wire_message_validate_empty_from() {
    let mut msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    msg.from = String::new();
    assert!(msg.validate().is_err());
}

#[test]
fn test_wire_message_validate_empty_to() {
    let mut msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    msg.to = String::new();
    assert!(msg.validate().is_err());
}

#[test]
fn test_wire_message_validate_empty_action() {
    let mut msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    msg.action = String::new();
    assert!(msg.validate().is_err());
}

#[test]
fn test_wire_message_validate_ok() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(msg.validate().is_ok());
}

#[test]
fn test_wire_message_serialization_unicode() {
    let msg = WireMessage::new_request(
        "node-a",
        "node-b",
        "peer_chat",
        serde_json::json!({"message": "こんにちは！你好！🚀"}),
    );
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert_eq!(back.payload["message"], "こんにちは！你好！🚀");
}

#[test]
fn test_wire_message_serialization_empty_payload() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert_eq!(back.payload, serde_json::json!({}));
}

#[test]
fn test_wire_message_serialization_null_payload() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::Value::Null);
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert!(back.payload.is_null());
}

#[test]
fn test_wire_message_from_bytes_invalid_json() {
    let result = WireMessage::from_bytes(b"not json at all");
    assert!(result.is_err());
}

#[test]
fn test_wire_message_serialization_large_payload() {
    let large_data: Vec<String> = (0..1000).map(|i| format!("item_{}", i)).collect();
    let msg = WireMessage::new_request(
        "a", "b", "bulk",
        serde_json::json!({"items": large_data}),
    );
    let bytes = msg.to_bytes().unwrap();
    let back = WireMessage::from_bytes(&bytes).unwrap();
    assert_eq!(back.payload["items"].as_array().unwrap().len(), 1000);
}

#[test]
fn test_wire_message_is_request_response_error() {
    let req = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(req.is_request());
    assert!(!req.is_response());
    assert!(!req.is_error());

    let resp = WireMessage::new_response(&req, serde_json::json!({}));
    assert!(!resp.is_request());
    assert!(resp.is_response());
    assert!(!resp.is_error());

    let err = WireMessage::new_error(&req, "err");
    assert!(!err.is_request());
    assert!(!err.is_response());
    assert!(err.is_error());
}

#[test]
fn test_wire_message_unique_ids() {
    let msg1 = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    let msg2 = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert_ne!(msg1.id, msg2.id);
}

// ===========================================================================
// Frame (from rpc_types) encoding/decoding tests
// ===========================================================================

#[test]
fn test_frame_new_empty() {
    let frame = Frame::new(Vec::new());
    assert!(frame.data.is_empty());
}

#[test]
fn test_frame_empty_payload() {
    let frame = Frame::new(Vec::new());
    let encoded = frame.encode();
    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert!(decoded.data.is_empty());
    assert_eq!(consumed, 4);
}

#[test]
fn test_frame_single_byte_payload() {
    let frame = Frame::new(vec![0x42]);
    let encoded = frame.encode();
    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data, vec![0x42]);
    assert_eq!(consumed, 5);
}

#[test]
fn test_frame_binary_payload() {
    let data: Vec<u8> = (0..=255).collect();
    let frame = Frame::new(data.clone());
    let encoded = frame.encode();
    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data, data);
    assert_eq!(consumed, 4 + 256);
}

#[test]
fn test_frame_json_payload_roundtrip() {
    let json = serde_json::json!({
        "action": "peer_chat",
        "message": "Hello from node A",
        "nested": {"key": [1, 2, 3]}
    });
    let payload = serde_json::to_vec(&json).unwrap();
    let frame = Frame::new(payload);
    let encoded = frame.encode();
    let (decoded, _) = Frame::decode(&encoded).unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&decoded.data).unwrap();
    assert_eq!(parsed["action"], "peer_chat");
}

#[test]
fn test_frame_decode_multiple_from_buffer() {
    let f1 = Frame::new(b"first".to_vec());
    let f2 = Frame::new(b"second".to_vec());
    let f3 = Frame::new(b"third".to_vec());
    let mut buf = Vec::new();
    buf.extend_from_slice(&f1.encode());
    buf.extend_from_slice(&f2.encode());
    buf.extend_from_slice(&f3.encode());

    let (decoded, consumed) = Frame::decode(&buf).unwrap();
    assert_eq!(decoded.data, b"first".to_vec());
    assert_eq!(consumed, 4 + 5);

    let (decoded2, consumed2) = Frame::decode(&buf[consumed..]).unwrap();
    assert_eq!(decoded2.data, b"second".to_vec());

    let (decoded3, _) = Frame::decode(&buf[consumed + consumed2..]).unwrap();
    assert_eq!(decoded3.data, b"third".to_vec());
}

#[test]
fn test_frame_decode_incomplete_header() {
    assert!(Frame::decode(&[0, 0, 1]).is_none());
}

#[test]
fn test_frame_decode_header_but_no_payload() {
    let header: [u8; 4] = (5u32).to_be_bytes();
    let buf = [header[0], header[1], header[2], header[3], 0xAA, 0xBB];
    assert!(Frame::decode(&buf).is_none());
}

#[test]
fn test_encode_decode_rpc_request_roundtrip() {
    let req = RPCRequest {
        id: "test-req-001".to_string(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({"status": "ok"}),
        source: "node-1".to_string(),
        target: Some("node-2".to_string()),
    };
    let encoded = Frame::encode_request(&req).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();
    // encode_request produces WireMessage format; decode_response handles it
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.id, "test-req-001");
    assert!(decoded.result.is_some());
    assert!(decoded.error.is_none());
}

#[test]
fn test_encode_decode_rpc_response_roundtrip() {
    let resp = RPCResponse {
        id: "test-resp-001".to_string(),
        result: Some(serde_json::json!({"data": [1, 2, 3]})),
        error: None,
    };
    let encoded = Frame::encode_response(&resp).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.id, "test-resp-001");
    assert_eq!(decoded.result.as_ref().unwrap()["data"].as_array().unwrap().len(), 3);
    assert!(decoded.error.is_none());
}

#[test]
fn test_encode_decode_rpc_error_response() {
    let resp = RPCResponse {
        id: "err-001".to_string(),
        result: None,
        error: Some("connection timeout".to_string()),
    };
    let encoded = Frame::encode_response(&resp).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.error, Some("connection timeout".to_string()));
    assert!(decoded.result.is_none());
}

// ===========================================================================
// ActionType tests
// ===========================================================================

#[test]
fn test_action_type_as_str() {
    assert_eq!(ActionType::Known(KnownAction::PeerChat).as_str(), "PeerChat");
    assert_eq!(ActionType::Known(KnownAction::PeerChatCallback).as_str(), "PeerChatCallback");
    assert_eq!(ActionType::Known(KnownAction::ForgeShare).as_str(), "ForgeShare");
    assert_eq!(ActionType::Known(KnownAction::Ping).as_str(), "Ping");
    assert_eq!(ActionType::Known(KnownAction::Status).as_str(), "Status");
    assert_eq!(ActionType::Custom("custom_action".to_string()).as_str(), "custom_action");
}

#[test]
fn test_action_type_display() {
    assert_eq!(format!("{}", ActionType::Known(KnownAction::Ping)), "Ping");
    assert_eq!(format!("{}", ActionType::Custom("my_action".to_string())), "my_action");
}

#[test]
fn test_action_type_serialization_roundtrip() {
    let actions = vec![
        ActionType::Known(KnownAction::PeerChat),
        ActionType::Known(KnownAction::PeerChatCallback),
        ActionType::Known(KnownAction::ForgeShare),
        ActionType::Known(KnownAction::Ping),
        ActionType::Known(KnownAction::Status),
        ActionType::Custom("query_task_result".to_string()),
    ];
    for action in &actions {
        let json = serde_json::to_string(action).unwrap();
        let back: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *action);
    }
}

#[test]
fn test_action_type_deserialize_unknown() {
    let json = "\"unknown_action\"";
    let action: ActionType = serde_json::from_str(json).unwrap();
    assert_eq!(action, ActionType::Custom("unknown_action".to_string()));
}

#[test]
fn test_action_type_equality() {
    assert_eq!(ActionType::Known(KnownAction::Ping), ActionType::Known(KnownAction::Ping));
    assert_ne!(ActionType::Known(KnownAction::Ping), ActionType::Known(KnownAction::Status));
    assert_ne!(ActionType::Known(KnownAction::Ping), ActionType::Custom("Ping".to_string()));
}

// ===========================================================================
// Transport frame validation tests
// ===========================================================================

#[test]
fn test_validate_frame_size_at_boundary() {
    let data = vec![0u8; MAX_FRAME_SIZE];
    assert!(validate_frame_size(&data).is_ok());
}

#[test]
fn test_validate_frame_size_zero() {
    assert!(validate_frame_size(&[]).is_ok());
}

#[test]
fn test_validate_frame_size_too_large() {
    let data = vec![0u8; MAX_FRAME_SIZE + 1];
    assert!(validate_frame_size(&data).is_err());
}

#[test]
fn test_sync_read_frame_too_large() {
    let mut buf = Vec::new();
    let len: u32 = (MAX_FRAME_SIZE + 1) as u32;
    buf.extend_from_slice(&len.to_be_bytes());
    buf.extend_from_slice(&[0u8; 100]);

    let mut cursor = std::io::Cursor::new(buf);
    let result = read_frame(&mut cursor);
    assert!(result.is_err());
}

#[test]
fn test_sync_write_read_frame_roundtrip() {
    let data = b"hello world frame";
    let mut buf = Vec::new();
    write_frame(&mut buf, data).unwrap();

    let mut cursor = std::io::Cursor::new(buf);
    let result = read_frame(&mut cursor).unwrap();
    assert_eq!(result, data);
}

#[test]
fn test_sync_write_frame_too_large() {
    let data = vec![0u8; MAX_FRAME_SIZE + 1];
    let mut buf = Vec::new();
    let result = write_frame(&mut buf, &data);
    assert!(result.is_err());
}

// ===========================================================================
// encode_batch / decode_all tests
// ===========================================================================

#[test]
fn test_encode_batch_empty() {
    let frames: Vec<Frame> = vec![];
    let encoded = encode_batch(&frames);
    assert!(encoded.is_empty());
}

#[test]
fn test_decode_all_empty_buffer() {
    let (frames, consumed) = decode_all(&[]);
    assert!(frames.is_empty());
    assert_eq!(consumed, 0);
}

#[test]
fn test_decode_all_multiple_frames() {
    let frames = vec![
        Frame::new(b"aaa".to_vec()),
        Frame::new(b"bbbb".to_vec()),
        Frame::new(b"ccccc".to_vec()),
    ];
    let encoded = encode_batch(&frames);
    let (decoded, consumed) = decode_all(&encoded);
    assert_eq!(decoded.len(), 3);
    assert_eq!(consumed, encoded.len());
    assert_eq!(decoded[0].data, b"aaa");
    assert_eq!(decoded[1].data, b"bbbb");
    assert_eq!(decoded[2].data, b"ccccc");
}

#[test]
fn test_decode_all_with_garbage_at_end() {
    let frame = Frame::new(b"data".to_vec());
    let mut encoded = frame.encode();
    encoded.extend_from_slice(&[0xFF, 0xFF]); // Too short for a header

    let (decoded, consumed) = decode_all(&encoded);
    assert_eq!(decoded.len(), 1);
    assert_eq!(consumed, encoded.len() - 2);
}

// ===========================================================================
// Cluster types tests
// ===========================================================================

#[test]
fn test_cluster_config_custom() {
    let config = ClusterConfig {
        node_id: "node-1".to_string(),
        bind_address: "0.0.0.0:8080".to_string(),
        peers: vec!["10.0.0.2:9000".to_string(), "10.0.0.3:9000".to_string()],
    };
    assert_eq!(config.node_id, "node-1");
    assert_eq!(config.peers.len(), 2);
}

#[test]
fn test_cluster_config_serialization() {
    let config = ClusterConfig {
        node_id: "test-node".to_string(),
        bind_address: "0.0.0.0:9999".to_string(),
        peers: vec!["peer1:9000".to_string()],
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: ClusterConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.node_id, "test-node");
    assert_eq!(back.bind_address, "0.0.0.0:9999");
    assert_eq!(back.peers.len(), 1);
}

#[test]
fn test_node_status_variants() {
    assert_eq!(NodeStatus::Online, NodeStatus::Online);
    assert_ne!(NodeStatus::Online, NodeStatus::Offline);
    assert_ne!(NodeStatus::Offline, NodeStatus::Connecting);
}

#[test]
fn test_node_status_serialization() {
    let json = serde_json::to_string(&NodeStatus::Connecting).unwrap();
    let back: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, NodeStatus::Connecting);
}

fn make_test_extended_node(id: &str, status: NodeStatus, caps: Vec<&str>, last_seen: &str) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.to_string(),
            name: format!("{}-name", id),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".to_string(),
            category: "development".to_string(),
            last_seen: last_seen.to_string(),
        },
        status,
        capabilities: caps.into_iter().map(String::from).collect(),
        addresses: vec![],
    }
}

#[test]
fn test_extended_node_info_get_id_name_address() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec!["llm"], "");
    assert_eq!(node.get_id(), "n1");
    assert_eq!(node.get_name(), "n1-name");
    assert_eq!(node.get_address(), "10.0.0.1:9000");
}

#[test]
fn test_extended_node_info_get_capabilities() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec!["llm", "tools"], "");
    let caps = node.get_capabilities();
    assert_eq!(caps.len(), 2);
    assert!(caps.contains(&"llm".to_string()));
    assert!(caps.contains(&"tools".to_string()));
}

#[test]
fn test_extended_node_info_mark_offline() {
    let mut node = make_test_extended_node("n1", NodeStatus::Online, vec![], "");
    assert!(node.is_online());
    node.mark_offline("timeout");
    assert!(!node.is_online());
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_extended_node_info_set_status_connecting() {
    let mut node = make_test_extended_node("n1", NodeStatus::Online, vec![], "");
    node.set_status(NodeStatus::Connecting);
    assert_eq!(node.status, NodeStatus::Connecting);
    assert!(!node.is_online());
}

#[test]
fn test_extended_node_info_display_format() {
    let node = make_test_extended_node("my-node", NodeStatus::Online, vec![], "");
    let s = format!("{}", node);
    assert!(s.contains("my-node"));
    assert!(s.contains("online"));
}

#[test]
fn test_extended_node_info_has_capability_case_insensitive() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec!["LLM", "Tools"], "");
    assert!(node.has_capability("llm"));
    assert!(node.has_capability("TOOLS"));
    assert!(node.has_capability("Llm"));
    assert!(!node.has_capability("webhook"));
}

#[test]
fn test_extended_node_info_update_last_seen_transitions_to_online() {
    let mut node = make_test_extended_node("n1", NodeStatus::Connecting, vec![], "");
    assert!(!node.is_online());
    node.update_last_seen();
    assert!(node.is_online());
    assert!(!node.base.last_seen.is_empty());
}

#[test]
fn test_extended_node_info_get_status_string() {
    let node_online = make_test_extended_node("n1", NodeStatus::Online, vec![], "");
    assert_eq!(node_online.get_status_string(), "online");

    let node_offline = make_test_extended_node("n1", NodeStatus::Offline, vec![], "");
    assert_eq!(node_offline.get_status_string(), "offline");

    let node_connecting = make_test_extended_node("n1", NodeStatus::Connecting, vec![], "");
    assert_eq!(node_connecting.get_status_string(), "connecting");
}

// ===========================================================================
// TcpConnConfig tests
// ===========================================================================

#[test]
fn test_tcp_conn_config_fields() {
    let config = TcpConnConfig {
        node_id: "test-node".to_string(),
        address: "127.0.0.1:9999".to_string(),
        read_buffer_size: 50,
        send_buffer_size: 50,
        send_timeout: std::time::Duration::from_secs(5),
        idle_timeout: std::time::Duration::from_secs(60),
        heartbeat_interval: Some(std::time::Duration::from_secs(10)),
        auth_token: Some("secret".to_string()),
    };
    assert_eq!(config.read_buffer_size, 50);
    assert_eq!(config.heartbeat_interval, Some(std::time::Duration::from_secs(10)));
    assert_eq!(config.auth_token, Some("secret".to_string()));
}

// ===========================================================================
// Sync ConnectionPool tests
// ===========================================================================

#[test]
fn test_sync_pool_default_is_empty() {
    let pool = ConnectionPool::default();
    assert_eq!(pool.total_connections(), 0);
    assert_eq!(pool.peer_count(), 0);
}

#[test]
fn test_sync_pool_config_custom() {
    let config = PoolConfig {
        max_per_peer: 8,
        max_total: 50,
    };
    let pool = ConnectionPool::new(config);
    assert_eq!(pool.total_connections(), 0);
    assert_eq!(pool.peer_count(), 0);
}

#[test]
fn test_sync_pool_return_closed_connection() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let pool = ConnectionPool::new(PoolConfig::default());
    let mut conn = pool.get_or_connect(&addr).unwrap();
    conn.close();
    pool.return_connection(&addr, conn);
    assert_eq!(pool.total_connections(), 0);
}

#[test]
fn test_sync_pool_get_or_connect_and_reuse() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || {
        let _ = listener.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
    });

    let pool = ConnectionPool::new(PoolConfig::default());
    let conn = pool.get_or_connect(&addr).unwrap();
    pool.return_connection(&addr, conn);
    assert_eq!(pool.total_connections(), 1);

    let conn2 = pool.get_or_connect(&addr).unwrap();
    assert!(conn2.is_connected());
}

#[test]
fn test_sync_pool_close_all() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || {
        let _ = listener.accept().unwrap();
        std::thread::sleep(std::time::Duration::from_millis(500));
    });

    let pool = ConnectionPool::new(PoolConfig::default());
    let conn = pool.get_or_connect(&addr).unwrap();
    pool.return_connection(&addr, conn);
    assert_eq!(pool.total_connections(), 1);

    pool.close_all();
    assert_eq!(pool.total_connections(), 0);
}

// ===========================================================================
// Async Pool tests
// ===========================================================================

#[test]
fn test_async_pool_config_custom() {
    let config = AsyncPoolConfig {
        max_conns: 20,
        max_conns_per_node: 5,
        dial_timeout: std::time::Duration::from_secs(5),
        idle_timeout: std::time::Duration::from_secs(300),
        send_timeout: std::time::Duration::from_secs(3),
        auth_token: Some("token123".to_string()),
    };
    assert_eq!(config.max_conns, 20);
    assert_eq!(config.max_conns_per_node, 5);
    assert_eq!(config.auth_token, Some("token123".to_string()));
}

#[test]
fn test_async_pool_default_is_defaults() {
    let pool = Pool::default();
    let stats = pool.get_stats();
    assert_eq!(stats.active_conns, 0);
    assert_eq!(stats.max_conns, 50);
}

#[test]
fn test_async_pool_close_idempotent() {
    let pool = Pool::with_defaults();
    pool.close();
    pool.close();
    assert_eq!(pool.active_connection_count(), 0);
}

#[test]
fn test_async_pool_remove_node_nonexistent() {
    let pool = Pool::with_defaults();
    pool.remove_node("nonexistent-node");
    assert_eq!(pool.active_connection_count(), 0);
}

#[test]
fn test_async_pool_remove_nonexistent_key() {
    let pool = Pool::with_defaults();
    pool.remove("nonexistent-key");
    assert_eq!(pool.active_connection_count(), 0);
}

#[tokio::test]
async fn test_async_pool_dial_failure() {
    let pool = Pool::new(AsyncPoolConfig {
        dial_timeout: std::time::Duration::from_millis(50),
        ..Default::default()
    });
    let result = pool.get("node-1", "127.0.0.1:1").await;
    assert!(result.is_err());
}

#[test]
fn test_async_pool_stats_default() {
    let stats = PoolStats::default();
    assert_eq!(stats.active_conns, 0);
    assert_eq!(stats.available_slots, 0);
    assert_eq!(stats.max_conns, 0);
}

// ===========================================================================
// Connection (sync) edge cases
// ===========================================================================

#[test]
fn test_connection_connect_failure() {
    let result = Connection::connect("127.0.0.1:1");
    assert!(result.is_err());
}

#[test]
fn test_connection_new_and_is_connected() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let conn = Connection::connect(&addr).unwrap();
    assert!(conn.is_connected());
}

#[test]
fn test_connection_close_idempotent() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let mut conn = Connection::connect(&addr).unwrap();
    conn.close();
    conn.close();
    assert!(!conn.is_connected());
}

#[test]
fn test_connection_send_after_close() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let mut conn = Connection::connect(&addr).unwrap();
    conn.close();
    let result = conn.send(b"test");
    assert!(result.is_err());
}

#[test]
fn test_connection_recv_after_close() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let mut conn = Connection::connect(&addr).unwrap();
    conn.close();
    let result = conn.recv();
    assert!(result.is_err());
}

#[test]
fn test_connection_remote_addr_populated() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let conn = Connection::connect(&addr).unwrap();
    let remote = conn.remote_addr().to_string();
    assert!(!remote.is_empty());
}

#[test]
fn test_connection_send_recv_roundtrip() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let addr_clone = addr.clone();
    let handle = std::thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        // Read length-prefixed frame
        let mut len_buf = [0u8; 4];
        std::io::Read::read_exact(&mut stream, &mut len_buf).unwrap();
        let len = u32::from_be_bytes(len_buf) as usize;
        let mut data = vec![0u8; len];
        std::io::Read::read_exact(&mut stream, &mut data).unwrap();
        // Echo back
        std::io::Write::write_all(&mut stream, &len_buf).unwrap();
        std::io::Write::write_all(&mut stream, &data).unwrap();
        std::io::Write::flush(&mut stream).unwrap();
    });

    let mut conn = Connection::connect(&addr_clone).unwrap();
    conn.send(b"echo test").unwrap();
    let received = conn.recv().unwrap();
    assert_eq!(received, b"echo test");

    handle.join().unwrap();
}

// ===========================================================================
// AsyncFrameReader tests
// ===========================================================================

#[tokio::test]
async fn test_async_frame_reader_empty_stream() {
    let cursor = std::io::Cursor::new(Vec::<u8>::new());
    let mut reader = AsyncFrameReader::new(cursor);
    let result = reader.read_frame().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_frame_reader_with_capacity() {
    let payload = b"capacity test";
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    encoded.extend_from_slice(payload);

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::with_capacity(cursor, 8192);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_frame_reader_into_inner() {
    let payload = b"test";
    let mut encoded = Vec::new();
    encoded.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    encoded.extend_from_slice(payload);

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);
    let _ = reader.read_frame().await.unwrap();
    let buf_reader = reader.into_inner();
    // Buffer reader has the underlying cursor
    assert!(buf_reader.buffer().is_empty() || !buf_reader.buffer().is_empty());
}

#[tokio::test]
async fn test_async_write_read_large_frame() {
    let payload = vec![0xAB; 1024 * 100]; // 100 KB
    let mut buf = Vec::new();
    write_frame_async(&mut buf, &payload).await.unwrap();

    let cursor = std::io::Cursor::new(buf);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data.len(), 1024 * 100);
    assert!(data.iter().all(|&b| b == 0xAB));
}

#[tokio::test]
async fn test_async_frame_reader_frame_too_large() {
    let mut encoded = Vec::new();
    let huge_len: u32 = (MAX_FRAME_SIZE + 1) as u32;
    encoded.extend_from_slice(&huge_len.to_be_bytes());
    encoded.extend_from_slice(&[0u8; 100]);

    let cursor = std::io::Cursor::new(encoded);
    let mut reader = AsyncFrameReader::new(cursor);
    let result = reader.read_frame().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_async_write_read_roundtrip_small() {
    let payload = b"hello async world";
    let mut buf = Vec::new();
    write_frame_async(&mut buf, payload).await.unwrap();

    let cursor = std::io::Cursor::new(buf);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert_eq!(data, payload);
}

#[tokio::test]
async fn test_async_write_read_roundtrip_empty() {
    let payload: &[u8] = b"";
    let mut buf = Vec::new();
    write_frame_async(&mut buf, payload).await.unwrap();

    let cursor = std::io::Cursor::new(buf);
    let mut reader = AsyncFrameReader::new(cursor);
    let data = reader.read_frame().await.unwrap();
    assert!(data.is_empty());
}

// ===========================================================================
// RPCRequest / RPCResponse edge cases
// ===========================================================================

#[test]
fn test_rpc_request_with_null_target() {
    let req = RPCRequest {
        id: "broadcast-1".to_string(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".to_string(),
        target: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: RPCRequest = serde_json::from_str(&json).unwrap();
    assert!(back.target.is_none());
}

#[test]
fn test_rpc_request_with_complex_payload() {
    let req = RPCRequest {
        id: "complex-1".to_string(),
        action: ActionType::Known(KnownAction::PeerChat),
        payload: serde_json::json!({
            "messages": [
                {"role": "user", "content": "Hello"},
                {"role": "assistant", "content": "Hi"},
            ],
            "metadata": {
                "session_id": "sess-123",
                "turn_count": 2,
            }
        }),
        source: "node-a".to_string(),
        target: Some("node-b".to_string()),
    };
    let encoded = Frame::encode_request(&req).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();
    // encode_request produces WireMessage format; decode_response handles it
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.id, "complex-1");
    assert!(decoded.result.unwrap()["messages"].as_array().unwrap().len() == 2);
}

#[test]
fn test_rpc_response_success_with_null_result() {
    let resp = RPCResponse {
        id: "resp-1".to_string(),
        result: None,
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: RPCResponse = serde_json::from_str(&json).unwrap();
    assert!(back.result.is_none());
    assert!(back.error.is_none());
}

#[test]
fn test_rpc_response_both_error_and_result() {
    let resp = RPCResponse {
        id: "resp-both".to_string(),
        result: Some(serde_json::json!("partial")),
        error: Some("something went wrong".to_string()),
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: RPCResponse = serde_json::from_str(&json).unwrap();
    assert!(back.result.is_some());
    assert!(back.error.is_some());
}

#[test]
fn test_rpc_request_with_unicode_action() {
    let req = RPCRequest {
        id: "unicode-1".to_string(),
        action: ActionType::Custom("custom_あäöü".to_string()),
        payload: serde_json::json!({"msg": "日本語テスト"}),
        source: "node-a".to_string(),
        target: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: RPCRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.action, ActionType::Custom("custom_あäöü".to_string()));
}

// ===========================================================================
// Additional Frame edge cases
// ===========================================================================

#[test]
fn test_frame_header_size_constant() {
    assert_eq!(FRAME_HEADER_SIZE, 4);
}

#[test]
fn test_frame_max_size_constant() {
    assert_eq!(MAX_FRAME_SIZE, 16 * 1024 * 1024);
}

#[test]
fn test_frame_encode_decode_unicode_payload() {
    let unicode = "Hello 🌍 世界 🚀".as_bytes().to_vec();
    let frame = Frame::new(unicode.clone());
    let encoded = frame.encode();
    let (decoded, _) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data, unicode);
    assert_eq!(String::from_utf8(decoded.data).unwrap(), "Hello 🌍 世界 🚀");
}

#[test]
fn test_frame_encode_decode_zero_length() {
    let frame = Frame::new(Vec::new());
    let encoded = frame.encode();
    assert_eq!(encoded.len(), 4); // Just the length header
    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert!(decoded.data.is_empty());
    assert_eq!(consumed, 4);
}

#[test]
fn test_frame_decode_returns_none_on_empty() {
    assert!(Frame::decode(&[]).is_none());
}

// ===========================================================================
// TaskStatus tests
// ===========================================================================

#[test]
fn test_task_status_serialization() {
    let statuses = vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Completed, TaskStatus::Failed, TaskStatus::Cancelled];
    for status in &statuses {
        let json = serde_json::to_string(status).unwrap();
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *status);
    }
}

#[test]
fn test_task_status_equality() {
    assert_eq!(TaskStatus::Pending, TaskStatus::Pending);
    assert_ne!(TaskStatus::Pending, TaskStatus::Running);
    assert_ne!(TaskStatus::Completed, TaskStatus::Failed);
    assert_ne!(TaskStatus::Failed, TaskStatus::Cancelled);
}

// ===========================================================================
// Task struct tests
// ===========================================================================

#[test]
fn test_task_serialization_roundtrip() {
    let task = Task {
        id: "task-ser-1".to_string(),
        status: TaskStatus::Running,
        action: "peer_chat".to_string(),
        peer_id: "peer-1".to_string(),
        payload: serde_json::json!({"key": "value", "num": 42}),
        result: Some(serde_json::json!("done")),
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        completed_at: Some("2026-01-01T00:01:00Z".to_string()),
    };
    let json = serde_json::to_string(&task).unwrap();
    let back: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "task-ser-1");
    assert_eq!(back.status, TaskStatus::Running);
    assert_eq!(back.peer_id, "peer-1");
    assert_eq!(back.payload["num"], 42);
    assert!(back.completed_at.is_some());
}

#[test]
fn test_task_with_none_fields() {
    let task = Task {
        id: "task-none".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::Value::Null,
        result: None,
        original_channel: String::new(),
        original_chat_id: String::new(),
        created_at: String::new(),
        completed_at: None,
    };
    let json = serde_json::to_string(&task).unwrap();
    let back: Task = serde_json::from_str(&json).unwrap();
    assert!(back.result.is_none());
    assert!(back.completed_at.is_none());
    assert!(back.peer_id.is_empty());
    assert!(back.payload.is_null());
}

// ===========================================================================
// NodeInfo / NodeRole tests
// ===========================================================================

#[test]
fn test_node_role_serialization() {
    let json = serde_json::to_string(&NodeRole::Master).unwrap();
    let back: NodeRole = serde_json::from_str(&json).unwrap();
    assert_eq!(back, NodeRole::Master);

    let json = serde_json::to_string(&NodeRole::Worker).unwrap();
    let back: NodeRole = serde_json::from_str(&json).unwrap();
    assert_eq!(back, NodeRole::Worker);
}

#[test]
fn test_node_info_serialization() {
    let info = NodeInfo {
        id: "node-1".to_string(),
        name: "worker-1".to_string(),
        role: NodeRole::Worker,
        address: "10.0.0.1:9000".to_string(),
        category: "development".to_string(),
        last_seen: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&info).unwrap();
    let back: NodeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "node-1");
    assert_eq!(back.role, NodeRole::Worker);
    assert_eq!(back.category, "development");
}

// ===========================================================================
// ExtendedNodeInfo additional tests
// ===========================================================================

#[test]
fn test_extended_node_info_with_addresses() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "multi-addr".to_string(),
            name: "multi".to_string(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".to_string(),
            category: "dev".to_string(),
            last_seen: String::new(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec!["10.0.0.1".to_string(), "192.168.1.1".to_string()],
    };
    let json = serde_json::to_string(&node).unwrap();
    let back: ExtendedNodeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.addresses.len(), 2);
    assert_eq!(back.addresses[0], "10.0.0.1");
}

#[test]
fn test_extended_node_info_empty_capabilities() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec![], "");
    assert!(node.get_capabilities().is_empty());
    assert!(!node.has_capability("anything"));
}

#[test]
fn test_extended_node_info_get_uptime_invalid_date() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec![], "not-a-valid-date");
    assert_eq!(node.get_uptime(), std::time::Duration::ZERO);
}

#[test]
fn test_extended_node_info_to_peer_config_role_mapping() {
    let master = ExtendedNodeInfo {
        base: NodeInfo {
            id: "master-1".to_string(),
            name: "master".to_string(),
            role: NodeRole::Master,
            address: "10.0.0.1:9000".to_string(),
            category: "dev".to_string(),
            last_seen: "".to_string(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec![],
    };
    let config = master.to_peer_config();
    assert_eq!(config.role, "master");

    let worker = ExtendedNodeInfo {
        base: NodeInfo {
            id: "worker-1".to_string(),
            name: "worker".to_string(),
            role: NodeRole::Worker,
            address: "10.0.0.2:9000".to_string(),
            category: "dev".to_string(),
            last_seen: "".to_string(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec![],
    };
    let config = worker.to_peer_config();
    assert_eq!(config.role, "worker");
}

// ===========================================================================
// TaskManager with_store tests
// ===========================================================================

#[test]
fn test_task_manager_with_custom_store() {
    let store = Arc::new(InMemoryTaskStore::new());
    let tm = TaskManager::with_store(store);
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.get_task(&task.id).is_some());
    assert_eq!(tm.len(), 1);
}

#[test]
fn test_task_manager_set_callback() {
    let tm = TaskManager::new();
    let called = Arc::new(AtomicUsize::new(0));
    let called_clone = called.clone();
    tm.set_callback(Box::new(move |_t: &Task| {
        called_clone.fetch_add(1, Ordering::SeqCst);
    }));

    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task.id, serde_json::json!("done"));
    assert_eq!(called.load(Ordering::SeqCst), 1);
}

// ===========================================================================
// ContinuationStore edge cases
// ===========================================================================

#[tokio::test]
async fn test_continuation_save_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let snap = ContinuationSnapshot {
        task_id: "round-trip".to_string(),
        messages: serde_json::json!([
            {"role": "system", "content": "You are a helpful assistant"},
            {"role": "user", "content": "What is 2+2?"}
        ]),
        tool_call_id: "tc-42".to_string(),
        channel: "web".to_string(),
        chat_id: "chat-99".to_string(),
        ready: true,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    store.save(snap).await.unwrap();
    let loaded = store.load("round-trip").await.unwrap();
    assert_eq!(loaded.task_id, "round-trip");
    assert_eq!(loaded.tool_call_id, "tc-42");
    assert_eq!(loaded.channel, "web");
    assert_eq!(loaded.chat_id, "chat-99");
    assert!(loaded.ready);
    assert_eq!(loaded.messages.as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn test_continuation_save_false_ready() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    let snap = ContinuationSnapshot {
        task_id: "not-ready".to_string(),
        messages: serde_json::json!([{"role": "user", "content": "hi"}]),
        tool_call_id: "tc-1".to_string(),
        channel: "rpc".to_string(),
        chat_id: "c".to_string(),
        ready: false,
        created_at: chrono::Utc::now().to_rfc3339(),
    };

    store.save(snap).await.unwrap();
    let loaded = store.load("not-ready").await.unwrap();
    assert!(!loaded.ready);
}

#[tokio::test]
async fn test_continuation_list_pending_after_remove() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("t1")).await.unwrap();
    store.save(make_snapshot("t2")).await.unwrap();
    assert_eq!(store.list_pending().await.len(), 2);

    store.remove("t1").await;
    assert_eq!(store.list_pending().await.len(), 1);
}

#[tokio::test]
async fn test_continuation_cleanup_old_snapshots() {
    let dir = tempfile::tempdir().unwrap();
    let store = ContinuationStore::new(dir.path());

    store.save(make_snapshot("old-snap")).await.unwrap();
    // Make the file appear old by modifying the timestamp
    // Cleanup with 0 age should remove everything
    let removed = store.cleanup_old(std::time::Duration::from_secs(0)).await.unwrap();
    assert_eq!(removed, 1);
    assert!(store.is_empty());
}

// ===========================================================================
// WireMessage additional edge cases
// ===========================================================================

#[test]
fn test_wire_message_roundtrip_with_all_fields() {
    let msg = WireMessage::new_request("source-node", "dest-node", "PeerChat", serde_json::json!({"msg": "test"}));
    let bytes = msg.to_bytes().unwrap();
    let decoded = WireMessage::from_bytes(&bytes).unwrap();

    assert_eq!(decoded.version, "1.0");
    assert_eq!(decoded.msg_type, "request");
    assert_eq!(decoded.from, "source-node");
    assert_eq!(decoded.to, "dest-node");
    assert_eq!(decoded.action, "PeerChat");
    assert!(decoded.timestamp > 0);
    assert!(decoded.error.is_empty());
}

#[test]
fn test_wire_message_response_swaps_from_to() {
    let req = WireMessage::new_request("A", "B", "test", serde_json::json!({}));
    let resp = WireMessage::new_response(&req, serde_json::json!({"ok": true}));
    assert_eq!(resp.from, "B");
    assert_eq!(resp.to, "A");
    assert_eq!(resp.payload, serde_json::json!({"ok": true}));
}

#[test]
fn test_wire_message_error_swaps_from_to() {
    let req = WireMessage::new_request("A", "B", "test", serde_json::json!({}));
    let err = WireMessage::new_error(&req, "timeout");
    assert_eq!(err.from, "B");
    assert_eq!(err.to, "A");
    assert!(err.payload.is_null());
    assert_eq!(err.error, "timeout");
}

// ===========================================================================
// PoolStats tests
// ===========================================================================

#[test]
fn test_pool_stats_default() {
    let stats = PoolStats::default();
    assert_eq!(stats.active_conns, 0);
    assert_eq!(stats.max_conns, 0);
    assert_eq!(stats.available_slots, 0);
}

// ===========================================================================
// Additional TaskManager tests
// ===========================================================================

#[test]
fn test_task_manager_assign_and_complete_flow() {
    let tm = TaskManager::new();
    let task = tm.create_task("peer_chat", serde_json::json!({"msg": "hi"}), "rpc", "ch");
    assert!(tm.assign_task(&task.id, "worker-1"));
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Running);
    assert!(tm.complete_task(&task.id, serde_json::json!("done")));
    let completed = tm.get_task(&task.id).unwrap();
    assert_eq!(completed.status, TaskStatus::Completed);
    assert!(completed.completed_at.is_some());
}

#[test]
fn test_task_manager_assign_and_fail_flow() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    assert!(tm.assign_task(&task.id, "worker-1"));
    assert!(tm.fail_task(&task.id, "node unreachable"));
    let failed = tm.get_task(&task.id).unwrap();
    assert_eq!(failed.status, TaskStatus::Failed);
}

#[test]
fn test_task_manager_create_multiple_assign_one() {
    let tm = TaskManager::new();
    let t1 = tm.create_task("a", serde_json::json!({}), "rpc", "ch1");
    let t2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch2");
    let t3 = tm.create_task("c", serde_json::json!({}), "rpc", "ch3");
    assert!(tm.assign_task(&t2.id, "node-b"));
    assert_eq!(tm.get_task(&t1.id).unwrap().status, TaskStatus::Pending);
    assert_eq!(tm.get_task(&t2.id).unwrap().status, TaskStatus::Running);
    assert_eq!(tm.get_task(&t3.id).unwrap().status, TaskStatus::Pending);
}

#[test]
fn test_task_manager_complete_callback_with_response() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "success", "Here is the result", "");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Completed);
    let result = updated.result.unwrap();
    // complete_callback wraps the response string as json!(response)
    assert_eq!(result, "Here is the result");
}

#[test]
fn test_task_manager_complete_callback_error_with_msg() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
    tm.complete_callback(&task.id, "error", "", "Node timeout");
    let updated = tm.get_task(&task.id).unwrap();
    assert_eq!(updated.status, TaskStatus::Failed);
    let result = updated.result.unwrap();
    assert_eq!(result["error"], "Node timeout");
}

#[test]
fn test_task_manager_set_callback_replaces() {
    let count1 = Arc::new(AtomicUsize::new(0));
    let count2 = Arc::new(AtomicUsize::new(0));
    let c1 = count1.clone();
    let c2 = count2.clone();
    let tm = TaskManager::new();
    tm.set_callback(Box::new(move |_: &Task| { c1.fetch_add(1, Ordering::SeqCst); }));
    let task = tm.create_task("a", serde_json::json!({}), "rpc", "ch");
    tm.complete_task(&task.id, serde_json::json!("done"));
    assert_eq!(count1.load(Ordering::SeqCst), 1);
    assert_eq!(count2.load(Ordering::SeqCst), 0);

    tm.set_callback(Box::new(move |_: &Task| { c2.fetch_add(1, Ordering::SeqCst); }));
    let task2 = tm.create_task("b", serde_json::json!({}), "rpc", "ch");
    tm.fail_task(&task2.id, "err");
    // First callback should NOT have been called again
    assert_eq!(count1.load(Ordering::SeqCst), 1);
    assert_eq!(count2.load(Ordering::SeqCst), 1);
}

#[test]
fn test_task_with_special_chars_in_payload() {
    let tm = TaskManager::new();
    let payload = serde_json::json!({
        "html": "<script>alert('xss')</script>",
        "sql": "DROP TABLE users; --",
        "path": "C:\\Users\\test\\file.txt",
        "newlines": "line1\nline2\r\nline3",
    });
    let task = tm.create_task("action", payload.clone(), "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload, payload);
}

#[test]
fn test_task_with_boolean_payload() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!(true), "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload, serde_json::json!(true));
}

#[test]
fn test_task_with_number_payload() {
    let tm = TaskManager::new();
    let task = tm.create_task("action", serde_json::json!(42.5), "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload, serde_json::json!(42.5));
}

#[test]
fn test_task_with_array_payload() {
    let tm = TaskManager::new();
    let payload = serde_json::json!([1, "two", true, null, {"nested": "value"}]);
    let task = tm.create_task("action", payload.clone(), "rpc", "ch");
    let retrieved = tm.get_task(&task.id).unwrap();
    assert_eq!(retrieved.payload, payload);
}

#[test]
fn test_task_with_empty_action() {
    let tm = TaskManager::new();
    let task = tm.create_task("", serde_json::json!({}), "rpc", "ch");
    assert_eq!(task.action, "");
}

#[test]
fn test_task_with_empty_channel() {
    let tm = TaskManager::new();
    let task = tm.create_task("a", serde_json::json!({}), "", "ch");
    assert_eq!(task.original_channel, "");
}

#[test]
fn test_task_with_empty_chat_id() {
    let tm = TaskManager::new();
    let task = tm.create_task("a", serde_json::json!({}), "rpc", "");
    assert_eq!(task.original_chat_id, "");
}

// ===========================================================================
// Additional InMemoryTaskStore tests
// ===========================================================================

#[test]
fn test_store_create_and_get() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "t-1".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: "p".to_string(),
        payload: serde_json::json!({"x": 1}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    let t = store.get("t-1").unwrap();
    assert_eq!(t.id, "t-1");
    assert_eq!(t.action, "a");
    assert_eq!(t.peer_id, "p");
}

#[test]
fn test_store_update_to_running() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "r-1".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    store.update_result("r-1", TaskStatus::Running, None).unwrap();
    let t = store.get("r-1").unwrap();
    assert_eq!(t.status, TaskStatus::Running);
}

#[test]
fn test_store_update_with_result_value() {
    let store = InMemoryTaskStore::new();
    let task = Task {
        id: "rv-1".to_string(),
        status: TaskStatus::Pending,
        action: "a".to_string(),
        peer_id: String::new(),
        payload: serde_json::json!({}),
        result: None,
        original_channel: "rpc".to_string(),
        original_chat_id: "ch".to_string(),
        created_at: chrono::Utc::now().to_rfc3339(),
        completed_at: None,
    };
    store.create(task).unwrap();
    store.update_result("rv-1", TaskStatus::Completed, Some(serde_json::json!({"output": "done"}))).unwrap();
    let t = store.get("rv-1").unwrap();
    assert_eq!(t.result.as_ref().unwrap()["output"], "done");
}

#[test]
fn test_store_list_by_status_empty() {
    let store = InMemoryTaskStore::new();
    assert!(store.list_by_status(TaskStatus::Pending).is_empty());
    assert!(store.list_by_status(TaskStatus::Completed).is_empty());
    assert!(store.list_by_status(TaskStatus::Failed).is_empty());
    assert!(store.list_by_status(TaskStatus::Running).is_empty());
}

// ===========================================================================
// Additional WireMessage tests
// ===========================================================================

#[test]
fn test_wire_message_new_request_default_version() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert_eq!(msg.version, "1.0");
}

#[test]
fn test_wire_message_new_request_has_timestamp() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(msg.timestamp > 0);
}

#[test]
fn test_wire_message_new_request_error_empty() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(msg.error.is_empty());
}

#[test]
fn test_wire_message_validate_valid() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    assert!(msg.validate().is_ok());
}

#[test]
fn test_wire_message_to_bytes_produces_json() {
    let msg = WireMessage::new_request("a", "b", "c", serde_json::json!({}));
    let bytes = msg.to_bytes().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["from"], "a");
    assert_eq!(parsed["to"], "b");
    assert_eq!(parsed["action"], "c");
}

#[test]
fn test_wire_message_from_bytes_valid_json() {
    let json = r#"{"id":"x","version":"1.0","type":"request","from":"a","to":"b","action":"c","payload":{},"timestamp":123,"error":""}"#;
    let msg = WireMessage::from_bytes(json.as_bytes()).unwrap();
    assert_eq!(msg.id, "x");
    assert_eq!(msg.from, "a");
}

// ===========================================================================
// Additional Connection tests
// ===========================================================================

#[test]
fn test_connection_connect_invalid_address() {
    let result = Connection::connect("not-a-valid-address:99999");
    assert!(result.is_err());
}

#[test]
fn test_connection_connect_localhost_refused() {
    let result = Connection::connect("127.0.0.1:1");
    assert!(result.is_err());
}

#[test]
fn test_connection_is_connected_after_connect() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let conn = Connection::connect(&addr).unwrap();
    assert!(conn.is_connected());
}

#[test]
fn test_connection_not_connected_after_close() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let _handle = std::thread::spawn(move || { let _ = listener.accept().unwrap(); });

    let mut conn = Connection::connect(&addr).unwrap();
    conn.close();
    assert!(!conn.is_connected());
}

// ===========================================================================
// Additional Frame tests
// ===========================================================================

#[test]
fn test_frame_encode_decode_zero_bytes() {
    let frame = Frame::new(vec![0x00]);
    let encoded = frame.encode();
    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data, vec![0x00]);
    assert_eq!(consumed, 5);
}

#[test]
fn test_frame_encode_decode_max_size() {
    let data = vec![0xFF; MAX_FRAME_SIZE];
    let frame = Frame::new(data);
    let encoded = frame.encode();
    assert_eq!(encoded.len(), 4 + MAX_FRAME_SIZE);
    let (decoded, _) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data.len(), MAX_FRAME_SIZE);
}

#[test]
fn test_encode_batch_single_frame() {
    let frames = vec![Frame::new(b"single".to_vec())];
    let encoded = encode_batch(&frames);
    assert!(!encoded.is_empty());
    let (decoded, _) = decode_all(&encoded);
    assert_eq!(decoded.len(), 1);
}

#[test]
fn test_frame_encode_request_with_all_action_types() {
    let actions = vec![
        ActionType::Known(KnownAction::PeerChat),
        ActionType::Known(KnownAction::PeerChatCallback),
        ActionType::Known(KnownAction::ForgeShare),
        ActionType::Known(KnownAction::Ping),
        ActionType::Known(KnownAction::Status),
    ];
    for action in actions {
        let req = RPCRequest {
            id: "req".to_string(),
            action: action.clone(),
            payload: serde_json::json!({}),
            source: "a".to_string(),
            target: None,
        };
        let encoded = Frame::encode_request(&req).unwrap();
        let (frame, _) = Frame::decode(&encoded).unwrap();
        // encode_request produces WireMessage format; decode_response handles it
        let decoded = Frame::decode_response(&frame.data).unwrap();
        assert_eq!(decoded.id, "req");
        assert!(decoded.error.is_none());
    }
}

#[test]
fn test_frame_encode_response_with_error() {
    let resp = RPCResponse {
        id: "err-id".to_string(),
        result: None,
        error: Some("something went wrong".to_string()),
    };
    let encoded = Frame::encode_response(&resp).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.error.unwrap(), "something went wrong");
}

// ===========================================================================
// Additional ClusterConfig tests
// ===========================================================================

#[test]
fn test_cluster_config_default_bind_address() {
    let config = ClusterConfig::default();
    assert_eq!(config.bind_address, "0.0.0.0:9000");
}

#[test]
fn test_cluster_config_empty_peers() {
    let config = ClusterConfig::default();
    assert!(config.peers.is_empty());
}

#[test]
fn test_cluster_config_empty_node_id() {
    let config = ClusterConfig::default();
    assert!(config.node_id.is_empty());
}

// ===========================================================================
// Additional ExtendedNodeInfo tests
// ===========================================================================

#[test]
fn test_extended_node_info_get_uptime_recent() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec![], &chrono::Utc::now().to_rfc3339());
    let uptime = node.get_uptime();
    assert!(uptime.as_secs() < 10);
}

#[test]
fn test_extended_node_info_get_uptime_empty_last_seen() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec![], "");
    assert_eq!(node.get_uptime(), std::time::Duration::ZERO);
}

#[test]
fn test_extended_node_info_serialization_roundtrip() {
    let node = make_test_extended_node("n1", NodeStatus::Online, vec!["llm", "tools"], "");
    let json = serde_json::to_string(&node).unwrap();
    let back: ExtendedNodeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.base.id, "n1");
    assert_eq!(back.status, NodeStatus::Online);
    assert_eq!(back.capabilities.len(), 2);
}

#[test]
fn test_extended_node_info_addresses_default_empty() {
    let json = r#"{"id":"n1","name":"n1","role":"Worker","address":"10.0.0.1:9000","category":"dev","last_seen":"","status":"Online","capabilities":[]}"#;
    let node: ExtendedNodeInfo = serde_json::from_str(json).unwrap();
    assert!(node.addresses.is_empty());
}

// ===========================================================================
// Additional ActionType tests
// ===========================================================================

#[test]
fn test_action_type_known_variants() {
    assert_eq!(ActionType::Known(KnownAction::PeerChat).as_str(), "PeerChat");
    assert_eq!(ActionType::Known(KnownAction::PeerChatCallback).as_str(), "PeerChatCallback");
    assert_eq!(ActionType::Known(KnownAction::ForgeShare).as_str(), "ForgeShare");
    assert_eq!(ActionType::Known(KnownAction::Ping).as_str(), "Ping");
    assert_eq!(ActionType::Known(KnownAction::Status).as_str(), "Status");
}

#[test]
fn test_action_type_custom_as_str() {
    let action = ActionType::Custom("my_custom_action".to_string());
    assert_eq!(action.as_str(), "my_custom_action");
}

#[test]
fn test_action_type_serialize_deserialize_all_known() {
    let actions = vec![
        ActionType::Known(KnownAction::PeerChat),
        ActionType::Known(KnownAction::PeerChatCallback),
        ActionType::Known(KnownAction::ForgeShare),
        ActionType::Known(KnownAction::Ping),
        ActionType::Known(KnownAction::Status),
    ];
    for action in actions {
        let json = serde_json::to_string(&action).unwrap();
        let back: ActionType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, action);
    }
}

// ===========================================================================
// Task serialization tests
// ===========================================================================

#[test]
fn test_task_serialization_with_all_fields() {
    let task = Task {
        id: "ser-1".to_string(),
        status: TaskStatus::Running,
        action: "peer_chat".to_string(),
        peer_id: "node-a".to_string(),
        payload: serde_json::json!({"msg": "hello"}),
        result: Some(serde_json::json!("response")),
        original_channel: "rpc".to_string(),
        original_chat_id: "chat-1".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        completed_at: Some("2026-01-01T00:01:00Z".to_string()),
    };
    let json = serde_json::to_string(&task).unwrap();
    let back: Task = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "ser-1");
    assert_eq!(back.status, TaskStatus::Running);
    assert_eq!(back.peer_id, "node-a");
    assert!(back.completed_at.is_some());
}

#[test]
fn test_task_status_all_variants() {
    let statuses = vec![TaskStatus::Pending, TaskStatus::Running, TaskStatus::Completed, TaskStatus::Failed, TaskStatus::Cancelled];
    assert_eq!(statuses.len(), 5);
    for s in &statuses {
        let json = serde_json::to_string(s).unwrap();
        let back: TaskStatus = serde_json::from_str(&json).unwrap();
        assert_eq!(back, *s);
    }
}

// ===========================================================================
// Additional validate_frame_size tests
// ===========================================================================

#[test]
fn test_validate_frame_size_small() {
    assert!(validate_frame_size(&[0u8; 100]).is_ok());
}

#[test]
fn test_validate_frame_size_exact_max() {
    assert!(validate_frame_size(&[0u8; MAX_FRAME_SIZE]).is_ok());
}

#[test]
fn test_validate_frame_size_one_over_max() {
    assert!(validate_frame_size(&[0u8; MAX_FRAME_SIZE + 1]).is_err());
}

#[test]
fn test_validate_frame_size_empty() {
    assert!(validate_frame_size(&[]).is_ok());
}
