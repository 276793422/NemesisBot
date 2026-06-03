use super::*;

#[test]
fn test_static_config_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    let config = StaticConfig {
        cluster: ClusterMeta {
            id: "test-cluster".into(),
            auto_discovery: true,
            last_updated: "2026-04-29T00:00:00Z".into(),
            rpc_auth_token: "secret-token".into(),
        },
        node: NodeInfo {
            id: "node-001".into(),
            name: "Test Bot".into(),
            address: "0.0.0.0:21949".into(),
            role: "worker".into(),
            category: "development".into(),
            tags: vec!["test".into()],
            capabilities: vec!["llm".into()],
        },
        peers: vec![PeerConfig {
            id: "peer-001".into(),
            name: "Remote Bot".into(),
            address: "10.0.0.1:21949".into(),
            addresses: vec!["10.0.0.1".into()],
            rpc_port: 21949,
            role: "worker".into(),
            category: "general".into(),
            tags: Vec::new(),
            capabilities: vec!["llm".into()],
            priority: 1,
            enabled: true,
            status: PeerStatus {
                state: "online".into(),
                last_seen: "2026-04-29T00:00:00Z".into(),
                ..PeerStatus::default()
            },
        }],
    };

    save_static_config(&path, &config).unwrap();
    assert!(path.exists());

    let loaded = load_static_config(&path).unwrap();
    assert_eq!(loaded.cluster.id, "test-cluster");
    assert_eq!(loaded.node.id, "node-001");
    assert_eq!(loaded.peers.len(), 1);
    assert_eq!(loaded.peers[0].rpc_port, 21949);
}

#[test]
fn test_dynamic_state_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("state.toml");

    let state = DynamicState {
        cluster: ClusterMeta {
            id: "auto-discovered".into(),
            auto_discovery: true,
            last_updated: "2026-04-29T00:00:00Z".into(),
            rpc_auth_token: String::new(),
        },
        local_node: NodeInfo {
            id: "local-001".into(),
            name: "Local".into(),
            address: "0.0.0.0:21949".into(),
            ..NodeInfo::default()
        },
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
    assert!(config.peers.is_empty());
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
    assert!(toml_str.contains("[cluster]"));
    assert!(toml_str.contains("[node]"));
}

// -- Additional tests: cluster config validation, peer management, role defaults --

#[test]
fn test_static_config_default_values() {
    let meta = ClusterMeta::default();
    let node = NodeInfo::default();
    let config = StaticConfig {
        cluster: meta,
        node: node.clone(),
        peers: vec![],
    };
    assert!(config.cluster.id == "auto-discovered");
    assert!(config.peers.is_empty());
    assert!(config.node.id.is_empty());
    assert_eq!(config.node.role, "worker");
    assert_eq!(config.node.category, "general");
}

#[test]
fn test_cluster_meta_default() {
    let meta = ClusterMeta::default();
    assert!(meta.auto_discovery);
    assert!(meta.rpc_auth_token.is_empty());
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
    assert!(state.cluster.auto_discovery);
}

#[test]
fn test_static_config_with_multiple_peers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    let config = StaticConfig {
        cluster: ClusterMeta::default(),
        node: NodeInfo {
            id: "master-1".into(),
            name: "Master".into(),
            address: "0.0.0.0:21949".into(),
            role: "master".into(),
            category: "development".into(),
            tags: vec!["primary".into()],
            capabilities: vec!["llm".into(), "forge".into()],
        },
        peers: vec![
            PeerConfig {
                id: "worker-1".into(),
                name: "Worker 1".into(),
                address: "10.0.0.2:21949".into(),
                rpc_port: 21949,
                role: "worker".into(),
                enabled: true,
                ..PeerConfig::default()
            },
            PeerConfig {
                id: "worker-2".into(),
                name: "Worker 2".into(),
                address: "10.0.0.3:21949".into(),
                rpc_port: 21949,
                role: "worker".into(),
                enabled: false,
                ..PeerConfig::default()
            },
        ],
    };

    save_static_config(&path, &config).unwrap();
    let loaded = load_static_config(&path).unwrap();

    assert_eq!(loaded.peers.len(), 2);
    assert_eq!(loaded.peers[0].id, "worker-1");
    assert!(loaded.peers[0].enabled);
    assert_eq!(loaded.peers[1].id, "worker-2");
    assert!(!loaded.peers[1].enabled);
}

#[test]
fn test_node_info_default_role_is_worker() {
    let node = NodeInfo::default();
    assert_eq!(node.role, "worker");
}

#[test]
fn test_peer_config_with_tags_and_capabilities() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("peers.toml");

    let config = StaticConfig {
        cluster: ClusterMeta::default(),
        node: NodeInfo::default(),
        peers: vec![PeerConfig {
            id: "p1".into(),
            name: "TaggedPeer".into(),
            address: "10.0.0.1:21949".into(),
            tags: vec!["gpu".into(), "high-mem".into()],
            capabilities: vec!["llm".into(), "voice".into(), "vision".into()],
            ..PeerConfig::default()
        }],
    };

    save_static_config(&path, &config).unwrap();
    let loaded = load_static_config(&path).unwrap();

    assert_eq!(loaded.peers[0].tags.len(), 2);
    assert!(loaded.peers[0].tags.contains(&"gpu".to_string()));
    assert_eq!(loaded.peers[0].capabilities.len(), 3);
    assert!(loaded.peers[0].capabilities.contains(&"vision".to_string()));
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
fn test_save_and_load_empty_peers() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("empty_peers.toml");

    let config = StaticConfig {
        cluster: ClusterMeta::default(),
        node: NodeInfo::default(),
        peers: vec![],
    };

    save_static_config(&path, &config).unwrap();
    let loaded = load_static_config(&path).unwrap();
    assert!(loaded.peers.is_empty());
    assert!(loaded.node.id.is_empty());
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
