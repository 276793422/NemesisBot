//! Cluster - manages node discovery, task distribution, and RPC lifecycle.
//!
//! The central orchestrator for a cluster node. Owns the registry, task manager,
//! continuation store, result store, RPC client/server, and discovery components.
//! Provides the `CallWithContext`, `SubmitTask`, and `SetMessageBus` APIs
//! consumed by the agent loop.

use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde_json;
use tokio::sync::broadcast;

use nemesis_types::cluster::Task;

use crate::cluster_config::{
    DynamicState, PeerConfig, PeerStatus,
};
use crate::config_loader::ConfigError;
use crate::continuation_store::ContinuationStore;
use crate::discovery::ClusterCallbacks;
use crate::logger;
use crate::network;
use crate::registry::{HealthConfig, PeerRegistry};
use crate::rpc::client::{LocalNetworkInterface, PeerResolver, RpcClient};
use crate::task_manager::TaskManager;
use crate::task_result_store::TaskResultStore;
use crate::types::{ClusterConfig, ExtendedNodeInfo, NodeStatus};

// ---------------------------------------------------------------------------
// Bus interface (decoupled from nemesis-bus to avoid circular deps)
// ---------------------------------------------------------------------------

/// Inbound message published to the message bus.
#[derive(Debug, Clone)]
pub struct BusInboundMessage {
    pub channel: String,
    pub sender_id: String,
    pub chat_id: String,
    pub content: String,
}

/// Trait for publishing messages to the message bus.
pub trait MessageBus: Send + Sync {
    fn publish_inbound(&self, msg: BusInboundMessage);
}

// ---------------------------------------------------------------------------
// Cluster
// ---------------------------------------------------------------------------

/// Default ports and intervals (matching Go implementation).
pub const DEFAULT_UDP_PORT: u16 = 11949;
pub const DEFAULT_RPC_PORT: u16 = 21949;
pub const DEFAULT_BROADCAST_INTERVAL: Duration = Duration::from_secs(30);

/// The cluster manages a set of nodes and distributes tasks.
pub struct Cluster {
    // -- Identity --
    node_id: String,
    node_name: parking_lot::RwLock<String>,
    node_type: String,
    address: String,
    role: parking_lot::RwLock<String>,
    category: parking_lot::RwLock<String>,
    tags: parking_lot::RwLock<Vec<String>>,
    /// Dynamic capabilities reported by the AgentLoop (tool names).
    /// Set via `set_capabilities()` after the agent is built.
    /// Wrapped in Arc for sharing with RPC handler closures (real-time reads).
    capabilities: Arc<std::sync::Mutex<Vec<String>>>,

    // -- Paths --
    workspace: PathBuf,
    #[allow(dead_code)]
    static_config_path: PathBuf,
    dynamic_state_path: PathBuf,

    // -- Components --
    registry: Arc<PeerRegistry>,
    task_manager: Arc<TaskManager>,
    cont_store: Arc<ContinuationStore>,
    result_store: Arc<TaskResultStore>,
    rpc_client: Mutex<Option<Arc<RpcClient>>>,
    /// RPC server instance.
    rpc_server: Option<Arc<crate::rpc::server::RpcServer>>,
    /// RPC channel for LLM communication (set by AgentLoop).
    rpc_channel: RwLock<Option<Arc<dyn crate::rpc::RpcChannel>>>,

    // -- Configuration --
    udp_port: u16,
    rpc_port: u16,
    broadcast_interval: Duration,

    // -- State --
    running: RwLock<bool>,
    discovery_running: Arc<AtomicBool>,
    discovery: Mutex<Option<crate::discovery::DiscoveryService>>,
    stop_tx: broadcast::Sender<()>,
    bus: Mutex<Option<Arc<dyn MessageBus>>>,
    /// Blacklisted peer IDs — removed nodes that should not be re-discovered.
    removed_peers: parking_lot::RwLock<std::collections::HashSet<String>>,

    // -- Cluster Agent --
    cluster_task_list: Mutex<Option<Arc<crate::cluster_task::ClusterTaskList>>>,
    cluster_work_queue: Mutex<Option<Arc<crate::cluster_task::ClusterWorkQueue>>>,

    // -- Testing override for CallWithContext --
    call_with_context_fn:
        Mutex<Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>>>,
}

impl Cluster {
    /// Create a new cluster with the given configuration.
    pub fn new(config: ClusterConfig) -> Self {
        let workspace = std::env::current_dir().unwrap_or_default();
        let cluster_dir_path = workspace.join("cluster");

        let (stop_tx, _) = broadcast::channel(1);

        let node_id = if config.node_id.is_empty() {
            generate_node_id()
        } else {
            config.node_id.clone()
        };

        Self {
            node_id: node_id.clone(),
            node_name: parking_lot::RwLock::new(format!("Bot {}", &node_id[..8.min(node_id.len())])),
            node_type: "agent".into(),
            address: config.bind_address.clone(),
            role: parking_lot::RwLock::new("worker".into()),
            category: parking_lot::RwLock::new("general".into()),
            tags: parking_lot::RwLock::new(Vec::new()),
            capabilities: Arc::new(std::sync::Mutex::new(Vec::new())),
            workspace: workspace.clone(),
            static_config_path: cluster_dir_path.join("peers.toml"),
            dynamic_state_path: cluster_dir_path.join("state.toml"),
            registry: Arc::new(PeerRegistry::new(HealthConfig::default())),
            task_manager: Arc::new(TaskManager::new()),
            cont_store: Arc::new(ContinuationStore::new(cluster_dir_path.join("rpc_cache"))),
            result_store: Arc::new(TaskResultStore::new(1000)),
            rpc_client: Mutex::new(None),
            rpc_server: None,
            rpc_channel: RwLock::new(None),
            udp_port: DEFAULT_UDP_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
            running: RwLock::new(false),
            discovery_running: Arc::new(AtomicBool::new(false)),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Mutex::new(None),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
        }
    }

    /// Create a cluster with a task manager callback.
    pub fn with_callback(
        config: ClusterConfig,
        on_complete: Box<dyn Fn(&Task) + Send + Sync>,
    ) -> Self {
        let cluster = Self::new(config);
        cluster.task_manager.set_callback(on_complete);
        cluster
    }

    /// Create a cluster with a workspace path for config loading.
    pub fn with_workspace(config: ClusterConfig, workspace: PathBuf) -> Self {
        let cluster_dir = workspace.join("cluster");
        let (stop_tx, _) = broadcast::channel(1);

        let node_id = if config.node_id.is_empty() {
            generate_node_id()
        } else {
            config.node_id.clone()
        };

        // Try to load existing node identity from static config
        let peers_path = cluster_dir.join("peers.toml");
        let sc = crate::cluster_config::load_static_config(&peers_path).ok();
        let node_id = sc.as_ref()
            .and_then(|s| if s.node.id.is_empty() { None } else { Some(s.node.id.clone()) })
            .unwrap_or(node_id);

        // Persist runtime-generated node_id to peers.toml [node].id so it
        // remains stable across restarts. No-op if user has already set one.
        if let Err(e) = crate::cluster_config::ensure_node_id(&peers_path, &node_id) {
            tracing::warn!("[Cluster] Failed to persist node_id to peers.toml: {}", e);
        }
        let node_name_default = sc.as_ref()
            .and_then(|s| if s.node.name.is_empty() { None } else { Some(s.node.name.clone()) })
            .unwrap_or_else(|| format!("Bot {}", &node_id[..8.min(node_id.len())]));
        let role_default = sc.as_ref()
            .map(|s| s.node.role.clone())
            .unwrap_or_else(|| "worker".into());
        let category_default = sc.as_ref()
            .map(|s| s.node.category.clone())
            .unwrap_or_else(|| "general".into());
        let tags_default = sc.as_ref()
            .map(|s| s.node.tags.clone())
            .unwrap_or_default();

        Self {
            node_id: node_id.clone(),
            node_name: parking_lot::RwLock::new(node_name_default),
            node_type: "agent".into(),
            address: config.bind_address.clone(),
            role: parking_lot::RwLock::new(role_default),
            category: parking_lot::RwLock::new(category_default),
            tags: parking_lot::RwLock::new(tags_default),
            capabilities: Arc::new(std::sync::Mutex::new(Vec::new())),
            workspace: workspace.clone(),
            static_config_path: cluster_dir.join("peers.toml"),
            dynamic_state_path: cluster_dir.join("state.toml"),
            registry: Arc::new(PeerRegistry::new(HealthConfig::default())),
            task_manager: Arc::new(TaskManager::new()),
            cont_store: Arc::new(ContinuationStore::new(cluster_dir.join("rpc_cache"))),
            result_store: Arc::new(TaskResultStore::new(1000)),
            rpc_client: Mutex::new(None),
            rpc_server: None,
            rpc_channel: RwLock::new(None),
            udp_port: DEFAULT_UDP_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
            running: RwLock::new(false),
            discovery_running: Arc::new(AtomicBool::new(false)),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Mutex::new(None),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
        }
    }

    // -- Lifecycle ------------------------------------------------------------

    /// Start the cluster. Registers the local node and initializes the RPC client.
    pub fn start(&self) {
        if *self.running.read() {
            tracing::debug!("[Cluster] Already running, skipping start()");
            return;
        }
        *self.running.write() = true;

        // Register local node
        // Ensure the local node always has the "cluster" capability.
        let mut local_caps = self.capabilities.lock().unwrap_or_else(|e| e.into_inner()).clone();
        if !local_caps.iter().any(|c| c.eq_ignore_ascii_case("cluster")) {
            local_caps.push("cluster".into());
        }

        // Resolve 0.0.0.0 to an actual IP for node registration.
        // TCP bind still uses 0.0.0.0 to listen on all interfaces.
        let display_address = if self.address.starts_with("0.0.0.0") {
            let port = self.address.rsplit(':').next().unwrap_or("");
            let ips = network::get_all_local_ips();
            let ip = ips.iter()
                .find(|ip| !ip.starts_with("127."))
                .or_else(|| ips.first())
                .map(|s| s.as_str())
                .unwrap_or("127.0.0.1");
            format!("{}:{}", ip, port)
        } else {
            self.address.clone()
        };

        let role_str = self.role.read().clone();
        let role = match role_str.as_str() {
            "master" | "manager" => nemesis_types::cluster::NodeRole::Master,
            _ => nemesis_types::cluster::NodeRole::Worker,
        };
        let local_node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: self.node_id.clone(),
                name: self.node_name.read().clone(),
                role,
                address: display_address,
                category: self.category.read().clone(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: local_caps.clone(),
            addresses: vec![],
            node_type: self.node_type.clone(),
        };
        self.registry.upsert(local_node);

        // Initialize RPC client with peer resolver backed by our registry.
        // If a client was already set via set_rpc_client(), this is a no-op.
        if self.rpc_client.lock().is_none() {
            let resolver = Arc::new(ClusterPeerResolver {
                registry: self.registry.clone(),
                node_id: self.node_id.clone(),
            });
            let client = Arc::new(RpcClient::with_resolver(resolver));
            tracing::info!(
                "[Cluster] RPC client initialized, node_id={}, rpc_port={}",
                self.node_id,
                self.rpc_port,
            );
            *self.rpc_client.lock() = Some(client);
        }

        // Load RPC auth token from config.cluster.json and apply to server/client.
        // MUST be after RPC client creation so token is set on both server and client.
        self.load_rpc_auth_token();

        // Start the recovery loop
        self.start_recovery_loop();

        // Start the sync loop (periodic node timeout check + disk persistence)
        self.start_sync_loop();

        logger::log_lifecycle("start", &self.node_id, &format!("rpc_port={}", self.rpc_port));
    }

    /// Load the RPC auth token from `workspace/config/config.cluster.json`
    /// and apply it to the RPC server and client.
    ///
    /// This is called automatically during `start()` so that token auth
    /// works without any manual wiring in gateway.rs or cluster node.
    fn load_rpc_auth_token(&self) {
        let cfg_path = self.workspace.join("config").join("config.cluster.json");
        if !cfg_path.exists() {
            return;
        }

        let data = match std::fs::read_to_string(&cfg_path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(path = %cfg_path.display(), error = %e, "[Cluster] Failed to read cluster config for token");
                return;
            }
        };

        let token = match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(v) => v.get("token").and_then(|t| t.as_str()).unwrap_or("").to_string(),
            Err(e) => {
                tracing::warn!(path = %cfg_path.display(), error = %e, "[Cluster] Failed to parse cluster config for token");
                return;
            }
        };

        if token.is_empty() {
            tracing::info!("[Cluster] No RPC auth token configured — running without auth");
            return;
        }

        // Apply to RPC server
        if let Some(ref server) = self.rpc_server {
            server.set_auth_token(&token);
            tracing::info!("[Cluster] RPC server auth token loaded");
        }

        // Apply to RPC client
        if let Some(ref client) = *self.rpc_client.lock() {
            client.set_auth_token(token.clone());
            tracing::info!("[Cluster] RPC client auth token loaded");
        }
    }

    /// Start UDP discovery service.
    ///
    /// Call this after wrapping Cluster in `Arc` and passing the `Arc` as `arc_self`.
    /// Reads the encryption key from the same `token` field in `config.cluster.json`
    /// used for RPC auth. If no token is configured, discovery runs without encryption.
    pub fn start_discovery(&self, arc_self: Arc<dyn ClusterCallbacks>) {
        if self.discovery_running.load(Ordering::SeqCst) {
            tracing::warn!("[Cluster] Discovery already running, skipping");
            return;
        }

        let secret = self.load_discovery_secret();

        let discovery_config = crate::discovery::DiscoveryConfig::with_encryption(
            self.udp_port,
            self.broadcast_interval,
            &secret,
        );

        match crate::discovery::DiscoveryService::new(arc_self, discovery_config) {
            Ok(discovery) => {
                match discovery.start() {
                    Ok(_) => {
                        self.discovery_running.store(true, Ordering::SeqCst);
                        tracing::info!(
                            port = %self.udp_port,
                            encrypted = !secret.is_empty(),
                            "[Cluster] UDP discovery started"
                        );
                    }
                    Err(e) => {
                        tracing::error!(error = %e, "[Cluster] Failed to start UDP discovery");
                    }
                }
                // Store discovery in Cluster for lifecycle management.
                // DiscoveryService holds Arc<dyn ClusterCallbacks> which keeps
                // the Cluster alive via Arc cycle. This cycle is broken when
                // stop_discovery() drops the DiscoveryService.
                *self.discovery.lock() = Some(discovery);
            }
            Err(e) => {
                tracing::error!(error = %e, "[Cluster] Failed to create discovery service");
            }
        }
    }

    /// Load the discovery encryption secret from `workspace/config/config.cluster.json`.
    ///
    /// Uses the same `token` field as RPC auth. Returns empty string if no token
    /// is configured, meaning discovery runs without encryption.
    fn load_discovery_secret(&self) -> String {
        let cfg_path = self.workspace.join("config").join("config.cluster.json");
        if !cfg_path.exists() {
            return String::new();
        }

        match std::fs::read_to_string(&cfg_path) {
            Ok(data) => {
                serde_json::from_str::<serde_json::Value>(&data)
                    .ok()
                    .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from))
                    .unwrap_or_default()
            }
            Err(_) => String::new(),
        }
    }

    /// Stop the cluster. Stops discovery (joins threads) and signals shutdown.
    pub fn stop(&self) {
        *self.running.write() = false;

        // Stop RPC server first — reject new connections, existing connections
        // drain naturally via idle_timeout.
        if let Some(ref server) = self.rpc_server {
            if let Err(e) = server.stop() {
                tracing::warn!(error = %e, "[Cluster] RPC server stop error");
            }
        }

        // Stop discovery service (joins broadcast + receive threads)
        self.discovery_running.store(false, Ordering::SeqCst);
        if let Some(discovery) = self.discovery.lock().take() {
            if let Err(e) = discovery.stop() {
                tracing::warn!(error = %e, "[Cluster] Discovery stop error");
            }
        }

        // Signal recovery/sync loops to exit
        let _ = self.stop_tx.send(());
        logger::log_lifecycle("stop", &self.node_id, "Cluster stopped");
    }

    /// Check whether the cluster is running.
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    // -- Recovery loop ---------------------------------------------------------

    /// Spawn the recovery loop as a background tokio task.
    ///
    /// Runs every 2 minutes, polling B-nodes for stale pending tasks whose
    /// results may have been lost (e.g. callback failure).  Uses the real
    /// RPC client when available, falling back to the test override.
    fn start_recovery_loop(&self) {
        // Only spawn if we're inside a tokio runtime
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return, // No runtime available (e.g. in unit tests)
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let task_manager = self.task_manager.clone();
        let call_fn = self.call_with_context_fn.lock().clone();
        let rpc_client = self.rpc_client.lock().clone();

        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        poll_stale_pending_tasks(
                            &task_manager,
                            &call_fn,
                            rpc_client.as_deref(),
                        ).await;
                    }
                }
            }
        });
    }

    /// Spawn the sync loop as a background tokio task.
    ///
    /// Mirrors Go's `Cluster.syncLoop()`. Runs every `broadcast_interval`,
    /// checks for node timeouts and persists state to disk.
    fn start_sync_loop(&self) {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return,
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let registry = self.registry.clone();
        let local_node_id = self.node_id.clone();
        let workspace = self.workspace.clone();

        // Interval matches Go's broadcastInterval
        let interval_duration = self.broadcast_interval;

        handle.spawn(async move {
            let mut interval = tokio::time::interval(interval_duration);
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        // Refresh self first. The sync loop cannot call
                        // sync_local_node_to_registry (it's &self and the
                        // spawned task is 'static), so without this the local
                        // node's last_health_check would age out and check_health
                        // would flip self to Offline.
                        registry.mark_healthy(&local_node_id);

                        // Mark Online peers as Offline if last_health_check is
                        // older than stale_timeout_secs (default 90s = 3×
                        // broadcast_interval, tolerates 2 consecutive dropped
                        // UDP announces). Offline peers STAY in the registry
                        // — users expect the node list to retain known
                        // peers (including self) even when they're offline,
                        // and removing them would cause "select a peer"
                        // lookups to degrade into self-calls.
                        let expired = registry.check_health();
                        for node_id in &expired {
                            logger::log_discovery_info(&format!("Node expired: {}", node_id));
                        }

                        // Sync state to disk
                        let state_path = workspace.join("cluster").join("state.toml");
                        let state = DynamicState {
                            discovered: registry.list_peers().iter().map(|n| {
                                let mut pc = n.to_peer_config();
                                pc.status.state = n.get_status_string().into();
                                pc
                            }).collect(),
                            last_sync: chrono::Local::now().to_rfc3339(),
                        };
                        if let Err(e) = crate::cluster_config::save_dynamic_state(&state_path, &state) {
                            logger::log_discovery_error(&format!("Failed to sync config: {}", e));
                        }
                    }
                }
            }
        });
    }

    // -- Node management ------------------------------------------------------

    /// Get info about a specific node.
    pub fn get_node_info(&self, node_id: &str) -> Option<ExtendedNodeInfo> {
        self.registry.get(node_id)
    }

    /// List all known nodes.
    pub fn list_nodes(&self) -> Vec<ExtendedNodeInfo> {
        self.registry.list_peers()
    }

    /// Register a remote node.
    pub fn register_node(&self, info: ExtendedNodeInfo) {
        self.registry.upsert(info);
    }

    /// Remove a node.
    pub fn remove_node(&self, node_id: &str) -> bool {
        let removed = self.registry.remove(node_id);
        if removed {
            self.removed_peers.write().insert(node_id.to_string());
            crate::logger::log_discovery("removed", "", Some(node_id));
        }
        // Persist deletion to peers.toml so the node does not reappear after
        // restart. `remove_peer_from_file` is idempotent, so calling it even
        // when nothing was removed from the registry is safe — but we always
        // call it to cover the case where the file has a stale entry that the
        // registry already forgot about. The in-memory blacklist above
        // prevents UDP from immediately re-adding the node this session; it
        // clears on restart, allowing re-discovery (matching the "until
        // re-discovered" semantics).
        if let Err(e) =
            crate::cluster_config::remove_peer_from_file(&self.static_config_path, node_id)
        {
            tracing::warn!(
                node_id = node_id,
                error = %e,
                "[Cluster] Failed to remove peer from peers.toml"
            );
        }
        removed
    }

    /// Remove a node from the blacklist, allowing it to be re-discovered.
    pub fn unban_node(&self, node_id: &str) -> bool {
        self.removed_peers.write().remove(node_id)
    }

    /// Handle a discovered node (from UDP broadcast or manual config).
    pub fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: Vec<String>,
        rpc_port: u16,
        _role: &str,
        category: &str,
        _tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: &str,
    ) -> bool {
        // Skip blacklisted nodes
        if self.removed_peers.read().contains(node_id) {
            return false;
        }

        let primary_address = if !addresses.is_empty() {
            format!("{}:{}", addresses[0], rpc_port)
        } else {
            String::new()
        };

        let was_known = self.registry.get(node_id).is_some();

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: node_id.into(),
                name: name.into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: primary_address.clone(),
                category: category.into(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities,
            addresses, // Preserve all addresses for multi-address failover
            node_type: node_type.to_string(),
        };
        let changed = self.registry.upsert_if_changed(node);

        // Phase 4: If a placeholder peer exists at the same address (i.e. a
        // manually-added entry keyed by name/address instead of the real ID),
        // upgrade it now. The placeholder is removed from both registry and
        // peers.toml, leaving only the canonical real_id entry.
        //
        // Note: find_by_address may return the just-inserted real_id entry
        // itself (since it also matches the address). We loop over all matches
        // via list_peers to find any *other* entry at the same address that
        // isn't the real_id and remove it.
        if !primary_address.is_empty() {
            let placeholders: Vec<String> = self
                .registry
                .list_peers()
                .into_iter()
                .filter(|p| {
                    p.base.id != node_id
                        && (addr_eq(&p.base.address, &primary_address)
                            || p.addresses.iter().any(|a| addr_eq(a, &primary_address)))
                })
                .map(|p| p.base.id)
                .collect();
            for placeholder_id in placeholders {
                tracing::info!(
                    real_id = node_id,
                    placeholder_id = %placeholder_id,
                    address = %primary_address,
                    "[Cluster] UDP discovery upgrading placeholder peer to real ID"
                );
                self.registry.remove(&placeholder_id);
                self.upgrade_peer_in_peers_toml(
                    &placeholder_id,
                    node_id,
                    &RealNodeInfo {
                        id: node_id.into(),
                        name: name.into(),
                        address: primary_address.clone(),
                        role: nemesis_types::cluster::NodeRole::Worker,
                        category: category.into(),
                        capabilities: Vec::new(),
                        node_type: node_type.into(),
                    },
                );
            }
        }

        if !changed && was_known {
            tracing::trace!(
                node_id = node_id,
                "[Cluster] Node unchanged, health refreshed"
            );
        } else if was_known {
            logger::log_discovery("updated", &primary_address, Some(node_id));
        } else {
            logger::log_discovery("discovered", &primary_address, Some(node_id));
            tracing::info!(
                node_id = node_id,
                name = name,
                addr = %primary_address,
                category = category,
                "[Cluster] Node discovered: id={}, addr={}",
                node_id,
                primary_address,
            );
        }
        changed
    }

    /// Mark a node as offline.
    pub fn handle_node_offline(&self, node_id: &str, _reason: &str) {
        if let Some(mut info) = self.registry.get(node_id) {
            tracing::warn!(
                node_id = node_id,
                name = %info.base.name,
                "[Cluster] Node went offline: id={}",
                node_id,
            );
            info.status = NodeStatus::Offline;
            self.registry.upsert(info);
            logger::log_discovery("offline", "", Some(node_id));
        } else {
            tracing::debug!(
                node_id = node_id,
                "[Cluster] Node offline event for unknown node: id={}",
                node_id,
            );
        }
    }

    /// Merge real node info obtained from RPC `get_info` or UDP AnnounceMessage.
    ///
    /// When a node is manually added (via `nodes.add` or cluster CLI) it is
    /// stored under a placeholder peer_id (user-supplied ID, or the node name,
    /// or the address). The real node ID is only learned when the remote comes
    /// online and we either:
    ///   - Phase 3: actively call `get_info` RPC, or
    ///   - Phase 4: passively receive an AnnounceMessage via UDP discovery.
    ///
    /// This function performs the merge:
    ///   1. If an entry with the real_id already exists in the registry,
    ///      update its fields (name, role, address, category, capabilities,
    ///      node_type) and refresh `last_seen`.
    ///   2. Otherwise, search for a placeholder by address. If found, remove
    ///      the placeholder and insert a new entry keyed by real_id.
    ///   3. Otherwise, insert a brand new entry under real_id.
    ///
    /// The peers.toml file is updated to reflect the merge: the placeholder
    /// subtable `[peers.{placeholder}]` is removed and a new subtable
    /// `[peers.{real_id}]` is added (or the existing one updated).
    ///
    /// `addresses` and `status` are NOT overwritten from the incoming data:
    ///   - `addresses` is a local perspective (we may know about IPs the
    ///     remote didn't broadcast in this payload).
    ///   - `status` is a local observation (we may have just failed a health
    ///     check). The caller can separately mark the node online if warranted.
    ///
    /// Returns the canonical node_id that was written (i.e. `real_id`).
    pub fn merge_real_node_info(&self, info: &RealNodeInfo) -> String {
        // 1. Existing entry with real_id → update fields
        if let Some(mut existing) = self.registry.get(&info.id) {
            existing.base.name = info.name.clone();
            existing.base.role = info.role;
            existing.base.category = info.category.clone();
            if !info.address.is_empty() {
                existing.base.address = info.address.clone();
            }
            existing.base.last_seen = chrono::Local::now().to_rfc3339();
            existing.capabilities = info.capabilities.clone();
            existing.node_type = info.node_type.clone();
            self.registry.upsert(existing);
            self.persist_real_peer_to_toml(&info.id, info);
            return info.id.clone();
        }

        // 2. Placeholder by address → remove + insert under real_id
        let placeholder_id = self
            .registry
            .find_by_address(&info.address)
            .map(|p| p.base.id.clone());

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: info.id.clone(),
                name: info.name.clone(),
                role: info.role,
                address: info.address.clone(),
                category: info.category.clone(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: info.capabilities.clone(),
            addresses: Vec::new(),
            node_type: info.node_type.clone(),
        };
        self.registry.upsert(node);

        if let Some(placeholder) = placeholder_id {
            if placeholder != info.id {
                tracing::info!(
                    real_id = %info.id,
                    placeholder_id = %placeholder,
                    address = %info.address,
                    "[Cluster] Upgrading placeholder peer to real ID"
                );
                self.registry.remove(&placeholder);
                self.upgrade_peer_in_peers_toml(&placeholder, &info.id, info);
            } else {
                self.persist_real_peer_to_toml(&info.id, info);
            }
        } else {
            // 3. Brand new entry — persist under real_id
            self.persist_real_peer_to_toml(&info.id, info);
        }

        info.id.clone()
    }

    /// Persist the real peer info to peers.toml under `[peers.{real_id}]`.
    fn persist_real_peer_to_toml(&self, real_id: &str, info: &RealNodeInfo) {
        let path = &self.static_config_path;
        let role_str = match info.role {
            nemesis_types::cluster::NodeRole::Master => "master",
            nemesis_types::cluster::NodeRole::Worker => "worker",
        };
        if let Err(e) = crate::cluster_config::append_peer_to_file(
            path,
            real_id,
            &info.address,
            role_str,
            &info.category,
        ) {
            tracing::warn!(
                real_id = real_id,
                error = %e,
                "[Cluster] Failed to persist real peer info to peers.toml"
            );
        }
    }

    /// Remove `[peers.{placeholder}]` and add `[peers.{real_id}]` in peers.toml.
    ///
    /// If both keys sanitize to the same value (i.e. the user already added the
    /// peer with the real ID), this is a no-op aside from a content refresh via
    /// `persist_real_peer_to_toml`. Otherwise the placeholder is deleted and the
    /// real ID is written.
    fn upgrade_peer_in_peers_toml(
        &self,
        placeholder: &str,
        real_id: &str,
        info: &RealNodeInfo,
    ) {
        let path = &self.static_config_path;
        // Same key after sanitization → just write the real_id content.
        if crate::cluster_config::sanitize_peer_key(placeholder)
            == crate::cluster_config::sanitize_peer_key(real_id)
        {
            self.persist_real_peer_to_toml(real_id, info);
            return;
        }

        // Load → strip placeholder → atomic write → re-add real_id.
        let mut doc: toml::Value = match (|| -> Result<toml::Value, String> {
            if !path.exists() {
                return Ok(toml::Value::Table(toml::value::Table::new()));
            }
            let content = std::fs::read_to_string(path)
                .map_err(|e| format!("read peers.toml: {}", e))?;
            content
                .parse::<toml::Value>()
                .or_else(|_| Ok(toml::Value::Table(toml::value::Table::new())))
        })() {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(error = %e, "[Cluster] Skipping peers.toml upgrade (load failed)");
                return;
            }
        };

        let table = match doc.as_table_mut() {
            Some(t) => t,
            None => return,
        };

        let peers_table = match table.get_mut("peers").and_then(|v| v.as_table_mut()) {
            Some(t) => t,
            None => {
                // No [peers] section yet → just write the real entry via append.
                self.persist_real_peer_to_toml(real_id, info);
                return;
            }
        };

        let placeholder_key = crate::cluster_config::sanitize_peer_key(placeholder);
        let removed = peers_table.remove(&placeholder_key);
        if removed.is_some() {
            tracing::info!(
                placeholder_key = %placeholder_key,
                real_id = real_id,
                "[Cluster] Removed placeholder from peers.toml"
            );
        }

        // Atomic write back the modified doc.
        let toml_str = match toml::to_string_pretty(&doc) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(error = %e, "[Cluster] Failed to serialize peers.toml after upgrade");
                return;
            }
        };
        if let Err(e) = write_atomic(path, toml_str.as_bytes()) {
            tracing::warn!(error = %e, "[Cluster] Failed to write peers.toml after upgrade");
        }

        // Append real_id entry (atomic).
        self.persist_real_peer_to_toml(real_id, info);
    }

    // -- Task management ------------------------------------------------------

    /// Submit a new task to the cluster. Returns the task ID.
    pub fn submit_task(
        &self,
        action: &str,
        payload: serde_json::Value,
        original_channel: &str,
        original_chat_id: &str,
    ) -> String {
        let task = self.task_manager.create_task(
            action,
            payload,
            original_channel,
            original_chat_id,
        );
        logger::log_task("submitted", &task.id, action);
        task.id
    }

    /// Submit an async peer_chat task to a remote node.
    ///
    /// 1. Creates a local task record
    /// 2. Makes a synchronous RPC call (gets ACK)
    /// 3. Returns the task ID for later continuation
    pub fn submit_peer_chat(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
        channel: &str,
        chat_id: &str,
    ) -> Result<String, String> {
        // Extract or generate task_id
        let task_id = payload
            .get("task_id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let _task_id = if task_id.is_empty() {
            uuid::Uuid::new_v4().to_string()
        } else {
            task_id
        };

        // Create local task with peer_id
        let task = self.task_manager.create_task_with_peer(
            action,
            payload.clone(),
            channel,
            chat_id,
            peer_id,
        );

        // Note: actual RPC call would be async, handled by the caller

        Ok(task.id)
    }

    /// Get a task by ID.
    pub fn get_task(&self, task_id: &str) -> Option<Task> {
        self.task_manager.get_task(task_id)
    }

    /// Assign a task to a specific node.
    pub fn assign_task(&self, task_id: &str, node_id: &str) -> bool {
        self.task_manager.assign_task(task_id, node_id)
    }

    /// Complete a task.
    pub fn complete_task(&self, task_id: &str, result: serde_json::Value) -> bool {
        let ok = self.task_manager.complete_task(task_id, result);
        if ok {
            logger::log_task("completed", task_id, "");
        }
        ok
    }

    /// Fail a task.
    pub fn fail_task(&self, task_id: &str, error: &str) -> bool {
        let ok = self.task_manager.fail_task(task_id, error);
        if ok {
            tracing::warn!(
                task_id = task_id,
                error = error,
                "[Cluster] Task failed",
            );
            logger::log_task("failed", task_id, error);
        }
        ok
    }

    /// List all tasks.
    pub fn list_tasks(&self) -> Vec<Task> {
        self.task_manager.list_tasks()
    }

    /// Get a reference to the task manager.
    pub fn task_manager(&self) -> &Arc<TaskManager> {
        &self.task_manager
    }

    /// Get the continuation store.
    pub fn continuation_store(&self) -> &Arc<ContinuationStore> {
        &self.cont_store
    }

    /// Get the result store.
    pub fn result_store(&self) -> &Arc<TaskResultStore> {
        &self.result_store
    }

    /// Clean up a completed task.
    ///
    /// **Intentionally no-op.** Go 版本的 `CleanupTask` 也是空操作——任务完成后保留在历史记录中
    /// 用于审计和状态查询，不做删除。这不是遗漏，是设计决策。
    pub fn cleanup_task(&self, _task_id: &str) {}

    // -- RPC ------------------------------------------------------------------

    /// Make an RPC call to a peer (synchronous wrapper).
    ///
    /// Mirrors Go's `Cluster.CallWithContext(ctx, peerID, action, payload)`.
    /// Selects a peer from the registry, builds an RPC request, sends it via
    /// the RPC client, and returns the raw response bytes.
    ///
    /// Falls back to the test override if set. Returns an error if no RPC
    /// client is available or if the call fails.
    pub fn call_with_context(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
    ) -> Result<Vec<u8>, String> {
        // Testing override
        if let Some(ref f) = *self.call_with_context_fn.lock() {
            return f(peer_id, action, payload);
        }

        // Production path: use the async RPC client.
        // Since call_with_context is synchronous, we bridge to async via
        // tokio::runtime::Handle::block_on when a runtime is available.
        let rpc_client = self.rpc_client.lock().clone();
        match rpc_client {
            Some(client) => {
                let request = crate::rpc_types::RPCRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    action: crate::rpc_types::ActionType::Custom(action.to_string()),
                    payload,
                    source: self.node_id.clone(),
                    target: Some(peer_id.to_string()),
                };

                tracing::debug!(
                    peer_id = peer_id,
                    action = action,
                    request_id = %request.id,
                    "[Cluster] Initiating RPC call_with_context",
                );

                // Try to run within an existing tokio runtime
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        // We may be inside a tokio worker thread (e.g. ClusterRpcTool::execute).
                        // Use block_in_place to avoid "Cannot start a runtime from within a runtime" panic.
                        let result = tokio::task::block_in_place(|| {
                            handle.block_on(client.call_with_timeout(
                                peer_id,
                                request,
                                client.timeout(),
                            ))
                        });
                        match result {
                            Ok(response) => {
                                if let Some(ref err) = response.error {
                                    tracing::error!(
                                        peer_id = peer_id,
                                        action = action,
                                        error = %err,
                                        "[Cluster] RPC call returned error",
                                    );
                                    Err(err.clone())
                                } else {
                                    tracing::debug!(
                                        peer_id = peer_id,
                                        action = action,
                                        "[Cluster] RPC call completed successfully",
                                    );
                                    // Serialize the result to bytes (matching Go's []byte return)
                                    match &response.result {
                                        Some(val) => {
                                            serde_json::to_vec(val)
                                                .map_err(|e| format!("serialize response: {}", e))
                                        }
                                        None => Ok(Vec::new()),
                                    }
                                }
                            }
                            Err(e) => {
                                tracing::error!(
                                    peer_id = peer_id,
                                    action = action,
                                    error = %e,
                                    "[Cluster] RPC call failed",
                                );
                                Err(format!("RPC call failed: {}", e))
                            }
                        }
                    }
                    Err(_) => {
                        // No tokio runtime available (e.g. in unit tests or CLI)
                        tracing::warn!(
                            peer_id = peer_id,
                            action = action,
                            "[Cluster] RPC client not available (no tokio runtime)",
                        );
                        Err("RPC client not initialized (no tokio runtime available)".into())
                    }
                }
            }
            None => {
                tracing::error!(
                    peer_id = peer_id,
                    action = action,
                    "[Cluster] RPC client not initialized",
                );
                Err("RPC client not initialized".into())
            }
        }
    }

    /// Make an async RPC call to a peer.
    ///
    /// This is the async counterpart to `call_with_context`, suitable for use
    /// from async contexts. Mirrors Go's `Cluster.CallWithContext` which
    /// natively supports context-based cancellation.
    pub async fn call_with_context_async(
        &self,
        peer_id: &str,
        action: &str,
        payload: serde_json::Value,
        timeout: Duration,
    ) -> Result<Vec<u8>, String> {
        // Testing override
        if let Some(ref f) = *self.call_with_context_fn.lock() {
            return f(peer_id, action, payload);
        }

        let rpc_client = self.rpc_client.lock().clone();
        match rpc_client {
            Some(client) => {
                let request = crate::rpc_types::RPCRequest {
                    id: uuid::Uuid::new_v4().to_string(),
                    action: crate::rpc_types::ActionType::Custom(action.to_string()),
                    payload,
                    source: self.node_id.clone(),
                    target: Some(peer_id.to_string()),
                };

                tracing::debug!(
                    peer_id = peer_id,
                    action = action,
                    timeout_secs = timeout.as_secs(),
                    request_id = %request.id,
                    "[Cluster] Initiating async RPC call",
                );

                match client.call_with_timeout(peer_id, request, timeout).await {
                    Ok(response) => {
                        if let Some(ref err) = response.error {
                            tracing::error!(
                                peer_id = peer_id,
                                action = action,
                                error = %err,
                                "[Cluster] Async RPC call returned error",
                            );
                            Err(err.clone())
                        } else {
                            tracing::debug!(
                                peer_id = peer_id,
                                action = action,
                                "[Cluster] Async RPC call completed successfully",
                            );
                            match &response.result {
                                Some(val) => {
                                    serde_json::to_vec(val)
                                        .map_err(|e| format!("serialize response: {}", e))
                                }
                                None => Ok(Vec::new()),
                            }
                        }
                    }
                    Err(e) => {
                        tracing::error!(
                            peer_id = peer_id,
                            action = action,
                            error = %e,
                            "[Cluster] Async RPC call failed",
                        );
                        Err(format!("RPC call failed: {}", e))
                    }
                }
            }
            None => {
                tracing::error!(
                    peer_id = peer_id,
                    action = action,
                    "[Cluster] Async RPC client not initialized",
                );
                Err("RPC client not initialized".into())
            }
        }
    }

    /// Set the testing override for call_with_context.
    pub fn set_call_with_context_fn(
        &self,
        f: Box<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    ) {
        *self.call_with_context_fn.lock() = Some(Arc::from(f));
    }


    // -- Bus integration ------------------------------------------------------

    /// Set the cluster agent task list and work queue for callback routing.
    pub fn set_cluster_task_queue(
        &self,
        task_list: Arc<crate::cluster_task::ClusterTaskList>,
        work_queue: Arc<crate::cluster_task::ClusterWorkQueue>,
    ) {
        *self.cluster_task_list.lock() = Some(task_list);
        *self.cluster_work_queue.lock() = Some(work_queue);
    }

    /// Set the message bus (called by AgentLoop during setup).
    pub fn set_message_bus(&self, bus: Arc<dyn MessageBus>) {
        *self.bus.lock() = Some(bus);
    }

    /// Handle task completion (callback from TaskManager).
    /// Publishes a continuation message to the bus.
    pub fn handle_task_complete(&self, task_id: &str) {
        let task = match self.task_manager.get_task(task_id) {
            Some(t) => t,
            None => return,
        };

        if task.original_channel.is_empty() {
            return;
        }

        let bus = match self.bus.lock().as_ref() {
            Some(b) => b.clone(),
            None => {
                logger::log_error(
                    "cluster",
                    "bus not set",
                    &format!("task {} completed but bus not available", task_id),
                );
                return;
            }
        };

        bus.publish_inbound(BusInboundMessage {
            channel: "system".into(),
            sender_id: format!("cluster_continuation:{}", task_id),
            chat_id: format!("{}:{}", task.original_channel, task.original_chat_id),
            content: String::new(),
        });

        logger::log_task("completed", task_id, &task.action);
    }

    // -- Accessors ------------------------------------------------------------

    /// Get the cluster configuration.
    pub fn config(&self) -> &ClusterConfig {
        // This is a static reference; in practice we'd store it
        static DEFAULT: std::sync::OnceLock<ClusterConfig> = std::sync::OnceLock::new();
        DEFAULT.get_or_init(|| ClusterConfig {
            node_id: String::new(),
            bind_address: "0.0.0.0:9000".into(),
            peers: Vec::new(),
        })
    }

    /// Get the node ID.
    pub fn node_id(&self) -> &str {
        &self.node_id
    }

    /// Get the node name.
    pub fn node_name(&self) -> String {
        self.node_name.read().clone()
    }

    /// Get the address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the role.
    pub fn role(&self) -> String {
        self.role.read().clone()
    }

    /// Get the category.
    pub fn category(&self) -> String {
        self.category.read().clone()
    }

    /// Get the tags.
    pub fn tags(&self) -> Vec<String> {
        self.tags.read().clone()
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &PathBuf {
        &self.workspace
    }

    /// Get the RPC port.
    pub fn rpc_port(&self) -> u16 {
        self.rpc_port
    }

    /// Get the UDP port.
    pub fn udp_port(&self) -> u16 {
        self.udp_port
    }

    /// Get the capabilities of all online nodes.
    pub fn get_capabilities(&self) -> Vec<String> {
        let mut caps: Vec<String> = self
            .registry
            .list_online()
            .iter()
            .flat_map(|n| n.capabilities.clone())
            .collect();
        caps.sort();
        caps.dedup();
        caps
    }

    /// Get all local IPs.
    pub fn get_all_local_ips(&self) -> Vec<String> {
        network::get_all_local_ips()
    }

    /// Get a peer by ID.
    pub fn get_peer(&self, peer_id: &str) -> Option<ExtendedNodeInfo> {
        self.registry.get(peer_id)
    }

    /// Temporarily mark a peer as Online so the RPC resolver doesn't block
    /// the call. Used by `nodes.refresh` to bypass the offline-check before
    /// attempting a `get_info` RPC; the caller should restore the original
    /// status if the call fails.
    pub fn mark_peer_online_for_refresh(&self, peer_id: &str) {
        if let Some(mut info) = self.registry.get(peer_id) {
            info.status = NodeStatus::Online;
            self.registry.upsert(info);
        }
    }

    /// Set a peer's status (used to restore state after a refresh attempt).
    pub fn set_peer_status(&self, peer_id: &str, status: NodeStatus) {
        if let Some(mut info) = self.registry.get(peer_id) {
            info.status = status;
            self.registry.upsert(info);
        }
    }

    /// Record a successful connectivity probe (e.g. `nodes_ping` TCP connect
    /// succeeded). Refreshes `last_health_check` to now and ensures the peer
    /// is marked Online. Use this whenever an external reachability check
    /// confirms the peer is alive — independent of the UDP discovery loop.
    pub fn mark_peer_healthy(&self, node_id: &str) {
        self.registry.mark_healthy(node_id);
    }

    /// Record a failed connectivity probe (e.g. `nodes_ping` timed out or
    /// TCP RST). Immediately flips the peer to Offline — does NOT go through
    /// the `consecutive_failures` accumulator because the user explicitly
    /// probed and observed failure. Also emits a discovery log entry so the
    /// event surfaces in cluster_{date}.log.
    pub fn mark_peer_offline(&self, node_id: &str, reason: &str) {
        self.registry.mark_offline(node_id, reason);
        crate::logger::log_discovery("offline", reason, Some(node_id));
    }

    /// Get online peers.
    pub fn get_online_peers(&self) -> Vec<ExtendedNodeInfo> {
        self.registry.list_online()
    }

    /// Get online peers excluding the local node.
    ///
    /// Use this when building "RPC target candidates" lists for tools (cluster_rpc)
    /// to prevent the LLM from selecting the local node as its target. Self-invocation
    /// creates nested child tasks that loop back to the same node.
    pub fn get_online_peers_excluding_self(&self) -> Vec<ExtendedNodeInfo> {
        self.registry.list_online_excluding(&self.node_id)
    }

    /// Set ports.
    pub fn set_ports(&mut self, udp: u16, rpc: u16) {
        self.udp_port = udp;
        self.rpc_port = rpc;
    }

    /// Set broadcast interval (UDP announce + sync_loop tick rate).
    /// If not called, defaults to DEFAULT_BROADCAST_INTERVAL (30s).
    /// Must be called BEFORE `start()` for the value to take effect.
    pub fn set_broadcast_interval(&mut self, interval: std::time::Duration) {
        self.broadcast_interval = interval;
    }

    /// Set the human-readable node name (e.g. "Node-A").
    /// Called from gateway.rs after loading the name from config.cluster.json,
    /// or from Dashboard via node.update_identity command.
    pub fn set_node_name(&self, name: impl Into<String>) {
        *self.node_name.write() = name.into();
        self.sync_local_node_to_registry();
    }

    /// Get the node type ("agent" or "node").
    pub fn node_type(&self) -> &str {
        &self.node_type
    }

    /// Set the node type: "agent" (full with LLM) or "node" (lightweight).
    pub fn set_node_type(&mut self, node_type: impl Into<String>) {
        self.node_type = node_type.into();
    }

    /// Set the node role ("master" or "worker").
    pub fn set_role(&self, role: impl Into<String>) {
        *self.role.write() = role.into();
        self.sync_local_node_to_registry();
    }

    /// Set the node category (e.g. "general", "development").
    pub fn set_category(&self, category: impl Into<String>) {
        *self.category.write() = category.into();
        self.sync_local_node_to_registry();
    }

    /// Set the node tags.
    pub fn set_tags(&self, tags: Vec<String>) {
        *self.tags.write() = tags;
        self.sync_local_node_to_registry();
    }

    /// Set the dynamic capabilities for this node (tool names from AgentLoop).
    ///
    /// Called after the AgentLoop is built so the discovery broadcast includes
    /// the actual tool set rather than a hardcoded empty list.
    pub fn set_capabilities(&self, caps: Vec<String>) {
        if let Ok(mut guard) = self.capabilities.lock() {
            *guard = caps;
        }
        self.sync_local_node_to_registry();
    }

    /// Get the stop channel receiver.
    pub fn stop_receiver(&self) -> broadcast::Receiver<()> {
        self.stop_tx.subscribe()
    }

    /// Rebuild the local node's registry entry from current identity fields
    /// and upsert it into the registry so list_nodes() reflects live state.
    fn sync_local_node_to_registry(&self) {
        // Preserve the resolved address from the existing registry entry,
        // since self.address may still be 0.0.0.0 before start() resolves it.
        let existing_address = self.registry.get(&self.node_id)
            .map(|e| e.base.address.clone())
            .unwrap_or_else(|| self.address.clone());

        let role_str = self.role.read().clone();
        let role = match role_str.as_str() {
            "master" | "manager" => nemesis_types::cluster::NodeRole::Master,
            _ => nemesis_types::cluster::NodeRole::Worker,
        };
        let caps = self.capabilities.lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let info = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: self.node_id.clone(),
                name: self.node_name.read().clone(),
                role,
                address: existing_address,
                category: self.category.read().clone(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: caps,
            addresses: vec![],
            node_type: self.node_type.clone(),
        };
        self.registry.upsert(info);
    }

    /// Sync state to disk.
    pub fn sync_to_disk(&self) -> Result<(), ConfigError> {
        let nodes = self.registry.list_peers();
        let mut discovered = Vec::new();
        for node in &nodes {
            if node.base.id == self.node_id {
                continue;
            }
            discovered.push(PeerConfig {
                id: node.base.id.clone(),
                name: node.base.name.clone(),
                address: node.base.address.clone(),
                addresses: Vec::new(),
                rpc_port: 0,
                role: String::new(),
                category: node.base.category.clone(),
                priority: 1,
                enabled: true,
                status: PeerStatus {
                    state: match node.status {
                        NodeStatus::Online => "online".into(),
                        NodeStatus::Offline => "offline".into(),
                        _ => "unknown".into(),
                    },
                    last_seen: node.base.last_seen.clone(),
                    uptime: String::new(),
                    tasks_completed: 0,
                    success_rate: 0.0,
                    avg_response_time: 0,
                    last_error: String::new(),
                },
            });
        }

        let state = DynamicState {
            discovered,
            last_sync: chrono::Local::now().to_rfc3339(),
        };

        crate::cluster_config::save_dynamic_state(&self.dynamic_state_path, &state)
            .map_err(|e| {
                tracing::error!(
                    path = %self.dynamic_state_path.display(),
                    error = %e,
                    "[Cluster] Failed to sync state to disk",
                );
                e
            })
    }

    // -- Peer capability search ------------------------------------------------

    /// Find all online peers that have a specific capability.
    ///
    /// Mirrors Go's `FindPeersByCapability(capability string) []*Node`.
    pub fn find_peers_by_capability(&self, capability: &str) -> Vec<ExtendedNodeInfo> {
        self.registry.find_by_capability(capability)
    }

    // -- RPC channel management -----------------------------------------------

    /// Get the RPC channel (may be None if not configured).
    ///
    /// Mirrors Go's `GetRPCChannel() *channels.RPCChannel`.
    pub fn get_rpc_channel(&self) -> Option<Arc<dyn crate::rpc::RpcChannel>> {
        self.rpc_channel.read().clone()
    }

    /// Set the RPC channel and trigger LLM handler registration.
    ///
    /// Called by the agent loop after creating the RPCChannel.
    /// Thread-safety: acquires write lock to set the channel, then releases
    /// before calling `register_peer_chat_handlers()` to avoid deadlock
    /// (register_peer_chatHandlers -> register_rpc_handler -> read lock).
    ///
    /// Mirrors Go's `SetRPCChannel(rpcCh *channels.RPCChannel)`.
    pub fn set_rpc_channel(&self, channel: Arc<dyn crate::rpc::RpcChannel>) {
        *self.rpc_channel.write() = Some(channel);

        // Register peer chat handlers if server is running
        if self.is_running() && self.rpc_server.is_some() {
            self.register_peer_chat_handlers();
        }
    }

    // -- RPC handler registration ---------------------------------------------

    /// Register an RPC handler for a specific action.
    ///
    /// Returns an error if the cluster is not running or the RPC server is not
    /// initialized.
    ///
    /// Mirrors Go's `RegisterRPCHandler(action, handler) error`.
    pub fn register_rpc_handler(
        &self,
        action: &str,
        handler: crate::rpc::server::RpcHandlerFn,
    ) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster is not running".into());
        }
        let server = match self.rpc_server.as_ref() {
            Some(s) => s,
            None => return Err("RPC server is not initialized".into()),
        };
        server.register_handler(action, handler);
        logger::log_rpc("register_handler", action, "", "", None);
        Ok(())
    }

    /// Register peer chat related handlers when RPCChannel is ready.
    ///
    /// This must be called after both RPC Server and RPC Channel are initialized.
    /// Registers: peer_chat, peer_chat_callback, query_task_result,
    /// confirm_task_delivery, hello, and other custom handlers.
    ///
    /// Mirrors Go's `registerPeerChatHandlers()`.
    pub fn register_peer_chat_handlers(&self) {
        let rpc_channel = self.rpc_channel.read();
        if rpc_channel.is_none() {
            logger::log_rpc(
                "register_peer_chat_handlers",
                "",
                "RPCChannel not ready",
                "",
                None,
            );
            return;
        }
        drop(rpc_channel);

        // Register peer_chat handler (B-side: receive message, ACK, process async)
        if let Err(e) = self.register_rpc_handler("peer_chat", self.build_peer_chat_handler()) {
            logger::log_error("cluster", &format!("register peer_chat: {}", e), "");
        }

        // Register peer_chat_callback handler (A-side: receive result from B)
        if let Err(e) = self.register_rpc_handler(
            "peer_chat_callback",
            self.build_callback_handler(),
        ) {
            logger::log_error("cluster", &format!("register peer_chat_callback: {}", e), "");
        }

        // Register hello handler
        let node_id = self.node_id.clone();
        if let Err(e) = self.register_rpc_handler("hello", Box::new(move |_payload| {
            Ok(serde_json::json!({
                "node_id": node_id,
                "status": "online",
                "message": "hello from cluster node",
            }))
        })) {
            logger::log_error("cluster", &format!("register hello: {}", e), "");
        }

        // H4: Register query_task_result handler (B-side responds to A's polling)
        if let Err(e) = self.register_rpc_handler(
            "query_task_result",
            self.build_query_task_result_handler(),
        ) {
            logger::log_error("cluster", &format!("register query_task_result: {}", e), "");
        }

        // H4: Register confirm_task_delivery handler
        if let Err(e) = self.register_rpc_handler(
            "confirm_task_delivery",
            self.build_confirm_task_delivery_handler(),
        ) {
            logger::log_error("cluster", &format!("register confirm_task_delivery: {}", e), "");
        }
    }

    /// Register basic RPC handlers (ping, info, etc.).
    ///
    /// This can be called directly in daemon mode where RPCChannel is not
    /// available. Registers: ping, get_capabilities, get_info, list_actions,
    /// hello, and other default handlers.
    ///
    /// Mirrors Go's `RegisterBasicHandlers() error`.
    pub fn register_basic_handlers(&self) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster not running".into());
        }

        // ping
        let node_id = self.node_id.clone();
        self.register_rpc_handler("ping", Box::new(move |_payload| {
            Ok(serde_json::json!({
                "status": "pong",
                "node_id": node_id,
            }))
        }))?;

        // get_capabilities — shares Arc with Cluster for real-time reads
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler("get_capabilities", Box::new(move |_payload| {
            let caps = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Ok(serde_json::json!({
                "capabilities": caps,
            }))
        }))?;

        // get_info — returns data matching DiscoveryMessage broadcast format
        // Static fields (cloned, immutable after startup):
        let node_id = self.node_id.clone();
        let node_name = self.node_name.read().clone();
        let role = self.role.read().clone();
        let category = self.category.read().clone();
        let tags = self.tags.read().clone();
        let node_type = self.node_type.clone();
        let rpc_port = self.rpc_port;
        // Dynamic fields (real-time):
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler("get_info", Box::new(move |_payload| {
            let addresses = network::get_all_local_ips();
            let capabilities = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
            Ok(serde_json::json!({
                "version": "1.0",
                "node_id": node_id,
                "name": node_name,
                "addresses": addresses,
                "rpc_port": rpc_port,
                "role": role,
                "category": category,
                "tags": tags,
                "capabilities": capabilities,
                "node_type": node_type,
                "status": "online",
            }))
        }))?;

        // list_actions
        let node_id = self.node_id.clone();
        self.register_rpc_handler("list_actions", Box::new(move |_payload| {
            let schemas = crate::actions_schema::builtin_schemas();
            let actions: Vec<String> = schemas.iter().map(|s| s.action.to_string()).collect();
            Ok(serde_json::json!({
                "node_id": node_id,
                "actions": actions,
            }))
        }))?;

        // hello
        let node_id = self.node_id.clone();
        self.register_rpc_handler("hello", Box::new(move |_payload| {
            Ok(serde_json::json!({
                "node_id": node_id,
                "status": "online",
                "message": "hello from cluster node",
            }))
        }))?;

        // diagnostics.system — OS, memory, uptime
        self.register_rpc_handler("diagnostics.system", Box::new(move |_payload| {
            let os = std::env::consts::OS.to_string();
            let arch = std::env::consts::ARCH.to_string();
            let hostname = crate::diagnostics::get_hostname();
            let (mem_total, mem_used, uptime_secs) = crate::diagnostics::collect_system_metrics();
            let os_version = crate::diagnostics::collect_os_version();
            Ok(serde_json::json!({
                "os": os, "os_version": os_version, "arch": arch,
                "hostname": hostname, "uptime_secs": uptime_secs,
                "memory_total_bytes": mem_total, "memory_used_bytes": mem_used,
            }))
        }))?;

        // diagnostics.network — network interfaces and IPs
        self.register_rpc_handler("diagnostics.network", Box::new(move |_payload| {
            let interfaces = network::get_local_network_interfaces();
            let all_ips = network::get_all_local_ips();
            Ok(serde_json::json!({
                "interfaces": interfaces,
                "all_ips": all_ips,
            }))
        }))?;

        // diagnostics.cluster_state — peers this node sees
        let registry_arc = self.registry.clone();
        self.register_rpc_handler("diagnostics.cluster_state", Box::new(move |_payload| {
            let all_nodes = registry_arc.list_peers();
            let online: Vec<_> = all_nodes.iter()
                .filter(|n| n.is_online())
                .map(|n| serde_json::json!({
                    "id": n.base.id,
                    "name": n.base.name,
                    "address": n.base.address,
                    "role": n.base.role,
                    "last_seen": n.base.last_seen,
                }))
                .collect();
            Ok(serde_json::json!({
                "node_count": all_nodes.len(),
                "online_count": online.len(),
                "nodes": online,
            }))
        }))?;

        Ok(())
    }


    /// Register forge-related RPC handlers for cross-node learning.
    ///
    /// This must be called when forge is enabled and the cluster is running.
    /// Registers: forge_share, forge_get_reflections.
    ///
    /// Mirrors Go's `RegisterForgeHandlers()` which is called from bot_service
    /// after Forge is initialized.
    pub fn register_forge_handlers(
        &self,
        provider: Box<dyn crate::handlers::ForgeDataProvider>,
    ) -> Result<(), String> {
        if !self.is_running() {
            return Err("cluster not running".into());
        }

        let node_id = self.node_id.clone();

        // forge_share: receive a remote reflection report
        let provider_share = provider.clone_boxed();
        let node_id_share = node_id.clone();
        self.register_rpc_handler("forge_share", Box::new(move |payload| {
            let from = payload
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            tracing::info!(
                source_node = from,
                local_node = %node_id_share,
                "[Cluster] Received forge reflection report from peer"
            );

            if let Err(e) = provider_share.receive_reflection(&payload) {
                tracing::error!(error = %e, "[Cluster] Failed to store reflection");
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": format!("Failed to store reflection: {}", e),
                }));
            }

            Ok(serde_json::json!({
                "status": "ok",
                "message": "Reflection received",
                "node_id": node_id_share,
                "timestamp": chrono::Local::now().to_rfc3339(),
            }))
        }))?;

        // forge_get_reflections: list available local reflections
        let provider_list = provider.clone_boxed();
        let node_id_list = node_id.clone();
        self.register_rpc_handler("forge_get_reflections", Box::new(move |payload| {
            let from = payload
                .get("from")
                .and_then(|v| v.as_str())
                .unwrap_or("unknown");

            tracing::info!(
                from = from,
                local_node = %node_id_list,
                "[Cluster] Reflections list requested by peer"
            );

            let mut result = provider_list.get_reflections_list_payload();

            // If a specific reflection is requested, include its content (sanitized)
            if let Some(filename) = payload.get("filename").and_then(|v| v.as_str()) {
                if !filename.is_empty() {
                    match provider_list.read_reflection_content(filename) {
                        Ok(content) => {
                            result["content"] =
                                serde_json::Value::String(provider_list.sanitize_content(&content));
                            result["filename"] = serde_json::Value::String(filename.into());
                        }
                        Err(e) => {
                            tracing::error!(
                                filename = filename,
                                error = %e,
                                "[Cluster] Failed to read reflection"
                            );
                            return Ok(serde_json::json!({
                                "status": "error",
                                "error": format!("Failed to read reflection: {}", e),
                            }));
                        }
                    }
                }
            }

            result["node_id"] = serde_json::Value::String(node_id_list.clone());

            Ok(result)
        }))?;

        tracing::info!("[Cluster] Registered forge RPC handlers: forge_share, forge_get_reflections");
        Ok(())
    }

    // -- RPC handler builders (extracted for testability) ----------------------

    /// Build the peer_chat handler (B-side: receive message, ACK, process async).
    ///
    /// **⚠️ 这是 ACK 桩，生产环境会被覆盖，不要在这里加业务逻辑。**
    ///
    /// 注册链路（后注册者覆盖先注册者）：
    ///   1. `register_default_handlers()`（rpc/server.rs:494）注册最初的 ACK 桩
    ///   2. `register_peer_chat_handlers()`（cluster.rs:1630）调用本函数注册此桩，覆盖 #1
    ///   3. **gateway.rs:1189 用真正的 PeerChatHandler 覆盖此桩**（生产路径）
    ///
    /// 真 handler（gateway.rs:1189）做的事情：从 `payload._rpc.from` 提取 source_node_id、
    /// 通过 RpcMeta 传给 PeerChatHandler（PeerChatHandler 从 rpc_meta.from 取 source_node_id、
    /// 从 `_source.chat_id` 取 chat_id，组合成 session_key 用于 LLM 会话隔离）、
    /// 自动注册未知节点到 registry、调用 PeerChatHandler 入队 ClusterTaskList 异步处理、
    /// callback 通过 peer_chat_callback 回 A 端。
    ///
    /// 此桩仅在非 gateway 场景（如独立 cluster daemon、轻量节点）下生效。
    /// **修改 peer_chat 行为的正确位置是 gateway.rs:1189，不是这里。**
    fn build_peer_chat_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let node_id = self.node_id.clone();
        Box::new(move |payload| {
            let content = payload
                .get("content")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if content.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "content is required",
                }));
            }
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("")
                .to_string();

            tracing::info!(
                node_id = %node_id,
                task_id = %task_id,
                "[Cluster] peer_chat received, returning ACK"
            );

            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        })
    }

    /// Build the callback handler (A-side: receive result from B).
    fn build_callback_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let task_manager = self.task_manager.clone();
        let cluster_task_list = self.cluster_task_list.lock().clone();
        let cluster_work_queue = self.cluster_work_queue.lock().clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            let status = payload
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("success");
            let response = payload
                .get("response")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            let error = payload
                .get("error")
                .and_then(|v| v.as_str())
                .unwrap_or("");

            tracing::info!(
                task_id = %task_id,
                status = %status,
                "[Cluster] peer_chat_callback received"
            );

            // Check if this callback is for a cluster agent task (B-side forwarding).
            if let (Some(tl), Some(wq)) = (&cluster_task_list, &cluster_work_queue) {
                if let Some(parent_id) = tl.find_by_child_task_id(task_id) {
                    tracing::info!(
                        child_task_id = %task_id,
                        parent_task_id = %parent_id,
                        "[Cluster] Routing callback to cluster agent task"
                    );
                    tl.inject_callback(&parent_id, response);
                    if let Err(e) = wq.submit(parent_id) {
                        tracing::error!(error = %e, "[Cluster] Failed to re-submit task to work queue");
                    }
                    return Ok(serde_json::json!({
                        "status": "accepted",
                        "task_id": task_id,
                    }));
                }
            }

            // Fall through to main agent's TaskManager (A-side continuation).
            task_manager.complete_callback(task_id, status, response, error);

            Ok(serde_json::json!({
                "status": "accepted",
                "task_id": task_id,
            }))
        })
    }

    /// Build the query_task_result handler (B-side responds to A's polling).
    ///
    /// Mirrors Go's `buildQueryTaskResultHandler()`.
    fn build_query_task_result_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let result_store = self.result_store.clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            match result_store.get(task_id) {
                Some(entry) => {
                    let result_status = if entry.success { "success" } else { "error" };
                    let response = entry
                        .result
                        .get("response")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let error = entry
                        .result
                        .get("error")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    Ok(serde_json::json!({
                        "status": "done",
                        "task_id": task_id,
                        "result_status": result_status,
                        "response": response,
                        "error": error,
                    }))
                }
                None => Ok(serde_json::json!({
                    "status": "not_found",
                    "task_id": task_id,
                })),
            }
        })
    }

    /// Build the confirm_task_delivery handler (A confirms it received result).
    ///
    /// Mirrors Go's `buildConfirmTaskDeliveryHandler()`.
    fn build_confirm_task_delivery_handler(&self) -> crate::rpc::server::RpcHandlerFn {
        let result_store = self.result_store.clone();
        Box::new(move |payload| {
            let task_id = payload
                .get("task_id")
                .and_then(|v| v.as_str())
                .unwrap_or("");
            if task_id.is_empty() {
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": "task_id is required",
                }));
            }

            result_store.remove(task_id);

            Ok(serde_json::json!({
                "status": "confirmed",
                "task_id": task_id,
            }))
        })
    }

    /// Poll stale pending tasks and recover results from B-nodes.
    ///
    /// Queries tasks that have been pending for more than 2 minutes. If the
    /// remote B-node reports a completed result, completes the task locally.
    /// If the task is not found on the B-node, fails it locally. Tasks older
    /// than 24 hours are timed out as a safety net.
    ///
    /// Mirrors Go's `Cluster.pollStalePendingTasks()`.
    pub async fn poll_stale_pending_tasks(&self) {
        let call_fn = self.call_with_context_fn.lock().clone();
        let rpc_client = self.rpc_client.lock().clone();
        poll_stale_pending_tasks(
            &self.task_manager,
            &call_fn,
            rpc_client.as_deref(),
        )
        .await;
    }

    /// Confirm task delivery to the B-node, allowing it to clean up the result.
    ///
    /// Mirrors Go's `Cluster.confirmDelivery(peerID, taskID)`.
    pub fn confirm_delivery(&self, peer_id: &str, task_id: &str) {
        let payload = serde_json::json!({"task_id": task_id});
        let call_fn = self.call_with_context_fn.lock();
        if let Some(f) = call_fn.as_ref() {
            let _ = f(peer_id, "confirm_task_delivery", payload);
        } else {
            // Fallback: try through call_with_context
            let _ = self.call_with_context(
                peer_id,
                "confirm_task_delivery",
                serde_json::json!({"task_id": task_id}),
            );
        }
    }

    /// Set the RPC client for the cluster.
    pub fn set_rpc_client(&self, client: Arc<RpcClient>) {
        tracing::info!(
            timeout_secs = client.timeout().as_secs(),
            "[Cluster] RPC client set externally",
        );
        *self.rpc_client.lock() = Some(client);
    }

    /// Get a cloned reference to the RPC client (if initialized).
    pub fn rpc_client_arc(&self) -> Option<Arc<RpcClient>> {
        self.rpc_client.lock().clone()
    }

    // -- RPC server accessor ---------------------------------------------------

    /// Set the RPC server instance.
    pub fn set_rpc_server(&mut self, server: Arc<crate::rpc::server::RpcServer>) {
        self.rpc_server = Some(server);
    }

    /// Get a reference to the RPC server (if initialized).
    pub fn rpc_server(&self) -> Option<&Arc<crate::rpc::server::RpcServer>> {
        self.rpc_server.as_ref()
    }

    /// Set the task manager for testing (allows injecting a custom TaskManager).
    pub fn set_task_manager_for_test(&mut self, tm: Arc<TaskManager>) {
        self.task_manager = tm;
    }

    // -- Get actions schema ----------------------------------------------------

    /// Get the actions schema for RPC actions (used by list_actions handler).
    pub fn get_actions_schema(&self) -> Vec<crate::actions_schema::ActionSchema> {
        crate::actions_schema::builtin_schemas()
    }

    /// Get the actions schema as a formatted JSON string.
    ///
    /// Mirrors Go's `Cluster.GetActionsSchemaJSON()`. Serializes the actions
    /// schema to pretty-printed JSON for use in RPC responses and debugging.
    pub fn get_actions_schema_json(&self) -> Result<String, serde_json::Error> {
        let schema = self.get_actions_schema();
        serde_json::to_string_pretty(&schema)
    }

    // -- Test helpers ---------------------------------------------------------

    /// Expose handle_task_complete to tests.
    pub fn handle_task_complete_for_test(&self, task_id: &str) {
        self.handle_task_complete(task_id);
    }
}

// ---------------------------------------------------------------------------
// Recovery loop free functions
// ---------------------------------------------------------------------------

/// Poll stale pending tasks: query the B-node for any task that has been
/// pending for longer than 2 minutes. If the B-node reports it done, complete
/// the task locally; if not found, fail it; if older than 24 h, time it out.
///
/// Uses the real RPC client when available, falling back to the synchronous
/// test override (`call_fn`).  This matches Go's `pollStalePendingTasks`
/// which calls `c.CallWithContext()`.
async fn poll_stale_pending_tasks(
    task_manager: &Arc<TaskManager>,
    call_fn: &Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>>,
    rpc_client: Option<&RpcClient>,
) {
    let tasks = task_manager.list_pending_tasks();

    if !tasks.is_empty() {
        tracing::debug!(
            count = tasks.len(),
            "[Cluster] Polling stale pending tasks",
        );
    }

    for task in tasks {
        // Parse created_at (RFC 3339) and compute age.
        let created = match chrono::DateTime::parse_from_rfc3339(&task.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Local),
            Err(_) => continue,
        };
        let age = chrono::Local::now() - created;

        // Skip tasks younger than 2 minutes.
        if age < chrono::Duration::minutes(2) {
            continue;
        }

        // Timeout tasks older than 24 hours.
        if age > chrono::Duration::hours(24) {
            tracing::warn!(
                task_id = %task.id,
                age_secs = age.num_seconds(),
                "[Cluster] Timing out stale task after 24h",
            );
            task_manager.complete_callback(
                &task.id,
                "error",
                "",
                "task timed out after 24h",
            );
            continue;
        }

        // Need a peer_id to query.
        if task.peer_id.is_empty() {
            continue;
        }

        // Query the remote peer for the task result.
        let payload = serde_json::json!({"task_id": task.id});

        // Prefer real RPC client (matching Go's c.CallWithContext), fall back
        // to synchronous test override.
        let result = if let Some(client) = rpc_client {
            let request = crate::rpc_types::RPCRequest {
                id: uuid::Uuid::new_v4().to_string(),
                action: crate::rpc_types::ActionType::Custom("query_task_result".to_string()),
                payload: payload.clone(),
                source: String::new(),
                target: Some(task.peer_id.clone()),
            };
            match client
                .call_with_timeout(
                    &task.peer_id,
                    request,
                    Duration::from_secs(30),
                )
                .await
            {
                Ok(resp) => {
                    if let Some(ref err) = resp.error {
                        tracing::warn!(
                            task_id = %task.id,
                            error = %err,
                            "[Cluster] query_task_result returned error"
                        );
                        continue;
                    }
                    match resp.result {
                        Some(val) => val.to_string().into_bytes(),
                        None => continue,
                    }
                }
                Err(_) => continue,
            }
        } else if let Some(call) = call_fn {
            match call(&task.peer_id, "query_task_result", payload) {
                Ok(data) => data,
                Err(_) => continue,
            }
        } else {
            // No client available, skip.
            continue;
        };

        let resp: serde_json::Value = match serde_json::from_slice(&result) {
            Ok(v) => v,
            Err(_) => continue,
        };

        let status = resp
            .get("status")
            .and_then(|v| v.as_str())
            .unwrap_or("");

        match status {
            "running" => {
                // Still running, nothing to do.
                continue;
            }
            "done" => {
                let result_status = string_value(resp.get("result_status"));
                let response = string_value(resp.get("response"));
                let error = string_value(resp.get("error"));
                tracing::info!(
                    task_id = %task.id,
                    result_status = %result_status,
                    peer_id = %task.peer_id,
                    "[Cluster] Stale task recovered from peer",
                );
                task_manager.complete_callback(
                    &task.id,
                    &result_status,
                    &response,
                    &error,
                );
                // Best-effort delivery confirmation
                confirm_delivery_with(
                    call_fn,
                    rpc_client,
                    &task.peer_id,
                    &task.id,
                )
                .await;
            }
            "not_found" => {
                tracing::warn!(
                    task_id = %task.id,
                    peer_id = %task.peer_id,
                    "[Cluster] Stale task not found on remote peer",
                );
                task_manager.complete_callback(
                    &task.id,
                    "error",
                    "",
                    "remote task not found",
                );
            }
            _ => {
                // Unknown status, skip.
                continue;
            }
        }
    }
}

/// Notify the B-node that the task result was received.
/// Uses the RPC client if available, otherwise the synchronous test override.
async fn confirm_delivery_with(
    call_fn: &Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>>,
    rpc_client: Option<&RpcClient>,
    peer_id: &str,
    task_id: &str,
) {
    let payload = serde_json::json!({"task_id": task_id});

    if let Some(client) = rpc_client {
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Custom("confirm_task_delivery".to_string()),
            payload,
            source: String::new(),
            target: Some(peer_id.to_string()),
        };
        let _ = client
            .call_with_timeout(peer_id, request, Duration::from_secs(30))
            .await;
    } else if let Some(call) = call_fn {
        let _ = call(peer_id, "confirm_task_delivery", payload);
    }
}

/// Extract a string value from a JSON Value, returning "" for null / missing.
fn string_value(v: Option<&serde_json::Value>) -> String {
    match v {
        Some(serde_json::Value::String(s)) => s.clone(),
        Some(serde_json::Value::Null) => String::new(),
        Some(other) => {
            // For non-string scalars, return the JSON representation without quotes.
            if other.is_number() || other.is_boolean() {
                other.to_string()
            } else {
                other.as_str().unwrap_or("").to_string()
            }
        }
        None => String::new(),
    }
}

// ---------------------------------------------------------------------------
// ClusterCallbacks implementation (for discovery service integration)
// ---------------------------------------------------------------------------

impl ClusterCallbacks for Cluster {
    fn node_id(&self) -> String {
        self.node_id.clone()
    }

    fn name(&self) -> String {
        self.node_name.read().clone()
    }

    fn address(&self) -> String {
        self.address.clone()
    }

    fn rpc_port(&self) -> u16 {
        self.rpc_port
    }

    fn all_local_ips(&self) -> Vec<String> {
        network::get_all_local_ips()
    }

    fn role(&self) -> String {
        self.role.read().clone()
    }

    fn category(&self) -> String {
        self.category.read().clone()
    }

    fn tags(&self) -> Vec<String> {
        self.tags.read().clone()
    }

    fn capabilities(&self) -> Vec<String> {
        self.capabilities.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    fn node_type(&self) -> String {
        self.node_type.clone()
    }

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
        node_type: &str,
    ) -> bool {
        self.handle_discovered_node(
            node_id,
            name,
            addresses.to_vec(),
            rpc_port,
            role,
            category,
            tags.to_vec(),
            capabilities.to_vec(),
            node_type,
        )
    }

    fn handle_node_offline(&self, node_id: &str, reason: &str) {
        self.handle_node_offline(node_id, reason);
    }

    fn sync_to_disk(&self) -> Result<(), String> {
        self.sync_to_disk().map_err(|e| e.to_string())
    }
}

// ---------------------------------------------------------------------------
// PeerResolver implementation
// ---------------------------------------------------------------------------

/// Adapts the Cluster's registry to the `PeerResolver` trait needed by `RpcClient`.
struct ClusterPeerResolver {
    registry: Arc<PeerRegistry>,
    node_id: String,
}

impl PeerResolver for ClusterPeerResolver {
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        // 1. Direct lookup by key (e.g. "Node-A" or a node_id)
        if let Some(info) = self.registry.get(peer_id) {
            let is_online = info.status == NodeStatus::Online;
            let (_, port) = parse_host_port(&info.base.address);
            let addresses = if !info.addresses.is_empty() {
                info.addresses.clone()
            } else {
                let (host, _) = parse_host_port(&info.base.address);
                if host.is_empty() { Vec::new() } else { vec![host] }
            };
            return Some((addresses, port, is_online));
        }

        // 2. Fallback: scan all peers for matching node_id or name.
        //    This handles cases where the caller uses a node_id (e.g. "node-laptop-xxx")
        //    but the registry key is a peer name (e.g. "Node-A").
        let all = self.registry.list_peers();
        for info in &all {
            if info.base.id == peer_id || info.base.name == peer_id {
                let is_online = info.status == NodeStatus::Online;
                let (_, port) = parse_host_port(&info.base.address);
                let addresses = if !info.addresses.is_empty() {
                    info.addresses.clone()
                } else {
                    let (host, _) = parse_host_port(&info.base.address);
                    if host.is_empty() { Vec::new() } else { vec![host] }
                };
                return Some((addresses, port, is_online));
            }
        }

        None
    }

    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface> {
        network::get_local_network_interfaces()
            .into_iter()
            .map(|iface| LocalNetworkInterface {
                ip: iface.ip,
                mask: iface.mask,
            })
            .collect()
    }

    fn get_node_id(&self) -> String {
        self.node_id.clone()
    }
}

fn parse_host_port(addr: &str) -> (String, u16) {
    if let Some(idx) = addr.rfind(':') {
        let host = &addr[..idx];
        let port_str = &addr[idx + 1..];
        let port = port_str.parse().unwrap_or(DEFAULT_RPC_PORT);
        (host.into(), port)
    } else {
        (addr.into(), DEFAULT_RPC_PORT)
    }
}

/// Generate a node ID based on hostname and timestamp.
fn generate_node_id() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".into());
    format!("node-{}-{}", hostname.to_lowercase(), uuid::Uuid::new_v4())
}

/// Atomic write helper: write to `{path}.tmp` then rename.
///
/// Mirrors the pattern in `cluster_config::atomic_write` (which is private
/// there). Defined here separately because `merge_real_node_info` needs to
/// rewrite peers.toml from a `toml::Value` doc that has had the placeholder
/// subtable removed, before re-adding the real_id entry.
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    let tmp_path = path.with_extension("toml.tmp");
    std::fs::write(&tmp_path, data)?;
    match std::fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            let _ = std::fs::remove_file(&tmp_path);
            Err(e)
        }
    }
}

/// Real node info obtained from RPC `get_info` or UDP AnnounceMessage.
///
/// Carries the authoritative identity of a remote node. Used by
/// `Cluster::merge_real_node_info` to upgrade placeholder peer entries
/// (created by manual `nodes.add`) to the remote's real ID, and to refresh
/// fields whenever the remote broadcasts a new state.
#[derive(Debug, Clone)]
pub struct RealNodeInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    pub role: nemesis_types::cluster::NodeRole,
    pub category: String,
    pub capabilities: Vec<String>,
    pub node_type: String,
}

/// Address comparison used by UDP-triggered placeholder upgrade.
///
/// Returns true if `cand` and `needle` resolve to the same host[:port],
/// case-insensitive on host, exact on port (with missing-port treated as
/// wildcard). Defined here in addition to `registry::addr_matches` because
/// the latter is private to the registry module.
fn addr_eq(cand: &str, needle: &str) -> bool {
    let cand_lc = cand.trim().to_lowercase();
    let needle_lc = needle.trim().to_lowercase();
    if cand_lc.is_empty() || needle_lc.is_empty() {
        return false;
    }
    let (ch, cp) = match cand_lc.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && !h.is_empty() => (h, Some(p)),
        _ => (cand_lc.as_str(), None),
    };
    let (nh, np) = match needle_lc.rsplit_once(':') {
        Some((h, p)) if !p.is_empty() && !h.is_empty() => (h, Some(p)),
        _ => (needle_lc.as_str(), None),
    };
    // 严格语义：host 必须相等，port 必须匹配（都存在且相等，或都不存在）。
    // 不再容忍"一边带 port 一边不带"——这是 placeholder filter 误删同 host
    // 不同 port peer 的根因（cluster-uat 历史 bug：addresses 字段是 host-only
    // 列表，跟 host:rpc_port 比较时宽松规则会判定相等，导致后续加入的 peer
    // 把前面同 host 的 placeholder 当重复项删掉）。
    ch == nh && cp == np
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
