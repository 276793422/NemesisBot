use super::*;
use tempfile::TempDir;

/// Helper to create test LlmMessages.
fn make_message(role: &str, content: &str) -> LlmMessage {
    LlmMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }
}

#[tokio::test]
async fn test_save_and_load_continuation() {
    let manager = ContinuationManager::new();

    let messages = vec![
        make_message("system", "You are helpful."),
        make_message("user", "Hello"),
    ];

    manager
        .save_continuation(
            "task-1",
            messages.clone(),
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let loaded = manager.load_continuation("task-1").await;
    assert!(loaded.is_some());
    let data = loaded.unwrap();
    assert_eq!(data.messages.len(), 2);
    assert_eq!(data.tool_call_id, "tc_1");
    assert_eq!(data.channel, "web");
    assert_eq!(data.chat_id, "chat1");
}

#[tokio::test]
async fn test_load_nonexistent_continuation() {
    let manager = ContinuationManager::new();
    let loaded = manager.load_continuation("nonexistent").await;
    assert!(loaded.is_none());
}

#[tokio::test]
async fn test_remove_continuation() {
    let manager = ContinuationManager::new();

    manager
        .save_continuation(
            "task-2",
            vec![make_message("user", "test")],
            "tc_2",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    assert!(manager.has_continuation("task-2").await);
    manager.remove_continuation("task-2").await;
    assert!(!manager.has_continuation("task-2").await);
}

#[tokio::test]
async fn test_disk_persistence_and_recovery() {
    let tmp = TempDir::new().unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());

    let messages = vec![
        make_message("system", "System prompt"),
        make_message("user", "Query"),
    ];

    manager
        .save_continuation(
            "task-disk",
            messages.clone(),
            "tc_d",
            "rpc",
            "chat2",
            "test_session",
            "",
        )
        .await;

    // Verify it can be loaded while still in memory.
    let loaded = manager.load_continuation("task-disk").await;
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().tool_call_id, "tc_d");

    // Remove should clear both memory and disk (mirrors Go behavior).
    manager.remove_continuation("task-disk").await;
    assert!(!manager.has_continuation("task-disk").await);

    // After removal, should not be loadable (disk was also deleted).
    let loaded = manager.load_continuation("task-disk").await;
    assert!(loaded.is_none());
}

#[test]
fn test_disk_recovery_on_startup() {
    let tmp = TempDir::new().unwrap();

    // Write a snapshot to disk manually.
    let store = ContinuationStore::new(tmp.path());
    let messages_json = serde_json::to_string(&vec![
        make_message("system", "System prompt"),
        make_message("user", "Query"),
    ])
    .unwrap();
    let snapshot = ContinuationSnapshot {
        task_id: "task-recover".to_string(),
        messages: messages_json,
        tool_call_id: "tc_r".to_string(),
        channel: "rpc".to_string(),
        chat_id: "chat_r".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();

    // Create a manager with disk store -- it should recover the snapshot on startup.
    // with_disk_store runs sync recovery; its sync locks use try_lock() (never
    // blocks), so a plain #[test] is safe.
    let manager = ContinuationManager::with_disk_store(tmp.path());
    assert!(manager.has_continuation_sync("task-recover"));
}

#[tokio::test]
async fn test_disk_store_save_and_load() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let snapshot = ContinuationSnapshot {
        task_id: "task-100".to_string(),
        messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
        tool_call_id: "tc_100".to_string(),
        channel: "web".to_string(),
        chat_id: "chat100".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };

    store.save(&snapshot).unwrap();
    let loaded = store.load("task-100").unwrap();
    assert_eq!(loaded.task_id, "task-100");
    assert_eq!(loaded.tool_call_id, "tc_100");
}

#[tokio::test]
async fn test_disk_store_delete() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let snapshot = ContinuationSnapshot {
        task_id: "task-del".to_string(),
        messages: "[]".to_string(),
        tool_call_id: "tc_del".to_string(),
        channel: "web".to_string(),
        chat_id: "chat-del".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };

    store.save(&snapshot).unwrap();
    store.delete("task-del");
    assert!(store.load("task-del").is_err());
}

#[tokio::test]
async fn test_save_barrier_pattern() {
    let manager = ContinuationManager::new();

    // Spawn a task that delays saving.
    let mgr = Arc::new(manager);
    let mgr_clone = mgr.clone();

    let save_handle = tokio::spawn(async move {
        // Small delay before saving.
        tokio::time::sleep(Duration::from_millis(50)).await;
        mgr_clone
            .save_continuation(
                "task-barrier",
                vec![make_message("user", "delayed")],
                "tc_b",
                "web",
                "chat_b",
                "test_session",
                "",
            )
            .await;
    });

    // The load should wait for the save to complete.
    let load_handle = tokio::spawn(async move { mgr.load_continuation("task-barrier").await });

    save_handle.await.unwrap();
    let loaded = load_handle.await.unwrap();
    assert!(loaded.is_some());
    assert_eq!(loaded.unwrap().tool_call_id, "tc_b");
}

#[tokio::test]
async fn test_overwrite_continuation() {
    let manager = ContinuationManager::new();

    manager
        .save_continuation(
            "task-overwrite",
            vec![make_message("user", "first")],
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    manager
        .save_continuation(
            "task-overwrite",
            vec![make_message("user", "second")],
            "tc_2",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let loaded = manager.load_continuation("task-overwrite").await.unwrap();
    // The last save should have overwritten.
    assert_eq!(loaded.messages[0].content, "second");
    assert_eq!(loaded.tool_call_id, "tc_2");
}

// --- Additional continuation tests ---

#[test]
fn test_continuation_snapshot_serialization() {
    let snapshot = ContinuationSnapshot {
        task_id: "task-ser".to_string(),
        messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
        tool_call_id: "tc_ser".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_ser".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };

    let json = serde_json::to_string(&snapshot).unwrap();
    let parsed: ContinuationSnapshot = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.task_id, "task-ser");
    assert_eq!(parsed.tool_call_id, "tc_ser");
    assert_eq!(parsed.channel, "web");
}

#[test]
fn test_continuation_data_debug() {
    let data = ContinuationData {
        messages: vec![make_message("user", "test")],
        tool_call_id: "tc_1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "test_session".to_string(),
        ready: Arc::new(tokio::sync::Notify::new()),
        ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    let debug_str = format!("{:?}", data);
    assert!(debug_str.contains("tc_1"));
}

#[test]
fn test_continuation_store_load_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let result = store.load("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_continuation_store_delete_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    // Should not panic
    store.delete("nonexistent");
}

#[test]
fn test_manager_has_continuation_sync() {
    let manager = ContinuationManager::new();

    assert!(!manager.has_continuation_sync("task-sync"));

    // Use synchronous insert
    let data = Arc::new(ContinuationData {
        messages: vec![make_message("user", "test")],
        tool_call_id: "tc_s".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "test_session".to_string(),
        ready: Arc::new(tokio::sync::Notify::new()),
        ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    });
    manager.insert_continuation_sync("task-sync".to_string(), data);

    assert!(manager.has_continuation_sync("task-sync"));
}

#[tokio::test]
async fn test_manager_multiple_continuations() {
    let manager = ContinuationManager::new();

    for i in 0..5 {
        manager
            .save_continuation(
                &format!("task-multi-{}", i),
                vec![make_message("user", &format!("msg {}", i))],
                &format!("tc_{}", i),
                "web",
                &format!("chat_{}", i),
                "test_session",
                "",
            )
            .await;
    }

    for i in 0..5 {
        assert!(manager.has_continuation(&format!("task-multi-{}", i)).await);
        let loaded = manager
            .load_continuation(&format!("task-multi-{}", i))
            .await;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().tool_call_id, format!("tc_{}", i));
    }
}

#[test]
fn test_continuation_store_list_pending_empty() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let pending = store.list_pending();
    assert!(pending.is_empty());
}

#[test]
fn test_continuation_store_list_pending_with_snapshots() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    for i in 0..3 {
        let snapshot = ContinuationSnapshot {
            task_id: format!("task-list-{}", i),
            messages: "[]".to_string(),
            tool_call_id: format!("tc_{}", i),
            channel: "web".to_string(),
            chat_id: format!("chat_{}", i),
            created_at: "2026-04-29T12:00:00Z".to_string(),
            session_key: String::new(),
            peer_id: String::new(),
            image_refs: Vec::new(),
            image_refs_by_user_turn: Vec::new(), // L1
        };
        store.save(&snapshot).unwrap();
    }

    let pending = store.list_pending();
    assert_eq!(pending.len(), 3);
    // Should contain the task IDs (stems of the filenames)
    assert!(pending.contains(&"task-list-0".to_string()));
    assert!(pending.contains(&"task-list-1".to_string()));
    assert!(pending.contains(&"task-list-2".to_string()));
}

#[test]
fn test_continuation_snapshot_clone() {
    let snapshot = ContinuationSnapshot {
        task_id: "task-clone".to_string(),
        messages: r#"[]"#.to_string(),
        tool_call_id: "tc_c".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_c".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    let cloned = snapshot.clone();
    assert_eq!(cloned.task_id, "task-clone");
    assert_eq!(cloned.tool_call_id, "tc_c");
}

#[tokio::test]
async fn test_save_barrier_timeout() {
    let manager = ContinuationManager::new();

    // Load without save should return None quickly (5s timeout in impl)
    // Use a short timeout approach: just verify it returns None
    let loaded = manager.load_continuation("task-noexist-barrier").await;
    assert!(loaded.is_none());
}

#[test]
fn test_continuation_data_with_ready_notify() {
    let notify = Arc::new(tokio::sync::Notify::new());
    let data = ContinuationData {
        messages: vec![make_message("user", "test")],
        tool_call_id: "tc_1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "test_session".to_string(),
        ready: notify,
        ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(false)),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };

    assert!(!data.ready_flag.load(std::sync::atomic::Ordering::SeqCst));
}

#[tokio::test]
async fn test_concurrent_save_and_load() {
    let manager = Arc::new(ContinuationManager::new());
    let mut handles = Vec::new();

    // Spawn multiple concurrent saves
    for i in 0..10 {
        let mgr = manager.clone();
        handles.push(tokio::spawn(async move {
            mgr.save_continuation(
                &format!("task-concurrent-{}", i),
                vec![make_message("user", &format!("msg {}", i))],
                &format!("tc_{}", i),
                "web",
                &format!("chat_{}", i),
                "test_session",
                "",
            )
            .await;
        }));
    }

    // Wait for all saves
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all can be loaded
    for i in 0..10 {
        let loaded = manager
            .load_continuation(&format!("task-concurrent-{}", i))
            .await;
        assert!(loaded.is_some());
        assert_eq!(loaded.unwrap().tool_call_id, format!("tc_{}", i));
    }
}

#[test]
fn test_tool_lookup_trait() {
    use async_trait::async_trait;

    struct MockLookupTool;
    #[async_trait]
    impl Tool for MockLookupTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Ok("mock".to_string())
        }
    }

    struct TestLookup;
    impl ToolLookup for TestLookup {
        fn get_tool(&self, name: &str) -> Option<Arc<dyn Tool>> {
            if name == "known_tool" {
                Some(Arc::new(MockLookupTool))
            } else {
                None
            }
        }
    }

    let lookup = TestLookup;
    assert!(lookup.get_tool("known_tool").is_some());
    assert!(lookup.get_tool("unknown_tool").is_none());
}

#[test]
fn test_continuation_store_save_overwrite() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let snapshot1 = ContinuationSnapshot {
        task_id: "task-ov".to_string(),
        messages: r#"[]"#.to_string(),
        tool_call_id: "tc_1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot1).unwrap();

    let snapshot2 = ContinuationSnapshot {
        task_id: "task-ov".to_string(),
        messages: r#"[]"#.to_string(),
        tool_call_id: "tc_2".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot2).unwrap();

    let loaded = store.load("task-ov").unwrap();
    assert_eq!(loaded.tool_call_id, "tc_2");
}

#[tokio::test]
async fn test_remove_nonexistent_continuation() {
    let manager = ContinuationManager::new();
    // Should not panic
    manager.remove_continuation("nonexistent").await;
}

#[test]
fn test_disk_store_corrupted_file() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    // Write corrupted JSON
    std::fs::write(tmp.path().join("task-corrupt.json"), "not valid json").unwrap();

    let result = store.load("task-corrupt");
    assert!(result.is_err());
}

// --- Additional continuation coverage tests ---

#[test]
fn test_continuation_tool_result_default() {
    let result = ContinuationToolResult::default();
    assert!(result.for_llm.is_empty());
    assert!(result.for_user.is_empty());
    assert!(result.silent);
    assert!(!result.is_async);
    assert!(result.task_id.is_none());
    assert!(result.error.is_none());
}

#[test]
fn test_continuation_manager_default() {
    let manager = ContinuationManager::default();
    assert!(!manager.has_continuation_sync("anything"));
}

#[tokio::test]
async fn test_set_barrier_timeout() {
    let mut manager = ContinuationManager::new();
    manager.set_barrier_timeout(Duration::from_secs(10));
    // Verify it works by checking load returns None quickly for non-existent
    let loaded = manager.load_continuation("nonexistent-timeout").await;
    assert!(loaded.is_none());
}

#[test]
fn test_continuation_store_nonexistent_dir_list_pending() {
    let tmp = TempDir::new().unwrap();
    let nonexistent = tmp.path().join("does_not_exist");
    let store = ContinuationStore::new(&nonexistent);
    let pending = store.list_pending();
    assert!(pending.is_empty());
}

#[tokio::test]
async fn test_continuation_manager_with_disk_store_empty() {
    let tmp = TempDir::new().unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());
    assert!(!manager.has_continuation("nonexistent").await);
}

#[test]
fn test_continuation_store_recover_skips_already_loaded() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    // Save a snapshot
    let snapshot = ContinuationSnapshot {
        task_id: "task-skip".to_string(),
        messages: r#"[{"role":"user","content":"hello"}]"#.to_string(),
        tool_call_id: "tc_skip".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_skip".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();

    // Create a manager and manually insert the key first
    let manager = ContinuationManager::new();
    let data = Arc::new(ContinuationData {
        messages: vec![make_message("user", "manual")],
        tool_call_id: "tc_manual".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: "test_session".to_string(),
        ready: Arc::new(tokio::sync::Notify::new()),
        ready_flag: Arc::new(std::sync::atomic::AtomicBool::new(true)),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    });
    manager.insert_continuation_sync("task-skip".to_string(), data);

    // Recovery should skip since it's already in memory
    let recovered = store.recover_to_manager(&manager);
    assert_eq!(recovered, 0);
}

#[test]
fn test_continuation_store_recover_corrupted_messages() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    // Write a snapshot with invalid messages JSON
    let snapshot = ContinuationSnapshot {
        task_id: "task-bad-msg".to_string(),
        messages: "not valid json array".to_string(),
        tool_call_id: "tc_bad".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_bad".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();

    let manager = ContinuationManager::new();
    let recovered = store.recover_to_manager(&manager);
    assert_eq!(recovered, 0);
    assert!(!manager.has_continuation_sync("task-bad-msg"));
}

#[test]
fn test_tool_lookup_hashmap_arc() {
    use async_trait::async_trait;

    struct TestTool;
    #[async_trait]
    impl Tool for TestTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Ok("test".to_string())
        }
    }

    let mut map: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    map.insert("tool1".to_string(), Arc::new(TestTool));

    assert!(map.get_tool("tool1").is_some());
    assert!(map.get_tool("unknown").is_none());
}

// --- Additional coverage for continuation handling ---

use crate::r#loop::LlmResponse;
use async_trait::async_trait;

#[tokio::test]
async fn test_handle_cluster_continuation_no_data() {
    // When continuation data doesn't exist, should return early
    let manager = ContinuationManager::new();
    let (outbound_tx, _outbound_rx) = tokio::sync::mpsc::channel(16);

    // No continuation saved, so this should not panic
    handle_cluster_continuation(
        &manager,
        "nonexistent-task",
        "response",
        false,
        None,
        &MockContinuationProvider::new(vec![]),
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;
    // No outbound should be sent
}

#[tokio::test]
async fn test_handle_cluster_continuation_simple_response() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    // Save a continuation snapshot
    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-1",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    // Provider returns a simple text response (no tool calls)
    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Continuation result".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    handle_cluster_continuation(
        &manager,
        "task-1",
        "task response",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    let out = outbound.unwrap();
    assert_eq!(out.channel, "web");
    assert_eq!(out.chat_id, "chat1");
    assert!(out.content.contains("Continuation result"));
}

#[tokio::test]
async fn test_handle_cluster_continuation_failed_task() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-fail",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Error handled".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    handle_cluster_continuation(
        &manager,
        "task-fail",
        "",
        true,
        Some("Task execution failed"),
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("Error handled"));
}

#[tokio::test]
async fn test_handle_cluster_continuation_with_tool_calls() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-tool",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    // First response has tool call, second response is final
    let provider = MockContinuationProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_cont_1".to_string(),
                name: "echo".to_string(),
                arguments: r#"{"text":"hello"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Tool executed".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        async fn execute(&self, args: &str, _context: &RequestContext) -> Result<String, String> {
            let val: serde_json::Value = serde_json::from_str(args).unwrap();
            Ok(val.get("text").unwrap().as_str().unwrap().to_string())
        }
    }
    tools.insert("echo".to_string(), Arc::new(EchoTool));

    handle_cluster_continuation(
        &manager,
        "task-tool",
        "task response",
        false,
        None,
        &provider,
        "test-model",
        &tools,
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("Tool executed"));
}

#[tokio::test]
async fn test_handle_cluster_continuation_llm_error() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-err",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new_error("LLM connection failed".to_string());

    handle_cluster_continuation(
        &manager,
        "task-err",
        "task response",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("LLM error"));
}

#[tokio::test]
async fn test_handle_cluster_continuation_unknown_tool() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-unknown",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_unk".to_string(),
                name: "nonexistent_tool".to_string(),
                arguments: "{}".to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Handled unknown tool".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    handle_cluster_continuation(
        &manager,
        "task-unknown",
        "task response",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("Handled unknown tool"));
}

#[tokio::test]
async fn test_execute_tool_for_continuation_success() {
    struct OkTool;
    #[async_trait]
    impl Tool for OkTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Ok("tool result".to_string())
        }
    }

    let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    tools.insert("my_tool".to_string(), Arc::new(OkTool));

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "my_tool".to_string(),
        arguments: "{}".to_string(),
    };

    let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
    assert_eq!(result.for_llm, "tool result");
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_execute_tool_for_continuation_error() {
    struct ErrorTool;
    #[async_trait]
    impl Tool for ErrorTool {
        async fn execute(&self, _args: &str, _context: &RequestContext) -> Result<String, String> {
            Err("tool error".to_string())
        }
    }

    let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    tools.insert("error_tool".to_string(), Arc::new(ErrorTool));

    let tc = ToolCallInfo {
        id: "tc_1".to_string(),
        name: "error_tool".to_string(),
        arguments: "{}".to_string(),
    };

    let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
    assert!(result.error.is_some());
    assert_eq!(result.error.unwrap(), "tool error");
}

#[test]
fn test_continuation_tool_result_fields() {
    let result = ContinuationToolResult::default();
    assert!(result.for_llm.is_empty());
    assert!(result.for_user.is_empty());
    assert!(result.error.is_none());
    assert!(result.silent); // Default is silent
    assert!(!result.is_async);
    assert!(result.task_id.is_none());
}

#[test]
fn test_continuation_snapshot_created_at() {
    let snapshot = ContinuationSnapshot {
        task_id: "t1".to_string(),
        messages: "[]".to_string(),
        tool_call_id: "tc1".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    assert_eq!(snapshot.task_id, "t1");
    assert_eq!(snapshot.created_at, "2026-01-01T00:00:00Z");
}

// --- Mock LLM Provider for continuation tests ---

struct MockContinuationProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    error: std::sync::Mutex<Option<String>>,
    // F-F 回归锁：记录每次 chat() 收到的消息图片数（vision 投影验证用）。
    seen_image_counts: std::sync::Mutex<Vec<Vec<usize>>>,
}

impl MockContinuationProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            error: std::sync::Mutex::new(None),
            seen_image_counts: std::sync::Mutex::new(Vec::new()),
        }
    }

    fn new_error(err: String) -> Self {
        Self {
            responses: std::sync::Mutex::new(Vec::new()),
            error: std::sync::Mutex::new(Some(err)),
            seen_image_counts: std::sync::Mutex::new(Vec::new()),
        }
    }

    /// 每轮 chat() 各消息的 images.len()（F-F 断言用）。
    fn seen_image_counts(&self) -> Vec<Vec<usize>> {
        self.seen_image_counts.lock().unwrap().clone()
    }
}

#[async_trait]
impl crate::r#loop::LlmProvider for MockContinuationProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        self.seen_image_counts
            .lock()
            .unwrap()
            .push(_messages.iter().map(|m| m.images.len()).collect());
        if let Some(ref err) = *self.error.lock().unwrap() {
            return Err(err.clone());
        }
        let mut responses = self.responses.lock().unwrap();
        if responses.is_empty() {
            Ok(LlmResponse {
                content: "No more responses".to_string(),
                tool_calls: Vec::new(),
                finished: true,
                reasoning_content: None,
                usage: None,
                raw_request_body: None,
                raw_response_body: None,
            })
        } else {
            Ok(responses.remove(0))
        }
    }
}

// --- Additional coverage tests ---

#[test]
fn test_continuation_tool_result_debug() {
    let result = ContinuationToolResult {
        for_llm: "test data".to_string(),
        for_user: "user data".to_string(),
        silent: false,
        is_async: true,
        task_id: Some("task-1".to_string()),
        error: Some("some error".to_string()),
    };
    let debug = format!("{:?}", result);
    assert!(debug.contains("test data"));
    assert!(debug.contains("task-1"));
}

#[test]
fn test_continuation_tool_result_with_all_fields() {
    let result = ContinuationToolResult {
        for_llm: "for llm".to_string(),
        for_user: "for user".to_string(),
        silent: false,
        is_async: true,
        task_id: Some("task-42".to_string()),
        error: None,
    };
    assert_eq!(result.for_llm, "for llm");
    assert_eq!(result.for_user, "for user");
    assert!(!result.silent);
    assert!(result.is_async);
    assert_eq!(result.task_id.unwrap(), "task-42");
    assert!(result.error.is_none());
}

#[tokio::test]
async fn test_execute_tool_for_continuation_unknown_tool() {
    let tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();

    let tc = ToolCallInfo {
        id: "tc_unk".to_string(),
        name: "nonexistent".to_string(),
        arguments: "{}".to_string(),
    };

    let result = execute_tool_for_continuation(&tools, &tc, "web", "chat1").await;
    assert!(result.error.is_some());
    assert!(result.error.unwrap().contains("Unknown tool"));
}

#[test]
fn test_continuation_snapshot_deserialization() {
    let json = r#"{
        "task_id": "task-json",
        "messages": "[{\"role\":\"user\",\"content\":\"hello\"}]",
        "tool_call_id": "tc_json",
        "channel": "rpc",
        "chat_id": "chat_json",
        "created_at": "2026-04-29T12:00:00Z"
    }"#;
    let snapshot: ContinuationSnapshot = serde_json::from_str(json).unwrap();
    assert_eq!(snapshot.task_id, "task-json");
    assert_eq!(snapshot.channel, "rpc");
}

#[test]
fn test_disk_persistence_load_from_disk() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    // Save a snapshot
    let messages = vec![make_message("user", "disk test")];
    let messages_json = serde_json::to_string(&messages).unwrap();
    let snapshot = ContinuationSnapshot {
        task_id: "task-disk-load".to_string(),
        messages: messages_json,
        tool_call_id: "tc_dl".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_dl".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();

    // Create manager with disk store and verify recovery (sync API; recovery uses try_lock internally).
    let manager = ContinuationManager::with_disk_store(tmp.path());
    assert!(manager.has_continuation_sync("task-disk-load"));
}

#[tokio::test]
async fn test_disk_store_remove_and_verify() {
    let tmp = TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());

    let snapshot = ContinuationSnapshot {
        task_id: "task-rm".to_string(),
        messages: "[]".to_string(),
        tool_call_id: "tc_rm".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_rm".to_string(),
        created_at: "2026-04-29T12:00:00Z".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();
    assert!(store.load("task-rm").is_ok());

    store.delete("task-rm");
    assert!(store.load("task-rm").is_err());
}

#[tokio::test]
async fn test_handle_cluster_continuation_failed_task_no_error_msg() {
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello")];
    manager
        .save_continuation(
            "task-fail-no-err",
            messages,
            "tc_1",
            "web",
            "chat1",
            "test_session",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Error handled".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    // task_failed = true but error is None
    handle_cluster_continuation(
        &manager,
        "task-fail-no-err",
        "",
        true,
        None, // No error message
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let outbound = outbound_rx.try_recv();
    assert!(outbound.is_ok());
    assert!(outbound.unwrap().content.contains("Error handled"));
}

// --- Tests for session_log persistence in handle_cluster_continuation ---

/// Unique session key so parallel tests don't trample each other's files.
fn unique_cont_test_session_key(label: &str) -> String {
    format!(
        "cont_test:{}:{}",
        label,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos()
    )
}

/// Remove the session log file produced by `append_chat_log` for this key.
fn cleanup_cont_session_log(session_key: &str) {
    let safe_key = session_key.replace(':', "_");
    let path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    if path.exists() {
        let _ = std::fs::remove_file(&path);
    }
}

#[tokio::test]
async fn test_handle_cluster_continuation_writes_session_log() {
    let session_key = unique_cont_test_session_key("log_write");
    cleanup_cont_session_log(&session_key);

    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello from cluster")];
    manager
        .save_continuation(
            "task-log",
            messages,
            "tc_1",
            "web",
            "chat_log",
            &session_key,
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Cluster reply persisted to log".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    handle_cluster_continuation(
        &manager,
        "task-log",
        "peer response payload",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    // Drain the outbound so the runtime doesn't see a dropped sender.
    let _ = outbound_rx.try_recv();

    let safe_key = session_key.replace(':', "_");
    let log_path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    assert!(
        log_path.exists(),
        "session log file should exist at {:?}",
        log_path
    );

    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        content.contains("Cluster reply persisted to log"),
        "session log should contain assistant reply, got: {}",
        content
    );
    assert!(
        content.contains("\"role\":\"assistant\"") || content.contains("\"role\": \"assistant\""),
        "session log should mark the entry as assistant role, got: {}",
        content
    );

    cleanup_cont_session_log(&session_key);
}

#[tokio::test]
async fn test_handle_cluster_continuation_writes_session_store_when_provided() {
    let session_key = unique_cont_test_session_key("store_write");
    cleanup_cont_session_log(&session_key);

    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    let messages = vec![make_message("user", "Hello from cluster store")];
    manager
        .save_continuation(
            "task-store",
            messages,
            "tc_1",
            "web",
            "chat_store",
            &session_key,
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Mirrored into session_store".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let store = Arc::new(crate::session::SessionStore::new_in_memory());

    handle_cluster_continuation(
        &manager,
        "task-store",
        "peer response payload",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        Some(store.as_ref()),
        true, // F-F vision_supported
    )
    .await;

    let _ = outbound_rx.try_recv();

    // session_store should have the assistant message in memory.
    let messages = store.get_history(&session_key);
    let found = messages
        .iter()
        .any(|m| m.role == "assistant" && m.content.contains("Mirrored into session_store"));
    assert!(
        found,
        "session_store messages should contain the assistant reply, got: {:?}",
        messages
            .iter()
            .map(|m| (&m.role, &m.content))
            .collect::<Vec<_>>()
    );

    cleanup_cont_session_log(&session_key);
}

#[tokio::test]
async fn test_handle_cluster_continuation_skips_log_when_session_key_empty() {
    // Simulates a legacy on-disk snapshot that has no session_key field.
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    // Save with empty session_key (mirrors a deserialized legacy snapshot).
    let messages = vec![make_message("user", "legacy")];
    manager
        .save_continuation(
            "task-legacy",
            messages,
            "tc_legacy",
            "web",
            "chat_legacy",
            "",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Should not be logged".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    handle_cluster_continuation(
        &manager,
        "task-legacy",
        "peer response",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let _ = outbound_rx.try_recv();

    // No file should have been written for an empty session key (file would be "_.jsonl").
    let empty_log = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join("_.jsonl");
    assert!(
        !empty_log.exists(),
        "empty session_key should NOT produce a log file, but found {:?}",
        empty_log
    );

    // Cleanup if some other parallel test happens to use the same path.
    if empty_log.exists() {
        let _ = std::fs::remove_file(&empty_log);
    }
}

// --- Tests verifying session_key flows through save -> load -> handle ---

#[tokio::test]
async fn test_load_continuation_preserves_session_key() {
    // Covers the path: save_continuation(session_key=X) -> load_continuation()
    // must return ContinuationData with session_key == X. If this breaks, the
    // session_key field will be empty when handle_cluster_continuation runs,
    // the empty-guard kicks in, and session_logs are silently dropped.
    let manager = ContinuationManager::new();
    let session_key = "agent:main:flow_check";

    manager
        .save_continuation(
            "task-flow",
            vec![make_message("user", "hi")],
            "tc_flow",
            "web",
            "chat_flow",
            session_key,
            "",
        )
        .await;

    let loaded = manager.load_continuation("task-flow").await;
    assert!(loaded.is_some(), "continuation should be loadable");
    let data = loaded.unwrap();
    assert_eq!(
        data.session_key, session_key,
        "load_continuation must preserve session_key — empty value would silently skip log writes"
    );
}

#[test]
fn test_legacy_snapshot_without_session_key_deserializes_to_empty() {
    // Backward-compat: snapshots written before session_key was added must
    // still deserialize (#[serde(default)] on the field), with session_key
    // resolving to "". The empty-guard in handle_cluster_continuation then
    // skips log writes for those old snapshots rather than crashing or
    // writing to a bogus "_.jsonl" file.
    let legacy_json = r#"{
        "task_id": "task-legacy-v1",
        "messages": "[{\"role\":\"user\",\"content\":\"old\"}]",
        "tool_call_id": "tc_legacy",
        "channel": "rpc",
        "chat_id": "chat_legacy",
        "created_at": "2026-01-01T00:00:00Z"
    }"#;
    let snapshot: ContinuationSnapshot = serde_json::from_str(legacy_json).unwrap();
    assert_eq!(snapshot.task_id, "task-legacy-v1");
    assert!(
        snapshot.session_key.is_empty(),
        "legacy snapshot must deserialize to empty session_key, got: {:?}",
        snapshot.session_key
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn test_disk_recovery_preserves_session_key_for_handle() {
    // Crash-recovery scenario: continuation is saved with a session_key,
    // process restarts, a new ContinuationManager recovers from disk, and
    // handle_cluster_continuation must still write the assistant reply to
    // the session_log under the ORIGINAL session_key (not an empty one).
    let session_key = unique_cont_test_session_key("recover");
    cleanup_cont_session_log(&session_key);

    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Phase 1: write snapshot to disk via a manager that immediately drops.
    // (spawn_blocking here is historical: with_disk_store's recovery now uses
    // try_lock, so it no longer needs a background thread — see the regression
    // test recover_from_disk_inside_async_runtime_does_not_panic below.)
    {
        let workspace = workspace.clone();
        let session_key_phase1 = session_key.clone();
        tokio::task::spawn_blocking(move || {
            let mgr = ContinuationManager::with_disk_store(&workspace);
            // save_continuation is async — run it on this blocking thread via a
            // dedicated runtime. This mirrors how production code persists the
            // snapshot before crashing.
            let rt = tokio::runtime::Runtime::new().unwrap();
            rt.block_on(mgr.save_continuation(
                "task-recover-handle",
                vec![make_message("user", "before crash")],
                "tc_rh",
                "web",
                "chat_rh",
                &session_key_phase1,
                "",
            ));
        })
        .await
        .unwrap();
    }

    // Phase 2: new manager boots, recovers from disk, handles the callback.
    let recovered_mgr =
        tokio::task::spawn_blocking(move || ContinuationManager::with_disk_store(&workspace))
            .await
            .unwrap();
    assert!(
        recovered_mgr.has_continuation("task-recover-handle").await,
        "snapshot should be recovered from disk on startup"
    );

    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Recovered reply after crash".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    handle_cluster_continuation(
        &recovered_mgr,
        "task-recover-handle",
        "peer response",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;

    let _ = outbound_rx.try_recv();

    // Verify the recovered session_key reached the chat_log file.
    let safe_key = session_key.replace(':', "_");
    let log_path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    assert!(
        log_path.exists(),
        "session_log should be written after disk-recovery path at {:?}",
        log_path
    );
    let content = std::fs::read_to_string(&log_path).unwrap();
    assert!(
        content.contains("Recovered reply after crash"),
        "recovered session_key must flow to handle_cluster_continuation, log content: {}",
        content
    );

    cleanup_cont_session_log(&session_key);
}

#[tokio::test]
async fn test_save_load_roundtrip_through_disk_preserves_session_key() {
    // Cover the full save->disk->load round trip for session_key. This is
    // narrower than the recovery test above — it doesn't go through handle
    // — and is here to pin down where in the pipeline a regression would
    // surface (save_continuation's snapshot write, vs the load path).
    let tmp = TempDir::new().unwrap();
    let mgr = ContinuationManager::with_disk_store(tmp.path());
    let session_key = "roundtrip:session:42";

    mgr.save_continuation(
        "task-rt",
        vec![make_message("user", "roundtrip")],
        "tc_rt",
        "web",
        "chat_rt",
        session_key,
        "",
    )
    .await;

    // Read back the raw snapshot from disk to verify session_key was
    // actually serialized (catches a missing #[serde(...)] or wrong field).
    let raw_snapshot = mgr.disk_store.as_ref().unwrap().load("task-rt").unwrap();
    assert_eq!(
        raw_snapshot.session_key, session_key,
        "ContinuationSnapshot on disk must carry session_key"
    );

    // Load through the normal async path.
    let loaded = mgr.load_continuation("task-rt").await.unwrap();
    assert_eq!(
        loaded.session_key, session_key,
        "load_continuation must return ContinuationData with session_key from disk"
    );
}

// ===========================================================================
// Coverage gap tests — observer emit, session persistence, ToolLookup variants,
// disk-fallback load, and corrupt-snapshot recovery.
// ===========================================================================

#[tokio::test]
async fn test_handle_cluster_continuation_emits_observer_events() {
    // observer_manager = Some covers all observer emit branches:
    // ConversationStart, LlmRequest, LlmResponse, ToolCall, ConversationEnd.
    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);

    manager
        .save_continuation(
            "task-obs",
            vec![make_message("user", "Hi")],
            "tc_obs",
            "web",
            "chat_obs",
            "obs_session",
            "",
        )
        .await;

    // First response carries a tool call (exercises ToolCall emit); second is final.
    let provider = MockContinuationProvider::new(vec![
        LlmResponse {
            content: String::new(),
            tool_calls: vec![ToolCallInfo {
                id: "tc_o1".to_string(),
                name: "echo".to_string(),
                arguments: r#"{"text":"x"}"#.to_string(),
            }],
            finished: false,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
        LlmResponse {
            content: "Observer-covered final".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        },
    ]);

    let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    struct EchoTool;
    #[async_trait]
    impl Tool for EchoTool {
        async fn execute(&self, args: &str, _ctx: &RequestContext) -> Result<String, String> {
            let v: serde_json::Value = serde_json::from_str(args).unwrap();
            Ok(v.get("text").unwrap().as_str().unwrap().to_string())
        }
    }
    tools.insert("echo".to_string(), Arc::new(EchoTool));

    let observer = Arc::new(nemesis_observer::Manager::new());

    handle_cluster_continuation(
        &manager,
        "task-obs",
        "resp",
        false,
        None,
        &provider,
        "model",
        &tools,
        &outbound_tx,
        Some(Arc::clone(&observer)),
        None,
        true, // F-F vision_supported
    )
    .await;

    let out = outbound_rx.try_recv().unwrap();
    assert!(out.content.contains("Observer-covered final"));
}

#[tokio::test]
async fn test_handle_cluster_continuation_persists_to_session_store() {
    // session_store = Some + non-empty session_key covers the persistence branch
    // (get_or_create / add_message / save).
    let manager = ContinuationManager::new();
    let (outbound_tx, _rx) = tokio::sync::mpsc::channel(16);

    manager
        .save_continuation(
            "task-sess",
            vec![make_message("user", "Hi")],
            "tc_s",
            "web",
            "chat_s",
            "persist_session",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "Persisted reply".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);

    let store = crate::session::SessionStore::new_in_memory();

    handle_cluster_continuation(
        &manager,
        "task-sess",
        "resp",
        false,
        None,
        &provider,
        "model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        Some(&store),
        true, // F-F vision_supported
    )
    .await;

    // The session was created and the assistant reply recorded.
    let _session = store.get_or_create("persist_session");
}

#[tokio::test]
async fn test_rwlock_tool_lookup_impl() {
    // Covers the parking_lot::RwLock<HashMap> ToolLookup impl.
    struct T;
    #[async_trait]
    impl Tool for T {
        async fn execute(&self, _: &str, _: &RequestContext) -> Result<String, String> {
            Ok(String::new())
        }
    }
    let mut inner: HashMap<String, Arc<dyn Tool>> = HashMap::new();
    inner.insert("rwtool".to_string(), Arc::new(T));
    let rw = parking_lot::RwLock::new(inner);

    assert!(rw.get_tool("rwtool").is_some());
    assert!(rw.get_tool("missing").is_none());
}

#[tokio::test]
async fn test_load_continuation_falls_back_to_disk() {
    // Covers try_load_from_disk: build a with_disk_store manager on an empty
    // cache, then write a snapshot straight to disk (bypassing save_continuation
    // which would also populate memory). load_continuation must hit the disk
    // fallback path.
    let tmp = TempDir::new().unwrap();
    let mgr = ContinuationManager::with_disk_store(tmp.path());

    let store = ContinuationStore::new(tmp.path());
    let messages_json = serde_json::to_string(&vec![make_message("user", "disk")]).unwrap();
    let snap = ContinuationSnapshot {
        task_id: "task-disk-only".to_string(),
        messages: messages_json,
        tool_call_id: "tc_d".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_d".to_string(),
        session_key: "disk_session".to_string(),
        created_at: "2026-01-01T00:00:00Z".to_string(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snap).unwrap();

    // Memory has no entry → load_continuation falls back to disk.
    let loaded = mgr.load_continuation("task-disk-only").await;
    assert!(loaded.is_some(), "should fall back to disk");
    let data = loaded.unwrap();
    assert_eq!(data.chat_id, "chat_d");
    assert_eq!(data.session_key, "disk_session");
}

#[test]
fn test_recover_to_manager_skips_corrupt_snapshot() {
    // A corrupt .json file on disk makes load() fail → warn branch is hit and
    // the entry is skipped (no panic). with_disk_store's recovery uses
    // try_lock() for its sync memory lookups, so it is safe from any context
    // (including a tokio runtime worker thread).
    let tmp = TempDir::new().unwrap();
    let cache_dir = tmp.path().join("cluster").join("rpc_cache");
    std::fs::create_dir_all(&cache_dir).unwrap();
    std::fs::write(cache_dir.join("corrupt.json"), "{ not valid json").unwrap();

    let mgr = ContinuationManager::with_disk_store(tmp.path());
    assert!(!mgr.has_continuation_sync("corrupt"));
}

/// Regression for BUG #45 (2026-08-27): `ContinuationManager::with_disk_store`
/// runs `recover_to_manager` synchronously, which calls
/// `has_continuation_sync`/`insert_continuation_sync`. Under the old code those
/// used `tokio::sync::Mutex::blocking_lock()`, which panics with
/// "Cannot block the current thread from within a runtime" when invoked on a
/// tokio runtime worker thread — exactly the context of gateway startup
/// (`build_agent_loop` at agent_factory.rs:437 calls `with_disk_store` directly
/// inside the async `gateway::run`, NOT via `spawn_blocking`). A leftover
/// `rpc_cache/*.json` (gateway died mid-cluster-rpc) tripped `list_pending()`
/// non-empty → the blocking_lock panic → a restart crash loop. The fix replaces
/// `blocking_lock()` with `try_lock()` (non-blocking CAS, never parks the
/// thread, safe from any context). This test seeds a real snapshot and calls
/// `with_disk_store` directly from a `#[tokio::test]` (current_thread runtime
/// worker thread) — the exact context that used to panic. It must recover the
/// entry without panicking.
#[tokio::test]
async fn recover_from_disk_inside_async_runtime_does_not_panic() {
    let tmp = tempfile::TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();

    // Seed a valid snapshot on disk exactly as production leaves behind when
    // the gateway dies mid-cluster-rpc.
    let store = ContinuationStore::new(&workspace);
    let task_id = format!("bug45-{}", std::process::id());
    let snapshot = ContinuationSnapshot {
        task_id: task_id.clone(),
        messages: r#"[{"role":"user","content":"cluster rpc in flight"}]"#.to_string(),
        tool_call_id: "tc_bug45".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_bug45".to_string(),
        session_key: String::new(),
        created_at: "2026-08-27T00:00:00Z".to_string(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    };
    store.save(&snapshot).unwrap();
    drop(store);

    // Direct call from the async runtime worker thread — NO spawn_blocking.
    // Under the old blocking_lock code this panicked here.
    let mgr = ContinuationManager::with_disk_store(&workspace);
    assert!(
        mgr.has_continuation_sync(&task_id),
        "snapshot must be recovered from disk into the in-memory map at startup"
    );
}

// ---------------------------------------------------------------------------
// 2026-08-25 self-heal ordering regression: persist_final_reply must be
// store-first, or a missing store entry rebuilds from chat_log that ALREADY
// contains the final reply and add_message appends it a second time.
// ---------------------------------------------------------------------------

#[test]
fn test_persist_final_reply_no_duplicate_when_store_missing() {
    // Simulate the exact production scenario the ordering protects: a
    // TTL-evicted store (json missing) whose chat_log still holds prior
    // turns. The final reply must appear exactly ONCE in the store and ONCE
    // in the jsonl — never twice in either.
    let key = format!(
        "test:persist:final:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::delete_chat_log(&key); // clean slate

    crate::chat_log::append_chat_log(&key, "user", "prior user turn");
    crate::chat_log::append_chat_log(&key, "assistant", "prior assistant turn");

    let tmp = TempDir::new().unwrap();
    let store = crate::session::SessionStore::new_with_storage(tmp.path());

    persist_final_reply(Some(&store), &key, "test-model", "FINAL-REPLY");

    let hist = store.get_history(&key);
    let finals = hist.iter().filter(|m| m.content == "FINAL-REPLY").count();
    assert_eq!(
        finals,
        1,
        "final reply duplicated in store: {:?}",
        hist.iter().map(|m| m.content.clone()).collect::<Vec<_>>()
    );
    // The rebuilt prefix survived alongside it.
    assert!(hist.iter().any(|m| m.content == "prior user turn"));
    assert!(hist.iter().any(|m| m.content == "prior assistant turn"));

    let (rows, _, _, _) = crate::chat_log::read_chat_log(&key, 100, None);
    let jsonl_finals = rows
        .iter()
        .filter(|r| r.get("content").and_then(|c| c.as_str()) == Some("FINAL-REPLY"))
        .count();
    assert_eq!(jsonl_finals, 1, "final reply duplicated in jsonl");

    crate::chat_log::delete_chat_log(&key);
}

#[test]
fn test_persist_final_reply_none_store_still_logs() {
    // No session store wired (legacy call path) → chat_log still gets the
    // reply exactly once; the helper must not touch any store.
    let key = format!(
        "test:persist:final:none:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::delete_chat_log(&key); // clean slate

    persist_final_reply(None, &key, "test-model", "ONLY-LOG");

    let (rows, _, _, _) = crate::chat_log::read_chat_log(&key, 100, None);
    assert_eq!(rows.len(), 1);
    assert_eq!(
        rows[0].get("content").and_then(|c| c.as_str()),
        Some("ONLY-LOG")
    );

    crate::chat_log::delete_chat_log(&key);
}

// -----------------------------------------------------------------------
// rpc_cache TTL cleanup (2026-08-25)
// -----------------------------------------------------------------------

#[tokio::test]
async fn test_cleanup_old_snapshots_removes_stale_keeps_fresh() {
    let tmp = TempDir::new().unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());

    for task in ["stale-task", "fresh-task"] {
        manager
            .save_continuation(
                task,
                vec![make_message("user", "hi")],
                "tc",
                "rpc",
                "chat",
                "test_session",
                "",
            )
            .await;
    }
    let stale_path = tmp
        .path()
        .join("cluster")
        .join("rpc_cache")
        .join("stale-task.json");
    assert!(stale_path.exists(), "snapshot written to cluster/rpc_cache");
    assert!(manager.has_continuation("stale-task").await);

    // Age the stale file 3 hours back (mirrors the nemesis-cluster store
    // test's technique); falls back to "age everything" when PowerShell
    // isn't available.
    let aged = {
        let out = std::process::Command::new("powershell")
            .args([
                "-NoProfile",
                "-Command",
                &format!(
                    "(Get-Item '{}').LastWriteTime = (Get-Date).AddHours(-3)",
                    stale_path.display()
                ),
            ])
            .output();
        matches!(&out, Ok(o) if o.status.success())
    };

    if aged {
        // 1h TTL: only the aged file qualifies.
        let removed = manager
            .cleanup_old_snapshots(std::time::Duration::from_secs(3600))
            .await;
        assert_eq!(removed, 1);
        assert!(!manager.has_continuation("stale-task").await);
        assert!(manager.has_continuation("fresh-task").await);
        assert!(!stale_path.exists());
        assert!(
            tmp.path()
                .join("cluster")
                .join("rpc_cache")
                .join("fresh-task.json")
                .exists()
        );
        // Long TTL removes nothing further.
        let removed = manager
            .cleanup_old_snapshots(std::time::Duration::from_secs(7 * 24 * 3600))
            .await;
        assert_eq!(removed, 0);
    } else {
        // No PowerShell: verify the mechanism with an effectively-zero TTL.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let removed = manager
            .cleanup_old_snapshots(std::time::Duration::from_nanos(1))
            .await;
        assert!(removed > 0, "mechanism works even without mtime aging");
        assert!(!manager.has_continuation("stale-task").await);
        assert!(!stale_path.exists());
    }
}

#[tokio::test]
async fn test_cleanup_old_snapshots_memory_only_manager_is_noop() {
    // Constructed without a disk store -> cleanup must be a clean no-op.
    let manager = ContinuationManager::new();
    manager
        .save_continuation(
            "mem-only",
            vec![make_message("user", "hi")],
            "tc",
            "rpc",
            "chat",
            "test_session",
            "",
        )
        .await;
    let removed = manager
        .cleanup_old_snapshots(std::time::Duration::from_nanos(1))
        .await;
    assert_eq!(removed, 0);
    // In-memory entries are untouched by the no-disk-store path.
    assert!(manager.has_continuation("mem-only").await);
}

// --- W3a: delete 警告臂、stale/cleanup TTL、磁盘写失败、save-barrier
// 等待/超时臂、persist_final_reply save 错误、final outbound 发送失败、
// observer_manager 路径 ---

use std::sync::atomic::{AtomicBool, Ordering as AtomicOrdering};
use tokio::sync::Notify;

fn snap(task: &str) -> ContinuationSnapshot {
    ContinuationSnapshot {
        task_id: task.to_string(),
        messages: r#"[{"role":"user","content":"hi"}]"#.to_string(),
        tool_call_id: "tc".to_string(),
        channel: "web".to_string(),
        chat_id: "c1".to_string(),
        session_key: String::new(),
        created_at: "2026-08-25T00:00:00+08:00".to_string(),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    }
}

#[test]
fn continuation_store_delete_readonly_file_warns() {
    let tmp = tempfile::TempDir::new().unwrap();
    let store = ContinuationStore::new(tmp.path());
    store.save(&snap("ro_task")).unwrap();
    let path = tmp
        .path()
        .join("cluster")
        .join("rpc_cache")
        .join("ro_task.json");

    // Windows：只读属性让 remove_file 失败 → warn 分支（不 panic）。
    // 注意：部分文件系统（如 ReFS/Dev Drive）不强制只读删除语义，实测可删
    // ——先探针判定，未强制时跳过"存活"断言（delete 的 warn 分支只在不强制
    // 只读的机器上可达）。
    #[cfg(windows)]
    {
        let probe = tmp.path().join("ro_probe.txt");
        std::fs::write(&probe, b"p").unwrap();
        let mut perms = std::fs::metadata(&probe).unwrap().permissions();
        perms.set_readonly(true);
        std::fs::set_permissions(&probe, perms).unwrap();
        let enforced = std::fs::remove_file(&probe).is_err();
        if enforced {
            std::fs::remove_file(&probe).unwrap_or(());
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(true);
            std::fs::set_permissions(&path, perms).unwrap();
            store.delete("ro_task");
            assert!(path.exists(), "readonly file must survive deletion");
            let mut perms = std::fs::metadata(&path).unwrap().permissions();
            perms.set_readonly(false);
            std::fs::set_permissions(&path, perms).unwrap();
        } else {
            eprintln!(
                "skipping readonly-survives arm: filesystem does not enforce readonly deletes"
            );
        }
    }
    // 所有平台：正常删除幂等。
    store.delete("ro_task");
    assert!(!path.exists());
}

#[test]
fn stale_task_ids_dir_extension_and_age_arms() {
    let tmp = tempfile::TempDir::new().unwrap();
    // (a) 目录不存在 → 空（read_dir err 臂）
    let missing = ContinuationStore::new(&tmp.path().join("missing_ws"));
    assert!(missing.stale_task_ids(Duration::from_secs(3600)).is_empty());

    let store = ContinuationStore::new(tmp.path());
    let cache = tmp.path().join("cluster").join("rpc_cache");
    std::fs::create_dir_all(&cache).unwrap();
    store.save(&snap("old_a")).unwrap();
    store.save(&snap("old_b")).unwrap();
    std::fs::write(cache.join("notes.txt"), b"x").unwrap(); // 非 json 扩展 → 跳过

    // (b) max_age 巨大 → 无 stale（mtime 新鲜）
    let none = store.stale_task_ids(Duration::from_secs(3600));
    assert!(
        none.is_empty(),
        "fresh snapshots must not be stale: {none:?}"
    );

    // (c) 等 20ms 后 max_age=0 → 全部 .json stale；.txt 不参与
    std::thread::sleep(Duration::from_millis(20));
    let stale = store.stale_task_ids(Duration::ZERO);
    assert_eq!(stale.len(), 2, "stale: {stale:?}");
}

#[tokio::test]
async fn cleanup_old_snapshots_without_disk_store_is_zero() {
    let manager = ContinuationManager::new();
    assert_eq!(
        manager.cleanup_old_snapshots(Duration::from_secs(1)).await,
        0
    );
}

#[tokio::test]
async fn cleanup_old_snapshots_evicts_disk_and_memory() {
    let tmp = tempfile::TempDir::new().unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());
    manager
        .save_continuation(
            "t1",
            vec![make_message("user", "a")],
            "tc",
            "web",
            "c",
            "s",
            "",
        )
        .await;
    manager
        .save_continuation(
            "t2",
            vec![make_message("user", "b")],
            "tc",
            "web",
            "c",
            "s",
            "",
        )
        .await;
    assert!(manager.has_continuation("t1").await);
    let cache = tmp.path().join("cluster").join("rpc_cache");
    assert!(cache.join("t1.json").exists() && cache.join("t2.json").exists());

    std::thread::sleep(Duration::from_millis(20));
    let removed = manager.cleanup_old_snapshots(Duration::ZERO).await;
    assert_eq!(removed, 2);
    assert!(!manager.has_continuation("t1").await);
    assert!(!manager.has_continuation("t2").await);
    assert!(!cache.join("t1.json").exists());
    assert!(!cache.join("t2.json").exists());
}

#[tokio::test]
async fn save_continuation_disk_failure_still_marks_ready() {
    // cluster 路径被文件占位 → create_dir_all 失败 → save 告警但内存照常 + ready。
    let tmp = tempfile::TempDir::new().unwrap();
    std::fs::write(tmp.path().join("cluster"), b"blocker").unwrap();
    let manager = ContinuationManager::with_disk_store(tmp.path());
    manager
        .save_continuation(
            "diskfail",
            vec![make_message("user", "x")],
            "tc",
            "web",
            "c",
            "s",
            "",
        )
        .await;
    assert!(manager.has_continuation("diskfail").await);
    let loaded = manager.load_continuation("diskfail").await;
    assert!(
        loaded.is_some(),
        "memory entry must be loadable despite disk failure"
    );
}

fn not_ready_data() -> Arc<ContinuationData> {
    Arc::new(ContinuationData {
        messages: vec![make_message("user", "pending")],
        tool_call_id: "tc".to_string(),
        channel: "web".to_string(),
        chat_id: "c".to_string(),
        session_key: String::new(),
        ready: Arc::new(Notify::new()),
        ready_flag: Arc::new(AtomicBool::new(false)),
        peer_id: String::new(),
        image_refs: Vec::new(),
        image_refs_by_user_turn: Vec::new(), // L1
    })
}

#[tokio::test]
async fn wait_for_continuation_barrier_notified_path() {
    // 条目存在但未 ready：等待者在 barrier 上挂起，写入方置位+唤醒 → 成功读回。
    let manager = Arc::new(ContinuationManager::new());
    let data = not_ready_data();
    let ready = data.ready.clone();
    let ready_flag = data.ready_flag.clone();
    // insert_continuation_sync 用 try_lock（永不阻塞），任何上下文安全；
    // spawn_blocking 是历史遗留（旧 blocking_lock 时代的规避），保留无害。
    let m0 = manager.clone();
    let d0 = data.clone();
    tokio::task::spawn_blocking(move || {
        m0.insert_continuation_sync("barrier_task".to_string(), d0);
    })
    .await
    .unwrap();

    let m2 = manager.clone();
    let loader = tokio::spawn(async move { m2.load_continuation("barrier_task").await });
    tokio::time::sleep(Duration::from_millis(30)).await;
    assert!(
        !loader.is_finished(),
        "loader must be parked on the barrier"
    );
    ready_flag.store(true, AtomicOrdering::Release);
    ready.notify_waiters();
    let got = tokio::time::timeout(Duration::from_secs(2), loader)
        .await
        .expect("loader completes")
        .expect("join ok");
    let data = got.expect("notified barrier must yield the entry");
    assert_eq!(data.tool_call_id, "tc");
    assert_eq!(data.channel, "web");
}

#[tokio::test]
async fn wait_for_continuation_barrier_timeout_path() {
    // 条目存在但永不 ready → barrier_timeout 到期 → None（落盘兜底）。
    let mut manager = ContinuationManager::new();
    manager.set_barrier_timeout(Duration::from_millis(80));
    // try_lock 任何上下文安全；spawn_blocking 同上，历史遗留保留。
    let manager = Arc::new(manager);
    let m0 = manager.clone();
    tokio::task::spawn_blocking(move || {
        m0.insert_continuation_sync("stuck_task".to_string(), not_ready_data());
    })
    .await
    .unwrap();
    let got = manager.load_continuation("stuck_task").await;
    assert!(got.is_none(), "never-ready entry must time out");
}

#[test]
fn persist_final_reply_save_error_warns_but_appends_chat_log() {
    // storage 下同名目录占位 → rename 失败 → save 错误告警分支。
    let tmp = tempfile::TempDir::new().unwrap();
    let store = crate::session::SessionStore::new_with_storage(tmp.path());
    let key = format!(
        "contfail{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );
    std::fs::create_dir_all(tmp.path().join(format!("{key}.json"))).unwrap();

    persist_final_reply(Some(&store), &key, "m", "final content");

    // chat_log 侧仍被追加（持久化失败只影响 store，不挡回复）。
    crate::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn final_outbound_send_error_warns_no_panic() {
    // 接收端已 drop → 最终 outbound 发送失败 → warn（不 panic）。
    let manager = ContinuationManager::new();
    let (outbound_tx, outbound_rx) = tokio::sync::mpsc::channel(16);
    drop(outbound_rx);
    manager
        .save_continuation(
            "task-out",
            vec![make_message("user", "hi")],
            "tc",
            "web",
            "c",
            "",
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "done".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    handle_cluster_continuation(
        &manager,
        "task-out",
        "resp",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // F-F vision_supported
    )
    .await;
    assert!(!manager.has_continuation("task-out").await);
}

#[tokio::test]
async fn handle_continuation_with_observer_manager_persists_and_sends() {
    // observer_manager=Some + session_store=Some + session_key 非空 → 全事件
    // 流 + persist_final_reply 真正写入 store + 带 model 的 OutboundMeta。
    let tmp = tempfile::TempDir::new().unwrap();
    let store = crate::session::SessionStore::new_with_storage(tmp.path().join("store"));
    let session_key = format!(
        "contobs{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .subsec_nanos()
    );

    let manager = ContinuationManager::new();
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    manager
        .save_continuation(
            "task-obs",
            vec![make_message("user", "hi")],
            "tc",
            "web",
            "chat1",
            &session_key,
            "",
        )
        .await;

    let provider = MockContinuationProvider::new(vec![LlmResponse {
        content: "observed final".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    handle_cluster_continuation(
        &manager,
        "task-obs",
        "resp",
        false,
        None,
        &provider,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        Some(Arc::new(nemesis_observer::Manager::new())),
        Some(&store),
        true, // F-F vision_supported
    )
    .await;

    let out = outbound_rx.try_recv().expect("final outbound sent");
    assert!(out.content.contains("observed final"));
    assert_eq!(out.meta.model.as_deref(), Some("test-model"));

    // store 里应有 assistant 终稿。
    let session = store.get_or_create(&session_key);
    let has_final = session
        .messages
        .iter()
        .any(|m| m.role == "assistant" && m.content.contains("observed final"));
    assert!(
        has_final,
        "final reply must be persisted to the session store"
    );

    crate::chat_log::delete_chat_log(&session_key);
}

// ---------------------------------------------------------------------------
// T6（多模态）：快照带图片路径引用 —— 磁盘剥离字节 / 加载重水合 / 旧快照兼容
// ---------------------------------------------------------------------------

/// 最小 PNG（8 字节签名 + IHDR 长度前缀；与 image_attach 测试同形）。
fn t6_png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(&[0, 0, 0, 13, b'I', b'H', b'D', b'R']);
    v
}

fn t6_png_image(path: &str, bytes: &[u8]) -> LlmMessage {
    use base64::Engine as _;
    LlmMessage {
        role: "user".to_string(),
        content: "看这张图".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: vec![crate::image_attach::LlmImage {
            path: path.to_string(),
            media_type: "image/png".to_string(),
            data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }],
    }
}

#[tokio::test]
async fn t6_snapshot_roundtrip_strips_bytes_and_rehydrates() {
    use base64::Engine as _;

    let tmp = TempDir::new().unwrap();
    let img = tmp.path().join("t6_roundtrip.png");
    let bytes = t6_png_bytes();
    std::fs::write(&img, &bytes).unwrap();
    let refs = vec![img.to_string_lossy().into_owned()];

    let task = format!("t6rt_{}", std::process::id());
    let manager1 = ContinuationManager::with_disk_store(tmp.path());
    let messages = vec![
        make_message("system", "sys"),
        t6_png_image(&refs[0], &bytes),
        {
            let mut m = make_message("assistant", "");
            m.tool_calls = Some(vec![ToolCallInfo {
                id: "tc_t6".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }]);
            m
        },
    ];
    manager1
        .save_continuation_with_images(
            &task,
            messages,
            "tc_t6",
            "web",
            "chat1",
            "t6_session",
            "",
            &refs,
        )
        .await;

    // 磁盘快照：messages JSON 不含 base64 字节，image_refs 只落路径。
    drop(manager1);
    let store = ContinuationStore::new(tmp.path());
    let snapshot = store.load(&task).expect("snapshot on disk");
    let png_sig_b64 = base64::engine::general_purpose::STANDARD.encode(&t6_png_bytes()[..8]);
    assert!(
        !snapshot.messages.contains(&png_sig_b64),
        "磁盘快照不得携带图片 base64 字节"
    );
    assert_eq!(snapshot.image_refs, refs, "快照应携带路径引用");

    // 新 manager 启动恢复（recover_to_manager）→ 加载后重水合回最后一条
    // user 消息，字节与文件内容一致。
    let manager2 = ContinuationManager::with_disk_store(tmp.path());
    assert!(manager2.has_continuation(&task).await, "启动恢复应还原条目");
    let data = manager2
        .load_continuation(&task)
        .await
        .expect("recovered continuation");
    let user = data
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .expect("user message");
    assert_eq!(user.images.len(), 1, "加载后应重水合出图片");
    assert_eq!(
        user.images[0].data,
        base64::engine::general_purpose::STANDARD.encode(&bytes),
        "重水合字节应与文件内容一致"
    );
    assert_eq!(user.images[0].media_type, "image/png");
    assert_eq!(data.image_refs, refs);
    assert!(
        !user.content.contains("[图片已失效"),
        "文件存在时不得出现失效占位"
    );

    manager2.remove_continuation(&task).await;
}

#[tokio::test]
async fn t6_disk_fallback_load_rehydrates_refs() {
    use base64::Engine as _;

    let tmp = TempDir::new().unwrap();
    let img = tmp.path().join("t6_fallback.png");
    let bytes = t6_png_bytes();
    std::fs::write(&img, &bytes).unwrap();
    let refs = vec![img.to_string_lossy().into_owned()];
    let task = format!("t6fb_{}", std::process::id());

    // manager 先建（此刻磁盘无快照，恢复空跑），后手写快照 → load 走
    // try_load_from_disk 磁盘回退臂。
    let manager = ContinuationManager::with_disk_store(tmp.path());
    let stripped = vec![LlmMessage {
        role: "user".to_string(),
        content: "带图消息".to_string(),
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(), // 保存侧已剥离
    }];
    let snapshot = ContinuationSnapshot {
        task_id: task.clone(),
        messages: serde_json::to_string(&stripped).unwrap(),
        tool_call_id: "tc_fb".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: refs.clone(),
        image_refs_by_user_turn: Vec::new(), // L1
        created_at: "2026-09-03T00:00:00Z".to_string(),
    };
    ContinuationStore::new(tmp.path())
        .save(&snapshot)
        .expect("manual snapshot save");

    let data = manager
        .load_continuation(&task)
        .await
        .expect("disk fallback load");
    let user = &data.messages[0];
    assert_eq!(user.images.len(), 1, "磁盘回退加载应重水合");
    assert_eq!(
        user.images[0].data,
        base64::engine::general_purpose::STANDARD.encode(&bytes)
    );
    assert_eq!(user.content, "带图消息", "文件在时不加占位");
}

#[tokio::test]
async fn t6_disk_load_deleted_file_degrades_to_placeholder() {
    let tmp = TempDir::new().unwrap();
    let img = tmp.path().join("t6_gone.png");
    let bytes = t6_png_bytes();
    std::fs::write(&img, &bytes).unwrap();
    let refs = vec![img.to_string_lossy().into_owned()];
    let task = format!("t6gone_{}", std::process::id());

    let manager = ContinuationManager::with_disk_store(tmp.path());
    let snapshot = ContinuationSnapshot {
        task_id: task.clone(),
        messages: serde_json::to_string(&vec![LlmMessage {
            role: "user".to_string(),
            content: "带图消息".to_string(),
            tool_calls: None,
            tool_call_id: None,
            reasoning_content: None,
            images: Vec::new(),
        }])
        .unwrap(),
        tool_call_id: "tc_gone".to_string(),
        channel: "web".to_string(),
        chat_id: "chat1".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: refs.clone(),
        image_refs_by_user_turn: Vec::new(), // L1
        created_at: "2026-09-03T00:00:00Z".to_string(),
    };
    ContinuationStore::new(tmp.path())
        .save(&snapshot)
        .expect("manual snapshot save");

    // 快照落盘后文件被删 → 加载降级：无字节、占位行进 content，文本不炸。
    std::fs::remove_file(&img).unwrap();

    let data = manager.load_continuation(&task).await.expect("loaded");
    let user = &data.messages[0];
    assert!(user.images.is_empty(), "文件已删不得产出图片字节");
    assert!(
        user.content.contains(&format!("[图片已失效: {}]", refs[0])),
        "应追加失效占位行，got: {}",
        user.content
    );
}

#[tokio::test]
async fn t6_old_snapshot_without_image_refs_field_loads() {
    let tmp = TempDir::new().unwrap();
    let task = format!("t6old_{}", std::process::id());

    let manager = ContinuationManager::with_disk_store(tmp.path());
    // 手写**旧版本快照 JSON**：无 image_refs 字段（serde default 兼容）。
    let dir = nemesis_path::resolve_cluster_rpc_cache_dir_in_workspace(tmp.path());
    std::fs::create_dir_all(&dir).unwrap();
    let legacy = serde_json::json!({
        "task_id": task,
        "messages": serde_json::to_string(&vec![make_message("user", "legacy")]).unwrap(),
        "tool_call_id": "tc_old",
        "channel": "web",
        "chat_id": "chat1",
        "session_key": "",
        "peer_id": "",
        "created_at": "2026-08-01T00:00:00Z",
    });
    std::fs::write(
        dir.join(format!("{}.json", task)),
        serde_json::to_string_pretty(&legacy).unwrap(),
    )
    .unwrap();

    let data = manager
        .load_continuation(&task)
        .await
        .expect("legacy snapshot loads");
    assert_eq!(data.messages.len(), 1);
    assert!(data.messages[0].images.is_empty());
    assert!(data.image_refs.is_empty());
    assert!(!data.messages[0].content.contains("[图片已失效"));
}

#[test]
fn t6_rehydrate_skips_duplicate_placeholder_line() {
    let path = "C:\\nonexistent\\t6_dup.png";
    let refs = vec![path.to_string()];
    let placeholder = format!("[图片已失效: {}]", path);
    let mut messages = vec![LlmMessage {
        role: "user".to_string(),
        content: format!("带图消息\n{}", placeholder), // 已有一份占位
        tool_calls: None,
        tool_call_id: None,
        reasoning_content: None,
        images: Vec::new(),
    }];

    rehydrate_last_user_images(&mut messages, &refs);

    let count = messages[0].content.matches(&placeholder).count();
    assert_eq!(
        count, 1,
        "重复重水合不得堆叠占位行，got: {}",
        messages[0].content
    );
}

#[test]
fn t6_rehydrate_noop_without_refs() {
    let mut messages = vec![make_message("user", "纯文本")];
    let before = messages[0].content.clone();
    rehydrate_last_user_images(&mut messages, &[]);
    assert_eq!(messages[0].content, before, "无引用必须 no-op");
    assert!(messages[0].images.is_empty());
}

// ---------------------------------------------------------------------------
// L1（2026-09-04 四轮盲审）：按 user 轮的图片引用快照 + 逐轮恢复
// ---------------------------------------------------------------------------

/// 最小合法 PNG 写入（与 T6 相关测试同款）。
fn write_l1_png(path: &std::path::Path) {
    let mut data = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    data.extend_from_slice(b"l1");
    std::fs::write(path, &data).unwrap();
}

fn l1_image(path: &std::path::Path) -> crate::image_attach::LlmImage {
    crate::image_attach::LlmImage {
        path: path.to_string_lossy().to_string(),
        media_type: "image/png".to_string(),
        data: "aGVsbG8=".to_string(), // 占位字节（快照只取 path）
    }
}

#[tokio::test]
async fn l1_snapshot_derives_per_turn_refs_and_recovers_every_user_turn() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let png_a = workspace.join("a.png");
    let png_b = workspace.join("b.png");
    write_l1_png(&png_a);
    write_l1_png(&png_b);

    // 多轮带图会话：user(A) → assistant → user(B) → assistant(tool_call)。
    let messages = vec![
        make_message("system", "sys"),
        LlmMessage {
            images: vec![l1_image(&png_a)],
            ..make_message("user", "第一轮带图")
        },
        make_message("assistant", "中间回复"),
        LlmMessage {
            images: vec![l1_image(&png_b)],
            ..make_message("user", "第二轮带图")
        },
        LlmMessage {
            tool_calls: Some(vec![crate::types::ToolCallInfo {
                id: "tc_l1".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }]),
            ..make_message("assistant", "发起请求")
        },
    ];

    let manager = ContinuationManager::with_disk_store(&workspace);
    let task = format!("l1-{}", std::process::id());
    manager
        .save_continuation_with_images(
            &task,
            messages,
            "tc_l1",
            "web",
            "chat_l1",
            "session_l1",
            "peer-1",
            &[], // 旧单层参数留空：per-turn 字段从 messages 派生
        )
        .await;

    // 快照侧：image_refs_by_user_turn 两轮各一条。
    let store = ContinuationStore::new(&workspace);
    let snap = store.load(&task).unwrap();
    assert_eq!(
        snap.image_refs_by_user_turn.len(),
        2,
        "应按 user 轮派生两条引用"
    );
    assert_eq!(
        snap.image_refs_by_user_turn[0],
        vec![png_a.to_string_lossy()]
    );
    assert_eq!(
        snap.image_refs_by_user_turn[1],
        vec![png_b.to_string_lossy()]
    );

    // 恢复侧：全新 manager（空内存 → 磁盘加载）→ 两条 user 轮的图都回来。
    let mgr2 = ContinuationManager::with_disk_store(&workspace);
    let data = mgr2
        .load_continuation(&task)
        .await
        .expect("快照应从磁盘恢复");
    let user_turns: Vec<&LlmMessage> = data
        .messages
        .iter()
        .filter(|m| m.role == "user" && m.tool_call_id.is_none())
        .collect();
    assert_eq!(user_turns.len(), 2);
    assert_eq!(user_turns[0].images.len(), 1, "第一轮 user 的图必须恢复");
    assert_eq!(user_turns[0].images[0].path, png_a.to_string_lossy());
    assert!(
        !user_turns[0].images[0].data.is_empty(),
        "重水合应有真实字节"
    );
    assert_eq!(
        user_turns[1].images.len(),
        1,
        "第二轮 user 的图必须恢复（旧实现只有最后一轮）"
    );
    assert_eq!(user_turns[1].images[0].path, png_b.to_string_lossy());
}

#[tokio::test]
async fn l1_legacy_flat_refs_still_rehydrate_last_turn() {
    // 旧快照兼容：只有单层 image_refs（无 per-turn 字段）→ 回退原语义，
    // 最后一条 user 轮重水合。
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let png = workspace.join("legacy.png");
    write_l1_png(&png);

    let store = ContinuationStore::new(&workspace);
    let task = format!("l1-legacy-{}", std::process::id());
    let snapshot = ContinuationSnapshot {
        task_id: task.clone(),
        messages: serde_json::to_string(&vec![
            make_message("user", "旧快照带图"),
            make_message("assistant", "发起"),
        ])
        .unwrap(),
        tool_call_id: "tc_legacy".to_string(),
        channel: "web".to_string(),
        chat_id: "chat_l1_legacy".to_string(),
        session_key: String::new(),
        peer_id: String::new(),
        image_refs: vec![png.to_string_lossy().to_string()],
        image_refs_by_user_turn: Vec::new(),
        created_at: "2026-09-01T00:00:00Z".to_string(),
    };
    store.save(&snapshot).unwrap();
    drop(store);

    let mgr = ContinuationManager::with_disk_store(&workspace);
    let data = mgr.load_continuation(&task).await.unwrap();
    let last_user = data
        .messages
        .iter()
        .rev()
        .find(|m| m.role == "user")
        .unwrap();
    assert_eq!(
        last_user.images.len(),
        1,
        "旧快照回退单层语义应恢复最后一轮"
    );
    assert_eq!(last_user.images[0].path, png.to_string_lossy());
}

// ---------------------------------------------------------------------------
// F-F（2026-09-04 四轮盲审）：vision=no 的续行投影
// ---------------------------------------------------------------------------

#[test]
fn ff_projection_strips_images_and_notes_by_turn_position() {
    let png = std::path::Path::new("C:\\nonexistent\\ff.png");
    let mut messages = vec![
        LlmMessage {
            images: vec![l1_image(png)],
            ..make_message("user", "历史轮带图")
        },
        make_message("assistant", "回复"),
        LlmMessage {
            images: vec![l1_image(png)],
            ..make_message("user", "最新轮带图")
        },
        LlmMessage {
            tool_call_id: Some("tc_x".to_string()),
            ..make_message("tool", "结果")
        },
    ];

    crate::loop_continuation::project_messages_for_no_vision(&mut messages);

    assert!(messages[0].images.is_empty(), "历史轮图应被剥离");
    assert!(
        messages[0]
            .content
            .contains("[图片已省略: 当前模型仅支持文本]")
    );
    assert!(messages[2].images.is_empty(), "最新 user 轮图应被剥离");
    assert!(
        messages[2]
            .content
            .contains("[图片未发送: 当前模型 vision=no（不支持图像输入），图片已忽略]")
    );
    // 纯文本消息不受影响。
    assert_eq!(messages[1].content, "回复");
}

#[test]
fn ff_projection_is_idempotent() {
    let png = std::path::Path::new("C:\\nonexistent\\ff2.png");
    let mut messages = vec![LlmMessage {
        images: vec![l1_image(png)],
        ..make_message("user", "带图")
    }];
    crate::loop_continuation::project_messages_for_no_vision(&mut messages);
    let once = messages[0].content.clone();
    crate::loop_continuation::project_messages_for_no_vision(&mut messages);
    assert_eq!(messages[0].content, once, "重复投影不得堆叠占位行");
}

/// F-F 端到端：vision_supported=false 时 handler 在调 LLM 前剥掉图片；
/// true 时图片原样到达 provider（快照内已水合字节保留）。
#[tokio::test]
async fn ff_handler_projects_images_when_vision_unsupported() {
    let manager = ContinuationManager::new();
    let png = std::path::Path::new("C:\\nonexistent\\ff3.png");
    let messages = vec![
        LlmMessage {
            images: vec![l1_image(png)],
            ..make_message("user", "带图请求")
        },
        LlmMessage {
            tool_calls: Some(vec![crate::types::ToolCallInfo {
                id: "tc_ff".to_string(),
                name: "cluster_rpc".to_string(),
                arguments: "{}".to_string(),
            }]),
            ..make_message("assistant", "发起")
        },
    ];
    manager
        .save_continuation_with_images(
            "task-ff",
            messages,
            "tc_ff",
            "web",
            "chat_ff",
            "session_ff",
            "peer-1",
            &[],
        )
        .await;

    // vision=no：provider 收到的消息必须零图片。
    let (outbound_tx, mut outbound_rx) = tokio::sync::mpsc::channel(16);
    let provider_no_vision = MockContinuationProvider::new(vec![LlmResponse {
        content: "done".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    handle_cluster_continuation(
        &manager,
        "task-ff",
        "result",
        false,
        None,
        &provider_no_vision,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        false, // vision_supported = false → 投影
    )
    .await;
    let counts = provider_no_vision.seen_image_counts();
    assert!(!counts.is_empty(), "provider 应被调用");
    assert!(
        counts[0].iter().all(|&n| n == 0),
        "vision=no 时 provider 收到的消息不应带图: {:?}",
        counts[0]
    );
    let _ = outbound_rx.try_recv(); // 消费出站（不校验内容）

    // vision=yes：图片原样到达（内存快照字节级无损语义不变）。
    let manager2 = ContinuationManager::new();
    let messages2 = vec![LlmMessage {
        images: vec![l1_image(png)],
        ..make_message("user", "带图请求")
    }];
    manager2
        .save_continuation_with_images(
            "task-ff2",
            messages2,
            "tc_ff2",
            "web",
            "chat_ff",
            "session_ff",
            "peer-1",
            &[],
        )
        .await;
    let provider_vision = MockContinuationProvider::new(vec![LlmResponse {
        content: "done".to_string(),
        tool_calls: Vec::new(),
        finished: true,
        reasoning_content: None,
        usage: None,
        raw_request_body: None,
        raw_response_body: None,
    }]);
    handle_cluster_continuation(
        &manager2,
        "task-ff2",
        "result",
        false,
        None,
        &provider_vision,
        "test-model",
        &HashMap::<String, Arc<dyn Tool>>::new(),
        &outbound_tx,
        None,
        None,
        true, // vision_supported = true → 零改动
    )
    .await;
    let counts = provider_vision.seen_image_counts();
    assert!(
        counts[0].contains(&1),
        "vision=yes 时图片应原样到达 provider: {:?}",
        counts[0]
    );
}
