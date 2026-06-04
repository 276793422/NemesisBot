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
    /// Get the human-readable node name (e.g. "Node-A").
    fn name(&self) -> &str;
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
    /// Get the dynamic capabilities (tool names from the AgentLoop).
    fn capabilities(&self) -> Vec<String>;
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
    fn name(&self) -> &str { &self.node_id }
    fn address(&self) -> &str { &self.address }
    fn rpc_port(&self) -> u16 { self.rpc_port }
    fn all_local_ips(&self) -> Vec<String> { get_all_local_ips() }
    fn role(&self) -> &str { &self.role }
    fn category(&self) -> &str { &self.category }
    fn tags(&self) -> Vec<String> { Vec::new() }
    fn capabilities(&self) -> Vec<String> { Vec::new() }
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
    fn name(&self) -> &str { &self.node_id }
    fn address(&self) -> &str { &self.address }
    fn rpc_port(&self) -> u16 { self.rpc_port }
    fn all_local_ips(&self) -> Vec<String> { get_all_local_ips() }
    fn role(&self) -> &str { &self.role }
    fn category(&self) -> &str { &self.category }
    fn tags(&self) -> Vec<String> { Vec::new() }
    fn capabilities(&self) -> Vec<String> { Vec::new() }

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
                tracing::debug!(node_id = %msg.node_id, "[Discovery] Ignoring expired discovery message");
                return;
            }

            tracing::info!(
                msg_type = %msg.msg_type,
                node_id = %msg.node_id,
                "[Discovery] Received discovery message"
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
                    tracing::info!(node_id = %msg.node_id, "[Discovery] Node discovered/updated");
                    if let Err(e) = cluster.sync_to_disk() {
                        tracing::error!(error = %e, "[Discovery] Failed to sync config");
                    }
                }
                super::message::DiscoveryMessageType::Bye => {
                    cluster.handle_node_offline(&msg.node_id, "node shutdown");
                    tracing::info!(node_id = %msg.node_id, "[Discovery] Node marked offline (bye)");
                    if let Err(e) = cluster.sync_to_disk() {
                        tracing::error!(error = %e, "[Discovery] Failed to sync config");
                    }
                }
            }
        }));

        // Start listener
        self.listener.start()?;

        tracing::info!(port = %self.listener.port(), "[Discovery] Discovery started");

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
            tracing::error!(error = %e, "[Discovery] Failed to broadcast bye message");
        }

        self.listener.stop()?;
        tracing::info!("[Discovery] Discovery stopped");
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
        tracing::error!("[Discovery] No local IP addresses available for broadcast");
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
        cluster.capabilities(),
    );

    if let Err(e) = listener.broadcast(&msg) {
        tracing::error!(error = %e, "[Discovery] Failed to send announce");
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
        tracing::error!("[Discovery] No local IP addresses available for broadcast");
        return;
    }

    let msg = DiscoveryMessage::new_announce(
        cluster.node_id(),
        cluster.name(),
        addresses,
        cluster.rpc_port(),
        cluster.role(),
        cluster.category(),
        cluster.tags(),
        cluster.capabilities(),
    );

    let data = match msg.to_bytes() {
        Ok(d) => d,
        Err(e) => {
            tracing::error!(error = %e, "[Discovery] Failed to marshal announce");
            return;
        }
    };

    let send_data = if let Some(key) = enc_key {
        match crate::discovery::crypto::encrypt_data(&key, &data) {
            Ok(encrypted) => encrypted,
            Err(_) => {
                tracing::error!("[Discovery] Failed to encrypt announce");
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
        "[Discovery] Announce sent"
    );
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
