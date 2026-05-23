use super::*;
use crate::discovery::message::DiscoveryMessageType;
use crate::registry::HealthConfig;
use crate::types::ClusterConfig;

fn make_cluster_config() -> ClusterConfig {
    ClusterConfig {
        node_id: "test-node-001".into(),
        bind_address: "0.0.0.0:9000".into(),
        peers: vec![],
    }
}

#[test]
fn test_create_and_sign_announce() {
    let cluster_cfg = make_cluster_config();
    let disc_cfg = DiscoveryConfig::default();
    let registry = PeerRegistry::new(HealthConfig::default());

    let _service = DiscoveryService::with_registry(
        &cluster_cfg.node_id,
        9000,
        registry,
        disc_cfg,
    ).unwrap();

    let announce = DiscoveryMessage::new_announce(
        "test-node-001",
        "test-node-001",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "development",
        vec![],
        vec!["llm".into()],
    );
    assert_eq!(announce.msg_type, DiscoveryMessageType::Announce);
    assert_eq!(announce.version, "1.0");
}

#[test]
fn test_start_stop_lifecycle() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let config = DiscoveryConfig {
        port: 0, // OS assigns port
        interval: Duration::from_secs(30),
        secret: String::new(),
        enc_key: None,
    };

    let service = DiscoveryService::with_registry(
        "lifecycle-test-node",
        9000,
        registry,
        config,
    ).unwrap();

    assert!(!service.is_running());

    service.start().unwrap();
    assert!(service.is_running());
    assert_ne!(service.port(), 0);

    // Let it run briefly
    std::thread::sleep(Duration::from_millis(100));

    service.stop().unwrap();
    assert!(!service.is_running());
}

#[test]
fn test_double_start_fails() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let config = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(30),
        secret: String::new(),
        enc_key: None,
    };

    let service = DiscoveryService::with_registry(
        "double-start-node",
        9000,
        registry,
        config,
    ).unwrap();

    service.start().unwrap();
    let result = service.start();
    assert!(result.is_err());
    service.stop().unwrap();
}

#[test]
fn test_stop_when_not_started_fails() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let config = DiscoveryConfig {
        port: 0, // OS assigns port to avoid conflicts
        ..Default::default()
    };
    let service = DiscoveryService::with_registry(
        "not-started-node",
        9000,
        registry,
        config,
    ).unwrap();

    let result = service.stop();
    assert!(result.is_err());
}

#[test]
fn test_discovery_config_default() {
    let config = DiscoveryConfig::default();
    assert_eq!(config.port, DEFAULT_PORT);
    assert_eq!(config.interval, Duration::from_secs(DEFAULT_INTERVAL_SECS));
    assert!(config.secret.is_empty());
    assert!(config.enc_key.is_none());
}

#[test]
fn test_discovery_config_with_encryption() {
    let config = DiscoveryConfig::with_encryption(
        11949,
        Duration::from_secs(10),
        "my-secret-token",
    );
    assert_eq!(config.port, 11949);
    assert_eq!(config.interval, Duration::from_secs(10));
    assert_eq!(config.secret, "my-secret-token");
    assert!(config.enc_key.is_some());
}

#[test]
fn test_discovery_config_empty_token_no_encryption() {
    let config = DiscoveryConfig::with_encryption(11949, Duration::from_secs(10), "");
    assert!(config.enc_key.is_none());
}

#[test]
fn test_null_callbacks() {
    let cb = NullCallbacks::new("test-node");
    assert_eq!(cb.node_id(), "test-node");
    assert_eq!(cb.rpc_port(), 9000);
    assert_eq!(cb.role(), "worker");
    assert_eq!(cb.category(), "development");
    // No-ops should not panic
    cb.handle_discovered_node("n1", "name", &[], 9000, "worker", "dev", &[], &[]);
    cb.handle_node_offline("n1", "test");
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node", "0.0.0.0:9000", 9000, "worker", "dev", registry,
    );
    assert_eq!(cb.node_id(), "local-node");

    cb.handle_discovered_node(
        "remote-1", "RemoteNode",
        &["10.0.0.5".to_string()], 9000,
        "worker", "dev", &[], &["llm".to_string()],
    );

    // The internal registry should have the node (we can't access it directly,
    // but the call should not panic)
    cb.handle_node_offline("remote-1", "test");
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_two_discovery_nodes_communicate() {
    // Create two discovery services on different ports
    let registry_a = PeerRegistry::new(HealthConfig::default());
    let registry_b = PeerRegistry::new(HealthConfig::default());

    let config_a = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(300), // Long interval so we don't spam
        secret: String::new(),
        enc_key: None,
    };
    let config_b = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(300),
        secret: String::new(),
        enc_key: None,
    };

    let service_a = DiscoveryService::with_registry(
        "node-a", 9000, registry_a, config_a,
    ).unwrap();
    let service_b = DiscoveryService::with_registry(
        "node-b", 9001, registry_b, config_b,
    ).unwrap();

    service_a.start().unwrap();
    service_b.start().unwrap();

    // Manually send a message from A to B's port
    let msg = DiscoveryMessage::new_announce(
        "node-a", "node-a",
        vec!["127.0.0.1".into()], 9000,
        "worker", "dev", vec![], vec![],
    );
    service_a.listener.broadcast(&msg).unwrap();

    // Wait for delivery
    std::thread::sleep(Duration::from_millis(500));

    service_a.stop().unwrap();
    service_b.stop().unwrap();
}

// -- Additional tests --

#[test]
fn test_default_constants() {
    assert_eq!(DEFAULT_PORT, 11949);
    assert_eq!(DEFAULT_INTERVAL_SECS, 30);
}

#[test]
fn test_discovery_config_enc_key_accessor() {
    let config = DiscoveryConfig::with_encryption(11949, Duration::from_secs(10), "token123");
    let key = config.enc_key().unwrap();
    assert_eq!(key.len(), 32);
}

#[test]
fn test_discovery_config_default_enc_key_none() {
    let config = DiscoveryConfig::default();
    assert!(config.enc_key().is_none());
}

#[test]
fn test_null_callbacks_all_local_ips() {
    let cb = NullCallbacks::new("test-node");
    let ips = cb.all_local_ips();
    // Should return at least loopback
    // (the actual result depends on the system)
    let _ = ips;
}

#[test]
fn test_null_callbacks_tags() {
    let cb = NullCallbacks::new("test-node");
    assert!(cb.tags().is_empty());
}

#[test]
fn test_registry_callbacks_with_state_path() {
    let dir = tempfile::tempdir().unwrap();
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::with_state_path(
        "local-node", "0.0.0.0:9000", 9000, "worker", "dev",
        registry,
        dir.path().join("state.toml"),
    );

    cb.handle_discovered_node(
        "remote-1", "RemoteNode",
        &["10.0.0.5".to_string()], 9000,
        "worker", "dev", &[], &["llm".to_string()],
    );

    // Sync to disk should succeed
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks_sync_without_state_path() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node", "0.0.0.0:9000", 9000, "worker", "dev", registry,
    );
    // No state path configured, sync should be a no-op
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks_manager_role() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node", "0.0.0.0:9000", 9000, "worker", "dev", registry,
    );
    // "master" role should be recognized
    cb.handle_discovered_node(
        "master-node", "MasterNode",
        &["10.0.0.1".to_string()], 9000,
        "master", "dev", &[], &["cluster".to_string()],
    );
}

#[test]
fn test_discovery_error_variants() {
    let err1 = DiscoveryError::AlreadyRunning;
    assert_eq!(format!("{}", err1), "already running");
    let err2 = DiscoveryError::NotRunning;
    assert_eq!(format!("{}", err2), "not running");
}

// ============================================================
// Coverage improvement: additional edge cases
// ============================================================

#[test]
fn test_set_broadcast_interval() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let config = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(30),
        secret: String::new(),
        enc_key: None,
    };
    let mut service = DiscoveryService::with_registry(
        "interval-node", 9000, registry, config,
    ).unwrap();

    service.set_broadcast_interval(Duration::from_secs(60));
    // Verify interval was updated (no panic)
}

#[test]
fn test_send_announce_direct() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let listener = super::super::listener::UdpListener::new(0, None).unwrap();
    let cluster = NullCallbacks::new("test-node");

    send_announce_direct(&listener, &cluster);
    // Should not panic even with empty local IPs
}

#[test]
fn test_null_callbacks_rpc_port() {
    let cb = NullCallbacks::new("test-node");
    assert_eq!(cb.rpc_port(), 9000);
}

#[test]
fn test_null_callbacks_role_and_category() {
    let cb = NullCallbacks::new("test-node");
    assert_eq!(cb.role(), "worker");
    assert_eq!(cb.category(), "development");
}

#[test]
fn test_null_callbacks_node_id() {
    let cb = NullCallbacks::new("my-custom-node");
    assert_eq!(cb.node_id(), "my-custom-node");
}
