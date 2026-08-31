use super::*;

// --- Session tests ---

#[test]
fn test_session_new() {
    let session = Session::new("web:chat1", "web", "chat1");
    assert_eq!(session.session_key, "web:chat1");
    assert_eq!(session.channel, "web");
    assert_eq!(session.chat_id, "chat1");
    assert!(!session.busy);
    assert!(session.last_channel.is_none());
    assert!(session.last_chat_id.is_none());
}

// --- SessionManager tests ---

#[test]
fn test_session_manager_get_or_create() {
    let mgr = SessionManager::with_default_timeout();
    assert!(mgr.is_empty());

    let session = mgr.get_or_create("web:chat1", "web", "chat1");
    assert_eq!(session.session_key, "web:chat1");
    assert_eq!(mgr.len(), 1);

    // Second call returns same session.
    let session2 = mgr.get_or_create("web:chat1", "web", "chat1");
    assert_eq!(mgr.len(), 1);
    assert_eq!(session2.session_key, session.session_key);
}

#[test]
fn test_session_manager_set_busy() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:chat1", "web", "chat1");

    assert_eq!(mgr.is_busy("web:chat1"), Some(false));
    assert!(mgr.set_busy("web:chat1", true));
    assert_eq!(mgr.is_busy("web:chat1"), Some(true));

    assert!(!mgr.set_busy("nonexistent", true));
    assert_eq!(mgr.is_busy("nonexistent"), None);
}

#[test]
fn test_session_manager_last_channel_chat_id() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:chat1", "web", "chat1");

    mgr.set_last_channel("web:chat1", "telegram");
    mgr.set_last_chat_id("web:chat1", "chat42");

    let session = mgr.get_or_create("web:chat1", "web", "chat1");
    assert_eq!(session.last_channel.as_deref(), Some("telegram"));
    assert_eq!(session.last_chat_id.as_deref(), Some("chat42"));
}

#[test]
fn test_session_manager_cleanup_expired() {
    let mgr = SessionManager::new(Duration::from_millis(50));
    mgr.get_or_create("web:chat1", "web", "chat1");

    // Force session into the past.
    {
        let mut session = mgr.sessions.get_mut("web:chat1").unwrap();
        session.last_active = Local::now() - chrono::Duration::seconds(60);
    }

    let removed = mgr.cleanup_expired();
    assert_eq!(removed.len(), 1);
    assert!(mgr.is_empty());
}

// --- SessionStore tests ---

#[test]
fn test_session_store_in_memory() {
    let store = SessionStore::new_in_memory();

    let session = store.get_or_create("test:key1");
    assert_eq!(session.key, "test:key1");
    assert!(session.messages.is_empty());
    assert!(session.summary.is_empty());
}

#[test]
fn test_session_store_history() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:key1");

    let messages = vec![
        StoredMessage {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        StoredMessage {
            role: "assistant".to_string(),
            content: "Hi there!".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];

    store.set_history("test:key1", messages.clone());
    let history = store.get_history("test:key1");
    assert_eq!(history.len(), 2);
    assert_eq!(history[0].content, "Hello");
    assert_eq!(history[1].content, "Hi there!");
}

#[test]
fn test_session_store_summary() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:key1");

    assert!(store.get_summary("test:key1").is_empty());

    store.set_summary("test:key1", "This is a summary of the conversation.");
    assert_eq!(
        store.get_summary("test:key1"),
        "This is a summary of the conversation."
    );
}

#[test]
fn test_session_store_truncate() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:key1");

    let messages: Vec<StoredMessage> = (0..10)
        .map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg_{}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();

    store.set_history("test:key1", messages);
    store.truncate_history("test:key1", 4);

    let history = store.get_history("test:key1");
    assert_eq!(history.len(), 4);
    assert_eq!(history[0].content, "msg_6");
    assert_eq!(history[3].content, "msg_9");
}

#[test]
fn test_session_store_disk_persistence() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());

    store.get_or_create("disk:key1");
    store.set_summary("disk:key1", "Test summary");
    store.set_history(
        "disk:key1",
        vec![StoredMessage {
            role: "user".to_string(),
            content: "Hello from disk".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
    );
    store.save("disk:key1").unwrap();

    // Create a new store from the same directory.
    let store2 = SessionStore::new_with_storage(dir.path());
    assert!(store2.contains("disk:key1"));
    assert_eq!(store2.get_summary("disk:key1"), "Test summary");
    let history = store2.get_history("disk:key1");
    assert_eq!(history.len(), 1);
    assert_eq!(history[0].content, "Hello from disk");
}

#[test]
fn test_session_store_save_invalid_key() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    // The key ".." should be rejected (it becomes "." after sanitize, which is rejected).
    store.get_or_create("..");
    let result = store.save("..");
    assert!(result.is_err());
}

#[test]
fn test_session_store_no_persistence() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("mem:key1");
    // save should succeed silently when no storage dir.
    assert!(store.save("mem:key1").is_ok());
}

// --- Token estimation tests ---

#[test]
fn test_estimate_tokens_empty() {
    assert_eq!(estimate_tokens(""), 0);
}

#[test]
fn test_estimate_tokens_ascii() {
    // "Hello world" = 11 chars, 11*2/5 = 4
    assert_eq!(estimate_tokens("Hello world"), 4);
}

#[test]
fn test_estimate_tokens_for_turns() {
    let turns = vec![
        ConversationTurn {
            role: "user".to_string(),
            content: "Hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "World".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    // "Hello" = 5 chars, "World" = 5 chars, total = 10, 10*2/5 = 4
    assert_eq!(estimate_tokens_for_turns(&turns), 4);
}

// --- Force compression tests ---

#[test]
fn test_force_compress_short() {
    let history: Vec<ConversationTurn> = (0..4)
        .map(|i| ConversationTurn {
            role: if i == 0 { "system" } else { "user" }.to_string(),
            content: format!("msg_{}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();

    let result = force_compress_turns(&history);
    assert_eq!(result.len(), 4);
    assert_eq!(result, history);
}

#[test]
fn test_force_compress_long() {
    let history: Vec<ConversationTurn> = (0..10)
        .map(|i| ConversationTurn {
            role: if i == 0 {
                "system".to_string()
            } else {
                format!("role_{}", i)
            },
            content: format!("msg_{}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();

    let result = force_compress_turns(&history);
    assert!(result.len() < history.len());
    assert_eq!(result[0].content, "msg_0"); // System prompt kept
    assert!(result[1].content.contains("Emergency compression")); // Compression note
    assert_eq!(result.last().unwrap().content, "msg_9"); // Last message kept
}

// --- Sanitize filename tests ---

#[test]
fn test_sanitize_filename() {
    assert_eq!(sanitize_filename("web:chat1"), "web_chat1");
    assert_eq!(sanitize_filename("rpc:12345"), "rpc_12345");
    assert_eq!(sanitize_filename("simple"), "simple");
    assert_eq!(sanitize_filename("a\\b/c:d"), "a_b_c_d");
}

// --- Internal channel tests ---

#[test]
fn test_is_internal_channel() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("rpc"));
    assert!(!is_internal_channel("discord"));
}

// --- StoredMessage conversion tests ---

#[test]
fn test_stored_message_roundtrip() {
    let turn = ConversationTurn {
        role: "assistant".to_string(),
        content: "Let me search for that.".to_string(),
        tool_calls: vec![crate::types::ToolCallInfo {
            id: "tc_1".to_string(),
            name: "search".to_string(),
            arguments: r#"{"query":"rust"}"#.to_string(),
        }],
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };

    let stored: StoredMessage = (&turn).into();
    assert_eq!(stored.role, "assistant");
    assert_eq!(stored.tool_calls.len(), 1);

    let back: ConversationTurn = stored.into();
    assert_eq!(back.role, "assistant");
    assert_eq!(back.tool_calls.len(), 1);
    assert_eq!(back.tool_calls[0].name, "search");
}

// --- Additional session coverage tests ---

#[test]
fn test_session_touch_updates_last_active() {
    let mut session = Session::new("web:chat1", "web", "chat1");
    let before = session.last_active;
    session.touch();
    assert!(session.last_active >= before);
}

#[test]
fn test_session_serialization_roundtrip() {
    let session = Session::new("web:chat1", "web", "chat1");
    let json = serde_json::to_string(&session).unwrap();
    let parsed: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.session_key, "web:chat1");
    assert_eq!(parsed.channel, "web");
    assert_eq!(parsed.chat_id, "chat1");
}

#[test]
fn test_session_manager_contains() {
    let mgr = SessionManager::with_default_timeout();
    assert!(!mgr.contains("web:chat1"));
    mgr.get_or_create("web:chat1", "web", "chat1");
    assert!(mgr.contains("web:chat1"));
}

#[test]
fn test_session_manager_remove() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:chat1", "web", "chat1");
    assert!(mgr.contains("web:chat1"));

    let removed = mgr.remove("web:chat1");
    assert!(removed.is_some());
    assert!(!mgr.contains("web:chat1"));

    let removed_again = mgr.remove("web:chat1");
    assert!(removed_again.is_none());
}

#[test]
fn test_session_manager_cleanup_with_timeout_no_expired() {
    let mgr = SessionManager::new(Duration::from_secs(3600));
    mgr.get_or_create("web:chat1", "web", "chat1");
    let removed = mgr.cleanup_expired();
    assert!(removed.is_empty());
    assert_eq!(mgr.len(), 1);
}

#[test]
fn test_session_manager_set_last_channel_nonexistent() {
    let mgr = SessionManager::with_default_timeout();
    // Should not panic when setting channel on nonexistent session
    mgr.set_last_channel("nonexistent", "web");
    mgr.set_last_chat_id("nonexistent", "chat1");
}

#[test]
fn test_session_store_set_history_nonexistent() {
    let store = SessionStore::new_in_memory();
    // Setting history on nonexistent session should do nothing
    store.set_history(
        "nonexistent",
        vec![StoredMessage {
            role: "user".to_string(),
            content: "test".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
    );
    assert!(store.get_history("nonexistent").is_empty());
}

#[test]
fn test_session_store_set_summary_nonexistent() {
    let store = SessionStore::new_in_memory();
    store.set_summary("nonexistent", "test summary");
    assert!(store.get_summary("nonexistent").is_empty());
}

#[test]
fn test_session_store_truncate_fewer_than_keep() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:trunc");
    store.set_history(
        "test:trunc",
        vec![StoredMessage {
            role: "user".to_string(),
            content: "msg".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
    );
    store.truncate_history("test:trunc", 10);
    let history = store.get_history("test:trunc");
    assert_eq!(history.len(), 1); // not truncated
}

#[test]
fn test_session_store_contains() {
    let store = SessionStore::new_in_memory();
    assert!(!store.contains("test:contains"));
    store.get_or_create("test:contains");
    assert!(store.contains("test:contains"));
}

#[test]
fn test_session_store_remove() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:remove");
    assert!(store.contains("test:remove"));
    let removed = store.remove("test:remove");
    assert!(removed.is_some());
    assert!(!store.contains("test:remove"));
}

#[test]
fn test_session_store_len_and_empty() {
    let store = SessionStore::new_in_memory();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);
    store.get_or_create("test:1");
    assert!(!store.is_empty());
    assert_eq!(store.len(), 1);
    store.get_or_create("test:2");
    assert_eq!(store.len(), 2);
}

#[test]
fn test_session_store_save_nonexistent_session() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    // Saving nonexistent session should succeed silently
    assert!(store.save("nonexistent").is_ok());
}

#[test]
fn test_stored_session_serialization() {
    let session = StoredSession {
        key: "test:ser".to_string(),
        messages: vec![StoredMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: vec![StoredToolCall {
                id: "tc_1".to_string(),
                name: "test".to_string(),
                arguments: "{}".to_string(),
            }],
            tool_call_id: Some("tc_1".to_string()),
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
        summary: "test summary".to_string(),
        summary_covers_up_to: None,
        created: Local::now(),
        updated: Local::now(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let parsed: StoredSession = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.key, "test:ser");
    assert_eq!(parsed.messages.len(), 1);
    assert_eq!(parsed.messages[0].tool_calls.len(), 1);
}

#[test]
fn test_estimate_tokens_for_messages() {
    let messages = vec![StoredMessage {
        role: "user".to_string(),
        content: "Hello world".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    let tokens = estimate_tokens_for_messages(&messages);
    assert!(tokens > 0);
}

#[test]
fn test_force_compress_exact_boundary() {
    // Test with exactly 5 messages (boundary for the > 4 check)
    let history: Vec<ConversationTurn> = (0..5)
        .map(|i| ConversationTurn {
            role: if i == 0 { "system" } else { "user" }.to_string(),
            content: format!("msg_{}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    let result = force_compress_turns(&history);
    assert!(result.len() <= history.len());
}

#[test]
fn test_force_compress_empty_conversation() {
    // History with just system and one message (no "conversation" part)
    let history = vec![
        ConversationTurn {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    let result = force_compress_turns(&history);
    // With only 2 messages (<=4), should return unchanged
    assert_eq!(result.len(), 2);
}

// --- Additional session coverage tests ---

#[test]
fn test_session_fields_after_create() {
    let session = Session::new("web:chat1", "web", "chat1");
    assert_eq!(session.session_key, "web:chat1");
    assert_eq!(session.channel, "web");
    assert_eq!(session.chat_id, "chat1");
    assert!(!session.busy);
}

#[test]
fn test_session_json_roundtrip() {
    let session = Session::new("web:chat1", "web", "chat1");
    let json = serde_json::to_string(&session).unwrap();
    let parsed: Session = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.session_key, "web:chat1");
    assert_eq!(parsed.channel, "web");
    assert_eq!(parsed.chat_id, "chat1");
}

#[test]
fn test_session_manager_with_default_timeout() {
    let mgr = SessionManager::with_default_timeout();
    let session = mgr.get_or_create("test-key", "web", "chat1");
    assert_eq!(session.session_key, "test-key");
    assert!(!session.busy);
}

#[test]
fn test_session_manager_set_last_channel() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("_default", "cli", "direct");
    mgr.set_last_channel("_default", "discord");
    let session = mgr.get_or_create("_default", "cli", "direct");
    assert_eq!(session.last_channel.as_deref(), Some("discord"));
}

#[test]
fn test_session_manager_set_last_chat_id() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("_default", "cli", "direct");
    mgr.set_last_chat_id("_default", "chat-99");
    let session = mgr.get_or_create("_default", "cli", "direct");
    assert_eq!(session.last_chat_id.as_deref(), Some("chat-99"));
}

#[test]
fn test_session_store_new_in_memory() {
    let store = SessionStore::new_in_memory();
    let data = store.get_or_create("test-key");
    assert!(data.messages.is_empty());
}

#[test]
fn test_session_store_set_and_get_history() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test-key"); // Must create first
    let messages = vec![
        StoredMessage {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        StoredMessage {
            role: "assistant".to_string(),
            content: "hi there".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    store.set_history("test-key", messages);
    let data = store.get_or_create("test-key");
    assert_eq!(data.messages.len(), 2);
    assert_eq!(data.messages[0].content, "hello");
}

#[test]
fn test_session_store_set_and_get_summary() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test-key"); // Must create first
    store.set_summary("test-key", "This is a summary of the conversation.");
    let summary = store.get_summary("test-key");
    assert_eq!(summary, "This is a summary of the conversation.");
}

#[test]
fn test_session_store_get_summary_nonexistent() {
    let store = SessionStore::new_in_memory();
    let summary = store.get_summary("nonexistent");
    assert!(summary.is_empty());
}

#[test]
fn test_estimate_tokens_basic() {
    assert_eq!(estimate_tokens(""), 0);
    // estimate_tokens uses char_count * 2 / 5, so needs at least 3 chars for > 0
    assert!(estimate_tokens("hello world") > 0);
}

#[test]
fn test_estimate_tokens_for_turns_basic() {
    let turns = vec![ConversationTurn {
        role: "user".to_string(),
        content: "Hello world".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    let tokens = estimate_tokens_for_turns(&turns);
    assert!(tokens > 0);
}

#[test]
fn test_estimate_tokens_for_turns_empty() {
    let turns: Vec<ConversationTurn> = vec![];
    let tokens = estimate_tokens_for_turns(&turns);
    assert_eq!(tokens, 0);
}

#[test]
fn test_stored_message_from_conversation_turn() {
    let turn = ConversationTurn {
        role: "user".to_string(),
        content: "hello".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };
    let stored: StoredMessage = StoredMessage::from(&turn);
    assert_eq!(stored.role, "user");
    assert_eq!(stored.content, "hello");
    assert_eq!(stored.timestamp, "2026-01-01T00:00:00Z");
}

// --- Additional session coverage ---

#[test]
fn test_session_store_truncate_history() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test-key");
    let msgs: Vec<StoredMessage> = (0..10)
        .map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    store.set_history("test-key", msgs);
    store.truncate_history("test-key", 3);
    let data = store.get_or_create("test-key");
    assert_eq!(data.messages.len(), 3);
    assert_eq!(data.messages[0].content, "msg 7");
}

#[test]
fn test_session_store_truncate_empty() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test-key");
    store.truncate_history("test-key", 5);
    let data = store.get_or_create("test-key");
    assert!(data.messages.is_empty());
}

#[test]
fn test_session_store_truncate_to_zero() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test-key");
    let msgs: Vec<StoredMessage> = (0..5)
        .map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    store.set_history("test-key", msgs);
    store.truncate_history("test-key", 0);
    let data = store.get_or_create("test-key");
    assert!(data.messages.is_empty());
}

#[test]
fn test_session_manager_get_or_create_default() {
    let mgr = SessionManager::with_default_timeout();
    let s1 = mgr.get_or_create("_default", "cli", "direct");
    let s2 = mgr.get_or_create("_default", "web", "chat1");
    assert_eq!(s1.session_key, s2.session_key);
}

#[test]
fn test_force_compress_with_many_messages() {
    let mut history: Vec<ConversationTurn> = vec![ConversationTurn {
        role: "system".to_string(),
        content: "You are helpful".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    for i in 0..20 {
        history.push(ConversationTurn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("message {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        });
    }
    let result = force_compress_turns(&history);
    // Should be compressed: system + compression note + kept half of conversation + last
    // Original: 1 system + 20 messages = 21 total
    // conversation = 19 messages, mid = 9, kept = 10, plus system + note + last = 13
    assert!(result.len() < history.len());
    assert!(result.len() <= 14);
    assert_eq!(result[0].role, "system");
}

#[test]
fn test_session_store_get_summary_default_empty() {
    let store = SessionStore::new_in_memory();
    let data = store.get_or_create("test-key");
    assert!(data.summary.is_empty());
}

#[test]
fn test_stored_session_debug() {
    let session = StoredSession {
        key: "test-key".to_string(),
        messages: Vec::new(),
        summary: String::new(),
        summary_covers_up_to: None,
        created: chrono::Local::now(),
        updated: chrono::Local::now(),
    };
    let debug_str = format!("{:?}", session);
    assert!(debug_str.contains("test-key"));
}

// --- S1: summary_covers_up_to field (zero-behavior addition) ---

#[test]
fn test_stored_session_legacy_json_loads_covers_none() {
    // A session file written before `summary_covers_up_to` existed must load
    // without error and default the new field to None (serde default). This is
    // the backward-compatibility guarantee for the refactor.
    let session = StoredSession {
        key: "legacy:key".to_string(),
        messages: Vec::new(),
        summary: "legacy summary".to_string(),
        summary_covers_up_to: Some(7), // set, then strip from JSON below
        created: Local::now(),
        updated: Local::now(),
    };
    let json = serde_json::to_string(&session).unwrap();
    let mut value: serde_json::Value = serde_json::from_str(&json).unwrap();
    // Simulate a legacy file: remove the new field entirely.
    assert!(value
        .as_object_mut()
        .unwrap()
        .remove("summary_covers_up_to")
        .is_some());
    let legacy_json = serde_json::to_string(&value).unwrap();

    let loaded: StoredSession = serde_json::from_str(&legacy_json).unwrap();
    assert_eq!(loaded.summary, "legacy summary");
    assert!(loaded.summary_covers_up_to.is_none());
}

#[test]
fn test_session_store_summary_covers_up_to_get_set() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("test:key");

    // Default is None.
    assert!(store.get_summary_covers_up_to("test:key").is_none());

    store.set_summary_covers_up_to("test:key", Some(12));
    assert_eq!(store.get_summary_covers_up_to("test:key"), Some(12));

    // Clear back to None.
    store.set_summary_covers_up_to("test:key", None);
    assert!(store.get_summary_covers_up_to("test:key").is_none());
}

#[test]
fn test_session_store_summary_covers_up_to_disk_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    {
        let store = SessionStore::new_with_storage(dir.path());
        store.get_or_create("disk:key");
        store.set_summary("disk:key", "persisted summary");
        store.set_summary_covers_up_to("disk:key", Some(9));
        store.save("disk:key").unwrap();
    }
    // Re-open from disk.
    let store2 = SessionStore::new_with_storage(dir.path());
    assert_eq!(store2.get_summary("disk:key"), "persisted summary");
    assert_eq!(store2.get_summary_covers_up_to("disk:key"), Some(9));
}

#[test]
fn test_session_store_clear_resets_covers_up_to() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("clear:key");
    store.set_summary("clear:key", "temp");
    store.set_summary_covers_up_to("clear:key", Some(5));

    store.clear_session("clear:key");
    assert!(store.get_summary("clear:key").is_empty());
    assert!(store.get_summary_covers_up_to("clear:key").is_none());
}

/// FIX (2026-08-25 两存储分叉摸底): "clear" must wipe the on-disk file too.
/// The old version only cleared the in-memory entry, so a gateway restart
/// reloaded the full history from `sessions/*.json` while chat_log had been
/// truncated — the model "remembered" a conversation the user just cleared.
#[test]
fn test_session_store_clear_removes_disk_file_no_revival_after_reload() {
    let dir = tempfile::tempdir().unwrap();
    let key = "clear:disk";
    {
        let store = SessionStore::new_with_storage(dir.path());
        store.get_or_create(key);
        store.set_history(key, stored_msgs(3));
        store.save(key).unwrap();
        assert!(dir.path().join("clear_disk.json").exists(), "seed file");
    }

    let store2 = SessionStore::new_with_storage(dir.path());
    assert_eq!(store2.get_history(key).len(), 3, "seed round-trips");
    store2.clear_session(key);
    // Disk file gone (not just the in-memory copy)…
    assert!(
        !dir.path().join("clear_disk.json").exists(),
        "clear must remove the on-disk store file"
    );
    // …and a fresh store (simulating a gateway restart) sees an EMPTY
    // session — no revival.
    let store3 = SessionStore::new_with_storage(dir.path());
    assert!(
        store3.get_history(key).is_empty(),
        "history must not come back to life after reload"
    );
    // The key stays usable (next turn lazily rebuilds an empty session).
    store3.get_or_create(key);
    assert!(store3.get_history(key).is_empty());
}

// --- S3.1: store-level C-aware trimming (MAX_STORED_MESSAGES = 1000) ---

fn stored_msgs(n: usize) -> Vec<StoredMessage> {
    (0..n)
        .map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("m{}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect()
}

#[test]
fn test_trim_to_limit_drops_oldest_adjusts_covers() {
    let mut session = StoredSession {
        key: "k".to_string(),
        messages: stored_msgs(1050),
        summary: "sum".to_string(),
        summary_covers_up_to: Some(1040),
        created: Local::now(),
        updated: Local::now(),
    };
    // overflow = 50, all within the covered prefix (1040 >= 50) → drop 50,
    // covers -= 50. The verbatim tail (original [1040..1050]) survives intact.
    SessionStore::trim_to_limit(&mut session);
    assert_eq!(session.messages.len(), SessionStore::MAX_STORED_MESSAGES);
    assert_eq!(session.summary_covers_up_to, Some(1040 - 50));
    // Oldest 50 dropped → first remaining is m50.
    assert_eq!(session.messages[0].content, "m50");
    // Tail preserved (last message unchanged).
    assert_eq!(session.messages.last().unwrap().content, "m1049");
}

#[test]
fn test_trim_to_limit_no_drop_without_summary() {
    let mut session = StoredSession {
        key: "k".to_string(),
        messages: stored_msgs(1050),
        summary: String::new(),
        summary_covers_up_to: None,
        created: Local::now(),
        updated: Local::now(),
    };
    // No summary → no covered prefix → dropping would lose verbatim context.
    // Refuse to trim rather than silently lose data.
    SessionStore::trim_to_limit(&mut session);
    assert_eq!(session.messages.len(), 1050);
    assert!(session.summary_covers_up_to.is_none());
}

#[test]
fn test_trim_to_limit_never_touches_verbatim_tail() {
    // covers (10) < overflow (50): can only drop 10 (the covered prefix),
    // never reach into the verbatim tail. (Degenerate config — in practice
    // tail ≈ K_target ≪ 1000 so overflow < covers always — but the clamp must
    // hold defensively.)
    let mut session = StoredSession {
        key: "k".to_string(),
        messages: stored_msgs(1050),
        summary: "sum".to_string(),
        summary_covers_up_to: Some(10),
        created: Local::now(),
        updated: Local::now(),
    };
    SessionStore::trim_to_limit(&mut session);
    assert_eq!(session.messages.len(), 1040); // dropped only 10
    assert_eq!(session.summary_covers_up_to, Some(0));
    // First 10 dropped → first remaining is m10.
    assert_eq!(session.messages[0].content, "m10");
}

#[test]
fn test_trim_to_limit_under_limit_noop() {
    let mut session = StoredSession {
        key: "k".to_string(),
        messages: stored_msgs(500),
        summary: "sum".to_string(),
        summary_covers_up_to: Some(100),
        created: Local::now(),
        updated: Local::now(),
    };
    SessionStore::trim_to_limit(&mut session);
    assert_eq!(session.messages.len(), 500);
    assert_eq!(session.summary_covers_up_to, Some(100));
}

#[test]
fn test_set_history_trims_and_adjusts_covers() {
    // Integration: set_history applies trim; accessors reflect it.
    let store = SessionStore::new_in_memory();
    store.get_or_create("k");
    store.set_summary_covers_up_to("k", Some(1040));
    store.set_history("k", stored_msgs(1050));
    assert_eq!(
        store.get_history("k").len(),
        SessionStore::MAX_STORED_MESSAGES
    );
    assert_eq!(store.get_summary_covers_up_to("k"), Some(1040 - 50));
}

#[test]
fn test_save_path_order_keeps_covers_coherent_after_trim() {
    // Regression for the save-clobbers-trim bug found in verification: the
    // AgentLoop save path must set the cache BEFORE set_history. Replicates
    // that ordering and verifies a long history (>MAX_STORED_MESSAGES) trims
    // coherently — covers_up_to is decremented to match the dropped oldest
    // messages, so the verbatim tail (last K_TARGET) survives. The OLD order
    // (history then cache) overwrote the trim's adjustment, leaving covers too
    // large and dropping the verbatim tail.
    let store = SessionStore::new_in_memory();
    store.get_or_create("long:k");
    let instance_covers = 1050 - 6; // tail = last 6 messages (K_TARGET)
    // Save-path order: cache first, then history.
    store.set_summary("long:k", "summary text");
    store.set_summary_covers_up_to("long:k", Some(instance_covers));
    store.set_history("long:k", stored_msgs(1050));

    let final_covers = store.get_summary_covers_up_to("long:k").expect("covers set");
    let final_len = store.get_history("long:k").len();
    assert_eq!(final_len, SessionStore::MAX_STORED_MESSAGES); // trimmed to 1000
    // covers decremented by the 50 dropped oldest: 1044 -> 994.
    assert_eq!(final_covers, instance_covers - 50);
    // Verbatim tail (last K_TARGET=6 messages) survives: len - covers == 6.
    assert_eq!(final_len - final_covers, 6, "verbatim tail must survive trim");
}

// --- Additional coverage for session and summarizer ---

use async_trait::async_trait;

/// A null LLM provider for testing summarization.
struct NullLlmProvider;

#[async_trait]
impl LlmProvider for NullLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Ok(crate::r#loop::LlmResponse {
            content: "summary".to_string(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

#[test]
fn test_session_store_disk_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(tmp.path());

    store.get_or_create("web:chat1");
    let messages: Vec<StoredMessage> = (0..5)
        .map(|i| StoredMessage {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("Message {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: chrono::Local::now().to_rfc3339(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    store.set_history("web:chat1", messages);
    store.set_summary("web:chat1", "A summary of the conversation");
    store.save("web:chat1").unwrap();

    // Create a new store from the same disk to verify persistence
    let store2 = SessionStore::new_with_storage(tmp.path());
    let loaded = store2.get_history("web:chat1");
    assert_eq!(loaded.len(), 5);
    assert_eq!(
        store2.get_summary("web:chat1"),
        "A summary of the conversation"
    );
}

#[test]
fn test_session_store_disk_corrupted_file() {
    let tmp = tempfile::tempdir().unwrap();
    // Write a corrupted JSON file
    let corrupted_path = tmp.path().join("corrupted.json");
    std::fs::write(&corrupted_path, "not valid json").unwrap();

    // Should not panic on load
    let store = SessionStore::new_with_storage(tmp.path());
    assert!(store.is_empty());
}

#[test]
fn test_session_store_disk_save_invalid_chars() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(tmp.path());
    store.get_or_create("test/session");

    // Keys with slashes should be sanitized for filename
    let result = store.save("test/session");
    assert!(result.is_ok());
}

#[test]
fn test_session_store_remove_with_disk() {
    let tmp = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(tmp.path());
    store.get_or_create("key1");
    store.set_summary("key1", "summary1");

    let removed = store.remove("key1");
    assert!(removed.is_some());
    assert!(!store.contains("key1"));
    assert!(store.get_history("key1").is_empty());
}

#[test]
fn test_cleanup_old_sessions_keeps_recent_deletes_old() {
    use chrono::Duration;
    use std::fs;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let store = SessionStore::new_with_storage(&dir);

    // Save a recent session normally.
    store.get_or_create("recent:key");
    store.set_history(
        "recent:key",
        vec![StoredMessage {
            role: "user".to_string(),
            content: "hi".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-06-18T00:00:00Z".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
    );
    store.save("recent:key").unwrap();

    // Manually craft an old session file by saving then back-dating the
    // `updated` field to 30 days ago.
    let old_key = "old:key".to_string();
    let old_filename = sanitize_filename(&old_key) + ".json";
    let old_path = dir.join(&old_filename);
    let old_snapshot = serde_json::json!({
        "key": old_key,
        "messages": [],
        "summary": "",
        "created": (Local::now() - Duration::days(40)).to_rfc3339(),
        "updated": (Local::now() - Duration::days(30)).to_rfc3339(),
    });
    fs::write(&old_path, old_snapshot.to_string()).unwrap();

    // Sanity: both files exist before cleanup.
    assert!(dir.join(sanitize_filename("recent:key") + ".json").exists());
    assert!(old_path.exists());

    let deleted = store.cleanup_old_sessions(7);

    // Only the old file should be removed.
    assert_eq!(deleted, 1);
    assert!(dir.join(sanitize_filename("recent:key") + ".json").exists());
    assert!(!old_path.exists());
    // The recent session should still be present.
    assert!(store.contains("recent:key"));
    // The old session was never loaded into memory by us, but if it had been,
    // cleanup would have dropped it.
}

#[test]
fn test_cleanup_old_sessions_in_memory_returns_zero() {
    // In-memory stores have no disk to clean; must return 0 without panicking.
    let store = SessionStore::new_in_memory();
    store.get_or_create("mem:key");
    let deleted = store.cleanup_old_sessions(7);
    assert_eq!(deleted, 0);
    assert!(store.contains("mem:key"));
}

#[test]
fn test_cleanup_old_sessions_skips_corrupt_json() {
    use std::fs;

    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let store = SessionStore::new_with_storage(&dir);

    // Write a corrupt JSON file (invalid syntax) — cleanup should skip it.
    let corrupt_path = dir.join("corrupt.json");
    fs::write(&corrupt_path, "{not valid json}").unwrap();

    // Write a valid JSON missing the `updated` field — cleanup should skip it too.
    let no_updated_path = dir.join("no_updated.json");
    fs::write(&no_updated_path, r#"{"key":"x","messages":[]}"#).unwrap();

    let deleted = store.cleanup_old_sessions(7);

    // Neither file should be deleted.
    assert_eq!(deleted, 0);
    assert!(corrupt_path.exists());
    assert!(no_updated_path.exists());
}

#[test]
fn test_session_store_len_empty_combined() {
    let store = SessionStore::new_in_memory();
    assert!(store.is_empty());
    assert_eq!(store.len(), 0);

    store.get_or_create("key1");
    assert!(!store.is_empty());
    assert_eq!(store.len(), 1);

    store.get_or_create("key2");
    assert_eq!(store.len(), 2);
}

#[test]
fn test_session_store_get_or_create_multiple() {
    let store = SessionStore::new_in_memory();
    let s1 = store.get_or_create("key1");
    assert!(s1.messages.is_empty());

    // Second call returns existing
    let s2 = store.get_or_create("key1");
    assert!(s2.messages.is_empty());
}

#[test]
fn test_session_store_truncate_exact() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("key1");
    let msgs: Vec<StoredMessage> = (0..5)
        .map(|i| StoredMessage {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    store.set_history("key1", msgs);

    // Truncate to exactly 3
    store.truncate_history("key1", 3);
    let history = store.get_history("key1");
    assert_eq!(history.len(), 3);
    assert_eq!(history[0].content, "msg 2");
    assert_eq!(history[2].content, "msg 4");
}

#[test]
fn test_stored_message_from_conversation_turn_with_tools() {
    let turn = ConversationTurn {
        role: "assistant".to_string(),
        content: "Using tool".to_string(),
        tool_calls: vec![crate::types::ToolCallInfo {
            id: "tc_1".to_string(),
            name: "read_file".to_string(),
            arguments: r#"{"path":"/test"}"#.to_string(),
        }],
        tool_call_id: Some("tc_1".to_string()),
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };

    let stored: StoredMessage = StoredMessage::from(&turn);
    assert_eq!(stored.role, "assistant");
    assert_eq!(stored.content, "Using tool");
    assert_eq!(stored.tool_calls.len(), 1);
    assert_eq!(stored.tool_calls[0].id, "tc_1");
    assert_eq!(stored.tool_call_id, Some("tc_1".to_string()));
}

#[test]
fn test_stored_message_into_conversation_turn() {
    let stored = StoredMessage {
        role: "user".to_string(),
        content: "Hello".to_string(),
        tool_calls: vec![StoredToolCall {
            id: "tc_1".to_string(),
            name: "echo".to_string(),
            arguments: "{}".to_string(),
        }],
        tool_call_id: None,
        timestamp: "2026-01-01T00:00:00Z".to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    };

    let turn: ConversationTurn = ConversationTurn::from(stored);
    assert_eq!(turn.role, "user");
    assert_eq!(turn.content, "Hello");
    assert_eq!(turn.tool_calls.len(), 1);
    assert_eq!(turn.tool_calls[0].name, "echo");
}

#[test]
fn test_stored_tool_call_debug() {
    let tc = StoredToolCall {
        id: "tc_1".to_string(),
        name: "tool1".to_string(),
        arguments: "{}".to_string(),
    };
    let debug_str = format!("{:?}", tc);
    assert!(debug_str.contains("tc_1"));
    assert!(debug_str.contains("tool1"));
}

#[test]
fn test_estimate_tokens_for_messages_empty() {
    let messages: Vec<StoredMessage> = Vec::new();
    assert_eq!(estimate_tokens_for_messages(&messages), 0);
}

#[test]
fn test_estimate_tokens_for_messages_with_content() {
    let messages = vec![StoredMessage {
        role: "user".to_string(),
        content: "Hello world".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    let tokens = estimate_tokens_for_messages(&messages);
    assert!(tokens > 0);
}

#[test]
fn test_session_manager_cleanup_with_custom_timeout() {
    let manager = SessionManager::new(std::time::Duration::from_secs(0)); // Immediate timeout
    manager.get_or_create("s1", "web", "chat1");
    manager.get_or_create("s2", "web", "chat2");

    // With 0 timeout, sessions should be expired immediately
    // Just verify it doesn't panic
    manager.cleanup_expired();
}

#[test]
fn test_session_manager_set_busy_and_check() {
    let manager = SessionManager::new(std::time::Duration::from_secs(3600));
    let session = manager.get_or_create("s1", "web", "chat1");
    assert!(!session.busy);

    manager.set_busy("s1", true);
    assert_eq!(manager.is_busy("s1"), Some(true));

    manager.set_busy("s1", false);
    assert_eq!(manager.is_busy("s1"), Some(false));
}

#[test]
fn test_session_manager_set_busy_nonexistent() {
    let manager = SessionManager::new(std::time::Duration::from_secs(3600));
    // Should not panic, returns false
    assert!(!manager.set_busy("nonexistent", true));
}

#[test]
fn test_session_manager_get_session_nonexistent() {
    let manager = SessionManager::new(std::time::Duration::from_secs(3600));
    assert!(manager.is_busy("nonexistent").is_none());
}

#[test]
fn test_sanitize_filename_special_chars() {
    assert_eq!(sanitize_filename("web:chat1"), "web_chat1");
    assert_eq!(sanitize_filename("a/b\\c"), "a_b_c");
    assert_eq!(sanitize_filename("normal"), "normal");
}

#[test]
fn test_session_store_new_with_storage_creates_dir() {
    let tmp = tempfile::tempdir().unwrap();
    let sub_dir = tmp.path().join("sessions");
    let store = SessionStore::new_with_storage(&sub_dir);
    assert!(sub_dir.exists());
    assert!(store.is_empty());
}

#[test]
fn test_summarizer_should_summarize_short_history() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..5)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!("Short {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    assert!(!summarizer.should_summarize(&history, 128000));
}

#[test]
fn test_summarizer_should_summarize_long_history() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    // Create history with enough messages and tokens
    let history: Vec<ConversationTurn> = (0..30)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!(
                "A longer message with more content to increase token estimation significantly {}",
                i
            ),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    // 30 messages > 20 threshold
    assert!(summarizer.should_summarize(&history, 128000));
}

#[test]
fn test_null_notifier() {
    let notifier = NullNotifier;
    // Should not panic
    notifier.notify("web", "chat1", "test message");
}

// --- Summarizer coverage tests ---

#[test]
fn test_summarizer_should_summarize_by_token_threshold() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        100, // Very small context window
        store,
    );
    // Create history with enough tokens to exceed 75% of 100 = 75 tokens
    let history: Vec<ConversationTurn> = (0..5)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!(
                "A reasonably long message with enough text to exceed token threshold {}",
                i
            ),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    // Token threshold is 100 * 75 / 100 = 75 tokens
    // With content ~70 chars each, 5 * 70 * 2/5 = 140 tokens > 75
    assert!(summarizer.should_summarize(&history, 100));
}

#[test]
fn test_summarizer_should_not_summarize_empty() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = vec![];
    assert!(!summarizer.should_summarize(&history, 128000));
}

#[test]
fn test_summarizer_summarize_session_too_few_messages() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..4)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    // Only 4 messages (<=4), so summarize_session returns empty
    let result = summarizer.summarize_session("test:session", &history);
    assert!(result.is_empty());
}

#[test]
fn test_summarizer_summarize_session_all_system_messages() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..10)
        .map(|i| ConversationTurn {
            role: "system".to_string(),
            content: format!("system msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    // All system messages -> none pass the user/assistant filter
    let result = summarizer.summarize_session("test:sys", &history);
    assert!(result.is_empty());
}

#[test]
fn test_summarizer_summarize_session_basic() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..8)
        .map(|i| ConversationTurn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("Conversation message {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    let result = summarizer.summarize_session("test:basic", &history);
    // NullLlmProvider returns "summary"
    assert_eq!(result, "summary");
}

#[test]
fn test_summarizer_maybe_summarize_internal_channel() {
    let store = Arc::new(SessionStore::new_in_memory());
    struct CountingNotifier {
        count: std::sync::atomic::AtomicUsize,
    }
    impl SummarizationNotifier for CountingNotifier {
        fn notify(&self, _channel: &str, _chat_id: &str, _content: &str) {
            self.count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        }
    }
    let notifier = Box::new(CountingNotifier {
        count: std::sync::atomic::AtomicUsize::new(0),
    });
    let summarizer = Summarizer::new(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
        notifier,
        None,
    );
    // Internal channels should not trigger notification
    let history: Vec<ConversationTurn> = (0..30)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!("Message {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    let result = summarizer.maybe_summarize("test:cli", "cli", "direct", &history, 128000);
    assert!(result);
}

#[test]
fn test_summarizer_maybe_summarize_not_triggered() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new_silent(
        Arc::new(NullLlmProvider),
        "test-model".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..5)
        .map(|i| ConversationTurn {
            role: "user".to_string(),
            content: format!("Short {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        })
        .collect();
    assert!(!summarizer.maybe_summarize("test:short", "web", "chat1", &history, 128000));
}

#[test]
fn test_force_compress_three_messages() {
    let history = vec![
        ConversationTurn {
            role: "system".to_string(),
            content: "You are helpful".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "hi".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        },
    ];
    // 3 messages (<=4), should return unchanged
    let result = force_compress_turns(&history);
    assert_eq!(result.len(), 3);
}

#[test]
fn test_force_compress_preserves_system_and_last() {
    let mut history: Vec<ConversationTurn> = vec![ConversationTurn {
        role: "system".to_string(),
        content: "System prompt".to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    for i in 0..10 {
        history.push(ConversationTurn {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        });
    }
    let result = force_compress_turns(&history);
    assert_eq!(result[0].content, "System prompt");
    assert!(result[1].content.contains("Emergency compression"));
    assert_eq!(result.last().unwrap().content, "msg 9");
}

#[test]
fn test_session_store_get_or_create_creates_with_timestamps() {
    let store = SessionStore::new_in_memory();
    let session = store.get_or_create("test:ts");
    assert!(session.created <= Local::now());
    assert!(session.updated <= Local::now());
}

#[test]
fn test_session_store_get_history_nonexistent_key() {
    let store = SessionStore::new_in_memory();
    let history = store.get_history("nonexistent");
    assert!(history.is_empty());
}

#[test]
fn test_stored_session_default_summary() {
    let store = SessionStore::new_in_memory();
    let session = store.get_or_create("test:default");
    assert!(session.summary.is_empty());
}

#[test]
fn test_session_manager_cleanup_expired_with_timeout_expired() {
    let mgr = SessionManager::new(Duration::from_millis(10));
    mgr.get_or_create("web:chat1", "web", "chat1");
    mgr.get_or_create("web:chat2", "web", "chat2");

    // Force sessions into the past
    {
        let mut s1 = mgr.sessions.get_mut("web:chat1").unwrap();
        s1.last_active = Local::now() - chrono::Duration::seconds(60);
    }
    {
        let mut s2 = mgr.sessions.get_mut("web:chat2").unwrap();
        s2.last_active = Local::now() - chrono::Duration::seconds(60);
    }

    let removed = mgr.cleanup_expired_with_timeout(Duration::from_millis(10));
    assert_eq!(removed.len(), 2);
    assert!(mgr.is_empty());
}

#[test]
fn test_session_manager_multiple_sessions() {
    let mgr = SessionManager::with_default_timeout();
    mgr.get_or_create("web:chat1", "web", "chat1");
    mgr.get_or_create("web:chat2", "web", "chat2");
    mgr.get_or_create("rpc:chat3", "rpc", "chat3");

    assert_eq!(mgr.len(), 3);
    assert!(mgr.contains("web:chat1"));
    assert!(mgr.contains("web:chat2"));
    assert!(mgr.contains("rpc:chat3"));

    mgr.remove("web:chat1");
    assert_eq!(mgr.len(), 2);
    assert!(!mgr.contains("web:chat1"));
}

#[test]
fn test_session_store_disk_save_and_reload_multiple() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());

    for i in 0..3 {
        let key = format!("multi:key{}", i);
        store.get_or_create(&key);
        store.set_summary(&key, &format!("Summary {}", i));
        store.save(&key).unwrap();
    }

    // Reload
    let store2 = SessionStore::new_with_storage(dir.path());
    assert_eq!(store2.len(), 3);
    for i in 0..3 {
        let key = format!("multi:key{}", i);
        assert!(store2.contains(&key));
        assert_eq!(store2.get_summary(&key), format!("Summary {}", i));
    }
}


// ---------------------------------------------------------------------------
// X1 (U3 projection prune): compaction pressure measures the projection.
// ---------------------------------------------------------------------------

/// Compaction decisions must reflect what the PROVIDER sees (the folded
/// projection), not the raw originals - otherwise large originals kept in
/// history would over-trigger summarization even though the request context
/// stays bounded.
#[test]
fn test_estimate_tokens_projected_bounded() {
    let big: String = "k".repeat(20_000);
    let turns = vec![ConversationTurn {
        role: "tool".to_string(),
        content: big.clone(),
        tool_calls: Vec::new(),
        tool_call_id: Some("tc_1".to_string()),
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: Some("exec".to_string()),
        tool_result_projection: None,
    }];
    let raw = super::estimate_tokens(&big);
    let projected = super::estimate_tokens_for_turns_projected(&turns);
    assert!(
        projected < raw,
        "projected estimate ({projected}) must be below the raw estimate ({raw})"
    );
    // The projection itself is bounded by the prune budget (head+marker+tail).
    assert!(projected < 3_000, "projected estimate stays bounded");
    // Non-tool content is estimated verbatim.
    let plain = vec![ConversationTurn {
        role: "user".to_string(),
        content: big.clone(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }];
    assert_eq!(
        super::estimate_tokens_for_turns_projected(&plain),
        super::estimate_tokens(&big)
    );
}

// ---------------------------------------------------------------------------
// 2026-08-25 自愈重建（TTL 生命周期不对称修复）: store json 被 7 天 TTL 清理
// （或缺失）而 chat_log 还活着 → get_or_create 重放 jsonl，而不是新建一个
// 空的失忆会话。夹具刻意是"脏数据"（不对称机制在野外产出的真实状态）——
// 干净夹具正是这类 bug 在此前几轮测试里始终不可见的原因。
// ---------------------------------------------------------------------------

/// Store 缺 + jsonl 在 → 重放 user/assistant 行（空 assistant 跳过），
/// 时间戳沿用 jsonl 原值，重建立即落盘（crash-safe），二次访问走内存。
#[test]
fn test_get_or_create_rebuilds_from_chat_log_when_store_missing() {
    let key = "test:rebuild:store-missing";
    crate::chat_log::delete_chat_log(key); // clean slate
    crate::chat_log::append_chat_log(key, "user", "第一问");
    // 纯 tool_calls 中间态（空 assistant）：chat_log 里可能有，重放必须跳过
    crate::chat_log::append_chat_log(key, "assistant", "");
    crate::chat_log::append_chat_log(key, "assistant", "第一答");
    let (rows, total, _, _) = crate::chat_log::read_chat_log(key, 10, None);
    assert_eq!(total, 3);

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path()); // 空目录：无 store 文件
    let session = store.get_or_create(key);

    assert_eq!(session.messages.len(), 2, "空 assistant 中间态必须被跳过");
    assert_eq!(session.messages[0].role, "user");
    assert_eq!(session.messages[0].content, "第一问");
    assert_eq!(session.messages[1].role, "assistant");
    assert_eq!(session.messages[1].content, "第一答");
    // 时间戳沿用 jsonl 原值（重建不得重盖时间戳）
    assert_eq!(
        session.messages[0].timestamp,
        rows[0]["timestamp"].as_str().unwrap()
    );
    assert_eq!(
        session.messages[1].timestamp,
        rows[2]["timestamp"].as_str().unwrap()
    );
    // TTL 记账：updated=now，重建后的文件能活过新一轮 7 天
    assert!(session.updated > session.created);

    // 重建立即持久化：第二个实例直接从磁盘加载，不发生二次重建
    assert!(store.file_exists(key));
    let store2 = SessionStore::new_with_storage(dir.path());
    assert_eq!(store2.get_or_create(key).messages.len(), 2);

    crate::chat_log::delete_chat_log(key); // cleanup
}

/// Store 文件在（Z1 场景：文件在 store 构造后才出现）→ 磁盘回退获胜，
/// 绝不用 jsonl 重放覆盖活上下文。
#[test]
fn test_get_or_create_no_rebuild_when_store_file_exists() {
    let key = "test:rebuild:store-wins";
    crate::chat_log::delete_chat_log(key);
    // jsonl 内容与 store 内容刻意不同：谁赢一目了然
    crate::chat_log::append_chat_log(key, "user", "jsonl 独有内容");

    let dir = tempfile::tempdir().unwrap();
    // 先构造（目录为空，不加载任何东西），再让文件出现 → 走磁盘回退层。
    // 种子直接手写 store 文件：任何走 get_or_create 的种子路径都会先撞上
    // 重建层（jsonl 已在），把夹具污染成"jsonl 内容 + 追加"的混合体。
    let store = SessionStore::new_with_storage(dir.path());
    let store_path = dir.path().join(format!("{}.json", sanitize_filename(key)));
    let session_json = serde_json::to_string_pretty(&StoredSession {
        key: key.to_string(),
        messages: vec![StoredMessage {
            role: "user".to_string(),
            content: "store 独有内容".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-08-25T00:00:00+08:00".to_string(),
            reasoning_content: None,
            tool_name: None,
            tool_result_projection: None,
        }],
        summary: String::new(),
        summary_covers_up_to: None,
        created: Local::now(),
        updated: Local::now(),
    })
    .unwrap();
    std::fs::write(&store_path, session_json).unwrap();

    let session = store.get_or_create(key);
    assert_eq!(session.messages.len(), 1);
    assert_eq!(session.messages[0].content, "store 独有内容");
    assert!(
        !session.messages.iter().any(|m| m.content.contains("jsonl")),
        "jsonl 重放不得覆盖在盘的 store 上下文"
    );

    crate::chat_log::delete_chat_log(key); // cleanup
}

/// 双侧都没有 → 行为不变：空会话，且不落盘空文件。
#[test]
fn test_get_or_create_fresh_key_without_log_stays_empty() {
    let key = "test:rebuild:fresh-no-log";
    crate::chat_log::delete_chat_log(key); // 确保无 jsonl
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let session = store.get_or_create(key);
    assert!(session.messages.is_empty());
    assert!(!store.file_exists(key), "无内容可重放时不得写出空 store 文件");
    crate::chat_log::delete_chat_log(key); // cleanup
}

/// 纯内存 store（无 storage_dir）不做重建——它既不落盘也不被 TTL 清理，
/// 且重放会触碰全局路径管理器（测试环境下即真实 sessions_log_dir）。
#[test]
fn test_in_memory_store_never_rebuilds_from_chat_log() {
    let key = "test:rebuild:in-memory-gate";
    crate::chat_log::delete_chat_log(key);
    crate::chat_log::append_chat_log(key, "user", "有 jsonl 但 store 无盘目录");
    let store = SessionStore::new_in_memory();
    let session = store.get_or_create(key);
    assert!(session.messages.is_empty(), "重建必须门控在 storage_dir 上");
    crate::chat_log::delete_chat_log(key); // cleanup
}

/// 超长 jsonl 只重放最新 MAX_STORED_MESSAGES 行（与 store 自身的截断上限
/// 同源）——重建恢复的是模型的工作尾部，不是无界档案。
#[test]
fn test_rebuild_caps_at_max_stored_messages_keeps_newest() {
    let key = "test:rebuild:cap";
    crate::chat_log::delete_chat_log(key);
    // 一次性写入 MAX+5 行（逐行 append 会对真实 sessions_log_dir 开关文件
    // 1000+ 次，没必要）
    let log_path = nemesis_path::default_path_manager()
        .sessions_log_dir()
        .join(format!("{}.jsonl", key.replace(':', "_")));
    std::fs::create_dir_all(log_path.parent().unwrap()).unwrap();
    let mut lines = String::new();
    for i in 0..(SessionStore::MAX_STORED_MESSAGES + 5) {
        let entry = serde_json::json!({
            "role": "user",
            "content": format!("row {}", i),
            "timestamp": "2026-08-25T00:00:00+08:00",
        });
        lines.push_str(&entry.to_string());
        lines.push('\n');
    }
    std::fs::write(&log_path, lines).unwrap();

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    let session = store.get_or_create(key);

    assert_eq!(session.messages.len(), SessionStore::MAX_STORED_MESSAGES);
    // 留下的是最新尾部
    assert_eq!(
        session.messages.last().unwrap().content,
        format!("row {}", SessionStore::MAX_STORED_MESSAGES + 4)
    );

    crate::chat_log::delete_chat_log(key); // cleanup
}

// ---------------------------------------------------------------------------
// 2026-08-25 management-op regression pins: clear/delete must leave BOTH
// stores empty, and the self-heal rebuild layer must NOT resurrect content
// the user asked to remove. The compositions below mirror the exact call
// sequences the Dashboard handlers run (handlers/sessions.rs).
// ---------------------------------------------------------------------------

#[test]
fn test_cleared_session_does_not_resurrect_via_rebuild() {
    // Handler "clear" sequence: truncate jsonl FIRST, then drop the store
    // entry. A subsequent get_or_create must yield an EMPTY session — the
    // truncated jsonl has nothing to replay.
    let key = format!(
        "test:mgmt:clear:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::append_chat_log(&key, "user", "secret to forget");
    crate::chat_log::append_chat_log(&key, "assistant", "ok noted");

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    // Materialize + persist so the store json exists (dirty-data shape).
    store.get_or_create(&key);
    store.add_message(&key, "user", "secret to forget");
    store.add_message(&key, "assistant", "ok noted");
    let _ = store.save(&key);

    // The clear sequence.
    crate::chat_log::clear_chat_log(&key);
    store.clear_session(&key);

    let after = store.get_or_create(&key);
    assert!(
        after.messages.is_empty(),
        "cleared session resurrected via rebuild: {:?}",
        after.messages
    );
    crate::chat_log::delete_chat_log(&key); // cleanup
}

#[test]
fn test_deleted_session_does_not_resurrect_via_rebuild() {
    // Handler "delete" sequence: store.delete_session removes the in-memory
    // entry, the store json AND the jsonl. A subsequent get_or_create must
    // yield an EMPTY session.
    let key = format!(
        "test:mgmt:delete:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    crate::chat_log::append_chat_log(&key, "user", "gone after delete");

    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create(&key);
    store.add_message(&key, "user", "gone after delete");
    let _ = store.save(&key);

    store.delete_session(&key);

    let after = store.get_or_create(&key);
    assert!(
        after.messages.is_empty(),
        "deleted session resurrected via rebuild: {:?}",
        after.messages
    );
    crate::chat_log::delete_chat_log(&key); // cleanup
}

// ---------------------------------------------------------------------------
// W3a branch coverage (capture, save/load edge arms, migrate, cleanup,
// summarizer edge arms)
// ---------------------------------------------------------------------------

fn conv_turn(role: &str, content: &str) -> ConversationTurn {
    ConversationTurn {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: Vec::new(),
        tool_call_id: None,
        timestamp: String::new(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// The ONLY `CaptureSink::init` caller in this test binary: after it runs the
/// capture branches in set_history/add_message are live, and a shrinking
/// set_history must auto-flush the write timeline to disk.
#[test]
fn test_capture_records_session_writes_when_enabled() {
    let cap_dir = tempfile::tempdir().unwrap();
    crate::capture_sink::CaptureSink::init(cap_dir.path().to_path_buf(), true);
    assert!(crate::capture_sink::CaptureSink::enabled());

    let store = SessionStore::new_in_memory();
    store.get_or_create("cap:key");
    store.set_history("cap:key", stored_msgs(3)); // growth: overwrite=false
    store.add_message("cap:key", "user", "hello"); // append capture
    // Shrink -> overwrite_detected=true -> immediate flush to disk.
    store.set_history("cap:key", stored_msgs(1));

    let base = cap_dir.path().join("logs").join("capture").join("cap_key");
    let entries: Vec<_> = std::fs::read_dir(&base).unwrap().collect();
    assert_eq!(entries.len(), 1, "overwrite should have flushed exactly once");
    let edir = entries[0].as_ref().unwrap().path();
    assert!(
        edir.to_string_lossy().contains("session_overwrite"),
        "flush dir: {}",
        edir.display()
    );
    assert!(
        edir.join("02.session_writes.jsonl").exists(),
        "write timeline must be on disk"
    );
}

/// `hash_messages` is stable for identical (role, content) sequences and
/// distinct for different ones.
#[test]
fn test_hash_messages_stable_and_distinct() {
    let a = stored_msgs(2);
    let b = stored_msgs(2);
    assert_eq!(SessionStore::hash_messages(&a), SessionStore::hash_messages(&b));
    let c = stored_msgs(3);
    assert_ne!(SessionStore::hash_messages(&a), SessionStore::hash_messages(&c));
    assert!(!SessionStore::hash_messages(&[]).is_empty(), "16-hex digits");
}

/// `sanitize_session_id` keeps [A-Za-z0-9_-] and collapses everything else
/// (per char, multi-byte safe).
#[test]
fn test_sanitize_session_id_charset() {
    assert_eq!(SessionStore::sanitize_session_id("abc-123_X"), "abc-123_X");
    assert_eq!(SessionStore::sanitize_session_id(r"a:b/c\d"), "a_b_c_d");
    assert_eq!(SessionStore::sanitize_session_id("中文.key"), "___key");
    assert_eq!(SessionStore::sanitize_session_id(""), "");
}

/// save(): rename failure (destination path is a DIRECTORY on Windows) ->
/// Err("rename error ...") and the temp file is cleaned up.
#[test]
fn test_save_rename_failure_returns_error_and_cleans_temp() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create("rn:key");
    std::fs::create_dir_all(dir.path().join("rn_key.json")).unwrap();
    let err = store.save("rn:key").unwrap_err();
    assert!(err.contains("rename error"), "unexpected error: {err}");
    let leftovers: Vec<String> = std::fs::read_dir(dir.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.file_name().to_string_lossy().to_string())
        .filter(|n| n.ends_with(".tmp"))
        .collect();
    assert!(leftovers.is_empty(), "temp removed: {leftovers:?}");
}

/// load_from_disk: non-`.json` entries skipped; a `.json`-named DIRECTORY
/// (unreadable) skipped; the valid file still loads.
#[test]
fn test_load_from_disk_skips_non_json_and_unreadable_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("notes.txt"), "ignore").unwrap();
    std::fs::create_dir_all(dir.path().join("dir_named.json")).unwrap();
    let good = StoredSession {
        key: "good:key".to_string(),
        messages: vec![],
        summary: String::new(),
        summary_covers_up_to: None,
        created: Local::now(),
        updated: Local::now(),
    };
    std::fs::write(
        dir.path().join("good_key.json"),
        serde_json::to_string(&good).unwrap(),
    )
    .unwrap();

    let store = SessionStore::new_with_storage(dir.path());
    assert_eq!(store.len(), 1, "only the valid json loads");
    assert!(store.contains("good:key"));
}

/// load_from_disk with an uncreatable storage dir (parent is a FILE): the
/// read_dir error arm is tolerated (empty store, no panic).
#[test]
fn test_load_from_disk_unreadable_dir_is_tolerated() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("blocker");
    std::fs::write(&blocker, b"x").unwrap();
    let store = SessionStore::new_with_storage(blocker.join("sessions"));
    assert!(store.is_empty());
}

/// delete_session for a key whose store file was never written: the json
/// removal hits NotFound (no warn), still deletes the jsonl best-effort and
/// reports the in-memory presence.
#[test]
fn test_delete_session_missing_files_is_quiet() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create("gone:key");
    assert!(store.delete_session("gone:key"));
    assert!(!store.contains("gone:key"));
    assert!(!store.file_exists("gone:key"));
}

/// cleanup_old_sessions skips non-json entries, `.json`-named directories
/// (unreadable), and files whose `updated` is not valid RFC3339.
#[test]
fn test_cleanup_skips_non_json_unreadable_and_bad_timestamp() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let store = SessionStore::new_with_storage(&dir);
    std::fs::write(dir.join("notes.txt"), "x").unwrap();
    std::fs::create_dir_all(dir.join("weird.json")).unwrap();
    let bad = serde_json::json!({"key":"bad:ts","messages":[],"updated":"not-a-date"});
    std::fs::write(dir.join("bad_ts.json"), bad.to_string()).unwrap();

    assert_eq!(store.cleanup_old_sessions(0), 0);
    assert!(dir.join("notes.txt").exists());
    assert!(dir.join("weird.json").exists());
    assert!(dir.join("bad_ts.json").exists());
}

/// cleanup_old_sessions with an unreadable storage dir: warns and returns 0.
#[test]
fn test_cleanup_unreadable_dir_returns_zero() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("f");
    std::fs::write(&blocker, b"x").unwrap();
    let store = SessionStore::new_with_storage(blocker.join("s"));
    assert_eq!(store.cleanup_old_sessions(7), 0);
}

/// An expired file that cannot be deleted (read-only on Windows): the sweep
/// warns per-file and continues; the count stays honest.
#[cfg(windows)]
#[test]
fn test_cleanup_readonly_expired_file_survives() {
    let tmp = tempfile::tempdir().unwrap();
    let dir = tmp.path().to_path_buf();
    let store = SessionStore::new_with_storage(&dir);
    let v = serde_json::json!({
        "key": "ro:key",
        "messages": [],
        "updated": "2020-01-01T00:00:00+00:00",
    });
    let p = dir.join("ro_key.json");
    std::fs::write(&p, v.to_string()).unwrap();

    // 探针判定只读删除语义：部分文件系统（ReFS/Dev Drive）不强制，实测可删
    // ——未强制时跳过存活断言（该 warn 分支只在不强制只读的机器上可达）。
    let probe = dir.join("ro_probe.txt");
    std::fs::write(&probe, b"p").unwrap();
    let mut perm = std::fs::metadata(&probe).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&probe, perm).unwrap();
    let enforced = std::fs::remove_file(&probe).is_err();
    if !enforced {
        eprintln!("skipping readonly-survives arm: filesystem does not enforce readonly deletes");
        return;
    }

    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(true);
    std::fs::set_permissions(&p, perm).unwrap();

    let deleted = store.cleanup_old_sessions(7);
    assert_eq!(deleted, 0, "read-only file could not be deleted");
    assert!(p.exists(), "locked file survives");

    // cleanup: clear readonly so the tempdir can be removed.
    let mut perm = std::fs::metadata(&p).unwrap().permissions();
    perm.set_readonly(false);
    std::fs::set_permissions(&p, perm).unwrap();
}

/// migrate_legacy_main happy path: jsonl renamed, store json rewritten with
/// the new key and the old file removed; second call is a no-op.
/// SAFETY: the jsonl side uses the REAL sessions_log_dir with FIXED names --
/// skipped entirely when either file already exists (real user data).
#[test]
fn test_migrate_legacy_main_happy_path() {
    let logs_dir = nemesis_path::default_path_manager().sessions_log_dir();
    let main_log = logs_dir.join("agent_main_main.jsonl");
    let legacy_log = logs_dir.join("agent_main_session_legacy.jsonl");
    if main_log.exists() || legacy_log.exists() {
        return; // production home carries real data; do not touch it
    }
    std::fs::create_dir_all(&logs_dir).unwrap();
    std::fs::write(&main_log, "{\"role\":\"user\",\"content\":\"legacy q\"}\n").unwrap();

    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("agent_main_main.json"),
        serde_json::json!({
            "key": "agent:main:main",
            "messages": [],
            "summary": "",
            "created": "2026-01-01T00:00:00+08:00",
            "updated": "2026-01-01T00:00:00+08:00",
        })
        .to_string(),
    )
    .unwrap();

    SessionStore::migrate_legacy_main(dir.path());

    assert!(legacy_log.exists(), "jsonl renamed to legacy name");
    assert!(!main_log.exists());
    let legacy_json = dir.path().join("agent_main_session_legacy.json");
    assert!(legacy_json.exists(), "store json rewritten");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&legacy_json).unwrap()).unwrap();
    assert_eq!(v["key"], "agent:main:session:legacy");
    assert!(!dir.path().join("agent_main_main.json").exists());

    // Idempotent: nothing left to migrate, no panic.
    SessionStore::migrate_legacy_main(dir.path());

    let _ = std::fs::remove_file(&legacy_log); // cleanup what we created
}

/// migrate_legacy_main json-side edge arms: corrupt source kept; a source
/// whose key does not match `agent:main:main` is still moved but its key is
/// NOT rewritten.
#[test]
fn test_migrate_legacy_main_json_edge_arms() {
    let logs_dir = nemesis_path::default_path_manager().sessions_log_dir();
    let main_log = logs_dir.join("agent_main_main.jsonl");
    let legacy_log = logs_dir.join("agent_main_session_legacy.jsonl");
    if main_log.exists() || legacy_log.exists() {
        return; // never touch real user data
    }

    // Corrupt json: read ok, parse fails -> warn, source untouched.
    let dir1 = tempfile::tempdir().unwrap();
    std::fs::write(dir1.path().join("agent_main_main.json"), "NOT JSON").unwrap();
    SessionStore::migrate_legacy_main(dir1.path());
    assert!(
        dir1.path().join("agent_main_main.json").exists(),
        "corrupt source kept (data not lost)"
    );
    assert!(!dir1.path().join("agent_main_session_legacy.json").exists());

    // Non-matching key: file migrated verbatim, key NOT rewritten.
    let dir2 = tempfile::tempdir().unwrap();
    std::fs::write(
        dir2.path().join("agent_main_main.json"),
        r#"{"key":"other:key","x":1}"#,
    )
    .unwrap();
    SessionStore::migrate_legacy_main(dir2.path());
    let p = dir2.path().join("agent_main_session_legacy.json");
    assert!(p.exists(), "migrated regardless of key value");
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&p).unwrap()).unwrap();
    assert_eq!(v["key"], "other:key", "non-matching key left as-is");
    assert!(!dir2.path().join("agent_main_main.json").exists());
}

// --- Summarizer edge arms (W3a) ---

struct ErrorLlmProvider;
#[async_trait]
impl LlmProvider for ErrorLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Err("provider down".to_string())
    }
}

struct EmptyContentLlmProvider;
#[async_trait]
impl LlmProvider for EmptyContentLlmProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<LlmMessage>,
        _options: Option<crate::types::ChatOptions>,
        _tools: Vec<crate::types::ToolDefinition>,
    ) -> Result<crate::r#loop::LlmResponse, String> {
        Ok(crate::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// Provider error in the batch path -> empty summary (never a panic, never a
/// partial), and the store is NOT updated.
#[test]
fn test_summarize_batch_provider_error_returns_empty() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("err:key");
    let summarizer =
        Summarizer::new_silent(Arc::new(ErrorLlmProvider), "m".to_string(), 128000, store.clone());
    let history: Vec<ConversationTurn> = (0..8)
        .map(|i| conv_turn(if i % 2 == 0 { "user" } else { "assistant" }, &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session("err:key", &history);
    assert_eq!(result, "");
    assert!(store.get_summary("err:key").is_empty(), "no summary on error");
}

/// Multipart path (>10 valid messages) with a failing provider: both halves
/// fall back to "", the merge falls back to concatenation.
#[test]
fn test_summarize_multipart_provider_error_falls_back_to_concat() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("mp:key");
    let summarizer =
        Summarizer::new_silent(Arc::new(ErrorLlmProvider), "m".to_string(), 128000, store.clone());
    let history: Vec<ConversationTurn> = (0..16)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session("mp:key", &history);
    assert_eq!(result, " ", "empty halves concatenated with a space");
}

/// Multipart path with a provider that returns Ok but EMPTY content: the
/// merge result is empty -> fallback concatenation (the `Ok(_)` empty arm).
#[test]
fn test_summarize_multipart_empty_merge_falls_back() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("mpe:key");
    let summarizer = Summarizer::new_silent(
        Arc::new(EmptyContentLlmProvider),
        "m".to_string(),
        128000,
        store,
    );
    let history: Vec<ConversationTurn> = (0..16)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session("mpe:key", &history);
    assert_eq!(result, " ", "empty merge falls back to concatenated halves");
}

/// An existing stored summary is folded into the batch prompt ("Existing
/// context" line).
#[test]
fn test_summarize_session_uses_existing_summary() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("prior:key");
    store.set_summary("prior:key", "prior context");
    let summarizer =
        Summarizer::new_silent(Arc::new(NullLlmProvider), "m".to_string(), 128000, store.clone());
    let history: Vec<ConversationTurn> = (0..8)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session("prior:key", &history);
    assert_eq!(result, "summary");
    assert_eq!(store.get_summary("prior:key"), "summary");
}

/// One oversized message (relative to context_window/2) is omitted and the
/// final summary carries the omission note.
#[test]
fn test_summarize_session_appends_omission_note_for_oversized() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("omit:key");
    let summarizer =
        Summarizer::new_silent(Arc::new(NullLlmProvider), "m".to_string(), 400, store);
    let history = vec![
        conv_turn("user", &"H".repeat(20_000)), // ~8000 tokens > 200 cap
        conv_turn("user", "q1"),
        conv_turn("assistant", "a1"),
        conv_turn("user", "q2"),
        conv_turn("assistant", "a2"),
        conv_turn("user", "q3"),
        conv_turn("assistant", "a3"),
    ];
    let result = summarizer.summarize_session("omit:key", &history);
    assert!(result.starts_with("summary"), "{result}");
    assert!(
        result.contains("oversized messages were omitted"),
        "omission note missing: {result}"
    );
}

/// summarize_session with a store whose save fails (broken storage dir):
/// warn only -- the summary is still returned.
#[test]
fn test_summarize_session_save_failure_warns_but_returns_summary() {
    let tmp = tempfile::tempdir().unwrap();
    let blocker = tmp.path().join("f");
    std::fs::write(&blocker, b"x").unwrap();
    let store = Arc::new(SessionStore::new_with_storage(blocker.join("s")));
    let key = format!(
        "test:savefail:{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    store.get_or_create(&key);
    let summarizer =
        Summarizer::new_silent(Arc::new(NullLlmProvider), "m".to_string(), 128000, store.clone());
    let history: Vec<ConversationTurn> = (0..8)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session(&key, &history);
    assert_eq!(result, "summary", "save failure must not eat the summary");
}

/// maybe_summarize with the session already mid-summarization -> false
/// (the dedup guard, hit BEFORE notify).
#[test]
fn test_maybe_summarize_skips_when_already_summarizing() {
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer =
        Summarizer::new_silent(Arc::new(NullLlmProvider), "m".to_string(), 128000, store.clone());
    summarizer.summarizing.insert("m:test:dup".to_string(), true);
    let history: Vec<ConversationTurn> = (0..30)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    assert!(!summarizer.maybe_summarize("test:dup", "web", "c1", &history, 128000));
}

/// maybe_summarize on a NON-internal channel notifies the user once.
#[test]
fn test_maybe_summarize_notifies_non_internal_channel() {
    use std::sync::atomic::{AtomicUsize, Ordering};
    struct CountingNotify {
        count: std::sync::Arc<AtomicUsize>,
    }
    impl SummarizationNotifier for CountingNotify {
        fn notify(&self, _c: &str, _id: &str, _msg: &str) {
            self.count.fetch_add(1, Ordering::SeqCst);
        }
    }
    let count = std::sync::Arc::new(AtomicUsize::new(0));
    let store = Arc::new(SessionStore::new_in_memory());
    let summarizer = Summarizer::new(
        Arc::new(NullLlmProvider),
        "m".to_string(),
        128000,
        store,
        Box::new(CountingNotify { count: count.clone() }),
        None,
    );
    let history: Vec<ConversationTurn> = (0..30)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    assert!(summarizer.maybe_summarize("test:notify", "web", "c1", &history, 128000));
    assert_eq!(count.load(Ordering::SeqCst), 1);
}

/// Summarizer with an observer manager attached: the batch path emits
/// ConversationStart/LlmRequest/LlmResponse/ConversationEnd without panicking.
#[test]
fn test_summarize_batch_emits_observer_events() {
    let store = Arc::new(SessionStore::new_in_memory());
    store.get_or_create("obs:key");
    let summarizer = Summarizer::new(
        Arc::new(NullLlmProvider),
        "m".to_string(),
        128000,
        store,
        Box::new(NullNotifier),
        Some(Arc::new(nemesis_observer::Manager::new())),
    );
    let history: Vec<ConversationTurn> = (0..8)
        .map(|i| conv_turn("user", &format!("m{i}")))
        .collect();
    let result = summarizer.summarize_session("obs:key", &history);
    assert_eq!(result, "summary");
}

/// tokio_block_on with NO ambient runtime creates one (Err arm).
#[test]
fn tokio_block_on_creates_runtime_when_none() {
    let v = tokio_block_on(async { 7u32 });
    assert_eq!(v, 7);
}

/// tokio_block_on INSIDE a multi-thread runtime uses block_in_place (Ok arm).
#[tokio::test(flavor = "multi_thread")]
async fn tokio_block_on_uses_current_runtime() {
    let v = tokio_block_on(async { 11u32 });
    assert_eq!(v, 11);
}

/// clear_session: in-memory entry dropped AND the on-disk json removed; the
/// key lazily rebuilds empty; clearing a never-saved key is a quiet no-op.
#[test]
fn test_clear_session_empties_memory_and_disk() {
    let dir = tempfile::tempdir().unwrap();
    let store = SessionStore::new_with_storage(dir.path());
    store.get_or_create("clr:key");
    store.set_history("clr:key", stored_msgs(2));
    store.save("clr:key").unwrap();
    assert!(dir.path().join("clr_key.json").exists());

    store.clear_session("clr:key");
    assert!(!dir.path().join("clr_key.json").exists(), "disk file removed");
    assert!(!store.contains("clr:key"));

    // NotFound arm: clearing again (no file) must not panic or warn.
    store.clear_session("clr:key");

    // Key rebuilds lazily as an empty session.
    store.get_or_create("clr:key");
    assert!(store.get_history("clr:key").is_empty());
}

/// load_from_disk on an in-memory store: returns immediately (None arm).
#[test]
fn test_load_from_disk_in_memory_store_is_noop() {
    let store = SessionStore::new_in_memory();
    store.get_or_create("mem:key");
    store.load_from_disk(); // must not panic or drop in-memory sessions
    assert!(store.contains("mem:key"));
}
