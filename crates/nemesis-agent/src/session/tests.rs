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
        },
        StoredMessage {
            role: "assistant".to_string(),
            content: "Hi there!".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            reasoning_content: None,
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
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "World".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
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
        }],
        summary: "test summary".to_string(),
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
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
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
        },
        StoredMessage {
            role: "assistant".to_string(),
            content: "hi there".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: "2026-01-01T00:00:01Z".to_string(),
            reasoning_content: None,
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
    }];
    for i in 0..20 {
        history.push(ConversationTurn {
            role: if i % 2 == 0 { "user" } else { "assistant" }.to_string(),
            content: format!("message {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
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
        created: chrono::Local::now(),
        updated: chrono::Local::now(),
    };
    let debug_str = format!("{:?}", session);
    assert!(debug_str.contains("test-key"));
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
        },
        ConversationTurn {
            role: "user".to_string(),
            content: "hello".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
        },
        ConversationTurn {
            role: "assistant".to_string(),
            content: "hi".to_string(),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
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
    }];
    for i in 0..10 {
        history.push(ConversationTurn {
            role: "user".to_string(),
            content: format!("msg {}", i),
            tool_calls: Vec::new(),
            tool_call_id: None,
            timestamp: String::new(),
            reasoning_content: None,
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
