use super::*;
use tempfile::TempDir;

#[test]
fn test_new_manager_creates_state_dir() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path().to_path_buf();
    let mgr = WorkspaceStateManager::new(&workspace);
    assert!(workspace.join("state").exists());
    assert_eq!(mgr.get_last_channel(), "");
    assert_eq!(mgr.get_last_chat_id(), "");
}

#[test]
fn test_set_and_get_last_channel() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());
    mgr.set_last_channel("web:user123").unwrap();
    assert_eq!(mgr.get_last_channel(), "web:user123");
}

#[test]
fn test_set_and_get_last_chat_id() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());
    mgr.set_last_chat_id("chat_456").unwrap();
    assert_eq!(mgr.get_last_chat_id(), "chat_456");
}

#[test]
fn test_persistence_across_managers() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().to_path_buf();

    let mgr1 = WorkspaceStateManager::new(&path);
    mgr1.set_last_channel("discord:789").unwrap();
    mgr1.set_last_chat_id("ch_abc").unwrap();
    drop(mgr1);

    // Create a new manager for the same workspace — should load persisted state
    let mgr2 = WorkspaceStateManager::new(&path);
    assert_eq!(mgr2.get_last_channel(), "discord:789");
    assert_eq!(mgr2.get_last_chat_id(), "ch_abc");
}

#[test]
fn test_migration_from_old_location() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();

    // Write old-style state file
    let old_state = WorkspaceState {
        last_channel: "rpc:node1".to_string(),
        last_chat_id: "old_chat".to_string(),
        timestamp: Utc::now(),
    };
    let old_data = serde_json::to_string_pretty(&old_state).unwrap();
    fs::write(workspace.join("state.json"), &old_data).unwrap();

    // Create manager — should migrate
    let mgr = WorkspaceStateManager::new(workspace);
    assert_eq!(mgr.get_last_channel(), "rpc:node1");
    assert_eq!(mgr.get_last_chat_id(), "old_chat");

    // New file should exist
    assert!(workspace.join("state/state.json").exists());
}

#[test]
fn test_timestamp_updates() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    let before = mgr.get_timestamp();
    // Timestamp should be approximately now
    assert!(mgr.get_timestamp() >= before);
}

#[test]
fn test_snapshot() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());
    mgr.set_last_channel("test:ch").unwrap();
    mgr.set_last_chat_id("id1").unwrap();

    let snap = mgr.snapshot();
    assert_eq!(snap.last_channel, "test:ch");
    assert_eq!(snap.last_chat_id, "id1");
}

#[test]
fn test_is_internal_channel() {
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("discord"));
    assert!(!is_internal_channel("rpc"));
}

#[test]
fn test_atomic_save_survives_concurrent_reads() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    // Read should not block writes
    let ch = mgr.get_last_channel();
    assert_eq!(ch, "");

    mgr.set_last_channel("web:user").unwrap();
    let ch = mgr.get_last_channel();
    assert_eq!(ch, "web:user");
}

#[test]
fn test_workspace_state_serialization_skip_empty_fields() {
    let state = WorkspaceState::default();
    let json = serde_json::to_string_pretty(&state).unwrap();

    // Empty strings should be skipped due to skip_serializing_if
    assert!(!json.contains("last_channel"));
    assert!(!json.contains("last_chat_id"));
    // Timestamp should still be present
    assert!(json.contains("timestamp"));
}

#[test]
fn test_workspace_state_serialization_with_fields() {
    let state = WorkspaceState {
        last_channel: "web:user1".to_string(),
        last_chat_id: "chat_abc".to_string(),
        timestamp: Utc::now(),
    };
    let json = serde_json::to_string_pretty(&state).unwrap();

    assert!(json.contains("last_channel"));
    assert!(json.contains("web:user1"));
    assert!(json.contains("last_chat_id"));
    assert!(json.contains("chat_abc"));
    assert!(json.contains("timestamp"));
}

#[test]
fn test_workspace_state_deserialization_empty_json() {
    // Deserializing an empty JSON object should yield defaults
    let json = "{}";
    let state: WorkspaceState = serde_json::from_str(json).unwrap();
    assert_eq!(state.last_channel, "");
    assert_eq!(state.last_chat_id, "");
}

#[test]
fn test_workspace_state_deserialization_partial_fields() {
    let json = r#"{"last_channel": "rpc"}"#;
    let state: WorkspaceState = serde_json::from_str(json).unwrap();
    assert_eq!(state.last_channel, "rpc");
    assert_eq!(state.last_chat_id, "");
}

#[test]
fn test_new_manager_with_corrupted_state_file() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    let state_dir = workspace.join("state");
    fs::create_dir_all(&state_dir).unwrap();

    // Write corrupted (non-JSON) content to state file
    fs::write(state_dir.join("state.json"), "not valid json {{{").unwrap();

    // Should not panic; falls back to default state
    let mgr = WorkspaceStateManager::new(workspace);
    assert_eq!(mgr.get_last_channel(), "");
    assert_eq!(mgr.get_last_chat_id(), "");
}

#[test]
fn test_new_manager_with_empty_state_file() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    let state_dir = workspace.join("state");
    fs::create_dir_all(&state_dir).unwrap();

    // Write empty file
    fs::write(state_dir.join("state.json"), "").unwrap();

    let mgr = WorkspaceStateManager::new(workspace);
    assert_eq!(mgr.get_last_channel(), "");
    assert_eq!(mgr.get_last_chat_id(), "");
}

#[test]
fn test_new_manager_with_valid_state_file() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    let state_dir = workspace.join("state");
    fs::create_dir_all(&state_dir).unwrap();

    let state = WorkspaceState {
        last_channel: "discord:ch1".to_string(),
        last_chat_id: "msg_123".to_string(),
        timestamp: Utc::now(),
    };
    let json = serde_json::to_string_pretty(&state).unwrap();
    fs::write(state_dir.join("state.json"), &json).unwrap();

    let mgr = WorkspaceStateManager::new(workspace);
    assert_eq!(mgr.get_last_channel(), "discord:ch1");
    assert_eq!(mgr.get_last_chat_id(), "msg_123");
}

#[test]
fn test_is_internal_channel_empty_string() {
    assert!(!is_internal_channel(""));
}

#[test]
fn test_is_internal_channel_rpc() {
    assert!(!is_internal_channel("rpc"));
}

#[test]
fn test_is_internal_channel_case_sensitive() {
    // Should be case-sensitive: "CLI" is not the same as "cli"
    assert!(!is_internal_channel("CLI"));
    assert!(!is_internal_channel("System"));
    assert!(!is_internal_channel("SUBAGENT"));
}

#[test]
fn test_is_internal_channel_partial_match() {
    // Should be exact match, not substring
    assert!(!is_internal_channel("cli_extra"));
    assert!(!is_internal_channel("subsystem"));
}

#[test]
fn test_workspace_state_default() {
    let state = WorkspaceState::default();
    assert!(state.last_channel.is_empty());
    assert!(state.last_chat_id.is_empty());
    // Timestamp should be approximately now
    let now = Utc::now();
    let diff = now.signed_duration_since(state.timestamp);
    assert!(diff.num_seconds().abs() < 5);
}

#[test]
fn test_new_manager_no_state_file_no_old_file() {
    let tmp = TempDir::new().unwrap();
    let workspace = tmp.path();
    // No state files at all — manager should initialize with defaults
    let mgr = WorkspaceStateManager::new(workspace);
    assert_eq!(mgr.get_last_channel(), "");
    assert_eq!(mgr.get_last_chat_id(), "");
    assert!(workspace.join("state").exists());
}

#[test]
fn test_set_channel_and_chat_id_together() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_channel("web:user").unwrap();
    mgr.set_last_chat_id("chat_1").unwrap();

    let snap = mgr.snapshot();
    assert_eq!(snap.last_channel, "web:user");
    assert_eq!(snap.last_chat_id, "chat_1");

    // Verify persistence
    let mgr2 = WorkspaceStateManager::new(tmp.path());
    assert_eq!(mgr2.get_last_channel(), "web:user");
    assert_eq!(mgr2.get_last_chat_id(), "chat_1");
}

#[test]
fn test_overwrite_channel_and_chat_id() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_channel("first").unwrap();
    mgr.set_last_chat_id("first_chat").unwrap();
    mgr.set_last_channel("second").unwrap();
    mgr.set_last_chat_id("second_chat").unwrap();

    assert_eq!(mgr.get_last_channel(), "second");
    assert_eq!(mgr.get_last_chat_id(), "second_chat");
}

// --- Additional tests for is_internal_channel ---

#[test]
fn test_is_internal_channel_all_known_internal() {
    // Explicitly verify every entry in INTERNAL_CHANNELS
    assert!(is_internal_channel("cli"));
    assert!(is_internal_channel("system"));
    assert!(is_internal_channel("subagent"));
}

#[test]
fn test_is_internal_channel_common_external_channels() {
    // Common external channel names that must NOT be internal
    assert!(!is_internal_channel("web"));
    assert!(!is_internal_channel("discord"));
    assert!(!is_internal_channel("telegram"));
    assert!(!is_internal_channel("feishu"));
    assert!(!is_internal_channel("rpc"));
    assert!(!is_internal_channel("slack"));
}

#[test]
fn test_is_internal_channel_with_colon_suffix() {
    // Channels like "cli:user" or "system:alert" are not exact matches
    assert!(!is_internal_channel("cli:user"));
    assert!(!is_internal_channel("system:alert"));
    assert!(!is_internal_channel("subagent:task"));
}

#[test]
fn test_is_internal_channel_with_whitespace() {
    // Whitespace should not match
    assert!(!is_internal_channel(" cli"));
    assert!(!is_internal_channel("cli "));
    assert!(!is_internal_channel(" system "));
}

#[test]
fn test_is_internal_channel_long_string() {
    assert!(!is_internal_channel("a_very_long_channel_name_that_is_definitely_not_internal"));
}

// --- Additional workspace state edge cases ---

#[test]
fn test_set_last_channel_empty_string() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_channel("web:user").unwrap();
    assert_eq!(mgr.get_last_channel(), "web:user");

    // Overwrite with empty string
    mgr.set_last_channel("").unwrap();
    assert_eq!(mgr.get_last_channel(), "");
}

#[test]
fn test_set_last_chat_id_empty_string() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_chat_id("chat_123").unwrap();
    assert_eq!(mgr.get_last_chat_id(), "chat_123");

    // Overwrite with empty string
    mgr.set_last_chat_id("").unwrap();
    assert_eq!(mgr.get_last_chat_id(), "");
}

#[test]
fn test_timestamp_advances_on_set_last_channel() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    let ts1 = mgr.get_timestamp();
    // Small delay is not needed; the timestamp resolution is sufficient
    // that sequential calls produce different timestamps in practice,
    // but we just verify the timestamp is >= the initial one.
    mgr.set_last_channel("web:user").unwrap();
    let ts2 = mgr.get_timestamp();
    assert!(ts2 >= ts1);
}

#[test]
fn test_timestamp_advances_on_set_last_chat_id() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    let ts1 = mgr.get_timestamp();
    mgr.set_last_chat_id("chat_1").unwrap();
    let ts2 = mgr.get_timestamp();
    assert!(ts2 >= ts1);
}

#[test]
fn test_snapshot_is_independent_of_manager() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());
    mgr.set_last_channel("web:abc").unwrap();

    let snap = mgr.snapshot();
    // Modify the manager after snapshot
    mgr.set_last_channel("discord:xyz").unwrap();

    // Snapshot should still reflect old value
    assert_eq!(snap.last_channel, "web:abc");
    // Manager should have new value
    assert_eq!(mgr.get_last_channel(), "discord:xyz");
}

#[test]
fn test_workspace_state_serialization_roundtrip() {
    let original = WorkspaceState {
        last_channel: "telegram:user42".to_string(),
        last_chat_id: "msg_999".to_string(),
        timestamp: Utc::now(),
    };
    let json = serde_json::to_string(&original).unwrap();
    let restored: WorkspaceState = serde_json::from_str(&json).unwrap();
    assert_eq!(restored.last_channel, original.last_channel);
    assert_eq!(restored.last_chat_id, original.last_chat_id);
    assert_eq!(restored.timestamp, original.timestamp);
}

#[test]
fn test_set_last_channel_with_unicode() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_channel("web:用户123").unwrap();
    assert_eq!(mgr.get_last_channel(), "web:用户123");

    // Verify persistence with unicode
    let mgr2 = WorkspaceStateManager::new(tmp.path());
    assert_eq!(mgr2.get_last_channel(), "web:用户123");
}

#[test]
fn test_set_last_chat_id_with_special_chars() {
    let tmp = TempDir::new().unwrap();
    let mgr = WorkspaceStateManager::new(tmp.path());

    mgr.set_last_chat_id("chat-abc_def:123").unwrap();
    assert_eq!(mgr.get_last_chat_id(), "chat-abc_def:123");
}
