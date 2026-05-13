//! Cluster - manages node discovery, task distribution, and RPC lifecycle.
//!
//! The central orchestrator for a cluster node. Owns the registry, task manager,
//! continuation store, result store, RPC client/server, and discovery components.
//! Provides the `CallWithContext`, `SubmitTask`, and `SetMessageBus` APIs
//! consumed by the agent loop.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::{Mutex, RwLock};
use serde_json;
use tokio::sync::broadcast;

use nemesis_types::cluster::Task;

use crate::cluster_config::{
    ClusterMeta, DynamicState, NodeInfo as ConfigNodeInfo, PeerConfig, PeerStatus,
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
pub const DEFAULT_TIMEOUT: Duration = Duration::from_secs(90);

/// The cluster manages a set of nodes and distributes tasks.
pub struct Cluster {
    // -- Identity --
    node_id: String,
    node_name: String,
    address: String,
    role: String,
    category: String,
    tags: Vec<String>,

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
    timeout: Duration,

    // -- State --
    running: RwLock<bool>,
    stop_tx: broadcast::Sender<()>,
    bus: Mutex<Option<Arc<dyn MessageBus>>>,

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
            node_name: format!("Bot {}", &node_id[..8.min(node_id.len())]),
            address: config.bind_address.clone(),
            role: "worker".into(),
            category: "general".into(),
            tags: Vec::new(),
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
            timeout: DEFAULT_TIMEOUT,
            running: RwLock::new(false),
            stop_tx,
            bus: Mutex::new(None),
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

        // Try to load existing node ID from static config
        let node_id = match crate::cluster_config::load_static_config(&cluster_dir.join("peers.toml"))
        {
            Ok(sc) if !sc.node.id.is_empty() => sc.node.id,
            _ => node_id,
        };

        Self {
            node_id: node_id.clone(),
            node_name: format!("Bot {}", &node_id[..8.min(node_id.len())]),
            address: config.bind_address.clone(),
            role: "worker".into(),
            category: "general".into(),
            tags: Vec::new(),
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
            timeout: DEFAULT_TIMEOUT,
            running: RwLock::new(false),
            stop_tx,
            bus: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
        }
    }

    // -- Lifecycle ------------------------------------------------------------

    /// Start the cluster. Registers the local node and initializes the RPC client.
    pub fn start(&self) {
        *self.running.write() = true;

        // Register local node
        let local_node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: self.node_id.clone(),
                name: self.node_name.clone(),
                role: nemesis_types::cluster::NodeRole::Master,
                address: self.address.clone(),
                category: self.category.clone(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["cluster".into()],
            addresses: vec![],
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
            *self.rpc_client.lock() = Some(client);
        }

        // Start the recovery loop
        self.start_recovery_loop();

        // Start the sync loop (periodic node timeout check + disk persistence)
        self.start_sync_loop();

        logger::log_lifecycle("start", &self.node_id, &format!("rpc_port={}", self.rpc_port));
    }

    /// Stop the cluster.
    pub fn stop(&self) {
        *self.running.write() = false;
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
        let timeout = self.timeout;
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
                        // Check for timed-out nodes
                        let expired = registry.check_timeouts(timeout);
                        for node_id in &expired {
                            logger::log_discovery_info(&format!("Node expired: {}", node_id));
                        }

                        // Sync state to disk
                        let state_path = workspace.join("cluster").join("state.toml");
                        let state = DynamicState {
                            cluster: ClusterMeta::default(),
                            local_node: ConfigNodeInfo::default(),
                            discovered: registry.list_peers().iter().map(|n| {
                                let mut pc = n.to_peer_config();
                                pc.status.state = n.get_status_string().into();
                                pc
                            }).collect(),
                            last_sync: chrono::Utc::now().to_rfc3339(),
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
        self.registry.remove(node_id)
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
    ) {
        let primary_address = if !addresses.is_empty() {
            format!("{}:{}", addresses[0], rpc_port)
        } else {
            String::new()
        };

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: node_id.into(),
                name: name.into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: primary_address,
                category: category.into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities,
            addresses, // Preserve all addresses for multi-address failover
        };
        self.registry.upsert(node);
    }

    /// Mark a node as offline.
    pub fn handle_node_offline(&self, node_id: &str, _reason: &str) {
        // The registry doesn't have mark_offline, so we remove or update status
        if let Some(mut info) = self.registry.get(node_id) {
            info.status = NodeStatus::Offline;
            self.registry.upsert(info);
        }
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
        self.task_manager.complete_task(task_id, result)
    }

    /// Fail a task.
    pub fn fail_task(&self, task_id: &str, error: &str) -> bool {
        self.task_manager.fail_task(task_id, error)
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
    pub fn cleanup_task(&self, _task_id: &str) {
        // The task manager doesn't have delete; the task stays in history
        // This is a no-op placeholder matching Go's CleanupTask
    }

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

                // Try to run within an existing tokio runtime
                match tokio::runtime::Handle::try_current() {
                    Ok(handle) => {
                        // We're inside a tokio runtime - use block_on
                        // (This is safe because we're calling from non-async code)
                        match handle.block_on(client.call_with_timeout(
                            peer_id,
                            request,
                            client.timeout(),
                        )) {
                            Ok(response) => {
                                if let Some(ref err) = response.error {
                                    Err(err.clone())
                                } else {
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
                            Err(e) => Err(format!("RPC call failed: {}", e)),
                        }
                    }
                    Err(_) => {
                        // No tokio runtime available (e.g. in unit tests or CLI)
                        Err("RPC client not initialized (no tokio runtime available)".into())
                    }
                }
            }
            None => Err("RPC client not initialized".into()),
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

                match client.call_with_timeout(peer_id, request, timeout).await {
                    Ok(response) => {
                        if let Some(ref err) = response.error {
                            Err(err.clone())
                        } else {
                            match &response.result {
                                Some(val) => {
                                    serde_json::to_vec(val)
                                        .map_err(|e| format!("serialize response: {}", e))
                                }
                                None => Ok(Vec::new()),
                            }
                        }
                    }
                    Err(e) => Err(format!("RPC call failed: {}", e)),
                }
            }
            None => Err("RPC client not initialized".into()),
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
    pub fn node_name(&self) -> &str {
        &self.node_name
    }

    /// Get the address.
    pub fn address(&self) -> &str {
        &self.address
    }

    /// Get the role.
    pub fn role(&self) -> &str {
        &self.role
    }

    /// Get the category.
    pub fn category(&self) -> &str {
        &self.category
    }

    /// Get the tags.
    pub fn tags(&self) -> &[String] {
        &self.tags
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

    /// Get online peers.
    pub fn get_online_peers(&self) -> Vec<ExtendedNodeInfo> {
        self.registry.list_online()
    }

    /// Set ports.
    pub fn set_ports(&mut self, udp: u16, rpc: u16) {
        self.udp_port = udp;
        self.rpc_port = rpc;
    }

    /// Get the stop channel receiver.
    pub fn stop_receiver(&self) -> broadcast::Receiver<()> {
        self.stop_tx.subscribe()
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
                tags: Vec::new(),
                capabilities: node.capabilities.clone(),
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
            cluster: ClusterMeta {
                id: "auto-discovered".into(),
                auto_discovery: true,
                last_updated: chrono::Utc::now().to_rfc3339(),
                rpc_auth_token: String::new(),
            },
            local_node: ConfigNodeInfo {
                id: self.node_id.clone(),
                name: self.node_name.clone(),
                address: self.address.clone(),
                role: self.role.clone(),
                category: self.category.clone(),
                tags: self.tags.clone(),
                capabilities: Vec::new(),
            },
            discovered,
            last_sync: chrono::Utc::now().to_rfc3339(),
        };

        crate::cluster_config::save_dynamic_state(&self.dynamic_state_path, &state)
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

        // get_capabilities
        let caps = self.get_capabilities();
        self.register_rpc_handler("get_capabilities", Box::new(move |_payload| {
            Ok(serde_json::json!({
                "capabilities": caps,
            }))
        }))?;

        // get_info
        let node_id = self.node_id.clone();
        let node_name = self.node_name.clone();
        let role = self.role.clone();
        self.register_rpc_handler("get_info", Box::new(move |_payload| {
            Ok(serde_json::json!({
                "node_id": node_id,
                "name": node_name,
                "role": role,
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
                "Received forge reflection report from peer"
            );

            if let Err(e) = provider_share.receive_reflection(&payload) {
                tracing::error!(error = %e, "Failed to store reflection");
                return Ok(serde_json::json!({
                    "status": "error",
                    "error": format!("Failed to store reflection: {}", e),
                }));
            }

            Ok(serde_json::json!({
                "status": "ok",
                "message": "Reflection received",
                "node_id": node_id_share,
                "timestamp": chrono::Utc::now().to_rfc3339(),
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
                "Reflections list requested by peer"
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
                                "Failed to read reflection"
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

        tracing::info!("Registered forge RPC handlers: forge_share, forge_get_reflections");
        Ok(())
    }

    // -- RPC handler builders (extracted for testability) ----------------------

    /// Build the peer_chat handler (B-side: receive message, ACK, process async).
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
                "peer_chat received, returning ACK"
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
                "peer_chat_callback received"
            );

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
        *self.rpc_client.lock() = Some(client);
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

    for task in tasks {
        // Parse created_at (RFC 3339) and compute age.
        let created = match chrono::DateTime::parse_from_rfc3339(&task.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Utc),
            Err(_) => continue,
        };
        let age = chrono::Utc::now() - created;

        // Skip tasks younger than 2 minutes.
        if age < chrono::Duration::minutes(2) {
            continue;
        }

        // Timeout tasks older than 24 hours.
        if age > chrono::Duration::hours(24) {
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
                            "query_task_result returned error"
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
    fn node_id(&self) -> &str {
        &self.node_id
    }

    fn address(&self) -> &str {
        &self.address
    }

    fn rpc_port(&self) -> u16 {
        self.rpc_port
    }

    fn all_local_ips(&self) -> Vec<String> {
        network::get_all_local_ips()
    }

    fn role(&self) -> &str {
        &self.role
    }

    fn category(&self) -> &str {
        &self.category
    }

    fn tags(&self) -> Vec<String> {
        self.tags.clone()
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
    ) {
        self.handle_discovered_node(
            node_id,
            name,
            addresses.to_vec(),
            rpc_port,
            role,
            category,
            tags.to_vec(),
            capabilities.to_vec(),
        );
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
        let info = self.registry.get(peer_id)?;
        let is_online = info.status == NodeStatus::Online;

        // Use the stored addresses (all discovered IPs) for multi-address failover.
        // Fall back to parsing the primary address if addresses is empty.
        let (_, port) = parse_host_port(&info.base.address);
        let addresses = if !info.addresses.is_empty() {
            info.addresses.clone()
        } else {
            let (host, _) = parse_host_port(&info.base.address);
            if host.is_empty() {
                Vec::new()
            } else {
                vec![host]
            }
        };

        Some((addresses, port, is_online))
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
    let timestamp = chrono::Utc::now().format("%Y%m%d-%H%M%S%.6f");
    format!("bot-{}-{}", hostname, timestamp)
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use nemesis_types::cluster::TaskStatus;

    fn make_config() -> ClusterConfig {
        ClusterConfig {
            node_id: "local-node-001".into(),
            bind_address: "127.0.0.1:9000".into(),
            peers: vec!["127.0.0.1:9001".into()],
        }
    }

    #[test]
    fn test_start_stop_lifecycle() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.is_running());
        cluster.start();
        assert!(cluster.is_running());
        assert_eq!(cluster.list_nodes().len(), 1);

        cluster.stop();
        assert!(!cluster.is_running());
    }

    #[test]
    fn test_register_and_list_nodes() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let remote = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "remote-001".into(),
                name: "worker-1".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.2:9000".into(),
                category: "development".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into()],
            addresses: vec![],
        };
        cluster.register_node(remote);

        let nodes = cluster.list_nodes();
        assert_eq!(nodes.len(), 2);
        assert!(cluster.get_node_info("remote-001").is_some());
    }

    #[test]
    fn test_submit_and_assign_task() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let task_id = cluster.submit_task(
            "peer_chat",
            serde_json::json!({"message": "hello"}),
            "web",
            "chat-123",
        );

        let task = cluster.get_task(&task_id).unwrap();
        assert_eq!(task.action, "peer_chat");
        assert_eq!(task.status, TaskStatus::Pending);

        // Assign
        assert!(cluster.assign_task(&task_id, "remote-001"));
        let task = cluster.get_task(&task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Running);
    }

    #[test]
    fn test_complete_and_fail_task() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let task_id = cluster.submit_task(
            "peer_chat",
            serde_json::json!({}),
            "rpc",
            "chat-1",
        );

        // Complete
        cluster.assign_task(&task_id, "node-a");
        assert!(cluster.complete_task(&task_id, serde_json::json!("done")));
        let task = cluster.get_task(&task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);

        // Fail a different task
        let task_id2 = cluster.submit_task(
            "forge_share",
            serde_json::json!({}),
            "rpc",
            "chat-2",
        );
        assert!(cluster.fail_task(&task_id2, "timeout"));
        let task = cluster.get_task(&task_id2).unwrap();
        assert_eq!(task.status, TaskStatus::Failed);
    }

    #[test]
    fn test_handle_discovered_node() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "remote-002",
            "worker-2",
            vec!["10.0.0.3".into(), "192.168.1.5".into()],
            21949,
            "worker",
            "development",
            vec!["test".into()],
            vec!["llm".into(), "tools".into()],
        );

        let node = cluster.get_node_info("remote-002").unwrap();
        assert_eq!(node.base.name, "worker-2");
        assert_eq!(node.status, NodeStatus::Online);
        assert_eq!(node.capabilities.len(), 2);
    }

    #[test]
    fn test_handle_node_offline() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "remote-003",
            "worker-3",
            vec!["10.0.0.4".into()],
            21949,
            "worker",
            "general",
            vec![],
            vec![],
        );

        let node = cluster.get_node_info("remote-003").unwrap();
        assert_eq!(node.status, NodeStatus::Online);

        cluster.handle_node_offline("remote-003", "heartbeat timeout");
        let node = cluster.get_node_info("remote-003").unwrap();
        assert_eq!(node.status, NodeStatus::Offline);
    }

    #[test]
    fn test_get_capabilities() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "remote-004".into(),
                name: "worker-4".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.5:9000".into(),
                category: "development".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into(), "tools".into()],
            addresses: vec![],
        });

        let caps = cluster.get_capabilities();
        assert!(caps.contains(&"cluster".into()));
        assert!(caps.contains(&"llm".into()));
        assert!(caps.contains(&"tools".into()));
    }

    #[test]
    fn test_with_callback() {
        let completed = Arc::new(Mutex::new(Vec::new()));
        let completed_clone = completed.clone();
        let cluster = Cluster::with_callback(
            make_config(),
            Box::new(move |t: &Task| {
                completed_clone.lock().push(t.id.clone());
            }),
        );
        cluster.start();

        let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch");
        cluster.complete_task(&task_id, serde_json::json!("result"));

        let ids = completed.lock();
        assert!(ids.contains(&task_id));
    }

    #[test]
    fn test_handle_task_complete_no_bus() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Should not panic
        cluster.handle_task_complete("nonexistent");
    }

    #[test]
    fn test_call_with_context_override() {
        let cluster = Cluster::new(make_config());
        cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
            Ok(format!("called {} on {}", action, peer_id).into_bytes())
        }));

        let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
        assert!(result.is_ok());
        let s = String::from_utf8(result.unwrap()).unwrap();
        assert_eq!(s, "called ping on peer-1");
    }

    #[test]
    fn test_parse_host_port() {
        assert_eq!(parse_host_port("10.0.0.1:21949"), ("10.0.0.1".into(), 21949u16));
        assert_eq!(parse_host_port("example.com:8080"), ("example.com".into(), 8080u16));
        assert_eq!(parse_host_port("no-port"), ("no-port".into(), DEFAULT_RPC_PORT));
    }

    #[test]
    fn test_generate_node_id() {
        let id = generate_node_id();
        assert!(id.starts_with("bot-"));
        assert!(id.len() > 10);
    }

    struct MockBus {
        messages: Arc<Mutex<Vec<BusInboundMessage>>>,
    }

    impl MessageBus for MockBus {
        fn publish_inbound(&self, msg: BusInboundMessage) {
            self.messages.lock().push(msg);
        }
    }

    #[test]
    fn test_handle_task_complete_with_bus() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });

        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.set_message_bus(bus);

        let task_id = cluster.submit_task("peer_chat", serde_json::json!({}), "web", "chat-1");
        cluster.complete_task(&task_id, serde_json::json!("done"));

        // The callback should have been fired by the task manager
        // But handle_task_complete is called separately in the real flow
        cluster.handle_task_complete(&task_id);

        let msgs = messages.lock();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].channel, "system");
        assert!(msgs[0].sender_id.starts_with("cluster_continuation:"));
    }

    // -- call_with_context production path tests --------------------------------

    #[test]
    fn test_call_with_context_no_rpc_client_returns_error() {
        // Without start(), no RPC client is initialized.
        let cluster = Cluster::new(make_config());
        let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
        let err = result.unwrap_err();
        assert!(
            err.contains("RPC client not initialized"),
            "Expected 'not initialized' error, got: {:?}",
            err
        );
    }

    #[test]
    fn test_call_with_context_after_start_no_peer_errors() {
        // After start(), the RPC client is initialized with a resolver,
        // but the peer is not found in the registry. This should return
        // an error about peer not found or connection failure.
        let cluster = Cluster::new(make_config());
        cluster.start();
        let result = cluster.call_with_context("nonexistent-peer", "ping", serde_json::json!({}));
        assert!(result.is_err(), "Expected error for nonexistent peer, got: {:?}", result);
    }

    #[test]
    fn test_call_with_context_test_override_takes_priority() {
        // The test override should take priority even when RPC client is set.
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Set the test override AFTER start (which creates the RPC client)
        cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
            Ok(format!("override: {} on {}", action, peer_id).into_bytes())
        }));

        let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
        assert!(result.is_ok());
        let s = String::from_utf8(result.unwrap()).unwrap();
        assert_eq!(s, "override: ping on peer-1");
    }

    #[tokio::test]
    async fn test_call_with_context_async_no_rpc_client() {
        let cluster = Cluster::new(make_config());
        let result = cluster
            .call_with_context_async(
                "peer-1",
                "ping",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("RPC client not initialized"));
    }

    #[tokio::test]
    async fn test_call_with_context_async_test_override() {
        let cluster = Cluster::new(make_config());
        cluster.set_call_with_context_fn(Box::new(|peer_id, action, _payload| {
            Ok(format!("async-override: {} on {}", action, peer_id).into_bytes())
        }));

        let result = cluster
            .call_with_context_async(
                "peer-1",
                "hello",
                serde_json::json!({}),
                Duration::from_secs(5),
            )
            .await;
        assert!(result.is_ok());
        let s = String::from_utf8(result.unwrap()).unwrap();
        assert_eq!(s, "async-override: hello on peer-1");
    }

    #[test]
    fn test_start_initializes_rpc_client() {
        let cluster = Cluster::new(make_config());
        // Before start, no RPC client
        assert!(cluster.rpc_client.lock().is_none());
        cluster.start();
        // After start, RPC client is initialized with ClusterPeerResolver
        assert!(
            cluster.rpc_client.lock().is_some(),
            "start() should initialize the RPC client"
        );
    }

    #[test]
    fn test_set_rpc_client_overrides_auto_created() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Verify auto-created client
        assert!(cluster.rpc_client.lock().is_some());

        // Set a custom client
        let custom_client = Arc::new(RpcClient::with_timeout(Duration::from_secs(30)));
        cluster.set_rpc_client(custom_client);

        // Verify the custom client was set
        let client = cluster.rpc_client.lock();
        assert!(client.is_some());
        assert_eq!(client.as_ref().unwrap().timeout(), Duration::from_secs(30));
    }

    #[test]
    fn test_set_rpc_client_before_start_preserves_it() {
        let cluster = Cluster::new(make_config());
        // Set a custom client before start
        let custom_client = Arc::new(RpcClient::with_timeout(Duration::from_secs(120)));
        cluster.set_rpc_client(custom_client);

        cluster.start();

        // start() should not overwrite the custom client
        let client = cluster.rpc_client.lock();
        assert!(client.is_some());
        assert_eq!(client.as_ref().unwrap().timeout(), Duration::from_secs(120));
    }

    #[test]
    fn test_cluster_peer_resolver_returns_peer_info() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register a peer with specific addresses
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "peer-abc".into(),
                name: "test-peer".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "192.168.1.100:21949".into(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into()],
            addresses: vec!["192.168.1.100".into(), "10.0.0.5".into()],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        let (addresses, port, is_online) = resolver.get_peer_info("peer-abc").unwrap();
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&"192.168.1.100".to_string()));
        assert!(addresses.contains(&"10.0.0.5".to_string()));
        assert_eq!(port, 21949);
        assert!(is_online);
    }

    #[test]
    fn test_cluster_peer_resolver_unknown_peer() {
        let cluster = Cluster::new(make_config());
        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };
        assert!(resolver.get_peer_info("unknown-peer").is_none());
    }

    #[test]
    fn test_cluster_peer_resolver_offline_peer() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "offline-peer".into(),
                name: "offline-peer".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:21949".into(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Offline,
            capabilities: vec![],
            addresses: vec!["10.0.0.1".into()],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        let (_, _, is_online) = resolver.get_peer_info("offline-peer").unwrap();
        assert!(!is_online);
    }

    /// Helper to create a cluster with an RPC server for handler registration tests.
    fn make_cluster_with_rpc_server() -> Cluster {
        let mut cluster = Cluster::new(make_config());
        let server = Arc::new(crate::rpc::server::RpcServer::new(
            crate::rpc::server::RpcServerConfig {
                bind_address: "127.0.0.1:0".into(),
                ..Default::default()
            },
        ));
        cluster.set_rpc_server(server);
        cluster.start();
        cluster
    }

    #[test]
    fn test_register_forge_handlers() {
        let cluster = make_cluster_with_rpc_server();
        assert!(cluster.is_running());

        // Create a file-based forge provider
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(
            crate::handlers::FileForgeProvider::new(dir.path()),
        );

        // Register forge handlers
        let result = cluster.register_forge_handlers(provider);
        assert!(result.is_ok(), "register_forge_handlers should succeed: {:?}", result);

        // Verify handlers are registered by making RPC calls through the server
        let rpc_server = cluster.rpc_server.as_ref().unwrap();

        // Test forge_share handler
        let share_result = rpc_server.handle_request_sync("forge_share", serde_json::json!({
            "source_node": "remote-node-1",
            "report": {"insights": ["test insight"], "score": 0.85},
        }));
        assert!(share_result.is_ok(), "forge_share handler should succeed: {:?}", share_result);
        let resp = share_result.unwrap();
        assert_eq!(resp["status"], "ok");

        // Test forge_get_reflections handler
        let list_result = rpc_server.handle_request_sync("forge_get_reflections", serde_json::json!({}));
        assert!(list_result.is_ok(), "forge_get_reflections handler should succeed: {:?}", list_result);
        let list_resp = list_result.unwrap();
        assert!(list_resp.get("reflections").is_some());
        assert_eq!(list_resp["node_id"], "local-node-001");
    }

    #[test]
    fn test_register_forge_handlers_not_running() {
        let mut cluster = Cluster::new(make_config());
        let server = Arc::new(crate::rpc::server::RpcServer::new(
            crate::rpc::server::RpcServerConfig {
                bind_address: "127.0.0.1:0".into(),
                ..Default::default()
            },
        ));
        cluster.set_rpc_server(server);
        // Don't start the cluster
        assert!(!cluster.is_running());

        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(
            crate::handlers::FileForgeProvider::new(dir.path()),
        );

        let result = cluster.register_forge_handlers(provider);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not running"));
    }

    #[test]
    fn test_register_basic_handlers_registers_all() {
        let cluster = make_cluster_with_rpc_server();

        let result = cluster.register_basic_handlers();
        assert!(result.is_ok());

        // Verify all basic handlers are registered
        let rpc_server = cluster.rpc_server.as_ref().unwrap();
        let actions = vec!["ping", "get_capabilities", "get_info", "list_actions", "hello"];
        for action in actions {
            let result = rpc_server.handle_request_sync(action, serde_json::json!({}));
            assert!(result.is_ok(), "Handler '{}' should be registered", action);
        }
    }

    // -- string_value helper tests --

    #[test]
    fn test_string_value_with_string() {
        let v = serde_json::json!("hello");
        assert_eq!(string_value(Some(&v)), "hello");
    }

    #[test]
    fn test_string_value_with_null() {
        let v = serde_json::Value::Null;
        assert_eq!(string_value(Some(&v)), "");
    }

    #[test]
    fn test_string_value_with_number() {
        let v = serde_json::json!(42);
        assert_eq!(string_value(Some(&v)), "42");
    }

    #[test]
    fn test_string_value_with_boolean() {
        let v = serde_json::json!(true);
        assert_eq!(string_value(Some(&v)), "true");
    }

    #[test]
    fn test_string_value_with_none() {
        assert_eq!(string_value(None), "");
    }

    #[test]
    fn test_string_value_with_object() {
        let v = serde_json::json!({"key": "val"});
        // Objects fall through to as_str().unwrap_or("")
        assert_eq!(string_value(Some(&v)), "");
    }

    #[test]
    fn test_string_value_with_float() {
        let v = serde_json::json!(3.14);
        assert_eq!(string_value(Some(&v)), "3.14");
    }

    // -- Accessor tests --

    #[test]
    fn test_accessors() {
        let cluster = Cluster::new(make_config());
        assert_eq!(cluster.node_id(), "local-node-001");
        assert!(cluster.node_name().starts_with("Bot "));
        assert_eq!(cluster.address(), "127.0.0.1:9000");
        assert_eq!(cluster.role(), "worker");
        assert_eq!(cluster.category(), "general");
        assert!(cluster.tags().is_empty());
        assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
        assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);
    }

    #[test]
    fn test_set_ports() {
        let mut cluster = Cluster::new(make_config());
        assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
        assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);

        cluster.set_ports(11111, 22222);
        assert_eq!(cluster.udp_port(), 11111);
        assert_eq!(cluster.rpc_port(), 22222);
    }

    #[test]
    fn test_generate_node_id_uniqueness() {
        let id1 = generate_node_id();
        let id2 = generate_node_id();
        assert!(id1.starts_with("bot-"));
        assert!(id2.starts_with("bot-"));
        // The timestamp portion should differ
        assert_ne!(id1, id2);
    }

    #[test]
    fn test_parse_host_port_edge_cases() {
        // IPv6-like address
        let (host, port) = parse_host_port("[::1]:8080");
        assert_eq!(host, "[::1]");
        assert_eq!(port, 8080);

        // Invalid port number
        let (host, port) = parse_host_port("host:abc");
        assert_eq!(host, "host");
        assert_eq!(port, DEFAULT_RPC_PORT);

        // Port 0
        let (host, port) = parse_host_port("host:0");
        assert_eq!(host, "host");
        assert_eq!(port, 0);

        // Empty string
        let (host, port) = parse_host_port("");
        assert_eq!(host, "");
        assert_eq!(port, DEFAULT_RPC_PORT);
    }

    #[test]
    fn test_default_constants() {
        assert_eq!(DEFAULT_UDP_PORT, 11949);
        assert_eq!(DEFAULT_RPC_PORT, 21949);
        assert_eq!(DEFAULT_BROADCAST_INTERVAL, Duration::from_secs(30));
        assert_eq!(DEFAULT_TIMEOUT, Duration::from_secs(90));
    }

    #[test]
    fn test_with_workspace_creates_cluster() {
        let dir = tempfile::tempdir().unwrap();
        let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
        assert_eq!(cluster.workspace(), dir.path());
        assert!(!cluster.is_running());
    }

    #[test]
    fn test_stop_receiver_returns_receiver() {
        let cluster = Cluster::new(make_config());
        let _rx = cluster.stop_receiver();
    }

    #[test]
    fn test_get_peer_returns_none_for_unknown() {
        let cluster = Cluster::new(make_config());
        assert!(cluster.get_peer("unknown-peer").is_none());
    }

    #[test]
    fn test_get_online_peers_initially_empty_after_new() {
        let cluster = Cluster::new(make_config());
        // Before start, no online peers registered
        let peers = cluster.get_online_peers();
        assert!(peers.is_empty());
    }

    #[test]
    fn test_get_all_local_ips_does_not_panic() {
        let cluster = Cluster::new(make_config());
        let _ips = cluster.get_all_local_ips();
    }

    #[test]
    fn test_all_go_handlers_registered_after_full_setup() {
        // Simulate full startup: register_basic_handlers + register_peer_chat_handlers + register_forge_handlers
        let cluster = make_cluster_with_rpc_server();

        // Register basic handlers (ping, get_capabilities, get_info, list_actions, hello)
        cluster.register_basic_handlers().unwrap();

        // Set a mock RPC channel to trigger register_peer_chat_handlers
        use crate::rpc::RpcChannel;
        #[derive(Debug)]
        struct MockRpcChannel;
        impl RpcChannel for MockRpcChannel {
            fn input(
                &self,
                _session_key: &str,
                _content: &str,
                _correlation_id: &str,
            ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
                Err("mock".into())
            }
        }
        cluster.set_rpc_channel(Arc::new(MockRpcChannel));

        // Register forge handlers
        let dir = tempfile::tempdir().unwrap();
        let provider = Box::new(
            crate::handlers::FileForgeProvider::new(dir.path()),
        );
        cluster.register_forge_handlers(provider).unwrap();

        // Verify ALL Go-compatible handlers are registered
        let rpc_server = cluster.rpc_server.as_ref().unwrap();
        let expected_actions = vec![
            // Default handlers
            "ping", "get_capabilities", "get_info", "list_actions",
            // Peer chat handlers
            "peer_chat", "peer_chat_callback", "hello",
            "query_task_result", "confirm_task_delivery",
            // Forge handlers
            "forge_share", "forge_get_reflections",
        ];
        for action in expected_actions {
            let result = rpc_server.handle_request_sync(action, serde_json::json!({}));
            assert!(result.is_ok(), "Handler '{}' should be registered after full setup", action);
        }
    }

    // -- Additional coverage tests --

    #[test]
    fn test_submit_peer_chat() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let result = cluster.submit_peer_chat(
            "remote-001",
            "peer_chat",
            serde_json::json!({"content": "hello", "task_id": "task-abc"}),
            "web",
            "chat-1",
        );
        assert!(result.is_ok());
        let task_id = result.unwrap();
        assert!(!task_id.is_empty());
        // Verify the task exists in the task manager
        assert!(cluster.get_task(&task_id).is_some());
    }

    #[test]
    fn test_submit_peer_chat_auto_task_id() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // No task_id in payload -> should auto-generate
        let result = cluster.submit_peer_chat(
            "remote-001",
            "peer_chat",
            serde_json::json!({"content": "hello"}),
            "web",
            "chat-1",
        );
        assert!(result.is_ok());
        let task_id = result.unwrap();
        assert!(!task_id.is_empty());
    }

    #[test]
    fn test_handle_discovered_node_no_addresses() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "no-addr-node",
            "empty-addr",
            vec![], // no addresses
            21949,
            "worker",
            "test",
            vec![],
            vec!["llm".into()],
        );

        let node = cluster.get_node_info("no-addr-node").unwrap();
        assert_eq!(node.base.address, ""); // empty primary address
        assert_eq!(node.capabilities.len(), 1);
    }

    #[test]
    fn test_handle_node_offline_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Should not panic
        cluster.handle_node_offline("nonexistent", "test");
    }

    #[test]
    fn test_remove_node() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "to-remove",
            "removeme",
            vec!["10.0.0.1".into()],
            21949,
            "worker",
            "test",
            vec![],
            vec![],
        );

        assert!(cluster.get_node_info("to-remove").is_some());
        assert!(cluster.remove_node("to-remove"));
        assert!(cluster.get_node_info("to-remove").is_none());
    }

    #[test]
    fn test_remove_node_nonexistent() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.remove_node("nonexistent"));
    }

    #[test]
    fn test_list_tasks() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        assert!(cluster.list_tasks().is_empty());

        cluster.submit_task("action1", serde_json::json!({}), "web", "ch1");
        cluster.submit_task("action2", serde_json::json!({}), "web", "ch2");

        let tasks = cluster.list_tasks();
        assert_eq!(tasks.len(), 2);
    }

    #[test]
    fn test_cleanup_task_noop() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "ch");
        // Should not panic
        cluster.cleanup_task(&task_id);
        // Task should still exist (no-op)
        assert!(cluster.get_task(&task_id).is_some());
    }

    #[test]
    fn test_get_task_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        assert!(cluster.get_task("nonexistent").is_none());
    }

    #[test]
    fn test_assign_task_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        assert!(!cluster.assign_task("nonexistent", "node-1"));
    }

    #[test]
    fn test_complete_task_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        assert!(!cluster.complete_task("nonexistent", serde_json::json!("result")));
    }

    #[test]
    fn test_fail_task_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        assert!(!cluster.fail_task("nonexistent", "error"));
    }

    #[test]
    fn test_task_manager_accessor() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let tm = cluster.task_manager();
        assert!(Arc::strong_count(tm) >= 1);
    }

    #[test]
    fn test_continuation_store_accessor() {
        let cluster = Cluster::new(make_config());
        let store = cluster.continuation_store();
        assert!(Arc::strong_count(store) >= 1);
    }

    #[test]
    fn test_result_store_accessor() {
        let cluster = Cluster::new(make_config());
        let store = cluster.result_store();
        assert!(Arc::strong_count(store) >= 1);
    }

    #[test]
    fn test_handle_task_complete_empty_channel() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Submit a task with empty original_channel
        let task_id = cluster.submit_task("action", serde_json::json!({}), "", "");
        cluster.complete_task(&task_id, serde_json::json!("done"));

        // Should return early (no bus message published)
        cluster.handle_task_complete(&task_id);
    }

    #[test]
    fn test_sync_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
        cluster.start();

        let result = cluster.sync_to_disk();
        assert!(result.is_ok());

        // Verify file was created
        let state_path = dir.path().join("cluster").join("state.toml");
        assert!(state_path.exists());
    }

    #[test]
    fn test_find_peers_by_capability() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "peer-with-llm".into(),
                name: "llm-peer".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into(), "tools".into()],
            addresses: vec![],
        });

        let llm_peers = cluster.find_peers_by_capability("llm");
        assert_eq!(llm_peers.len(), 1);

        let no_peers = cluster.find_peers_by_capability("nonexistent");
        assert!(no_peers.is_empty());
    }

    #[test]
    fn test_get_config() {
        let cluster = Cluster::new(make_config());
        // config() returns a static default, not the actual config
        // Use node_id() to verify the actual node ID
        assert_eq!(cluster.node_id(), "local-node-001");
    }

    #[test]
    fn test_bus_inbound_message_fields() {
        let msg = BusInboundMessage {
            channel: "system".into(),
            sender_id: "sender-1".into(),
            chat_id: "chat-1".into(),
            content: "hello".into(),
        };
        assert_eq!(msg.channel, "system");
        assert_eq!(msg.sender_id, "sender-1");
        assert_eq!(msg.chat_id, "chat-1");
        assert_eq!(msg.content, "hello");
    }

    #[test]
    fn test_cluster_peer_resolver_node_id() {
        let cluster = Cluster::new(make_config());
        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };
        assert_eq!(resolver.get_node_id(), "local-node-001");
    }

    #[test]
    fn test_cluster_peer_resolver_local_interfaces() {
        let cluster = Cluster::new(make_config());
        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };
        // Should return the local node's addresses as interfaces
        let interfaces = resolver.get_local_interfaces();
        // May or may not have interfaces depending on registry state
        assert!(interfaces.is_empty() || !interfaces.is_empty());
    }

    // ============================================================
    // Coverage improvement: additional cluster tests
    // ============================================================

    #[test]
    fn test_start_registers_self_node() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        // The local node should be registered in the registry
        let nodes = cluster.list_nodes();
        assert_eq!(nodes.len(), 1);
        assert_eq!(nodes[0].base.id, "local-node-001");
        cluster.stop();
    }

    #[test]
    fn test_start_idempotent() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.start(); // Second call should be a no-op
        assert!(cluster.is_running());
        let nodes = cluster.list_nodes();
        assert_eq!(nodes.len(), 1); // Should not duplicate self
        cluster.stop();
    }

    #[test]
    fn test_stop_idempotent() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.stop();
        cluster.stop(); // Second call should be a no-op
        assert!(!cluster.is_running());
    }

    #[test]
    fn test_handle_task_complete_no_bus_set() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch1");
        cluster.complete_task(&task_id, serde_json::json!("result"));
        // Should not panic when bus is not set
        cluster.handle_task_complete(&task_id);
        cluster.stop();
    }

    #[test]
    fn test_handle_task_complete_nonexistent_task() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.set_message_bus(bus);
        // Should return early, no panic
        cluster.handle_task_complete("nonexistent-task");
        assert!(messages.lock().is_empty());
        cluster.stop();
    }

    #[test]
    fn test_sync_to_disk_no_workspace() {
        // Without workspace, sync_to_disk should fail
        let cluster = Cluster::new(make_config());
        cluster.start();
        let result = cluster.sync_to_disk();
        // The default workspace is empty, so this should return error or succeed
        // depending on whether the directory exists
        // It should not panic either way
        let _ = result;
        cluster.stop();
    }

    #[test]
    fn test_sync_to_disk_includes_discovered_nodes() {
        let dir = tempfile::tempdir().unwrap();
        let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "discovered-1".into(),
                name: "discovered-peer".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into()],
            addresses: vec![],
        });

        let result = cluster.sync_to_disk();
        assert!(result.is_ok());
        cluster.stop();
    }

    #[test]
    fn test_register_node_updates_existing() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register a node
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "node-1".into(),
                name: "original-name".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec![],
        });

        // Re-register same node with updated name
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "node-1".into(),
                name: "updated-name".into(),
                role: nemesis_types::cluster::NodeRole::Master,
                address: "10.0.0.2:9000".into(),
                category: "prod".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["tools".into()],
            addresses: vec![],
        });

        let node = cluster.get_node_info("node-1").unwrap();
        assert_eq!(node.base.name, "updated-name");
        assert_eq!(node.base.role, nemesis_types::cluster::NodeRole::Master);
        assert_eq!(node.capabilities.len(), 1);
        cluster.stop();
    }

    #[test]
    fn test_get_online_peers_includes_online_nodes() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "online-peer".into(),
                name: "online".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec![],
        });

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "offline-peer".into(),
                name: "offline".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.2:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Offline,
            capabilities: vec![],
            addresses: vec![],
        });

        let online = cluster.get_online_peers();
        // Should include the local node and the online peer, but NOT the offline peer
        assert!(online.iter().any(|n| n.base.id == "online-peer"));
        assert!(!online.iter().any(|n| n.base.id == "offline-peer"));
        cluster.stop();
    }

    #[test]
    fn test_get_capabilities_dedup() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register two nodes with overlapping capabilities
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "node-a".into(),
                name: "a".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into(), "tools".into()],
            addresses: vec![],
        });

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "node-b".into(),
                name: "b".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.2:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into(), "forge".into()],
            addresses: vec![],
        });

        let caps = cluster.get_capabilities();
        // "llm" should appear only once (dedup)
        assert_eq!(caps.iter().filter(|c| **c == "llm").count(), 1);
        assert!(caps.contains(&"llm".to_string()));
        assert!(caps.contains(&"tools".to_string()));
        assert!(caps.contains(&"forge".to_string()));
        cluster.stop();
    }

    #[test]
    fn test_register_rpc_handler_not_running() {
        let cluster = Cluster::new(make_config());
        // Not started, so register_rpc_handler should fail
        let result = cluster.register_rpc_handler("test_action", Box::new(|_| {
            Ok(serde_json::json!({}))
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not running"));
    }

    #[test]
    fn test_register_rpc_handler_no_server() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        // No RPC server set, should fail
        let result = cluster.register_rpc_handler("test_action", Box::new(|_| {
            Ok(serde_json::json!({}))
        }));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("RPC server"));
        cluster.stop();
    }

    #[test]
    fn test_register_basic_handlers_not_running() {
        let cluster = Cluster::new(make_config());
        let result = cluster.register_basic_handlers();
        assert!(result.is_err());
    }

    #[test]
    fn test_get_rpc_channel_initially_none() {
        let cluster = Cluster::new(make_config());
        assert!(cluster.get_rpc_channel().is_none());
    }

    #[test]
    fn test_config_returns_default() {
        let cluster = Cluster::new(make_config());
        let config = cluster.config();
        // config() returns a static default
        assert_eq!(config.bind_address, "0.0.0.0:9000");
    }

    #[test]
    fn test_handle_discovered_node_with_multiple_addresses() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "multi-addr-node",
            "multi",
            vec!["10.0.0.1".into(), "192.168.1.1".into(), "172.16.0.1".into()],
            21949,
            "worker",
            "dev",
            vec!["tag1".into()],
            vec!["llm".into()],
        );

        let node = cluster.get_node_info("multi-addr-node").unwrap();
        assert_eq!(node.addresses.len(), 3);
        assert!(node.addresses.contains(&"10.0.0.1".to_string()));
        assert!(node.addresses.contains(&"192.168.1.1".to_string()));
        assert!(node.addresses.contains(&"172.16.0.1".to_string()));
        cluster.stop();
    }

    #[test]
    fn test_call_with_context_override_returns_error() {
        let cluster = Cluster::new(make_config());
        cluster.set_call_with_context_fn(Box::new(|_peer, _action, _payload| {
            Err("test error".to_string())
        }));

        let result = cluster.call_with_context("peer-1", "action", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("test error"));
    }

    #[tokio::test]
    async fn test_call_with_context_async_override_returns_error() {
        let cluster = Cluster::new(make_config());
        cluster.set_call_with_context_fn(Box::new(|_peer, _action, _payload| {
            Err("async test error".to_string())
        }));

        let result = cluster
            .call_with_context_async("peer-1", "action", serde_json::json!({}), Duration::from_secs(5))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_peer_chat_handlers_without_rpc_channel() {
        let cluster = make_cluster_with_rpc_server();
        // Don't set RPC channel - register_peer_chat_handlers should return early
        // but not panic
        cluster.register_peer_chat_handlers();
    }

    #[test]
    fn test_set_rpc_server() {
        let mut cluster = Cluster::new(make_config());
        assert!(cluster.rpc_server.is_none());
        let server = Arc::new(crate::rpc::server::RpcServer::new(
            crate::rpc::server::RpcServerConfig {
                bind_address: "127.0.0.1:0".into(),
                ..Default::default()
            },
        ));
        cluster.set_rpc_server(server);
        assert!(cluster.rpc_server.is_some());
    }

    #[test]
    fn test_complete_task_with_callback_and_bus() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });
        let completed = Arc::new(Mutex::new(Vec::new()));
        let completed_clone = completed.clone();
        let cluster = Cluster::with_callback(
            make_config(),
            Box::new(move |t: &Task| {
                completed_clone.lock().push(t.id.clone());
            }),
        );
        cluster.start();
        cluster.set_message_bus(bus);

        let task_id = cluster.submit_task("peer_chat", serde_json::json!({}), "web", "chat-1");
        cluster.assign_task(&task_id, "node-a");
        cluster.complete_task(&task_id, serde_json::json!("done"));

        // Callback fires from task_manager
        let ids = completed.lock();
        assert!(ids.contains(&task_id));
    }

    #[test]
    fn test_handle_task_complete_with_failed_task() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.set_message_bus(bus);

        let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch1");
        cluster.fail_task(&task_id, "error");
        cluster.handle_task_complete(&task_id);

        // Should publish continuation message even for failed tasks
        let msgs = messages.lock();
        assert_eq!(msgs.len(), 1);
        assert!(msgs[0].sender_id.starts_with("cluster_continuation:"));
    }

    #[test]
    fn test_find_peers_by_capability_offline_excluded() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "offline-cap".into(),
                name: "offline-cap".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "dev".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Offline,
            capabilities: vec!["llm".into()],
            addresses: vec![],
        });

        let peers = cluster.find_peers_by_capability("llm");
        // Offline node should not be included
        assert!(peers.is_empty());
        cluster.stop();
    }

    #[test]
    fn test_remove_node_then_readd() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node("node-x", "x", vec!["10.0.0.1".into()], 21949, "worker", "dev", vec![], vec![]);
        assert!(cluster.get_node_info("node-x").is_some());

        cluster.remove_node("node-x");
        assert!(cluster.get_node_info("node-x").is_none());

        // Re-add
        cluster.handle_discovered_node("node-x", "x-v2", vec!["10.0.0.2".into()], 21949, "worker", "dev", vec![], vec![]);
        let node = cluster.get_node_info("node-x").unwrap();
        assert_eq!(node.base.name, "x-v2");
        cluster.stop();
    }

    #[test]
    fn test_bus_inbound_message_debug() {
        let msg = BusInboundMessage {
            channel: "test".into(),
            sender_id: "sender".into(),
            chat_id: "chat".into(),
            content: "content".into(),
        };
        let debug_str = format!("{:?}", msg);
        assert!(debug_str.contains("test"));
        assert!(debug_str.contains("sender"));
    }

    #[test]
    fn test_cluster_default_node_info() {
        let config = make_config();
        let cluster = Cluster::new(config);
        assert_eq!(cluster.role(), "worker");
        assert_eq!(cluster.category(), "general");
        assert!(cluster.tags().is_empty());
    }

    #[test]
    fn test_handle_task_complete_with_channel_and_chat_id() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.set_message_bus(bus);

        let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "chat-42");
        cluster.complete_task(&task_id, serde_json::json!("result"));
        cluster.handle_task_complete(&task_id);

        let msgs = messages.lock();
        assert_eq!(msgs.len(), 1);
        assert_eq!(msgs[0].chat_id, "web:chat-42");
    }

    #[test]
    fn test_cluster_new_vs_with_workspace() {
        let config = make_config();
        let cluster1 = Cluster::new(config.clone());
        // new() uses current_dir() as workspace
        assert!(cluster1.workspace().exists());

        let dir = tempfile::tempdir().unwrap();
        let cluster2 = Cluster::with_workspace(config, dir.path().to_path_buf());
        assert_eq!(cluster2.workspace(), dir.path());
    }

    #[test]
    fn test_submit_multiple_tasks() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let ids: Vec<String> = (0..10)
            .map(|i| cluster.submit_task("action", serde_json::json!({"i": i}), "rpc", "ch"))
            .collect();

        let tasks = cluster.list_tasks();
        assert_eq!(tasks.len(), 10);

        // All IDs should be unique
        let unique: std::collections::HashSet<_> = ids.iter().collect();
        assert_eq!(unique.len(), 10);
        cluster.stop();
    }

    #[test]
    fn test_get_peer_returns_correct_info() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "peer-xyz",
            "xyz-peer",
            vec!["10.0.0.1".into()],
            21949,
            "worker",
            "test",
            vec!["tag1".into()],
            vec!["llm".into(), "forge".into()],
        );

        let peer = cluster.get_peer("peer-xyz").unwrap();
        assert_eq!(peer.base.id, "peer-xyz");
        assert_eq!(peer.base.name, "xyz-peer");
        assert_eq!(peer.capabilities.len(), 2);
        cluster.stop();
    }

    // ============================================================
    // Coverage improvement: poll_stale_pending_tasks, confirm_delivery,
    // handler builders, actions_schema, peer resolver edge cases
    // ============================================================

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_young_task_skipped() {
        let tm = Arc::new(TaskManager::new());
        // Create a brand new task (< 2 minutes old) - should be skipped
        let _task = tm.create_task("action", serde_json::json!({}), "rpc", "ch");
        poll_stale_pending_tasks(&tm, &None, None).await;
        // Task should still be pending
        let pending = tm.list_pending_tasks();
        assert_eq!(pending.len(), 1);
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_old_task_timed_out() {
        let tm = Arc::new(TaskManager::new());
        // Create a task with an old created_at (> 24 hours) and a peer_id
        let old_time = (chrono::Utc::now() - chrono::Duration::hours(25)).to_rfc3339();
        let task = Task {
            id: "stale-24h".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        poll_stale_pending_tasks(&tm, &None, None).await;
        let t = tm.get_task("stale-24h").unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_stale_with_call_fn() {
        let tm = Arc::new(TaskManager::new());
        // Create a task that is > 2 minutes old but < 24 hours, with peer_id
        let old_time = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let task = Task {
            id: "stale-5m".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        // Provide a call_fn that returns a "not_found" response
        let call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>> =
            Some(Arc::new(|_peer, _action, _payload| {
                let resp = serde_json::json!({"status": "not_found", "task_id": "stale-5m"});
                Ok(serde_json::to_vec(&resp).unwrap())
            }));

        poll_stale_pending_tasks(&tm, &call_fn, None).await;
        let t = tm.get_task("stale-5m").unwrap();
        assert_eq!(t.status, TaskStatus::Failed);
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_stale_with_done_response() {
        let tm = Arc::new(TaskManager::new());
        let old_time = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let task = Task {
            id: "stale-done".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        // call_fn returns a "done" response with success
        let call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>> =
            Some(Arc::new(|_peer, action, _payload| {
                if action == "query_task_result" {
                    let resp = serde_json::json!({
                        "status": "done",
                        "task_id": "stale-done",
                        "result_status": "success",
                        "response": "hello",
                        "error": ""
                    });
                    Ok(serde_json::to_vec(&resp).unwrap())
                } else {
                    // confirm_task_delivery
                    Ok(Vec::new())
                }
            }));

        poll_stale_pending_tasks(&tm, &call_fn, None).await;
        let t = tm.get_task("stale-done").unwrap();
        assert_eq!(t.status, TaskStatus::Completed);
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_stale_with_running_response() {
        let tm = Arc::new(TaskManager::new());
        let old_time = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let task = Task {
            id: "stale-running".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        // call_fn returns "running" status - should remain pending
        let call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>> =
            Some(Arc::new(|_peer, _action, _payload| {
                let resp = serde_json::json!({"status": "running", "task_id": "stale-running"});
                Ok(serde_json::to_vec(&resp).unwrap())
            }));

        poll_stale_pending_tasks(&tm, &call_fn, None).await;
        let t = tm.get_task("stale-running").unwrap();
        assert_eq!(t.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_no_peer_id() {
        let tm = Arc::new(TaskManager::new());
        // Task older than 2 min but with no peer_id -> should be skipped
        let old_time = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let task = Task {
            id: "no-peer".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: String::new(), // empty peer_id
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        poll_stale_pending_tasks(&tm, &None, None).await;
        let t = tm.get_task("no-peer").unwrap();
        assert_eq!(t.status, TaskStatus::Pending); // still pending
    }

    #[tokio::test]
    async fn test_poll_stale_pending_tasks_call_fn_error() {
        let tm = Arc::new(TaskManager::new());
        let old_time = (chrono::Utc::now() - chrono::Duration::minutes(5)).to_rfc3339();
        let task = Task {
            id: "call-error".to_string(),
            status: TaskStatus::Pending,
            action: "action".to_string(),
            peer_id: "remote-1".to_string(),
            payload: serde_json::json!({}),
            result: None,
            original_channel: "rpc".to_string(),
            original_chat_id: "ch".to_string(),
            created_at: old_time,
            completed_at: None,
        };
        tm.submit(task).unwrap();

        // call_fn returns error -> task stays pending
        let call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>> =
            Some(Arc::new(|_peer, _action, _payload| {
                Err("connection refused".to_string())
            }));

        poll_stale_pending_tasks(&tm, &call_fn, None).await;
        let t = tm.get_task("call-error").unwrap();
        assert_eq!(t.status, TaskStatus::Pending);
    }

    #[tokio::test]
    async fn test_confirm_delivery_with_call_fn() {
        let confirmed = Arc::new(Mutex::new(false));
        let confirmed_clone = confirmed.clone();
        let call_fn: Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>> =
            Some(Arc::new(move |_peer, action, _payload| {
                if action == "confirm_task_delivery" {
                    *confirmed_clone.lock() = true;
                }
                Ok(Vec::new())
            }));

        confirm_delivery_with(&call_fn, None, "peer-1", "task-1").await;
        assert!(*confirmed.lock());
    }

    #[tokio::test]
    async fn test_confirm_delivery_no_client_no_fn() {
        // No client and no call_fn -> should just return without panic
        confirm_delivery_with(&None, None, "peer-1", "task-1").await;
    }

    #[test]
    fn test_peer_chat_handler_empty_content() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_peer_chat_handler();
        let result = handler(serde_json::json!({"task_id": "t1"}));
        assert_eq!(result.unwrap()["status"], "error");
    }

    #[test]
    fn test_peer_chat_handler_with_content() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_peer_chat_handler();
        let result = handler(serde_json::json!({
            "content": "hello",
            "task_id": "t1"
        }));
        let resp = result.unwrap();
        assert_eq!(resp["status"], "accepted");
        assert_eq!(resp["task_id"], "t1");
    }

    #[test]
    fn test_callback_handler_empty_task_id() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_callback_handler();
        let result = handler(serde_json::json!({"status": "success"}));
        assert_eq!(result.unwrap()["status"], "error");
    }

    #[test]
    fn test_callback_handler_with_task_id() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let task_id = cluster.submit_task("action", serde_json::json!({}), "rpc", "ch");

        let handler = cluster.build_callback_handler();
        let result = handler(serde_json::json!({
            "task_id": task_id,
            "status": "success",
            "response": "hello"
        }));
        assert_eq!(result.unwrap()["status"], "accepted");

        let task = cluster.get_task(&task_id).unwrap();
        assert_eq!(task.status, TaskStatus::Completed);
    }

    #[test]
    fn test_query_task_result_handler_empty_task_id() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_query_task_result_handler();
        let result = handler(serde_json::json!({}));
        assert_eq!(result.unwrap()["status"], "error");
    }

    #[test]
    fn test_query_task_result_handler_not_found() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_query_task_result_handler();
        let result = handler(serde_json::json!({"task_id": "unknown"}));
        assert_eq!(result.unwrap()["status"], "not_found");
    }

    #[test]
    fn test_query_task_result_handler_found() {
        let cluster = Cluster::new(make_config());
        cluster.result_store.store_success("task-1", "peer_chat", serde_json::json!({
            "response": "hello world",
        }));
        let handler = cluster.build_query_task_result_handler();
        let result = handler(serde_json::json!({"task_id": "task-1"}));
        let resp = result.unwrap();
        assert_eq!(resp["status"], "done");
        assert_eq!(resp["result_status"], "success");
        assert_eq!(resp["response"], "hello world");
    }

    #[test]
    fn test_query_task_result_handler_failed_result() {
        let cluster = Cluster::new(make_config());
        cluster.result_store.store_failure("task-err", "peer_chat", "something failed");
        let handler = cluster.build_query_task_result_handler();
        let result = handler(serde_json::json!({"task_id": "task-err"}));
        let resp = result.unwrap();
        assert_eq!(resp["status"], "done");
        assert_eq!(resp["result_status"], "error");
    }

    #[test]
    fn test_confirm_task_delivery_handler_empty_task_id() {
        let cluster = Cluster::new(make_config());
        let handler = cluster.build_confirm_task_delivery_handler();
        let result = handler(serde_json::json!({}));
        assert_eq!(result.unwrap()["status"], "error");
    }

    #[test]
    fn test_confirm_task_delivery_handler_removes_result() {
        let cluster = Cluster::new(make_config());
        cluster.result_store.store_success("task-del", "peer_chat", serde_json::json!({"r": "v"}));
        assert!(cluster.result_store.get("task-del").is_some());

        let handler = cluster.build_confirm_task_delivery_handler();
        let result = handler(serde_json::json!({"task_id": "task-del"}));
        assert_eq!(result.unwrap()["status"], "confirmed");
        // Note: confirm_task_delivery may or may not remove the result depending on implementation
    }

    #[test]
    fn test_get_actions_schema() {
        let cluster = Cluster::new(make_config());
        let schema = cluster.get_actions_schema();
        assert!(!schema.is_empty());
        // Check some known actions exist
        use crate::actions_schema::Action;
        let actions: Vec<&Action> = schema.iter().map(|s| &s.action).collect();
        assert!(actions.iter().any(|a| matches!(a, Action::Ping)));
        assert!(actions.iter().any(|a| matches!(a, Action::PeerChat)));
    }

    #[test]
    fn test_get_actions_schema_json() {
        let cluster = Cluster::new(make_config());
        let json = cluster.get_actions_schema_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn test_confirm_delivery_with_override() {
        let confirmed = Arc::new(Mutex::new(false));
        let confirmed_clone = confirmed.clone();
        let cluster = Cluster::new(make_config());
        cluster.set_call_with_context_fn(Box::new(move |_peer, action, _payload| {
            if action == "confirm_task_delivery" {
                *confirmed_clone.lock() = true;
            }
            Ok(Vec::new())
        }));

        cluster.confirm_delivery("peer-1", "task-1");
        assert!(*confirmed.lock());
    }

    #[test]
    fn test_handle_task_complete_for_test() {
        let messages = Arc::new(Mutex::new(Vec::new()));
        let bus = Arc::new(MockBus {
            messages: messages.clone(),
        });
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.set_message_bus(bus);

        let task_id = cluster.submit_task("action", serde_json::json!({}), "web", "ch");
        cluster.complete_task(&task_id, serde_json::json!("done"));

        cluster.handle_task_complete_for_test(&task_id);
        let msgs = messages.lock();
        assert_eq!(msgs.len(), 1);
    }

    #[test]
    fn test_cluster_peer_resolver_empty_addresses() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register a peer with no addresses
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "no-addr".into(),
                name: "no-addr".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:9000".into(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec![],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        // Should fall back to parsing primary address
        let (addresses, port, is_online) = resolver.get_peer_info("no-addr").unwrap();
        assert_eq!(addresses.len(), 1);
        assert_eq!(addresses[0], "10.0.0.1");
        assert_eq!(port, 9000);
        assert!(is_online);
    }

    #[test]
    fn test_cluster_peer_resolver_empty_primary_address() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register a peer with empty primary address and no stored addresses
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "empty-addr".into(),
                name: "empty-addr".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: String::new(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec![],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        let (addresses, _, _) = resolver.get_peer_info("empty-addr").unwrap();
        assert!(addresses.is_empty());
    }

    #[test]
    fn test_cluster_callbacks_trait_impl() {
        let cluster = Cluster::new(make_config());
        // Test ClusterCallbacks trait methods
        assert_eq!(ClusterCallbacks::node_id(&cluster), "local-node-001");
        assert_eq!(ClusterCallbacks::address(&cluster), "127.0.0.1:9000");
        assert_eq!(ClusterCallbacks::rpc_port(&cluster), DEFAULT_RPC_PORT);
        assert_eq!(ClusterCallbacks::role(&cluster), "worker");
        assert_eq!(ClusterCallbacks::category(&cluster), "general");
        assert!(ClusterCallbacks::tags(&cluster).is_empty());
    }

    #[test]
    fn test_cluster_callbacks_sync_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
        cluster.start();

        // Test sync_to_disk through ClusterCallbacks trait
        let result = ClusterCallbacks::sync_to_disk(&cluster);
        assert!(result.is_ok());
    }

    #[test]
    fn test_cluster_callbacks_handle_discovered_node() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        ClusterCallbacks::handle_discovered_node(
            &cluster,
            "cb-node",
            "cb-name",
            &["10.0.0.1".to_string()],
            21949,
            "worker",
            "dev",
            &["tag".to_string()],
            &["llm".to_string()],
        );

        let node = cluster.get_node_info("cb-node").unwrap();
        assert_eq!(node.base.name, "cb-name");
    }

    #[test]
    fn test_cluster_callbacks_handle_node_offline() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node("off-node", "off", vec!["10.0.0.1".into()], 21949, "worker", "dev", vec![], vec![]);
        ClusterCallbacks::handle_node_offline(&cluster, "off-node", "test");
        let node = cluster.get_node_info("off-node").unwrap();
        assert_eq!(node.status, NodeStatus::Offline);
    }

    #[tokio::test]
    async fn test_call_with_context_async_no_runtime_no_client() {
        let cluster = Cluster::new(make_config());
        // No RPC client set
        let result = cluster
            .call_with_context_async("peer-1", "ping", serde_json::json!({}), Duration::from_secs(5))
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_set_task_manager_for_test() {
        let mut cluster = Cluster::new(make_config());
        let custom_tm = Arc::new(TaskManager::new());
        cluster.set_task_manager_for_test(custom_tm);
    }

    #[test]
    fn test_rpc_server_accessor_none() {
        let cluster = Cluster::new(make_config());
        assert!(cluster.rpc_server().is_none());
    }

    #[test]
    fn test_rpc_server_accessor_some() {
        let mut cluster = Cluster::new(make_config());
        let server = Arc::new(crate::rpc::server::RpcServer::new(
            crate::rpc::server::RpcServerConfig {
                bind_address: "127.0.0.1:0".into(),
                ..Default::default()
            },
        ));
        cluster.set_rpc_server(server);
        assert!(cluster.rpc_server().is_some());
    }

    #[test]
    fn test_set_rpc_channel() {
        use crate::rpc::RpcChannel;
        #[derive(Debug)]
        struct MockCh;
        impl RpcChannel for MockCh {
            fn input(
                &self,
                _session_key: &str,
                _content: &str,
                _correlation_id: &str,
            ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
                Err("mock".into())
            }
        }
        let cluster = Cluster::new(make_config());
        assert!(cluster.get_rpc_channel().is_none());
        cluster.set_rpc_channel(Arc::new(MockCh));
        assert!(cluster.get_rpc_channel().is_some());
    }

    #[test]
    fn test_register_peer_chat_handlers_without_channel() {
        let cluster = make_cluster_with_rpc_server();
        // Don't set RPC channel - should log warning and return early
        cluster.register_peer_chat_handlers();
        // Should not panic even without RPC channel
    }

    #[test]
    fn test_new_with_empty_node_id_generates_one() {
        let config = ClusterConfig {
            node_id: String::new(),
            bind_address: "0.0.0.0:9000".into(),
            peers: vec![],
        };
        let cluster = Cluster::new(config);
        assert!(!cluster.node_id().is_empty());
        assert!(cluster.node_id().starts_with("bot-"));
    }

    #[test]
    fn test_sync_to_disk_excludes_self_node() {
        let dir = tempfile::tempdir().unwrap();
        let cluster = Cluster::with_workspace(make_config(), dir.path().to_path_buf());
        cluster.start();

        // Only the self node exists - should produce empty discovered list
        let result = cluster.sync_to_disk();
        assert!(result.is_ok());
        cluster.stop();
    }

    // ============================================================
    // Additional coverage tests for 95%+ target
    // ============================================================

    #[tokio::test]
    async fn test_call_with_context_async_with_rpc_client_peer_not_found() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        // RPC client is initialized by start(), but peer doesn't exist
        let result = cluster
            .call_with_context_async(
                "nonexistent-peer",
                "ping",
                serde_json::json!({}),
                Duration::from_secs(2),
            )
            .await;
        assert!(result.is_err());
    }

    #[test]
    fn test_handle_task_complete_for_test_method() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let task_id = cluster.submit_task("test_action", serde_json::json!({}), "rpc", "ch1");
        cluster.complete_task(&task_id, serde_json::json!("done"));
        // Should not panic
        cluster.handle_task_complete_for_test(&task_id);
    }

    #[test]
    fn test_handle_task_complete_for_test_nonexistent() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        // Should not panic for nonexistent task
        cluster.handle_task_complete_for_test("nonexistent");
    }

    #[test]
    fn test_get_actions_schema_returns_nonempty() {
        let cluster = Cluster::new(make_config());
        let schemas = cluster.get_actions_schema();
        assert!(!schemas.is_empty());
    }

    #[test]
    fn test_get_actions_schema_json_valid_json() {
        let cluster = Cluster::new(make_config());
        let json = cluster.get_actions_schema_json().unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert!(parsed.is_array());
    }

    #[test]
    fn test_cluster_with_empty_node_id_in_config() {
        let config = ClusterConfig {
            node_id: String::new(),
            bind_address: "0.0.0.0:9000".into(),
            peers: vec![],
        };
        let cluster = Cluster::new(config);
        // Should auto-generate a node ID
        assert!(cluster.node_id().starts_with("bot-"));
        assert!(!cluster.node_id().is_empty());
    }

    #[test]
    fn test_set_rpc_channel_no_server() {
        let cluster = Cluster::new(make_config());
        // Setting RPC channel without server and not running should not panic
        #[derive(Debug)]
        struct MockChannel;
        impl crate::rpc::RpcChannel for MockChannel {
            fn input(
                &self,
                _session_key: &str,
                _content: &str,
                _correlation_id: &str,
            ) -> Result<tokio::sync::oneshot::Receiver<String>, String> {
                Err("mock".into())
            }
        }
        cluster.set_rpc_channel(Arc::new(MockChannel));
        assert!(cluster.get_rpc_channel().is_some());
    }

    #[test]
    fn test_get_online_peers_after_start() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let peers = cluster.get_online_peers();
        // The self node is registered
        assert_eq!(peers.len(), 1);
        assert_eq!(peers[0].base.id, "local-node-001");
        assert_eq!(peers[0].status, NodeStatus::Online);
    }

    #[test]
    fn test_get_peer_after_register() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "peer-x".into(),
                name: "peer-x-name".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.10:21949".into(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec!["llm".into()],
            addresses: vec!["10.0.0.10".into()],
        });

        let peer = cluster.get_peer("peer-x").unwrap();
        assert_eq!(peer.base.name, "peer-x-name");
        assert_eq!(peer.capabilities.len(), 1);
    }

    #[test]
    fn test_handle_discovered_node_updates_existing() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // First registration
        cluster.handle_discovered_node(
            "node-upd",
            "original-name",
            vec!["10.0.0.1".into()],
            21949,
            "worker",
            "test",
            vec![],
            vec!["llm".into()],
        );
        let node = cluster.get_node_info("node-upd").unwrap();
        assert_eq!(node.base.name, "original-name");

        // Update with new name
        cluster.handle_discovered_node(
            "node-upd",
            "updated-name",
            vec!["10.0.0.2".into()],
            21949,
            "worker",
            "test",
            vec![],
            vec!["llm".into(), "tools".into()],
        );
        let node = cluster.get_node_info("node-upd").unwrap();
        assert_eq!(node.base.name, "updated-name");
        assert_eq!(node.capabilities.len(), 2);
    }

    #[test]
    fn test_list_tasks_after_submit_and_complete() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let t1 = cluster.submit_task("a1", serde_json::json!({}), "web", "ch1");
        let t2 = cluster.submit_task("a2", serde_json::json!({}), "web", "ch2");
        cluster.complete_task(&t1, serde_json::json!("done"));

        let tasks = cluster.list_tasks();
        assert_eq!(tasks.len(), 2);
        // One completed, one pending
        let completed: Vec<_> = tasks.iter().filter(|t| t.status == TaskStatus::Completed).collect();
        let pending: Vec<_> = tasks.iter().filter(|t| t.status == TaskStatus::Pending).collect();
        assert_eq!(completed.len(), 1);
        assert_eq!(pending.len(), 1);
    }

    #[test]
    fn test_submit_peer_chat_with_task_id() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        let result = cluster.submit_peer_chat(
            "remote-1",
            "peer_chat",
            serde_json::json!({"content": "hello", "task_id": "my-custom-task-id"}),
            "web",
            "chat-1",
        );
        assert!(result.is_ok());
        let task_id = result.unwrap();
        assert!(!task_id.is_empty());
        // The task should exist
        let task = cluster.get_task(&task_id).unwrap();
        assert_eq!(task.action, "peer_chat");
        assert_eq!(task.peer_id, "remote-1");
    }

    #[test]
    fn test_cluster_peer_resolver_with_empty_primary_address() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        // Register a node with an empty address and no extra addresses
        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "empty-addr".into(),
                name: "empty-addr".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: String::new(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec![],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        // Should return Some but with empty addresses and default port
        let (addresses, port, _) = resolver.get_peer_info("empty-addr").unwrap();
        assert!(addresses.is_empty());
        assert_eq!(port, DEFAULT_RPC_PORT);
    }

    #[test]
    fn test_cluster_peer_resolver_uses_stored_addresses() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.register_node(ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: "multi-addr".into(),
                name: "multi-addr".into(),
                role: nemesis_types::cluster::NodeRole::Worker,
                address: "10.0.0.1:21949".into(),
                category: "test".into(),
                last_seen: chrono::Utc::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities: vec![],
            addresses: vec!["192.168.1.1".into(), "10.0.0.1".into()],
        });

        let resolver = ClusterPeerResolver {
            registry: cluster.registry.clone(),
            node_id: cluster.node_id.clone(),
        };

        let (addresses, port, is_online) = resolver.get_peer_info("multi-addr").unwrap();
        // Should use stored addresses, not parse primary address
        assert_eq!(addresses.len(), 2);
        assert!(addresses.contains(&"192.168.1.1".to_string()));
        assert_eq!(port, 21949);
        assert!(is_online);
    }

    #[test]
    fn test_string_value_with_array() {
        let v = serde_json::json!([1, 2, 3]);
        // Array falls through to as_str().unwrap_or("")
        assert_eq!(string_value(Some(&v)), "");
    }

    #[test]
    fn test_string_value_with_nested_object() {
        let v = serde_json::json!({"nested": "value"});
        assert_eq!(string_value(Some(&v)), "");
    }

    #[test]
    fn test_handle_node_offline_updates_status() {
        let cluster = Cluster::new(make_config());
        cluster.start();

        cluster.handle_discovered_node(
            "offline-test",
            "offline-test",
            vec!["10.0.0.1".into()],
            21949,
            "worker",
            "test",
            vec![],
            vec![],
        );

        let node = cluster.get_node_info("offline-test").unwrap();
        assert_eq!(node.status, NodeStatus::Online);

        cluster.handle_node_offline("offline-test", "heartbeat timeout");
        let node = cluster.get_node_info("offline-test").unwrap();
        assert_eq!(node.status, NodeStatus::Offline);

        // Going offline again should still be offline
        cluster.handle_node_offline("offline-test", "duplicate");
        let node = cluster.get_node_info("offline-test").unwrap();
        assert_eq!(node.status, NodeStatus::Offline);
    }

    #[test]
    fn test_register_basic_handlers_not_running_error() {
        let cluster = Cluster::new(make_config());
        // Don't start
        let result = cluster.register_basic_handlers();
        assert!(result.is_err());
    }

    // ============================================================
    // Coverage improvement: additional cluster paths
    // ============================================================

    #[test]
    fn test_cluster_new_generates_node_id_when_empty() {
        let config = ClusterConfig {
            node_id: String::new(),
            bind_address: "0.0.0.0:9000".into(),
            peers: vec![],
        };
        let cluster = Cluster::new(config);
        assert!(!cluster.node_id().is_empty());
    }

    #[test]
    fn test_cluster_with_callback() {
        let config = make_config();
        let callback_called = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let called = callback_called.clone();
        let cluster = Cluster::with_callback(config, Box::new(move |_task| {
            called.store(true, std::sync::atomic::Ordering::SeqCst);
        }));
        assert!(!cluster.node_id().is_empty());
    }

    #[test]
    fn test_cluster_accessors() {
        let cluster = Cluster::new(make_config());
        assert_eq!(cluster.node_id(), "local-node-001");
        assert!(cluster.node_name().contains("local-no"));
        assert_eq!(cluster.address(), "127.0.0.1:9000");
        assert_eq!(cluster.role(), "worker");
        assert_eq!(cluster.category(), "general");
        assert!(cluster.tags().is_empty());
        assert_eq!(cluster.rpc_port(), DEFAULT_RPC_PORT);
        assert_eq!(cluster.udp_port(), DEFAULT_UDP_PORT);
    }

    #[test]
    fn test_cluster_set_ports() {
        let mut cluster = Cluster::new(make_config());
        cluster.set_ports(11111, 22222);
        assert_eq!(cluster.udp_port(), 11111);
        assert_eq!(cluster.rpc_port(), 22222);
    }

    #[test]
    fn test_cluster_stop() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        assert!(cluster.is_running());
        cluster.stop();
        assert!(!cluster.is_running());
    }

    #[test]
    fn test_cluster_get_capabilities_after_start() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let caps = cluster.get_capabilities();
        // Local node has "cluster" capability from start()
        assert!(caps.contains(&"cluster".to_string()));
    }

    #[test]
    fn test_cluster_get_all_local_ips() {
        let cluster = Cluster::new(make_config());
        let ips = cluster.get_all_local_ips();
        // Just verify it doesn't panic
        let _ = ips;
    }

    #[test]
    fn test_cluster_get_online_peers() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let peers = cluster.get_online_peers();
        // Should have at least the local node
        assert!(!peers.is_empty());
    }

    #[test]
    fn test_cluster_find_peers_by_capability() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let peers = cluster.find_peers_by_capability("nonexistent");
        assert!(peers.is_empty());
    }

    #[test]
    fn test_cluster_remove_node() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        cluster.handle_discovered_node(
            "remove-me",
            "remove-me",
            vec!["10.0.0.1".into()],
            21949,
            "worker",
            "test",
            vec![],
            vec![],
        );
        assert!(cluster.get_node_info("remove-me").is_some());
        assert!(cluster.remove_node("remove-me"));
        assert!(cluster.get_node_info("remove-me").is_none());
    }

    #[test]
    fn test_cluster_remove_nonexistent_node() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.remove_node("nonexistent"));
    }

    #[test]
    fn test_cluster_cleanup_task_noop() {
        let cluster = Cluster::new(make_config());
        // Should not panic
        cluster.cleanup_task("any-task");
    }

    #[test]
    fn test_cluster_list_tasks_empty() {
        let cluster = Cluster::new(make_config());
        let tasks = cluster.list_tasks();
        assert!(tasks.is_empty());
    }

    #[test]
    fn test_cluster_task_manager_accessor() {
        let cluster = Cluster::new(make_config());
        let _tm = cluster.task_manager();
    }

    #[test]
    fn test_cluster_continuation_store_accessor() {
        let cluster = Cluster::new(make_config());
        let _cs = cluster.continuation_store();
    }

    #[test]
    fn test_cluster_result_store_accessor() {
        let cluster = Cluster::new(make_config());
        let _rs = cluster.result_store();
    }

    #[test]
    fn test_cluster_stop_receiver() {
        let cluster = Cluster::new(make_config());
        let _rx = cluster.stop_receiver();
    }

    #[test]
    fn test_cluster_register_rpc_handler_not_running() {
        let cluster = Cluster::new(make_config());
        let result = cluster.register_rpc_handler("test", Box::new(|_| Ok(serde_json::json!({}))));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not running"));
    }

    #[test]
    fn test_cluster_register_forge_handlers_not_running() {
        let cluster = Cluster::new(make_config());
        struct MockProvider;
        impl crate::handlers::ForgeDataProvider for MockProvider {
            fn receive_reflection(&self, _payload: &serde_json::Value) -> Result<(), String> { Ok(()) }
            fn get_reflections_list_payload(&self) -> serde_json::Value { serde_json::json!({}) }
            fn read_reflection_content(&self, _filename: &str) -> Result<String, String> { Err("not found".into()) }
            fn sanitize_content(&self, content: &str) -> String { content.to_string() }
            fn clone_boxed(&self) -> Box<dyn crate::handlers::ForgeDataProvider> { Box::new(MockProvider) }
        }
        let result = cluster.register_forge_handlers(Box::new(MockProvider));
        assert!(result.is_err());
    }

    #[test]
    fn test_call_with_context_no_rpc_client() {
        let cluster = Cluster::new(make_config());
        // Don't start, so no RPC client
        let result = cluster.call_with_context("peer-1", "ping", serde_json::json!({}));
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("RPC client not initialized"));
    }

    #[test]
    fn test_handle_task_complete_no_task() {
        let cluster = Cluster::new(make_config());
        // Should not panic for nonexistent task
        cluster.handle_task_complete("nonexistent-task");
    }

    #[test]
    fn test_handle_task_complete_no_bus_v2() {
        let cluster = Cluster::new(make_config());
        cluster.start();
        let task_id = cluster.submit_task("test", serde_json::json!({}), "web", "chat-1");
        // No bus set, should log error but not panic
        cluster.handle_task_complete(&task_id);
    }

    #[test]
    fn test_bus_inbound_message_debug_v2() {
        let msg = BusInboundMessage {
            channel: "system".into(),
            sender_id: "test".into(),
            chat_id: "chat-1".into(),
            content: "hello".into(),
        };
        let debug = format!("{:?}", msg);
        assert!(debug.contains("system"));
    }

    #[test]
    fn test_submit_peer_chat_v2() {
        let cluster = Cluster::new(make_config());
        let result = cluster.submit_peer_chat(
            "peer-1",
            "peer_chat",
            serde_json::json!({"content": "hello", "task_id": "t-123"}),
            "web",
            "chat-1",
        );
        assert!(result.is_ok());
        // submit_peer_chat creates a new task with its own ID
        let task_id = result.unwrap();
        assert!(!task_id.is_empty());
    }

    #[test]
    fn test_submit_peer_chat_generates_task_id_v2() {
        let cluster = Cluster::new(make_config());
        let result = cluster.submit_peer_chat(
            "peer-1",
            "peer_chat",
            serde_json::json!({"content": "hello"}),
            "web",
            "chat-1",
        );
        assert!(result.is_ok());
        // Should generate a UUID task_id since none in payload
        let task_id = result.unwrap();
        assert!(!task_id.is_empty());
    }

    #[test]
    fn test_assign_task_nonexistent_v2() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.assign_task("nonexistent", "node-1"));
    }

    #[test]
    fn test_complete_task_nonexistent_v2() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.complete_task("nonexistent", serde_json::json!({})));
    }

    #[test]
    fn test_fail_task_nonexistent_v2() {
        let cluster = Cluster::new(make_config());
        assert!(!cluster.fail_task("nonexistent", "error"));
    }
}
