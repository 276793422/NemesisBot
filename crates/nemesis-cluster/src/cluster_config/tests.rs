use super::*;

#[test]
fn test_static_config_roundtrip() {
    // StaticConfig only carries [node]; peers are written via append_peer_to_file.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    let config = StaticConfig {
        node: NodeInfo {
            id: "node-001".into(),
            name: "Test Bot".into(),
            address: "0.0.0.0:21949".into(),
            role: "worker".into(),
            category: "development".into(),
            tags: vec!["test".into()],
        },
    };

    save_static_config(&path, &config).unwrap();
    assert!(path.exists());

    let loaded = load_static_config(&path).unwrap();
    assert_eq!(loaded.node.id, "node-001");
    assert_eq!(loaded.node.name, "Test Bot");
}

#[test]
fn test_dynamic_state_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");

    let state = DynamicState {
        discovered: vec![PeerConfig {
            id: "discovered-001".into(),
            name: "Found Bot".into(),
            address: "10.0.0.2:21949".into(),
            ..PeerConfig::default()
        }],
        last_sync: "2026-04-29T00:00:00Z".into(),
    };

    save_dynamic_state(&path, &state).unwrap();
    let loaded = load_dynamic_state(&path).unwrap();
    assert_eq!(loaded.discovered.len(), 1);
    assert_eq!(loaded.discovered[0].id, "discovered-001");
}

#[test]
fn test_load_static_config_not_found() {
    let result = load_static_config(Path::new("/nonexistent/peers.toml"));
    assert!(result.is_err());
}

#[test]
fn test_load_dynamic_state_not_found_returns_default() {
    let result = load_dynamic_state(Path::new("/nonexistent/state.toml"));
    assert!(result.is_ok());
    let state = result.unwrap();
    assert!(state.discovered.is_empty());
}

#[test]
fn test_create_static_config() {
    let config = create_static_config("node-123", "Test Bot", "0.0.0.0:9000");
    assert_eq!(config.node.id, "node-123");
    assert_eq!(config.node.name, "Test Bot");
}

#[test]
fn test_load_or_create_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // First call: file doesn't exist -> creates default (but doesn't save)
    let config = load_or_create_config(&path, "node-xyz");
    assert_eq!(config.node.id, "node-xyz");

    // Manually save it
    save_static_config(&path, &config).unwrap();

    // Second call: file exists -> loads it
    let loaded = load_or_create_config(&path, "different-id");
    assert_eq!(loaded.node.id, "node-xyz");
}

#[test]
fn test_peer_status_default() {
    let status = PeerStatus::default();
    assert_eq!(status.state, "unknown");
    assert_eq!(status.tasks_completed, 0);
}

#[test]
fn test_toml_serialization_format() {
    let config = create_static_config("node-1", "Bot 1", "0.0.0.0:21949");
    let toml_str = toml::to_string_pretty(&config).unwrap();
    // peers.toml should only contain [node] section (no [cluster], no peers field)
    assert!(!toml_str.contains("[cluster]"));
    assert!(toml_str.contains("[node]"));
    assert!(!toml_str.contains("peers")); // peers field removed from StaticConfig
}

// -- Additional tests: cluster config validation, role defaults --

#[test]
fn test_static_config_default_values() {
    let node = NodeInfo::default();
    let config = StaticConfig { node: node.clone() };
    assert!(config.node.id.is_empty());
    assert_eq!(config.node.role, "worker");
    assert_eq!(config.node.category, "general");
}

#[test]
fn test_peer_config_default() {
    let peer = PeerConfig::default();
    assert_eq!(peer.priority, 1);
    assert!(peer.enabled);
    assert_eq!(peer.rpc_port, 0);
    assert_eq!(peer.status.state, "unknown");
    assert_eq!(peer.status.success_rate, 0.0);
}

#[test]
fn test_dynamic_state_default() {
    let state = DynamicState::default();
    assert!(state.discovered.is_empty());
    assert!(!state.last_sync.is_empty());
}

#[test]
fn test_node_info_default_role_is_worker() {
    let node = NodeInfo::default();
    assert_eq!(node.role, "worker");
}

#[test]
fn test_peer_config_basic() {
    // PeerConfig no longer has tags/capabilities (dead fields removed).
    // Capabilities for remote nodes come from UDP discovery broadcasts.
    let peer = PeerConfig {
        id: "p1".into(),
        name: "TaggedPeer".into(),
        address: "10.0.0.1:21949".into(),
        role: "worker".into(),
        category: "development".into(),
        ..PeerConfig::default()
    };
    assert_eq!(peer.id, "p1");
    assert_eq!(peer.name, "TaggedPeer");
    assert_eq!(peer.role, "worker");
    assert_eq!(peer.category, "development");
}

// -- Additional tests: invalid TOML, directory creation, atomic write edge cases --

#[test]
fn test_load_static_config_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("bad_peers.toml");
    std::fs::write(&path, "this is [not valid {{{{toml").unwrap();

    let result = load_static_config(&path);
    assert!(result.is_err(), "expected error for invalid TOML, got {:?}", result);
}

#[test]
fn test_load_dynamic_state_invalid_toml() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");
    std::fs::write(&path, "broken [toml {{ }} ][").unwrap();

    let result = load_dynamic_state(&path);
    assert!(result.is_err(), "expected error for invalid TOML, got {:?}", result);
}

#[test]
fn test_save_static_config_creates_directory() {
    let dir = tempfile::tempdir().unwrap();
    // Use a path where the parent directory doesn't exist yet
    let path = dir.path().join("subdir/nested/peers.toml");

    let config = create_static_config("node-mkdir", "DirTest", "0.0.0.0:9000");
    save_static_config(&path, &config).unwrap();

    assert!(path.exists());
    let loaded = load_static_config(&path).unwrap();
    assert_eq!(loaded.node.id, "node-mkdir");
    assert_eq!(loaded.node.name, "DirTest");
}

#[test]
fn test_save_dynamic_state_creates_directory() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/nested/dir/state.toml");

    let state = DynamicState::default();
    save_dynamic_state(&path, &state).unwrap();

    assert!(path.exists());
    let loaded = load_dynamic_state(&path).unwrap();
    assert!(loaded.discovered.is_empty());
}

#[test]
fn test_atomic_write_rename_failure() {
    // Write to a path with a null byte which is invalid on both Windows and Unix.
    let dir = tempfile::tempdir().unwrap();
    let invalid_path = dir.path().join("bad\0file.toml");
    let config = create_static_config("node-fail", "FailTest", "0.0.0.0:9000");
    let result = save_static_config(&invalid_path, &config);
    assert!(result.is_err(), "expected error for invalid path, got {:?}", result);
}

#[test]
fn test_atomic_write_cleanup_on_failure() {
    // Verify that the .tmp file is cleaned up when rename fails.
    // We test this indirectly: save to a valid path first, then verify no leftover .tmp
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("cleanup.toml");

    let config = create_static_config("node-clean", "CleanupTest", "0.0.0.0:9000");
    save_static_config(&path, &config).unwrap();

    // After successful save, no .tmp file should remain
    let tmp_path = path.with_extension("toml.tmp");
    assert!(!tmp_path.exists(), "temp file should have been renamed");
    assert!(path.exists());
}

// -- New tests for append_peer_to_file (canonical peers write path) --

#[test]
fn test_append_peer_to_file_creates_new() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // File does not exist; append_peer_to_file should create it.
    append_peer_to_file(&path, "Node-B", "127.0.0.1:11950", "worker", "general").unwrap();

    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[peers.Node-B]"), "content was: {}", content);
    assert!(content.contains("address = \"127.0.0.1:11950\""));
}

#[test]
fn test_append_peer_to_file_preserves_node_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // Start with a StaticConfig containing [node]
    let initial = StaticConfig {
        node: NodeInfo {
            id: "node-A".into(),
            name: "Node A".into(),
            ..NodeInfo::default()
        },
    };
    save_static_config(&path, &initial).unwrap();

    // Now append a peer
    append_peer_to_file(&path, "Node-B", "127.0.0.1:11950", "worker", "general").unwrap();

    // Verify [node] is preserved and [peers.Node-B] was added
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[node]"));
    assert!(content.contains("id = \"node-A\""));
    assert!(content.contains("[peers.Node-B]"));

    // Verify StaticConfig can still be loaded (peers section ignored)
    let reloaded = load_static_config(&path).unwrap();
    assert_eq!(reloaded.node.id, "node-A");
}

#[test]
fn test_append_peer_to_file_appends_multiple() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    append_peer_to_file(&path, "Node-B", "127.0.0.1:11950", "worker", "general").unwrap();
    append_peer_to_file(&path, "Node-C", "127.0.0.1:11951", "worker", "general").unwrap();
    append_peer_to_file(&path, "Node-D", "127.0.0.1:11952", "worker", "general").unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[peers.Node-B]"));
    assert!(content.contains("[peers.Node-C]"));
    assert!(content.contains("[peers.Node-D]"));
}

#[test]
fn test_append_peer_to_file_corrupt_fallback() {
    // If peers.toml is corrupt (invalid TOML), append_peer_to_file should
    // fall back to a fresh table rather than failing.
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");
    std::fs::write(&path, "this is [not valid {{{{toml").unwrap();

    // Should succeed by falling back to empty table
    append_peer_to_file(&path, "Node-X", "10.0.0.5:11950", "worker", "general").unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[peers.Node-X]"), "content was: {}", content);
}

#[test]
fn test_append_peer_to_file_duplicate_warns_and_overwrites() {
    // Adding the same peer_id twice should not panic; it should overwrite
    // the existing entry. (A tracing::warn! is emitted but we don't assert
    // log output here — we just verify no panic and content is updated.)
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    append_peer_to_file(&path, "Node-Dup", "10.0.0.1:11950", "worker", "general").unwrap();
    // Overwrite with different address
    append_peer_to_file(&path, "Node-Dup", "10.0.0.2:11951", "manager", "ml").unwrap();

    let content = std::fs::read_to_string(&path).unwrap();
    // Should have only ONE [peers.Node-Dup] section
    assert_eq!(
        content.matches("[peers.Node-Dup]").count(),
        1,
        "expected single peer entry, content was: {}",
        content
    );
    // Should reflect the updated values
    assert!(content.contains("10.0.0.2:11951"));
    assert!(content.contains("manager"));
    assert!(content.contains("ml"));
    // Old values should NOT be present
    assert!(!content.contains("10.0.0.1:11950"));
}

#[test]
fn test_sanitize_peer_key() {
    // Only `.` and `:` are replaced. `-` and `_` are preserved (TOML v1.0.0
    // allows A-Za-z0-9_- in bare keys).
    assert_eq!(sanitize_peer_key("Node-A"), "Node-A");        // dash preserved
    assert_eq!(sanitize_peer_key("Node_A"), "Node_A");        // underscore preserved
    assert_eq!(sanitize_peer_key("node.example.com"), "node_example_com"); // dots replaced
    assert_eq!(sanitize_peer_key("host:1234"), "host_1234");  // colon replaced
    assert_eq!(
        sanitize_peer_key("Mixed-Case.Host:9"),
        "Mixed-Case_Host_9"
    ); // mixed: dash preserved, dot/colon replaced
}

#[test]
fn test_sanitize_peer_key_dash_preserved() {
    // Regression test: previously sanitize replaced `-` with `_`, which made
    // the mapping non-reversible (gateway.rs reversed `_` to `-`, breaking
    // peer IDs that originally contained `_`). Now both are preserved.
    assert_eq!(sanitize_peer_key("a-b-c"), "a-b-c");
}

#[test]
fn test_sanitize_peer_key_underscore_preserved() {
    assert_eq!(sanitize_peer_key("a_b_c"), "a_b_c");
}

// -- ensure_node_id tests (phase 1: local ID persistence) --

#[test]
fn test_ensure_node_id_creates_file_when_missing() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // File doesn't exist → should create with [node].id
    let modified = ensure_node_id(&path, "node-host-abc123").unwrap();
    assert!(modified, "should report file was modified");

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("[node]"));
    assert!(content.contains("id = \"node-host-abc123\""));
}

#[test]
fn test_ensure_node_id_writes_when_id_empty() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // Pre-create a file with empty id
    std::fs::write(&path, "[node]\nid = \"\"\nname = \"Bot\"\n").unwrap();

    let modified = ensure_node_id(&path, "node-host-def456").unwrap();
    assert!(modified);

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("id = \"node-host-def456\""));
    // Other fields preserved
    assert!(content.contains("name = \"Bot\""));
}

#[test]
fn test_ensure_node_id_noop_when_id_already_set() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // Pre-create with user-set ID
    let original = "[node]\nid = \"user-custom-id\"\nname = \"MyBot\"\n";
    std::fs::write(&path, original).unwrap();

    let modified = ensure_node_id(&path, "node-auto-generated").unwrap();
    assert!(!modified, "should NOT modify when user already set an id");

    let content = std::fs::read_to_string(&path).unwrap();
    // User's id should be preserved (not overwritten by auto-generated)
    assert!(content.contains("id = \"user-custom-id\""));
    assert!(!content.contains("node-auto-generated"));
}

#[test]
fn test_ensure_node_id_preserves_peers_subtables() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    // Pre-create with empty [node].id AND a [peers.X] subtable
    let original = r#"[node]
id = ""
name = "Bot"

[peers.friend]
address = "10.0.0.5:11950"
role = "worker"
"#;
    std::fs::write(&path, original).unwrap();

    let modified = ensure_node_id(&path, "node-auto-xyz").unwrap();
    assert!(modified);

    let content = std::fs::read_to_string(&path).unwrap();
    // [node].id written
    assert!(content.contains("id = \"node-auto-xyz\""));
    // [peers.friend] preserved (NOT lost)
    assert!(content.contains("[peers.friend]"));
    assert!(content.contains("10.0.0.5:11950"));
}

#[test]
fn test_ensure_node_id_creates_parent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("deep/nested/peers.toml");

    let modified = ensure_node_id(&path, "node-host-789").unwrap();
    assert!(modified);
    assert!(path.exists());
}

#[test]
fn test_ensure_node_id_handles_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");
    // Corrupt TOML → ensure_node_id should fall back to fresh table
    std::fs::write(&path, "this is [not valid {{{{toml").unwrap();

    let modified = ensure_node_id(&path, "node-host-recovery").unwrap();
    assert!(modified, "should recover from corrupt file");

    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("id = \"node-host-recovery\""));
}
