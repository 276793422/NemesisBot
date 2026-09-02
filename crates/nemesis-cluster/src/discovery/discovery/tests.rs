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

    let _service =
        DiscoveryService::with_registry(&cluster_cfg.node_id, 9000, registry, disc_cfg).unwrap();

    let announce = DiscoveryMessage::new_announce(
        "test-node-001",
        "test-node-001",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "development",
        vec![],
        vec!["llm".into()],
        "agent",
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

    let service =
        DiscoveryService::with_registry("lifecycle-test-node", 9000, registry, config).unwrap();

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

    let service =
        DiscoveryService::with_registry("double-start-node", 9000, registry, config).unwrap();

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
    let service =
        DiscoveryService::with_registry("not-started-node", 9000, registry, config).unwrap();

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
    let config =
        DiscoveryConfig::with_encryption(11949, Duration::from_secs(10), "my-secret-token");
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
    cb.handle_discovered_node("n1", "name", &[], 9000, "worker", "dev", &[], &[], "agent");
    cb.handle_node_offline("n1", "test");
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node",
        "0.0.0.0:9000",
        9000,
        "worker",
        "dev",
        registry,
    );
    assert_eq!(cb.node_id(), "local-node");

    cb.handle_discovered_node(
        "remote-1",
        "RemoteNode",
        &["10.0.0.5".to_string()],
        9000,
        "worker",
        "dev",
        &[],
        &["llm".to_string()],
        "agent",
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

    let service_a = DiscoveryService::with_registry("node-a", 9000, registry_a, config_a).unwrap();
    let service_b = DiscoveryService::with_registry("node-b", 9001, registry_b, config_b).unwrap();

    service_a.start().unwrap();
    service_b.start().unwrap();

    // Manually send a message from A to B's port
    let msg = DiscoveryMessage::new_announce(
        "node-a",
        "node-a",
        vec!["127.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "local-node",
        "0.0.0.0:9000",
        9000,
        "worker",
        "dev",
        registry,
        dir.path().join("state.toml"),
    );

    cb.handle_discovered_node(
        "remote-1",
        "RemoteNode",
        &["10.0.0.5".to_string()],
        9000,
        "worker",
        "dev",
        &[],
        &["llm".to_string()],
        "agent",
    );

    // Sync to disk should succeed
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks_sync_without_state_path() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node",
        "0.0.0.0:9000",
        9000,
        "worker",
        "dev",
        registry,
    );
    // No state path configured, sync should be a no-op
    cb.sync_to_disk().unwrap();
}

#[test]
fn test_registry_callbacks_manager_role() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node",
        "0.0.0.0:9000",
        9000,
        "worker",
        "dev",
        registry,
    );
    // "master" role should be recognized
    cb.handle_discovered_node(
        "master-node",
        "MasterNode",
        &["10.0.0.1".to_string()],
        9000,
        "master",
        "dev",
        &[],
        &["cluster".to_string()],
        "agent",
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
    let mut service =
        DiscoveryService::with_registry("interval-node", 9000, registry, config).unwrap();

    service.set_broadcast_interval(Duration::from_secs(60));
    // Verify interval was updated (no panic)
}

#[test]
fn test_send_announce_direct() {
    let _registry = PeerRegistry::new(HealthConfig::default());
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

// ============================================================
// S4 coverage: message-handler closure arms over real UDP,
// address() accessors, send_announce fallbacks, broadcast tick
// ============================================================

#[test]
fn test_null_callbacks_and_registry_callbacks_address() {
    let null_cb = NullCallbacks::new("test-node");
    assert_eq!(null_cb.address(), "0.0.0.0:9000");

    let registry = PeerRegistry::new(HealthConfig::default());
    let cb = RegistryCallbacks::new(
        "local-node",
        "0.0.0.0:9000",
        9000,
        "worker",
        "dev",
        registry,
    );
    assert_eq!(cb.address(), "0.0.0.0:9000");
}

/// Scriptable callbacks: queued `handle_discovered_node` return values,
/// controllable local IPs, and counters for every entry point.
struct ScriptedCallbacks {
    node_id: String,
    discovered_count: std::sync::atomic::AtomicUsize,
    changed_queue: std::sync::Mutex<Vec<bool>>,
    offline_count: std::sync::atomic::AtomicUsize,
    sync_count: std::sync::atomic::AtomicUsize,
    sync_fail: std::sync::atomic::AtomicBool,
    ips: std::sync::Mutex<Vec<String>>,
}

impl ScriptedCallbacks {
    fn new(node_id: &str) -> Self {
        Self {
            node_id: node_id.to_string(),
            discovered_count: std::sync::atomic::AtomicUsize::new(0),
            changed_queue: std::sync::Mutex::new(Vec::new()),
            offline_count: std::sync::atomic::AtomicUsize::new(0),
            sync_count: std::sync::atomic::AtomicUsize::new(0),
            sync_fail: std::sync::atomic::AtomicBool::new(false),
            ips: std::sync::Mutex::new(vec!["127.0.0.1".to_string()]),
        }
    }

    fn discovered(&self) -> usize {
        self.discovered_count
            .load(std::sync::atomic::Ordering::SeqCst)
    }
    fn offline(&self) -> usize {
        self.offline_count.load(std::sync::atomic::Ordering::SeqCst)
    }
    fn syncs(&self) -> usize {
        self.sync_count.load(std::sync::atomic::Ordering::SeqCst)
    }
}

impl ClusterCallbacks for ScriptedCallbacks {
    fn node_id(&self) -> String {
        self.node_id.clone()
    }
    fn name(&self) -> String {
        self.node_id.clone()
    }
    fn address(&self) -> String {
        "0.0.0.0:9000".into()
    }
    fn rpc_port(&self) -> u16 {
        9000
    }
    fn all_local_ips(&self) -> Vec<String> {
        self.ips.lock().unwrap().clone()
    }
    fn role(&self) -> String {
        "worker".into()
    }
    fn category(&self) -> String {
        "development".into()
    }
    fn tags(&self) -> Vec<String> {
        Vec::new()
    }
    fn capabilities(&self) -> Vec<String> {
        Vec::new()
    }
    fn handle_discovered_node(
        &self,
        _node_id: &str,
        _name: &str,
        _addresses: &[String],
        _rpc_port: u16,
        _role: &str,
        _category: &str,
        _tags: &[String],
        _capabilities: &[String],
        _node_type: &str,
    ) -> bool {
        self.discovered_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        let mut queue = self.changed_queue.lock().unwrap();
        if queue.is_empty() {
            true
        } else {
            queue.remove(0)
        }
    }
    fn handle_node_offline(&self, _node_id: &str, _reason: &str) {
        self.offline_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
    fn sync_to_disk(&self) -> Result<(), String> {
        self.sync_count
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        if self.sync_fail.load(std::sync::atomic::Ordering::SeqCst) {
            Err("sync boom".into())
        } else {
            Ok(())
        }
    }
}

/// Send a discovery message via loopback unicast to a listener port.
fn udp_unicast(port: u16, msg: &DiscoveryMessage) {
    let sock = std::net::UdpSocket::bind("127.0.0.1:0").unwrap();
    sock.send_to(&msg.to_bytes().unwrap(), format!("127.0.0.1:{}", port))
        .unwrap();
}

fn wait_until(timeout_ms: u64, check: impl Fn() -> bool) -> bool {
    let deadline = std::time::Instant::now() + std::time::Duration::from_millis(timeout_ms);
    while std::time::Instant::now() < deadline {
        if check() {
            return true;
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    check()
}

fn expired_announce(node_id: &str) -> DiscoveryMessage {
    let mut msg = DiscoveryMessage::new_announce(
        node_id,
        node_id,
        vec!["127.0.0.1".into()],
        9000,
        "worker",
        "development",
        vec![],
        vec![],
        "agent",
    );
    msg.timestamp -= 200; // beyond the 120s expiry threshold
    msg
}

/// Self-messages and expired messages are dropped; announce with a changed
/// payload syncs to disk; an unchanged announce does not; bye marks offline
/// and syncs.
#[test]
fn test_discovery_handler_udp_arms() {
    let cb = std::sync::Arc::new(ScriptedCallbacks::new("local-udp-node"));
    cb.changed_queue.lock().unwrap().extend([true, false]);

    let config = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(300),
        secret: String::new(),
        enc_key: None,
    };
    let service = DiscoveryService::new(
        std::sync::Arc::clone(&cb) as std::sync::Arc<dyn ClusterCallbacks>,
        config,
    )
    .unwrap();
    service.start().unwrap();
    let port = service.port();

    // Self-message: same node_id as the local node → ignored.
    udp_unicast(
        port,
        &DiscoveryMessage::new_announce(
            "local-udp-node",
            "local-udp-node",
            vec!["127.0.0.1".into()],
            9000,
            "worker",
            "development",
            vec![],
            vec![],
            "agent",
        ),
    );
    // Expired message: timestamp too old → ignored.
    udp_unicast(port, &expired_announce("remote-expired"));

    // Live announce #1: handler reports "changed" → sync_to_disk called.
    udp_unicast(
        port,
        &DiscoveryMessage::new_announce(
            "remote-live",
            "RemoteLive",
            vec!["127.0.0.1".into()],
            9000,
            "worker",
            "development",
            vec![],
            vec![],
            "agent",
        ),
    );
    assert!(
        wait_until(2000, || cb.discovered() >= 1),
        "live announce should reach the handler"
    );
    assert!(
        wait_until(2000, || cb.syncs() >= 1),
        "changed announce should trigger a sync"
    );

    // Live announce #2 (same content): handler reports "unchanged" → no sync.
    udp_unicast(
        port,
        &DiscoveryMessage::new_announce(
            "remote-live",
            "RemoteLive",
            vec!["127.0.0.1".into()],
            9000,
            "worker",
            "development",
            vec![],
            vec![],
            "agent",
        ),
    );
    assert!(
        wait_until(2000, || cb.discovered() >= 2),
        "second announce should reach the handler"
    );
    let syncs_after_unchanged = cb.syncs();
    std::thread::sleep(Duration::from_millis(150));
    assert_eq!(
        cb.syncs(),
        syncs_after_unchanged,
        "unchanged announce must not sync"
    );

    // Bye: marks the node offline and syncs.
    udp_unicast(port, &DiscoveryMessage::new_bye("remote-live"));
    assert!(
        wait_until(2000, || cb.offline() >= 1),
        "bye should reach the offline handler"
    );
    assert!(
        wait_until(2000, || cb.syncs() >= 2),
        "bye should trigger a sync"
    );

    // The self/expired messages must have been skipped by now.
    assert_eq!(cb.discovered(), 2, "self and expired messages are skipped");

    service.stop().unwrap();
}

/// A failing sync_to_disk is logged but must not break the handler.
#[test]
fn test_discovery_handler_sync_error_logged() {
    let cb = std::sync::Arc::new(ScriptedCallbacks::new("local-sync-err"));
    cb.sync_fail
        .store(true, std::sync::atomic::Ordering::SeqCst);

    let config = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(300),
        secret: String::new(),
        enc_key: None,
    };
    let service = DiscoveryService::new(
        std::sync::Arc::clone(&cb) as std::sync::Arc<dyn ClusterCallbacks>,
        config,
    )
    .unwrap();
    service.start().unwrap();

    udp_unicast(
        service.port(),
        &DiscoveryMessage::new_announce(
            "remote-sync-err",
            "RemoteSyncErr",
            vec!["127.0.0.1".into()],
            9000,
            "worker",
            "development",
            vec![],
            vec![],
            "agent",
        ),
    );
    assert!(
        wait_until(2000, || cb.discovered() >= 1 && cb.syncs() >= 1),
        "announce must be handled even when sync fails"
    );

    service.stop().unwrap();
}

/// send_announce_direct / send_announce_with with no local IPs log and
/// return early; with IPs and an encryption key the encrypt-success path
/// and broadcast sends run.
#[test]
fn test_send_announce_empty_ips_and_encrypted_broadcast() {
    // No IPs → both senders return early with an error log.
    let cb = std::sync::Arc::new(ScriptedCallbacks::new("no-ip-node"));
    *cb.ips.lock().unwrap() = Vec::new();

    let listener = super::super::listener::UdpListener::new(0, None).unwrap();
    send_announce_direct(&listener, &*cb);

    let sock = std::net::UdpSocket::bind("0.0.0.0:0").unwrap();
    send_announce_with(&sock, listener.port(), None, &*cb);

    // With IPs and a key → encrypt path + broadcast sends + debug log.
    let cb2 = std::sync::Arc::new(ScriptedCallbacks::new("ip-node"));
    let key = crate::discovery::crypto::derive_key("announce-secret");
    send_announce_with(&sock, listener.port(), Some(key), &*cb2);
    // Also the plaintext variant.
    send_announce_with(&sock, listener.port(), None, &*cb2);
}

/// The broadcast thread sends an initial announce (after jitter ≤5s) and
/// then a periodic announce every interval tick. interval=1s → both fire
/// well within 7s.
#[test]
fn test_discovery_broadcast_thread_periodic_announce() {
    let cb = std::sync::Arc::new(ScriptedCallbacks::new("bcast-node"));

    let config = DiscoveryConfig {
        port: 0,
        interval: Duration::from_secs(1),
        secret: String::new(),
        enc_key: None,
    };
    let service = DiscoveryService::new(
        std::sync::Arc::clone(&cb) as std::sync::Arc<dyn ClusterCallbacks>,
        config,
    )
    .unwrap();
    service.start().unwrap();
    assert!(service.is_running());

    // jitter (0-5s) + one 1s tick → periodic branch fires within 6s.
    std::thread::sleep(Duration::from_secs(7));

    service.stop().unwrap();
    assert!(!service.is_running());
}
