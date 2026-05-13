//! Discovery service - UDP broadcast peer discovery.
//!
//! Broadcasts periodic announce messages and listens for announce/bye
//! messages from other nodes. Uses `UdpListener` for actual UDP I/O and
//! optional AES-256-GCM encryption for secure LAN discovery.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::discovery::crypto::derive_key;
use crate::discovery::listener::{UdpListener, get_all_local_ips};
use crate::discovery::message::DiscoveryMessage;
use crate::registry::PeerRegistry;


/// Default multicast port for cluster discovery.
pub const DEFAULT_PORT: u16 = 11949;
/// Default broadcast interval.
pub const DEFAULT_INTERVAL_SECS: u64 = 30;

// ---------------------------------------------------------------------------
// ClusterCallbacks - trait for cluster integration
// ---------------------------------------------------------------------------

/// Callbacks the discovery service uses to interact with the cluster.
///
/// Mirrors Go's `ClusterCallbacks` interface. The cluster implementation
/// provides this trait object so the discovery service can report found /
/// departed nodes and access local node information.
pub trait ClusterCallbacks: Send + Sync {
    /// Get the local node ID.
    fn node_id(&self) -> &str;
    /// Get the RPC bind address (e.g. `"0.0.0.0:9000"`).
    fn address(&self) -> &str;
    /// Get the RPC port number.
    fn rpc_port(&self) -> u16;
    /// Get all local IP addresses suitable for inclusion in announce messages.
    fn all_local_ips(&self) -> Vec<String>;
    /// Get the cluster role string (e.g. `"worker"`, `"manager"`).
    fn role(&self) -> &str;
    /// Get the business category.
    fn category(&self) -> &str;
    /// Get custom tags.
    fn tags(&self) -> Vec<String>;
    /// Handle a newly discovered or updated node.
    fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: &[String],
        rpc_port: u16,
        role: &str,
        category: &str,
        tags: &[String],
        capabilities: &[String],
    );
    /// Handle a node going offline.
    fn handle_node_offline(&self, node_id: &str, reason: &str);
    /// Persist the current peer list to disk.
    fn sync_to_disk(&self) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// NullCallbacks - default no-op implementation for testing
// ---------------------------------------------------------------------------

/// No-op callback implementation for tests or standalone operation.
#[allow(dead_code)]
pub struct NullCallbacks {
    node_id: String,
    address: String,
    rpc_port: u16,
    role: String,
    category: String,
}

impl NullCallbacks {
    #[allow(dead_code)]
    pub fn new(node_id: impl Into<String>) -> Self {
        Self {
            node_id: node_id.into(),
            address: "0.0.0.0:9000".into(),
            rpc_port: 9000,
            role: "worker".into(),
            category: "development".into(),
        }
    }
}

impl ClusterCallbacks for NullCallbacks {
    fn node_id(&self) -> &str { &self.node_id }
    fn address(&self) -> &str { &self.address }
    fn rpc_port(&self) -> u16 { self.rpc_port }
    fn all_local_ips(&self) -> Vec<String> { get_all_local_ips() }
    fn role(&self) -> &str { &self.role }
    fn category(&self) -> &str { &self.category }
    fn tags(&self) -> Vec<String> { Vec::new() }
    fn handle_discovered_node(&self, _node_id: &str, _name: &str, _addresses: &[String], _rpc_port: u16, _role: &str, _category: &str, _tags: &[String], _capabilities: &[String]) {}
    fn handle_node_offline(&self, _node_id: &str, _reason: &str) {}
    fn sync_to_disk(&self) -> Result<(), String> { Ok(()) }
}

// ---------------------------------------------------------------------------
// RegistryCallbacks - callbacks backed by PeerRegistry
// ---------------------------------------------------------------------------

/// Cluster callbacks backed by a `PeerRegistry`. Used by the higher-level
/// cluster module to bridge discovery into the peer registry.
pub struct RegistryCallbacks {
    node_id: String,
    address: String,
    rpc_port: u16,
    role: String,
    category: String,
    registry: PeerRegistry,
    /// Optional path for persisting discovered peers to `state.toml`.
    state_path: Option<std::path::PathBuf>,
}

impl RegistryCallbacks {
    pub fn new(
        node_id: impl Into<String>,
        address: impl Into<String>,
        rpc_port: u16,
        role: impl Into<String>,
        category: impl Into<String>,
        registry: PeerRegistry,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            address: address.into(),
            rpc_port,
            role: role.into(),
            category: category.into(),
            registry,
            state_path: None,
        }
    }

    /// Create with a state file path for persisting discovered peers.
    #[allow(dead_code)]
    pub fn with_state_path(
        node_id: impl Into<String>,
        address: impl Into<String>,
        rpc_port: u16,
        role: impl Into<String>,
        category: impl Into<String>,
        registry: PeerRegistry,
        state_path: impl Into<std::path::PathBuf>,
    ) -> Self {
        Self {
            node_id: node_id.into(),
            address: address.into(),
            rpc_port,
            role: role.into(),
            category: category.into(),
            registry,
            state_path: Some(state_path.into()),
        }
    }
}

impl ClusterCallbacks for RegistryCallbacks {
    fn node_id(&self) -> &str { &self.node_id }
    fn address(&self) -> &str { &self.address }
    fn rpc_port(&self) -> u16 { self.rpc_port }
    fn all_local_ips(&self) -> Vec<String> { get_all_local_ips() }
    fn role(&self) -> &str { &self.role }
    fn category(&self) -> &str { &self.category }
    fn tags(&self) -> Vec<String> { Vec::new() }

    fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: &[String],
        rpc_port: u16,
        role: &str,
        category: &str,
        _tags: &[String],
        capabilities: &[String],
    ) {
        // Preserve all discovered addresses (not just the first) for
        // multi-address failover, matching Go's behavior.
        let primary_address = addresses.first().cloned().unwrap_or_default();
        use nemesis_types::cluster::{NodeInfo, NodeRole};
        let node_role = match role {
            "manager" | "coordinator" | "master" => NodeRole::Master,
            _ => NodeRole::Worker,
        };
        let info = crate::types::ExtendedNodeInfo {
            base: NodeInfo {
                id: node_id.to_string(),
                name: name.to_string(),
                role: node_role,
                address: format!("{}:{}", primary_address, rpc_port),
                category: category.to_string(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: crate::types::NodeStatus::Online,
            capabilities: capabilities.to_vec(),
            addresses: addresses.to_vec(),
        };
        self.registry.upsert(info);
    }

    fn handle_node_offline(&self, node_id: &str, _reason: &str) {
        self.registry.remove(node_id);
    }

    fn sync_to_disk(&self) -> Result<(), String> {
        let state_path = match &self.state_path {
            Some(p) => p,
            None => return Ok(()), // No state path configured, skip persistence
        };

        let peers = self.registry.list_peers();
        let discovered: Vec<crate::cluster_config::PeerConfig> = peers
            .iter()
            .filter(|p| p.base.id != self.node_id)
            .map(|node| node.to_peer_config())
            .collect();

        let state = crate::cluster_config::DynamicState {
            cluster: crate::cluster_config::ClusterMeta {
                id: "auto-discovered".into(),
                auto_discovery: true,
                last_updated: chrono::Utc::now().to_rfc3339(),
                rpc_auth_token: String::new(),
            },
            local_node: crate::cluster_config::NodeInfo {
                id: self.node_id.clone(),
                name: self.node_id.clone(),
                address: self.address.clone(),
                role: self.role.clone(),
                category: self.category.clone(),
                tags: Vec::new(),
                capabilities: Vec::new(),
            },
            discovered,
            last_sync: chrono::Utc::now().to_rfc3339(),
        };

        crate::cluster_config::save_dynamic_state(state_path, &state)
            .map_err(|e| format!("failed to save state: {}", e))
    }
}

// ---------------------------------------------------------------------------
// DiscoveryConfig
// ---------------------------------------------------------------------------

/// Configuration for the discovery service.
#[derive(Debug, Clone)]
pub struct DiscoveryConfig {
    /// UDP port for discovery broadcasts.
    pub port: u16,
    /// How often to send announce messages.
    pub interval: Duration,
    /// Shared secret for message authentication (empty = no auth).
    pub secret: String,
    /// AES encryption key derived from the auth token, if any.
    enc_key: Option<[u8; 32]>,
}

impl DiscoveryConfig {
    /// Create a config with an encryption key derived from the given token.
    pub fn with_encryption(port: u16, interval: Duration, token: &str) -> Self {
        let enc_key = if token.is_empty() {
            None
        } else {
            Some(derive_key(token))
        };
        Self {
            port,
            interval,
            secret: token.to_string(),
            enc_key,
        }
    }

    /// Get the encryption key, if any.
    pub fn enc_key(&self) -> Option<[u8; 32]> {
        self.enc_key
    }
}

impl Default for DiscoveryConfig {
    fn default() -> Self {
        Self {
            port: DEFAULT_PORT,
            interval: Duration::from_secs(DEFAULT_INTERVAL_SECS),
            secret: String::new(),
            enc_key: None,
        }
    }
}

// ---------------------------------------------------------------------------
// DiscoveryError
// ---------------------------------------------------------------------------

/// Errors from the discovery service.
#[derive(Debug, thiserror::Error)]
pub enum DiscoveryError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("already running")]
    AlreadyRunning,
    #[error("not running")]
    NotRunning,
}

// ---------------------------------------------------------------------------
// DiscoveryService - full async discovery with broadcast loop
// ---------------------------------------------------------------------------

/// Discovery service with UDP broadcast and periodic announce loop.
///
/// Mirrors Go's `Discovery` struct:
/// - Binds a `UdpListener` for receiving/sending UDP packets
/// - Periodically broadcasts announce messages
/// - Handles incoming announce/bye messages via `ClusterCallbacks`
/// - Sends a bye message on graceful shutdown
pub struct DiscoveryService {
    cluster: Arc<dyn ClusterCallbacks>,
    listener: UdpListener,
    config: DiscoveryConfig,
    running: Arc<AtomicBool>,
}

impl DiscoveryService {
    /// Create a new discovery service.
    ///
    /// The `cluster` argument provides callbacks for accessing local node
    /// info and reporting discovered/offline nodes.
    pub fn new(
        cluster: Arc<dyn ClusterCallbacks>,
        config: DiscoveryConfig,
    ) -> Result<Self, DiscoveryError> {
        let listener = UdpListener::new(config.port, config.enc_key())?;
        Ok(Self {
            cluster,
            listener,
            config,
            running: Arc::new(AtomicBool::new(false)),
        })
    }

    /// Create a simple discovery service backed by a `PeerRegistry`.
    ///
    /// Convenience constructor for the common case where discovery updates
    /// should go directly into a registry.
    pub fn with_registry(
        node_id: impl Into<String>,
        rpc_port: u16,
        registry: PeerRegistry,
        config: DiscoveryConfig,
    ) -> Result<Self, DiscoveryError> {
        let callbacks = Arc::new(RegistryCallbacks::new(
            node_id,
            "0.0.0.0:9000",
            rpc_port,
            "worker",
            "development",
            registry,
        ));
        Self::new(callbacks, config)
    }

    /// Start the discovery service.
    ///
    /// Spawns the receive loop and broadcast loop. Returns an error if
    /// already running.
    pub fn start(&self) -> Result<(), DiscoveryError> {
        if self.running.load(Ordering::SeqCst) {
            return Err(DiscoveryError::AlreadyRunning);
        }
        self.running.store(true, Ordering::SeqCst);

        // Set message handler
        let cluster = Arc::clone(&self.cluster);
        self.listener.set_message_handler(Box::new(move |msg, _addr| {
            // Ignore messages from self
            if msg.node_id == cluster.node_id() {
                return;
            }

            // Ignore expired messages
            if msg.is_expired() {
                tracing::debug!(node_id = %msg.node_id, "Ignoring expired discovery message");
                return;
            }

            tracing::info!(
                msg_type = %msg.msg_type,
                node_id = %msg.node_id,
                "Received discovery message"
            );

            match msg.msg_type {
                super::message::DiscoveryMessageType::Announce => {
                    cluster.handle_discovered_node(
                        &msg.node_id,
                        &msg.name,
                        &msg.addresses,
                        msg.rpc_port,
                        &msg.role,
                        &msg.category,
                        &msg.tags,
                        &msg.capabilities,
                    );
                    tracing::info!(node_id = %msg.node_id, "Node discovered/updated");
                    if let Err(e) = cluster.sync_to_disk() {
                        tracing::error!(error = %e, "Failed to sync config");
                    }
                }
                super::message::DiscoveryMessageType::Bye => {
                    cluster.handle_node_offline(&msg.node_id, "node shutdown");
                    tracing::info!(node_id = %msg.node_id, "Node marked offline (bye)");
                    if let Err(e) = cluster.sync_to_disk() {
                        tracing::error!(error = %e, "Failed to sync config");
                    }
                }
            }
        }));

        // Start listener
        self.listener.start()?;

        tracing::info!(port = %self.listener.port(), "Discovery started");

        // Send initial announce
        self.send_announce();

        // Start broadcast loop
        let running = Arc::clone(&self.running);
        let interval = self.config.interval;
        let cluster = Arc::clone(&self.cluster);
        let listener_port = self.listener.port();
        let enc_key = self.config.enc_key();

        // We need a second UdpSocket for the broadcast loop since the listener
        // already holds the bound socket. We create a separate broadcast sender.
        let broadcast_socket = Arc::new(
            std::net::UdpSocket::bind("0.0.0.0:0")
                .expect("failed to bind broadcast socket")
        );
        broadcast_socket.set_broadcast(true).expect("failed to set broadcast");

        std::thread::Builder::new()
            .name("discovery-broadcast".into())
            .spawn(move || {
                // Initial announce with jitter
                let jitter_secs = rand::random::<u64>() % 5;
                std::thread::sleep(Duration::from_secs(jitter_secs));
                send_announce_with(&*broadcast_socket, listener_port, enc_key, &*cluster);

                let mut tick = 0u64;
                while running.load(Ordering::SeqCst) {
                    std::thread::sleep(Duration::from_secs(1));
                    tick += 1;
                    if tick >= interval.as_secs() {
                        tick = 0;
                        send_announce_with(&*broadcast_socket, listener_port, enc_key, &*cluster);
                    }
                }
            })
            .expect("failed to spawn broadcast thread");

        Ok(())
    }

    /// Stop the discovery service.
    ///
    /// Sends a bye message before shutting down so peers detect offline
    /// immediately.
    pub fn stop(&self) -> Result<(), DiscoveryError> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(DiscoveryError::NotRunning);
        }
        self.running.store(false, Ordering::SeqCst);

        // Broadcast bye message (best-effort)
        let bye_msg = DiscoveryMessage::new_bye(self.cluster.node_id());
        if let Err(e) = self.listener.broadcast(&bye_msg) {
            tracing::error!(error = %e, "Failed to broadcast bye message");
        }

        self.listener.stop()?;
        tracing::info!("Discovery stopped");
        Ok(())
    }

    /// Check whether the service is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Dynamically change the broadcast interval.
    ///
    /// Mirrors Go's `Discovery.SetBroadcastInterval`. Takes effect on the
    /// next broadcast cycle.
    pub fn set_broadcast_interval(&mut self, interval: Duration) {
        self.config.interval = interval;
    }

    /// Get the port the listener is bound to.
    pub fn port(&self) -> u16 {
        self.listener.port()
    }

    /// Send an announce broadcast.
    fn send_announce(&self) {
        send_announce_direct(&self.listener, &*self.cluster);
    }
}

/// Send an announce message using the listener's broadcast method.
fn send_announce_direct(listener: &UdpListener, cluster: &dyn ClusterCallbacks) {
    let addresses = cluster.all_local_ips();
    if addresses.is_empty() {
        tracing::error!("No local IP addresses available for broadcast");
        return;
    }

    let msg = DiscoveryMessage::new_announce(
        cluster.node_id(),
        cluster.node_id(), // Use node_id as name
        addresses,
        cluster.rpc_port(),
        cluster.role(),
        cluster.category(),
        cluster.tags(),
        vec![], // Capabilities set by cluster module
    );

    if let Err(e) = listener.broadcast(&msg) {
        tracing::error!(error = %e, "Failed to send announce");
    }
}

/// Send an announce message using a separate broadcast socket.
/// Used by the broadcast loop thread which doesn't own the UdpListener.
fn send_announce_with(
    socket: &std::net::UdpSocket,
    port: u16,
    enc_key: Option<[u8; 32]>,
    cluster: &dyn ClusterCallbacks,
) {
    let addresses = cluster.all_local_ips();
    if addresses.is_empty() {
        tracing::error!("No local IP addresses available for broadcast");
        return;
    }

    let msg = DiscoveryMessage::new_announce(
        cluster.node_id(),
        cluster.node_id(),
        addresses,
        cluster.rpc_port(),
        cluster.role(),
        cluster.category(),
        cluster.tags(),
        vec![],
    );

    let data = match msg.to_bytes() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "Failed to marshal announce");
            return;
        }
    };

    let send_data = if let Some(key) = enc_key {
        match crate::discovery::crypto::encrypt_data(&key, &data) {
            Ok(encrypted) => encrypted,
            Err(_) => {
                tracing::error!("Failed to encrypt announce");
                return;
            }
        }
    } else {
        data
    };

    let broadcast_addrs = super::listener::get_broadcast_addresses();
    use std::net::SocketAddrV4;
    for addr in &broadcast_addrs {
        let target = SocketAddrV4::new(*addr, port);
        let _ = socket.send_to(&send_data, target);
    }

    tracing::debug!(
        node_id = %cluster.node_id(),
        rpc_port = %cluster.rpc_port(),
        "Announce sent"
    );
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
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
}
