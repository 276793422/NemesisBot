use super::*;

#[test]
fn test_create_and_get() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.create_session("web", "user1", "chat1");
    assert_eq!(session.channel, "web");
    let found = mgr.get(&session.id).unwrap();
    assert_eq!(found.sender_id, "user1");
}

#[test]
fn test_remove() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.create_session("web", "user1", "chat1");
    assert!(mgr.remove(&session.id).is_some());
    assert!(mgr.get(&session.id).is_none());
}

#[test]
fn test_count() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    assert_eq!(mgr.count(), 0);
    mgr.create_session("web", "u1", "c1");
    mgr.create_session("web", "u2", "c2");
    assert_eq!(mgr.count(), 2);
}

#[test]
fn test_chat_session_get_or_create() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.get_or_create_chat("web:user1");
    assert_eq!(session.key, "web:user1");
    assert!(session.messages.is_empty());

    let session2 = mgr.get_or_create_chat("web:user1");
    assert_eq!(session2.key, session.key);
}

#[test]
fn test_chat_session_add_message() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "hello");
    mgr.add_message("web:user1", "assistant", "hi there");

    let history = mgr.get_history("web:user1");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].role, "user");
    assert_eq!(history[1].role, "assistant");
}

#[test]
fn test_chat_session_set_summary() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "hello");
    mgr.set_summary("web:user1", "A greeting");
    assert_eq!(mgr.get_summary("web:user1"), Some("A greeting".to_string()));
}

#[test]
fn test_chat_session_truncate() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    for i in 0..10 {
        mgr.add_message("web:user1", "user", &format!("msg {}", i));
    }
    mgr.truncate_history("web:user1", 3);
    let history = mgr.get_history("web:user1");
    assert_eq!(history.len(), 3);
}

#[test]
fn test_chat_session_set_history() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "old");
    let new_messages = vec![Message {
        role: "system".to_string(),
        content: "new".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: None,
    }];
    mgr.set_history("web:user1", new_messages);
    let history = mgr.get_history("web:user1");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "new");
}

#[test]
fn test_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();

    {
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
        mgr.add_message("web:user1", "user", "persistent message");
    }

    // Reload
    let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    let history = mgr2.get_history("web:user1");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "persistent message");
}

#[test]
fn test_sanitize_filename() {
    assert_eq!(sanitize_filename("web:user1"), "web_user1");
    assert_eq!(sanitize_filename("telegram:123456"), "telegram_123456");
}

#[test]
fn test_stats() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.create_session("web", "u1", "c1");
    mgr.add_message("web:user1", "user", "hello");
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 1);
    assert_eq!(stats["chat_sessions"], 1);
}

#[test]
fn test_add_full_message_with_tool_calls() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));

    // Add a message with tool calls
    let tool_msg = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_123".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/test"}"#.to_string(),
            }),
        }],
        tool_call_id: None,
        timestamp: None, // should be auto-filled
    };
    mgr.add_full_message("web:user1", tool_msg);

    // Add a tool result message
    let result_msg = Message {
        role: "tool".to_string(),
        content: "file contents here".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: Some("call_123".to_string()),
        timestamp: Some(Local::now().to_rfc3339()),
    };
    mgr.add_full_message("web:user1", result_msg);

    let history = mgr.get_history("web:user1");
    assert_eq!(history.len(), 2);

    // Check tool call message
    assert_eq!(history[0].role, "assistant");
    assert_eq!(history[0].tool_calls.len(), 1);
    assert_eq!(history[0].tool_calls[0].id, "call_123");
    assert!(history[0].timestamp.is_some()); // auto-filled

    // Check tool result message
    assert_eq!(history[1].role, "tool");
    assert_eq!(history[1].tool_call_id, Some("call_123".to_string()));
}

#[test]
fn test_add_full_message_creates_session() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let msg = Message {
        role: "user".to_string(),
        content: "hello".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: None,
    };
    mgr.add_full_message("web:auto_created", msg);

    let history = mgr.get_history("web:auto_created");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "hello");
}

// ==================== Additional coverage tests ====================

#[test]
fn test_touch_updates_last_active() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.create_session("web", "user1", "chat1");
    let original_last_active = session.last_active;

    // Sleep briefly so the timestamp changes
    std::thread::sleep(std::time::Duration::from_millis(10));
    mgr.touch(&session.id);

    let updated = mgr.get(&session.id).unwrap();
    assert!(updated.last_active > original_last_active);
}

#[test]
fn test_touch_nonexistent_session_no_panic() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    // Should silently do nothing
    mgr.touch("nonexistent_id");
}

#[test]
fn test_cleanup_expired_removes_expired_sessions() {
    // Use a very short timeout (1ms) so sessions expire immediately
    let mgr = SessionMgr::new(Duration::from_millis(1));
    mgr.create_session("web", "u1", "c1");
    mgr.create_session("web", "u2", "c2");
    assert_eq!(mgr.count(), 2);

    // Sleep to let sessions expire
    std::thread::sleep(std::time::Duration::from_millis(50));

    let removed = mgr.cleanup_expired();
    assert_eq!(removed, 2);
    assert_eq!(mgr.count(), 0);
}

#[test]
fn test_cleanup_expired_keeps_active_sessions() {
    // Use a long timeout so sessions won't expire
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.create_session("web", "u1", "c1");
    mgr.create_session("web", "u2", "c2");

    let removed = mgr.cleanup_expired();
    assert_eq!(removed, 0);
    assert_eq!(mgr.count(), 2);
}

#[test]
fn test_shutdown_clears_all_sessions() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.create_session("web", "u1", "c1");
    mgr.create_session("web", "u2", "c2");
    mgr.create_session("web", "u3", "c3");
    assert_eq!(mgr.count(), 3);

    mgr.shutdown();
    assert_eq!(mgr.count(), 0);
}

#[test]
fn test_with_default_timeout_creates_3600s_timeout() {
    let mgr = SessionMgr::with_default_timeout();
    // Create a session and verify it doesn't expire immediately
    let _session = mgr.create_session("web", "u1", "c1");
    assert_eq!(mgr.count(), 1);

    // Cleanup should not remove it since timeout is 3600s
    let removed = mgr.cleanup_expired();
    assert_eq!(removed, 0);
    assert_eq!(mgr.count(), 1);
}

#[test]
fn test_with_storage_empty_path_no_storage() {
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), "");
    // Should work fine, just no storage
    mgr.add_message("web:user1", "user", "hello");
    let history = mgr.get_history("web:user1");
    assert_eq!(history.len(), 1);
    assert!(mgr.storage_path.is_none());
}

#[test]
fn test_set_history_nonexistent_session_silent_return() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    // Setting history on a non-existent session should not panic
    let msgs = vec![Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: None,
    }];
    mgr.set_history("nonexistent:key", msgs);
    // Verify the session was not created
    assert!(mgr.get_history("nonexistent:key").is_empty());
}

#[test]
fn test_set_summary_nonexistent_session_silent_return() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    // Setting summary on a non-existent session should not panic
    mgr.set_summary("nonexistent:key", "some summary");
    // Verify the session was not created
    assert!(mgr.get_summary("nonexistent:key").is_none());
}

#[test]
fn test_truncate_history_keep_zero_clears_all() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "msg1");
    mgr.add_message("web:user1", "user", "msg2");
    mgr.add_message("web:user1", "user", "msg3");
    assert_eq!(mgr.get_history("web:user1").len(), 3);

    mgr.truncate_history("web:user1", 0);
    assert!(mgr.get_history("web:user1").is_empty());
}

#[test]
fn test_truncate_history_nonexistent_session_silent_return() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    // Should not panic
    mgr.truncate_history("nonexistent:key", 5);
}

// SAN-05 单一真相源契约（2026-09-16）：退化/含分隔符键不再报错，而是消毒成
// 合法段落盘（save_chat_session 的 post-sanitize 守卫降级为不变量 tripwire）。
#[test]
fn test_save_chat_session_invalid_key_dot_normalizes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

    mgr.add_message(".", "user", "test");
    let result = mgr.save_chat_session(".");
    assert!(result.is_ok(), "degenerate key normalizes: {result:?}");
    assert!(dir.path().join("sessions").join("_.json").exists());
}

#[test]
fn test_save_chat_session_invalid_key_slash_normalizes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

    mgr.add_message("bad/key", "user", "test");
    let result = mgr.save_chat_session("bad/key");
    assert!(result.is_ok(), "separator collapses: {result:?}");
    assert!(dir.path().join("sessions").join("bad_key.json").exists());
}

#[test]
fn test_save_chat_session_invalid_key_backslash_normalizes() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

    mgr.add_message("bad\\key", "user", "test");
    let result = mgr.save_chat_session("bad\\key");
    assert!(result.is_ok(), "separator collapses: {result:?}");
    assert!(dir.path().join("sessions").join("bad_key.json").exists());
}

#[test]
fn test_message_serialization_deserialization() {
    let msg = Message {
        role: "assistant".to_string(),
        content: "hello world".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: Some("2026-01-01T00:00:00Z".to_string()),
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.role, "assistant");
    assert_eq!(deserialized.content, "hello world");
    assert_eq!(
        deserialized.timestamp,
        Some("2026-01-01T00:00:00Z".to_string())
    );
}

#[test]
fn test_message_with_tool_calls_serialization() {
    let msg = Message {
        role: "assistant".to_string(),
        content: String::new(),
        tool_calls: vec![ToolCall {
            id: "call_abc".to_string(),
            call_type: Some("function".to_string()),
            function: Some(FunctionCall {
                name: "read_file".to_string(),
                arguments: r#"{"path":"/tmp/test.txt"}"#.to_string(),
            }),
        }],
        tool_call_id: None,
        timestamp: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    let deserialized: Message = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.tool_calls.len(), 1);
    assert_eq!(deserialized.tool_calls[0].id, "call_abc");
    let func = deserialized.tool_calls[0].function.as_ref().unwrap();
    assert_eq!(func.name, "read_file");
}

#[test]
fn test_tool_call_serialization() {
    let tc = ToolCall {
        id: "tc_001".to_string(),
        call_type: Some("function".to_string()),
        function: Some(FunctionCall {
            name: "execute".to_string(),
            arguments: "{}".to_string(),
        }),
    };
    let json = serde_json::to_string(&tc).unwrap();
    let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, "tc_001");
    assert_eq!(deserialized.call_type, Some("function".to_string()));
}

#[test]
fn test_tool_call_skip_none_fields() {
    let tc = ToolCall {
        id: "tc_002".to_string(),
        call_type: None,
        function: None,
    };
    let json = serde_json::to_string(&tc).unwrap();
    // call_type and function should be absent from JSON since they are None
    assert!(!json.contains("call_type"));
    assert!(!json.contains("function"));
    let deserialized: ToolCall = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.id, "tc_002");
    assert!(deserialized.call_type.is_none());
    assert!(deserialized.function.is_none());
}

#[test]
fn test_function_call_serialization() {
    let fc = FunctionCall {
        name: "search".to_string(),
        arguments: r#"{"query":"test"}"#.to_string(),
    };
    let json = serde_json::to_string(&fc).unwrap();
    let deserialized: FunctionCall = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "search");
    assert_eq!(deserialized.arguments, r#"{"query":"test"}"#);
}

#[test]
fn test_chat_session_serialization_roundtrip() {
    let session = ChatSession {
        key: "web:user1".to_string(),
        messages: vec![
            Message {
                role: "user".to_string(),
                content: "hello".to_string(),
                tool_calls: Vec::new(),
                tool_call_id: None,
                timestamp: Some("2026-01-01T12:00:00Z".to_string()),
            },
            Message {
                role: "assistant".to_string(),
                content: "hi".to_string(),
                tool_calls: vec![ToolCall {
                    id: "call_1".to_string(),
                    call_type: Some("function".to_string()),
                    function: Some(FunctionCall {
                        name: "tool1".to_string(),
                        arguments: "{}".to_string(),
                    }),
                }],
                tool_call_id: None,
                timestamp: Some("2026-01-01T12:00:01Z".to_string()),
            },
        ],
        summary: Some("A greeting exchange".to_string()),
        created: "2026-01-01T12:00:00Z".to_string(),
        updated: "2026-01-01T12:00:01Z".to_string(),
    };

    let json = serde_json::to_string_pretty(&session).unwrap();
    let deserialized: ChatSession = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.key, "web:user1");
    assert_eq!(deserialized.messages.len(), 2);
    assert_eq!(
        deserialized.summary,
        Some("A greeting exchange".to_string())
    );
    assert_eq!(deserialized.created, "2026-01-01T12:00:00Z");
    assert_eq!(deserialized.messages[0].role, "user");
    assert_eq!(deserialized.messages[1].tool_calls.len(), 1);
}

#[test]
fn test_chat_session_skip_none_summary() {
    let session = ChatSession {
        key: "web:user1".to_string(),
        messages: Vec::new(),
        summary: None,
        created: "2026-01-01T00:00:00Z".to_string(),
        updated: "2026-01-01T00:00:00Z".to_string(),
    };
    let json = serde_json::to_string(&session).unwrap();
    // summary should be absent from JSON when None
    assert!(!json.contains("summary"));
    let deserialized: ChatSession = serde_json::from_str(&json).unwrap();
    assert!(deserialized.summary.is_none());
}

#[test]
fn test_message_skip_empty_tool_calls() {
    let msg = Message {
        role: "user".to_string(),
        content: "hi".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: None,
    };
    let json = serde_json::to_string(&msg).unwrap();
    // tool_calls and tool_call_id should be absent
    assert!(!json.contains("tool_calls"));
    assert!(!json.contains("tool_call_id"));
}

#[test]
fn test_save_chat_session_no_storage_path() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "hello");
    // No storage path configured, should return Ok
    let result = mgr.save_chat_session("web:user1");
    assert!(result.is_ok());
}

#[test]
fn test_save_chat_session_nonexistent_key_no_storage() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    // Non-existent key with no storage path, should return Ok (early return for None)
    let result = mgr.save_chat_session("nonexistent");
    assert!(result.is_ok());
}

#[test]
fn test_save_chat_session_nonexistent_key_with_storage() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    // Non-existent key, should return Ok (no session found to save)
    let result = mgr.save_chat_session("nonexistent");
    assert!(result.is_ok());
}

#[test]
fn test_get_nonexistent_session() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    assert!(mgr.get("nonexistent").is_none());
}

#[test]
fn test_remove_nonexistent_session() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    assert!(mgr.remove("nonexistent").is_none());
}

#[test]
fn test_session_counter_increments() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let s1 = mgr.create_session("web", "u1", "c1");
    let s2 = mgr.create_session("web", "u2", "c2");
    let s3 = mgr.create_session("web", "u3", "c3");
    assert!(s1.id != s2.id);
    assert!(s2.id != s3.id);
    // All should start with "sess_"
    assert!(s1.id.starts_with("sess_"));
    assert!(s2.id.starts_with("sess_"));
}

#[test]
fn test_session_metadata_default_empty() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.create_session("web", "u1", "c1");
    assert!(session.metadata.is_empty());
}

#[test]
fn test_get_history_nonexistent_session() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let history = mgr.get_history("nonexistent:key");
    assert!(history.is_empty());
}

// ==================== Additional requested tests ====================

#[test]
fn test_with_default_timeout_custom_timeout_behavior() {
    // Verify that SessionMgr::new with a custom timeout correctly controls
    // expiry behavior, while with_default_timeout uses 3600s.
    // Custom short timeout: sessions should expire quickly.
    let short_mgr = SessionMgr::new(Duration::from_millis(1));
    let s = short_mgr.create_session("web", "u1", "c1");
    assert_eq!(short_mgr.count(), 1);

    // Wait for the session to exceed the 1ms timeout
    std::thread::sleep(std::time::Duration::from_millis(50));
    let removed = short_mgr.cleanup_expired();
    assert_eq!(removed, 1);
    assert_eq!(short_mgr.count(), 0);
    // Session ID should no longer be retrievable
    assert!(short_mgr.get(&s.id).is_none());

    // with_default_timeout uses 3600s, so sessions should survive cleanup.
    let default_mgr = SessionMgr::with_default_timeout();
    default_mgr.create_session("web", "u1", "c1");
    assert_eq!(default_mgr.cleanup_expired(), 0);
    assert_eq!(default_mgr.count(), 1);
}

#[test]
fn test_shutdown_sessions_inaccessible_after_shutdown() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let s1 = mgr.create_session("web", "u1", "c1");
    let s2 = mgr.create_session("rpc", "u2", "c2");
    assert_eq!(mgr.count(), 2);

    mgr.shutdown();

    // All sessions should be gone
    assert_eq!(mgr.count(), 0);
    assert!(mgr.get(&s1.id).is_none());
    assert!(mgr.get(&s2.id).is_none());

    // Operations on cleared IDs should not panic
    mgr.touch(&s1.id);
    assert!(mgr.remove(&s1.id).is_none());

    // The manager should still be usable after shutdown (new sessions work)
    let s3 = mgr.create_session("web", "u3", "c3");
    assert_eq!(mgr.count(), 1);
    assert!(mgr.get(&s3.id).is_some());
}

#[test]
fn test_stats_tracks_active_and_chat_sessions_correctly() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));

    // Initially both counters should be zero
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 0);
    assert_eq!(stats["chat_sessions"], 0);

    // Add connection sessions
    mgr.create_session("web", "u1", "c1");
    mgr.create_session("rpc", "u2", "c2");
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 2);
    assert_eq!(stats["chat_sessions"], 0);

    // Add chat sessions
    mgr.add_message("web:user1", "user", "hello");
    mgr.get_or_create_chat("rpc:user2");
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 2);
    assert_eq!(stats["chat_sessions"], 2);

    // Remove one connection session
    mgr.remove(&mgr.get("sess_0").unwrap().id);
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 1);
    assert_eq!(stats["chat_sessions"], 2);

    // Shutdown clears connection sessions but not chat sessions
    mgr.shutdown();
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 0);
    assert_eq!(stats["chat_sessions"], 2);
}

#[test]
fn test_access_nonexistent_session_operations_safe() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));

    // get on non-existent ID returns None
    assert!(mgr.get("does_not_exist").is_none());

    // remove on non-existent ID returns None
    assert!(mgr.remove("does_not_exist").is_none());

    // touch on non-existent ID is a no-op
    mgr.touch("does_not_exist");

    // get_history on non-existent chat key returns empty vec
    assert!(mgr.get_history("no:such:key").is_empty());

    // get_summary on non-existent chat key returns None
    assert!(mgr.get_summary("no:such:key").is_none());

    // count should still be zero
    assert_eq!(mgr.count(), 0);

    // stats should report zero for everything
    let stats = mgr.stats();
    assert_eq!(stats["active_sessions"], 0);
    assert_eq!(stats["chat_sessions"], 0);
}

#[test]
fn test_touch_on_expired_session_is_no_op() {
    // Create a manager with a very short timeout
    let mgr = SessionMgr::new(Duration::from_millis(1));
    let session = mgr.create_session("web", "u1", "c1");
    let id = session.id.clone();

    // Wait for the session to expire
    std::thread::sleep(std::time::Duration::from_millis(50));

    // Clean up expired sessions
    let removed = mgr.cleanup_expired();
    assert_eq!(removed, 1);
    assert_eq!(mgr.count(), 0);

    // Touch the now-expired (removed) session ID — should not panic
    // and should not re-create the session
    mgr.touch(&id);
    assert_eq!(mgr.count(), 0);
    assert!(mgr.get(&id).is_none());
}

// ==================== Additional edge case tests for comprehensive coverage ====================

#[test]
fn test_concurrent_session_access() {
    use std::sync::Arc;
    let mgr = Arc::new(SessionMgr::new(Duration::from_secs(3600)));
    let mut handles = vec![];

    // Spawn multiple threads creating sessions concurrently
    for i in 0..10 {
        let mgr_clone = Arc::clone(&mgr);
        let handle = std::thread::spawn(move || {
            mgr_clone.create_session("web", &format!("user{}", i), &format!("chat{}", i));
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // All sessions should be created successfully
    assert_eq!(mgr.count(), 10);
}

#[test]
fn test_load_chat_sessions_from_corrupted_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();

    // Create a corrupted JSON file
    std::fs::create_dir_all(&path).unwrap();
    let corrupted_file = std::path::PathBuf::from(&path).join("web_user1.json");
    std::fs::write(&corrupted_file, "this is not valid json {{{").unwrap();

    // Should not panic, just skip corrupted files
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    assert_eq!(mgr.count(), 0);
}

#[test]
fn test_persistence_with_special_characters_in_key() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();

    {
        let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);
        mgr.add_message("web:user:with:colons", "user", "test message");
    }

    // Reload and verify
    let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    let history = mgr2.get_history("web:user:with:colons");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "test message");
}

#[test]
fn test_save_chat_session_with_empty_messages() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

    // Create session with empty messages
    let session = mgr.get_or_create_chat("web:empty_session");
    assert!(session.messages.is_empty());

    // Should save successfully
    let result = mgr.save_chat_session("web:empty_session");
    assert!(result.is_ok());

    // Reload and verify
    let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    let loaded_session = mgr2.get_or_create_chat("web:empty_session");
    assert!(loaded_session.messages.is_empty());
}

#[test]
fn test_truncate_history_with_large_keep_value() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));

    // Add some messages
    for i in 0..5 {
        mgr.add_message("web:user1", "user", &format!("msg {}", i));
    }

    // Try to keep more messages than exist
    mgr.truncate_history("web:user1", 1000);
    let history = mgr.get_history("web:user1");
    // Should keep all 5 messages
    assert_eq!(history.len(), 5);
}

#[test]
fn test_session_metadata_manipulation() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    let session = mgr.create_session("web", "user1", "chat1");

    // Get the session and manipulate metadata
    let mut session_mut = mgr.get(&session.id).unwrap();
    session_mut
        .metadata
        .insert("user_agent".to_string(), "Mozilla".to_string());
    session_mut
        .metadata
        .insert("ip".to_string(), "127.0.0.1".to_string());

    // The modified session won't affect the stored one since we work with clones
    // This test demonstrates the clone behavior
    let stored = mgr.get(&session.id).unwrap();
    assert!(stored.metadata.is_empty());
}

#[test]
fn test_multiple_summaries_for_same_session() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));
    mgr.add_message("web:user1", "user", "hello");

    mgr.set_summary("web:user1", "First summary");
    assert_eq!(
        mgr.get_summary("web:user1"),
        Some("First summary".to_string())
    );

    mgr.set_summary("web:user1", "Updated summary");
    assert_eq!(
        mgr.get_summary("web:user1"),
        Some("Updated summary".to_string())
    );
}

#[test]
fn test_save_chat_session_invalid_directory_permission() {
    // This test verifies graceful handling of filesystem errors
    let mgr = SessionMgr::with_storage(
        Duration::from_secs(3600),
        "/root/nonexistent/path/that/likely/fails",
    );

    // Should not panic, just fail silently
    mgr.add_message("web:user_failing", "user", "test");
    let _result = mgr.save_chat_session("web:user_failing");
    // The result might be Ok or Err depending on filesystem, but shouldn't panic
    // We just verify the manager is still functional
    let history = mgr.get_history("web:user_failing");
    assert!(!history.is_empty());
}

#[test]
fn test_save_chat_session_with_complex_serialization() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions").to_string_lossy().to_string();
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &path);

    // Create a complex message with all fields populated
    let complex_msg = Message {
        role: "assistant".to_string(),
        content: "Complex response".to_string(),
        tool_calls: vec![
            ToolCall {
                id: "call_1".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "tool1".to_string(),
                    arguments: r#"{"arg1":"value1"}"#.to_string(),
                }),
            },
            ToolCall {
                id: "call_2".to_string(),
                call_type: Some("function".to_string()),
                function: Some(FunctionCall {
                    name: "tool2".to_string(),
                    arguments: r#"{"arg2":"value2"}"#.to_string(),
                }),
            },
        ],
        tool_call_id: None,
        timestamp: Some("2026-06-11T12:00:00Z".to_string()),
    };

    mgr.add_full_message("web:complex", complex_msg);

    // Add a tool response
    let tool_response = Message {
        role: "tool".to_string(),
        content: "Tool result".to_string(),
        tool_calls: vec![],
        tool_call_id: Some("call_1".to_string()),
        timestamp: Some("2026-06-11T12:00:01Z".to_string()),
    };

    mgr.add_full_message("web:complex", tool_response);

    // Set a summary
    mgr.set_summary("web:complex", "Complex tool interaction");

    // Reload and verify all data persisted correctly
    let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), &path);
    let history = mgr2.get_history("web:complex");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].tool_calls.len(), 2);
    assert_eq!(history[1].tool_call_id, Some("call_1".to_string()));
    assert_eq!(
        mgr2.get_summary("web:complex"),
        Some("Complex tool interaction".to_string())
    );
}

#[test]
fn test_concurrent_chat_session_operations() {
    use std::sync::Arc;
    use std::sync::Barrier;
    let mgr = Arc::new(SessionMgr::new(Duration::from_secs(3600)));
    let barrier = Arc::new(Barrier::new(3));
    let mut handles = vec![];

    // Spawn multiple threads performing different operations concurrently
    for i in 0..3 {
        let mgr_clone = Arc::clone(&mgr);
        let barrier_clone = Arc::clone(&barrier);
        let handle = std::thread::spawn(move || {
            barrier_clone.wait();
            mgr_clone.add_message(
                &format!("web:user{}", i),
                "user",
                &format!("message from thread {}", i),
            );
            mgr_clone.set_summary(
                &format!("web:user{}", i),
                &format!("Summary from thread {}", i),
            );
            mgr_clone.get_history(&format!("web:user{}", i));
        });
        handles.push(handle);
    }

    // Wait for all threads to complete
    for handle in handles {
        handle.join().unwrap();
    }

    // Verify all operations completed successfully
    for i in 0..3 {
        let history = mgr.get_history(&format!("web:user{}", i));
        assert_eq!(history.len(), 1);
        assert_eq!(history[0].content, format!("message from thread {}", i));
        assert_eq!(
            mgr.get_summary(&format!("web:user{}", i)),
            Some(format!("Summary from thread {}", i))
        );
    }
}

#[test]
fn test_chat_session_timestamp_auto_fill() {
    let mgr = SessionMgr::new(Duration::from_secs(3600));

    // Create message without timestamp
    let msg_no_timestamp = Message {
        role: "user".to_string(),
        content: "message without timestamp".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: None,
    };

    mgr.add_full_message("web:timestamp_test", msg_no_timestamp);

    let history = mgr.get_history("web:timestamp_test");
    assert_eq!(history.len(), 1);
    // Timestamp should be auto-filled
    assert!(history[0].timestamp.is_some());

    // Verify the timestamp is in valid RFC3339 format
    let timestamp_str = history[0].timestamp.as_ref().unwrap();
    assert!(!timestamp_str.is_empty()); // Basic check that timestamp was filled
}

#[test]
fn test_message_fields_serialization_order() {
    let msg = Message {
        role: "user".to_string(),
        content: "test".to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: Some("2026-06-11T00:00:00Z".to_string()),
    };

    let json = serde_json::to_string(&msg).unwrap();
    // Verify that the JSON contains expected fields
    assert!(json.contains("\"role\""));
    assert!(json.contains("\"content\""));
    assert!(json.contains("\"timestamp\""));
    // Empty collections should be skipped
    assert!(!json.contains("\"tool_calls\""));
    assert!(!json.contains("\"tool_call_id\""));
}

// ===========================================================================
// S12b batch（quality-hardening goal 冲刺）：load_chat_sessions 目录扫描防御臂
// ===========================================================================

#[test]
fn s12b_load_chat_sessions_skips_non_json_and_corrupt_files() {
    use std::io::Write as _;

    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("sessions");
    std::fs::create_dir_all(&storage).unwrap();

    // 非 .json 文件 → extension 守卫 continue
    let mut txt = std::fs::File::create(storage.join("notes.txt")).unwrap();
    txt.write_all(b"not a session").unwrap();
    // .json 但内容损坏 → 反序列化失败被静默跳过
    let mut bad = std::fs::File::create(storage.join("corrupt.json")).unwrap();
    bad.write_all(b"{ not valid json !!!").unwrap();
    // 合法会话文件 → 正常载入
    let good = r#"{
        "key": "legacy",
        "messages": [],
        "created": "2026-08-23T00:00:00Z",
        "updated": "2026-08-23T00:00:00Z"
    }"#;
    let mut ok = std::fs::File::create(storage.join("legacy.json")).unwrap();
    ok.write_all(good.as_bytes()).unwrap();

    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &storage.to_string_lossy());
    // 只有合法那条真正进内存
    let history = mgr.get_history("legacy");
    assert_eq!(history.len(), 0, "空消息数组载入为空历史（不 panic 即可）");
    // 坏文件没有把垃圾塞进任何键
    assert_eq!(mgr.get_history("corrupt").len(), 0);
}

// BUG-20261025-44（BUG-HUNT R121）：损坏会话 json 静默跳过 → loud 留痕回归锁。
// 跳过语义不变（坏档不进索引、好档照常载入）；warn 本身无法在进程内断言
// （无 tracing 捕获夹具，BUG-43 同族诚实边界），本锁钉「混档目录载入不炸、
// 好坏分拣正确、原子写残留被扩展名过滤无视」这一可观察行为。
#[test]
fn load_chat_sessions_corrupt_file_skipped_with_valid_ones_loaded() {
    let dir = tempfile::tempdir().unwrap();
    let storage = dir.path().join("sessions");
    std::fs::create_dir_all(&storage).unwrap();

    // 好档（含消息与 summary，验证字段保真载入）。
    let good = r#"{
        "key": "web:good",
        "messages": [{"role": "user", "content": "hi"}],
        "summary": "s",
        "created": "2026-10-25T00:00:00Z",
        "updated": "2026-10-25T00:00:00Z"
    }"#;
    std::fs::write(storage.join("web_good.json"), good).unwrap();
    // 坏档（写中崩溃撕裂形态）。
    std::fs::write(storage.join("torn.json"), r#"{"key": "web:torn", "mess"#).unwrap();
    // 原子写残留（.json.tmp 扩展名）必须被扩展名过滤无视。
    std::fs::write(storage.join("torn.json.tmp"), "garbage").unwrap();

    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), &storage.to_string_lossy());
    let loaded = mgr.chat_sessions.lock();
    assert!(loaded.contains_key("web:good"), "好档必须载入");
    assert!(!loaded.contains_key("web:torn"), "坏档不得进索引");
}

/// BUG-20261025-51 回归锁：同键并发写不产损坏文件、成功后零临时残留。
/// 旧固定 `{key}.json.tmp` 实现下，多线程 add_message 的 File::create 截断
/// 互踩可把混合字节 rename 落盘（概率性损坏——本测试为并发烟测 + 零残留
/// 不变量；确定性保障在 nemesis_utils::write_file_atomic 自身测试 + 本次
/// 委托消除全部手搓 tmp+rename 路径）。
#[test]
fn test_concurrent_same_key_saves_produce_valid_file_and_no_tmp_residue() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("sessions");
    let mgr = std::sync::Arc::new(SessionMgr::with_storage(
        Duration::from_secs(3600),
        &path.to_string_lossy(),
    ));

    const THREADS: usize = 8;
    const MSGS: usize = 25;
    let mut handles = Vec::new();
    for t in 0..THREADS {
        let mgr = mgr.clone();
        handles.push(std::thread::spawn(move || {
            for i in 0..MSGS {
                mgr.add_message(
                    "telegram:123",
                    "user",
                    &format!("t{t}-m{i}-{}", "x".repeat(512)),
                );
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }

    // 最终文件可整体解析（损坏 JSON = 混合字节落盘的证据）。
    let file = path.join("telegram_123.json");
    let raw = std::fs::read_to_string(&file).unwrap();
    let session: ChatSession = serde_json::from_str(&raw).unwrap();
    assert_eq!(session.key, "telegram:123");
    assert!(session.messages.len() >= MSGS);

    // 成功路径零残留：目录里只有目标 .json（唯一临时名也不允许残留）。
    let residue: Vec<_> = std::fs::read_dir(&path)
        .unwrap()
        .map(|e| e.unwrap().file_name())
        .filter(|n| n.to_string_lossy() != "telegram_123.json")
        .collect();
    assert!(residue.is_empty(), "residue: {residue:?}");
}

// —— BUG-20261028-95 红证：会话持久化失败不得静默 ——

#[derive(Clone)]
struct CaptureWriter(std::sync::Arc<std::sync::Mutex<Vec<u8>>>);

impl std::io::Write for CaptureWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(buf);
        Ok(buf.len())
    }
    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl tracing_subscriber::fmt::MakeWriter<'_> for CaptureWriter {
    type Writer = CaptureWriter;
    fn make_writer(&self) -> Self::Writer {
        self.clone()
    }
}

/// 保存失败（存储路径被普通文件挡死，write_file_atomic 必 Err）必须留
/// 可见信号（warn 日志）——旧行为 `let _ = save` 全静默，磁盘故障时用户
/// 的会话历史悄悄丢，重启后凭空消失，无任何现场可查。
#[test]
fn chat_session_save_failure_warns_not_silent() {
    let dir = tempfile::tempdir().unwrap();
    // 用普通文件占住存储目录位：save 时 create_dir_all/写 tmp 全部失败。
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let mgr = SessionMgr::with_storage(
        std::time::Duration::from_secs(3600),
        blocker.to_str().unwrap(),
    );

    let captured = std::sync::Arc::new(std::sync::Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::WARN)
        .with_ansi(false)
        .with_writer(CaptureWriter(captured.clone()))
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);

    mgr.add_message("web/save-fail-case", "user", "hello");

    let logs = String::from_utf8(captured.lock().unwrap().clone()).unwrap();
    assert!(
        logs.contains("Failed to save chat session"),
        "save 失败必须有可见信号（warn），实际日志: {logs}"
    );
}

// --- BUG-20261030-213：同键并发保存 lost-update（确定性构造）-------------
// save_chat_session 快照在状态锁内、落盘在锁外——旧快照的 rename 可能晚于
// 新快照落盘（last-rename-wins = 已完成保存的消息重启后丢失；BUG-51 只修了
// 撕裂写，没修这个序问题）。修复 = 快照+落盘用同一把 save 闸串行化。
// 确定性复现：save_delay 测试钩子把 T0 钉在「快照后、落盘前」的停顿窗口，
// T1 的完整保存整体落在窗口内——修复前 T0 的旧快照 rename 必然最后落地
// （丢 T1 消息）；修复后 T1 阻塞在闸上、快照+rename 都晚于 T0，盘上完整。
#[test]
fn test_concurrent_same_key_saves_never_lose_messages() {
    let dir = std::env::temp_dir().join(format!(
        "nemesis_session_race_{}_{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    let mgr = SessionMgr::with_storage(Duration::from_secs(3600), dir.to_str().unwrap());
    // 底噪大消息：让每次保存的序列化+写盘都有稳定耗时（窗口不依赖机器速度）。
    mgr.add_message(
        "gw:race",
        "user",
        &format!("base-{}", "x".repeat(4 * 1024 * 1024)),
    );
    // 一次性钩子：只延迟 T0 的首个保存（消费即归零，T1 的保存不受影响）。
    mgr.save_delay_ms
        .store(1200, std::sync::atomic::Ordering::SeqCst);
    let mgr = std::sync::Arc::new(mgr);
    // T0：先快照（含 base），随后在钩子停顿里挂起 1200ms 再落盘。
    let t0 = {
        let mgr = mgr.clone();
        std::thread::spawn(move || {
            mgr.add_message("gw:race", "user", "from-t0");
        })
    };
    // 等 T0 进入停顿窗口（快照已拍），再让 T1 完成整轮保存。
    std::thread::sleep(Duration::from_millis(300));
    mgr.add_message("gw:race", "user", "from-t1");
    t0.join().unwrap();
    // 直接读盘比对（无收尾保存）。
    let mgr2 = SessionMgr::with_storage(Duration::from_secs(3600), dir.to_str().unwrap());
    let disk: Vec<String> = mgr2
        .get_history("gw:race")
        .iter()
        .map(|m| m.content.chars().take(16).collect())
        .collect();
    let _ = std::fs::remove_dir_all(&dir);
    assert!(
        disk.iter().any(|c| c == "from-t0"),
        "盘上缺 from-t0：{disk:?}"
    );
    assert!(
        disk.iter().any(|c| c == "from-t1"),
        "盘上缺 from-t1（T0 旧快照 rename 后落地覆盖了 T1 的已完成保存）：{disk:?}"
    );
}
