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
        .save_continuation("task-1", messages.clone(), "tc_1", "web", "chat1", "test_session")
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
        .save_continuation("task-2", vec![make_message("user", "test")], "tc_2", "web", "chat1", "test_session")
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
        .save_continuation("task-disk", messages.clone(), "tc_d", "rpc", "chat2", "test_session")
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
    };
    store.save(&snapshot).unwrap();

    // Create a manager with disk store -- it should recover the snapshot on startup.
    // Uses a synchronous test since with_disk_store uses blocking_lock internally.
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
            )
            .await;
    });

    // The load should wait for the save to complete.
    let load_handle = tokio::spawn(async move {
        mgr.load_continuation("task-barrier").await
    });

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
            )
            .await;
    }

    for i in 0..5 {
        assert!(manager.has_continuation(&format!("task-multi-{}", i)).await);
        let loaded = manager.load_continuation(&format!("task-multi-{}", i)).await;
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
            ).await;
        }));
    }

    // Wait for all saves
    for handle in handles {
        handle.await.unwrap();
    }

    // Verify all can be loaded
    for i in 0..10 {
        let loaded = manager.load_continuation(&format!("task-concurrent-{}", i)).await;
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
        .save_continuation("task-1", messages, "tc_1", "web", "chat1", "test_session")
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
        .save_continuation("task-fail", messages, "tc_1", "web", "chat1", "test_session")
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
        .save_continuation("task-tool", messages, "tc_1", "web", "chat1", "test_session")
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
        .save_continuation("task-err", messages, "tc_1", "web", "chat1", "test_session")
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
        .save_continuation("task-unknown", messages, "tc_1", "web", "chat1", "test_session")
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
    };
    assert_eq!(snapshot.task_id, "t1");
    assert_eq!(snapshot.created_at, "2026-01-01T00:00:00Z");
}

// --- Mock LLM Provider for continuation tests ---

struct MockContinuationProvider {
    responses: std::sync::Mutex<Vec<LlmResponse>>,
    error: std::sync::Mutex<Option<String>>,
}

impl MockContinuationProvider {
    fn new(responses: Vec<LlmResponse>) -> Self {
        Self {
            responses: std::sync::Mutex::new(responses),
            error: std::sync::Mutex::new(None),
        }
    }

    fn new_error(err: String) -> Self {
        Self {
            responses: std::sync::Mutex::new(Vec::new()),
            error: std::sync::Mutex::new(Some(err)),
        }
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
    };
    store.save(&snapshot).unwrap();

    // Create manager with disk store and verify recovery (sync test because with_disk_store uses blocking_lock)
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
        .save_continuation("task-fail-no-err", messages, "tc_1", "web", "chat1", "test_session")
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
    )
    .await;

    // Drain the outbound so the runtime doesn't see a dropped sender.
    let _ = outbound_rx.try_recv();

    let safe_key = session_key.replace(':', "_");
    let log_path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", safe_key));
    assert!(log_path.exists(), "session log file should exist at {:?}", log_path);

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
    )
    .await;

    let _ = outbound_rx.try_recv();

    // session_store should have the assistant message in memory.
    let messages = store.get_history(&session_key);
    let found = messages.iter().any(|m| {
        m.role == "assistant" && m.content.contains("Mirrored into session_store")
    });
    assert!(
        found,
        "session_store messages should contain the assistant reply, got: {:?}",
        messages.iter().map(|m| (&m.role, &m.content)).collect::<Vec<_>>()
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
