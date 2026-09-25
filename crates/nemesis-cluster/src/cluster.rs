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

use crate::cluster_config::{DynamicState, PeerConfig, PeerStatus};
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
    /// G5: 结构化元数据（如 cluster_continuation 的 status / source_node /
    /// error）。`BusToClusterAdapter` 原样映射到 `InboundMessage.metadata`，
    /// 供 AgentLoop 的续行拦截逻辑读取。空 = 无元数据。
    pub metadata: std::collections::HashMap<String, String>,
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
    /// 批次四：显示名为显式配置（config.cluster.json `node_name` 或
    /// peers.toml [node].name）时 = true——撞名后缀免疫（用户意志优先；
    /// 自动名 hostname / `Bot {id8}` 才允许撞名收敛）。
    node_name_locked: std::sync::atomic::AtomicBool,
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
    /// P0 vault fail-closed：config.cluster.json 的 `token` 若是 vault:/env:/yaml:
    /// 引用且解析失败，置位。RPC 绑定点据此拒绝启动（宁可没有 RPC，不可
    /// 无认证 RPC）；start_discovery 据此拒绝无加密发现。空串/字面量/读文件
    /// 失败不置位（维持既有语义）。
    rpc_reference_broken: AtomicBool,
    discovery: Mutex<Option<crate::discovery::DiscoveryService>>,
    stop_tx: broadcast::Sender<()>,
    /// G5: Arc 包一层 —— 恢复循环 spawn 时克隆 Arc，之后每 tick 重新
    /// `.lock().clone()` 读取，兼容 `set_message_bus` 在 `start()` 之后才
    /// 被调用的装配顺序（gateway 生产路径即如此）。
    bus: Arc<Mutex<Option<Arc<dyn MessageBus>>>>,
    /// Blacklisted peer IDs — removed nodes that should not be re-discovered.
    removed_peers: parking_lot::RwLock<std::collections::HashSet<String>>,

    // -- Cluster Agent --
    cluster_task_list: Mutex<Option<Arc<crate::cluster_task::ClusterTaskList>>>,
    cluster_work_queue: Mutex<Option<Arc<crate::cluster_task::ClusterWorkQueue>>>,

    // -- Testing override for CallWithContext --
    call_with_context_fn: Mutex<
        Option<Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>>,
    >,

    /// 节点发现回调（Swarm M2 落点）：对端节点被发现/刷新时触发
    /// `(node_id, role, category)`。gateway 用它把新节点自动收编进看板
    /// 频道；first-join 语义（成员零行才入）由闭包实现方裁决——cluster
    /// 不依赖 board。未注册时为零开销 no-op。
    ///
    /// Arc 包裹：G2 探针循环在 spawn 出去的任务里运行（不持 &self），
    /// 必须能活读这个槽——探针复活也是「节点发现」（见
    /// `fire_recovered_callback`）。
    on_node_discovered: Arc<Mutex<Option<Arc<dyn Fn(&str, &str, &str) + Send + Sync>>>>,

    /// CD3（2026-09-17）：恢复交付回调。恢复轮询（poll_stale_pending_tasks）
    /// 从 worker 查回任务结果后、向 worker 发 confirm（删除其本地副本）之前
    /// 触发；返回 true = 交付完成（可 confirm 删副本），false = 交付失败
    /// （跳过 confirm，worker 端副本走 7 天 TTL 兜底——宁留勿丢）。
    ///
    /// gateway 注入的闭包内部路由：issue_dispatch 反查命中 → 看板写回
    /// （write_back_board_dispatch）；未命中（chat 任务）→ 交付 = 已发 bus
    /// 续行帧。路由判断留在 gateway，cluster 不依赖 board。未注册 = 旧行为
    /// （无条件 confirm）。参数 `(task_id, status, response, error)`，
    /// error 为 Some 表示 result_status=error。
    on_recovered_delivery:
        Arc<Mutex<Option<Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>>>>,
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

        // 批次四：显示名解析链（config 显式 → hostname → `Bot {id8}` 兜底）。
        let (node_name_init, name_locked) = resolve_node_name_from_env(&config.node_name, &node_id);

        Self {
            node_id: node_id.clone(),
            node_name: parking_lot::RwLock::new(node_name_init),
            node_name_locked: std::sync::atomic::AtomicBool::new(name_locked),
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
            rpc_reference_broken: AtomicBool::new(false),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Arc::new(Mutex::new(None)),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
            on_node_discovered: Arc::new(Mutex::new(None)),
            on_recovered_delivery: Arc::new(Mutex::new(None)),
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
        // 路径唯一真相源 = nemesis-path（peers.toml 跨 crate 读写：本 crate /
        // nemesis-web / CLI 三方共用，禁止各自 join）。
        let cluster_dir = nemesis_path::cluster_dir_in_workspace(&workspace);
        let (stop_tx, _) = broadcast::channel(1);

        let node_id = if config.node_id.is_empty() {
            generate_node_id()
        } else {
            config.node_id.clone()
        };

        // Try to load existing node identity from static config
        let peers_path = cluster_dir.join("peers.toml");
        let sc = crate::cluster_config::load_static_config(&peers_path).ok();
        let node_id = sc
            .as_ref()
            .and_then(|s| {
                if s.node.id.is_empty() {
                    None
                } else {
                    Some(s.node.id.clone())
                }
            })
            .unwrap_or(node_id);

        // Persist runtime-generated node_id to peers.toml [node].id so it
        // remains stable across restarts. No-op if user has already set one.
        if let Err(e) = crate::cluster_config::ensure_node_id(&peers_path, &node_id) {
            tracing::warn!("[Cluster] Failed to persist node_id to peers.toml: {}", e);
        }
        // 批次四：显示名解析链。peers.toml [node].name（既有显式持久化位）
        // 最优先且免疫撞名后缀；其次 config.cluster.json `node_name`（同样
        // 显式免疫）；否则自动链 hostname → `Bot {id8}` 兜底（允许撞名收敛）。
        let (node_name_default, name_locked) = sc
            .as_ref()
            .and_then(|s| {
                if s.node.name.is_empty() {
                    None
                } else {
                    Some((s.node.name.clone(), true))
                }
            })
            .unwrap_or_else(|| resolve_node_name_from_env(&config.node_name, &node_id));
        let role_default = sc
            .as_ref()
            .map(|s| s.node.role.clone())
            .unwrap_or_else(|| "worker".into());
        let category_default = sc
            .as_ref()
            .map(|s| s.node.category.clone())
            .unwrap_or_else(|| "general".into());
        let tags_default = sc.as_ref().map(|s| s.node.tags.clone()).unwrap_or_default();

        Self {
            node_id: node_id.clone(),
            node_name: parking_lot::RwLock::new(node_name_default),
            node_name_locked: std::sync::atomic::AtomicBool::new(name_locked),
            node_type: "agent".into(),
            address: config.bind_address.clone(),
            role: parking_lot::RwLock::new(role_default),
            category: parking_lot::RwLock::new(category_default),
            tags: parking_lot::RwLock::new(tags_default),
            capabilities: Arc::new(std::sync::Mutex::new(Vec::new())),
            workspace: workspace.clone(),
            static_config_path: nemesis_path::resolve_cluster_peers_path_in_workspace(&workspace),
            dynamic_state_path: nemesis_path::resolve_cluster_state_path_in_workspace(&workspace),
            registry: Arc::new(PeerRegistry::new(HealthConfig::default())),
            task_manager: Arc::new(TaskManager::new()),
            cont_store: Arc::new(ContinuationStore::new(
                nemesis_path::resolve_cluster_rpc_cache_dir_in_workspace(&workspace),
            )),
            // G1: 磁盘持久化 —— B 端回调失败时结果落盘（rpc_cache/results/），
            // A 端重启恢复的 poll（query_task_result）才有真实数据可查。
            // 构造期同步建目录；启动后的清扫+恢复在 ClusterService::first_start。
            result_store: Arc::new(TaskResultStore::with_disk_persistence(
                1000,
                nemesis_path::resolve_cluster_results_dir_in_workspace(&workspace),
            )),
            rpc_client: Mutex::new(None),
            rpc_server: None,
            rpc_channel: RwLock::new(None),
            udp_port: DEFAULT_UDP_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
            running: RwLock::new(false),
            discovery_running: Arc::new(AtomicBool::new(false)),
            rpc_reference_broken: AtomicBool::new(false),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Arc::new(Mutex::new(None)),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
            on_node_discovered: Arc::new(Mutex::new(None)),
            on_recovered_delivery: Arc::new(Mutex::new(None)),
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
        let mut local_caps = self
            .capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        if !local_caps.iter().any(|c| c.eq_ignore_ascii_case("cluster")) {
            local_caps.push("cluster".into());
        }

        // Resolve 0.0.0.0 to an actual IP for node registration.
        // TCP bind still uses 0.0.0.0 to listen on all interfaces.
        let display_address = if self.address.starts_with("0.0.0.0") {
            let port = self.address.rsplit(':').next().unwrap_or("");
            let ips = network::get_all_local_ips();
            let ip = ips
                .iter()
                .find(|ip| !ip.starts_with("127."))
                .or_else(|| ips.first())
                .map(|s| s.as_str())
                .unwrap_or("127.0.0.1");
            format!("{}:{}", ip, port)
        } else {
            self.address.clone()
        };

        let role_str = self.role.read().clone();
        let role = nemesis_types::cluster::NodeRole::from_role_str(&role_str);
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
            tags: self.tags.read().clone(),
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

        // G4: 超时阶梯 —— pending 任务安全网 = max(24h, 2×B端LLM超时)，
        // 保证安全网永远在 B 端 LLM 处理超时（llm_timeout_secs）之后才触发，
        // 不会把仍在正常处理中的任务提前判死。
        let app_cfg = crate::config_loader::load_app_config(&self.workspace);
        let effective_llm_timeout = if app_cfg.llm_timeout_secs == 0 {
            // 0 = 不设超时：按 peer_chat_handler 的协议默认（2h）推阶梯
            crate::rpc::peer_chat_handler::DEFAULT_LLM_TIMEOUT
        } else {
            Duration::from_secs(app_cfg.llm_timeout_secs)
        };
        let safety_net = stale_task_safety_net(app_cfg.llm_timeout_secs);
        self.task_manager.set_pending_timeout(safety_net);
        if effective_llm_timeout >= Duration::from_secs(24 * 3600) {
            tracing::warn!(
                llm_timeout_secs = app_cfg.llm_timeout_secs,
                safety_net_secs = safety_net.num_seconds(),
                "[Cluster] B-side LLM timeout >= 24h: the pending-task safety net is now larger than the result-store TTL (7d at 2x) — timed-out tasks may lose their results"
            );
        }

        // G2: 主动健康探针循环（health_check_interval_secs=0 关闭）。
        if app_cfg.health_check_interval_secs > 0 {
            self.start_health_check_loop(
                Duration::from_secs(app_cfg.health_check_interval_secs),
                app_cfg.health_check_failure_threshold,
            );
        }

        // state.toml 回读（集群完备性加固 2026-09-11）：必须在 start_sync_loop
        // 之前——sync_loop 的 tokio interval 首 tick 即时触发，先启动会把磁盘
        // discovered 历史（上次运行的已知节点）以「只有本节点」的注册表覆盖掉。
        self.restore_discovered_from_state();

        // Start the recovery loop
        self.start_recovery_loop();

        // Start the sync loop (periodic node timeout check + disk persistence)
        self.start_sync_loop();

        logger::log_lifecycle(
            "start",
            &self.node_id,
            &format!("rpc_port={}", self.rpc_port),
        );
    }

    /// state.toml 回读（集群完备性加固 2026-09-11）：sync_loop 周期把注册表
    /// 持久化到 state.toml，但重启后从未回读——discovered 历史在首个 sync
    /// tick 就被「只有本节点」的注册表覆盖（write-only 假持久化）。本函数把
    /// 磁盘 discovered 条目以 **Offline** 状态种回注册表（G2 探针 1/5 降频
    /// 或 announce 到达时复活；**绝不因回读直接 Online**——诚实语义：存活
    /// 未知）。跳过：空 id、本节点、黑名单节点（removed_peers 不复活）、
    /// 已有条目（静态 peers 在 start() 前装载，不覆盖）、不可拨号地址
    /// （空/无 host:port 形态，与 G14 的 rpc_port=0 同语义）、**同地址或
    /// 同名的既有条目**（2026-09-12 T13 根修：注册表里已有该物理节点时再
    /// 种一份会造出「同节点双条目」—— RpcClient 直接键命中离线影子就拒绝
    /// 拨号，遮蔽同地址的运行时 id 在线条目）。返回种入数。
    fn restore_discovered_from_state(&self) -> usize {
        let state = match crate::cluster_config::load_dynamic_state(&self.dynamic_state_path) {
            Ok(s) => s,
            Err(e) => {
                tracing::warn!(
                    path = %self.dynamic_state_path.display(),
                    error = %e,
                    "[Cluster] state.toml 回读失败（忽略，按空注册表启动）"
                );
                return 0;
            }
        };
        let mut seeded = 0usize;
        for pc in state.discovered {
            if pc.id.trim().is_empty() || pc.id == self.node_id {
                continue;
            }
            if self.removed_peers.read().contains(&pc.id) {
                continue; // 用户显式移除的节点不复活
            }
            if self.registry.get(&pc.id).is_some() {
                continue; // 静态 peers / 已有条目：原值保留
            }
            let addr = pc.address.trim();
            if addr.is_empty() || !addr.contains(':') {
                continue; // 不可拨号 → 不种（state.toml 不存 unusable 条目）
            }
            // 同地址 / 同名既有条目 → 同一物理节点已在注册表（典型：静态
            // peer 按运行时 id 键控，state.toml 还留着升级前的占位名条目，
            // 如 id="Node-A"）。再种一份 = RpcClient 直接键命中离线影子拒绝
            // 拨号（UAT T13 实证：D 重启后 D→A 全部 duration_ms=0 失败）。
            // 跳过，让既有条目承担探针/announce 复活。
            let name = pc.name.trim();
            let shadows_existing = self.registry.find_by_address(addr).is_some()
                || (!name.is_empty()
                    && self
                        .registry
                        .list_peers()
                        .iter()
                        .any(|p| p.base.name == name));
            if shadows_existing {
                tracing::debug!(
                    id = %pc.id,
                    name = %name,
                    address = %addr,
                    "[Cluster] state.toml 回读跳过：同地址/同名条目已在注册表（防同节点双条目）"
                );
                continue;
            }
            self.registry.upsert(ExtendedNodeInfo {
                base: nemesis_types::cluster::NodeInfo {
                    id: pc.id.clone(),
                    name: if pc.name.trim().is_empty() {
                        pc.id.clone()
                    } else {
                        pc.name.clone()
                    },
                    role: nemesis_types::cluster::NodeRole::from_role_str(&pc.role),
                    address: addr.to_string(),
                    category: pc.category.clone(),
                    last_seen: if pc.status.last_seen.is_empty() {
                        chrono::Local::now().to_rfc3339()
                    } else {
                        pc.status.last_seen.clone()
                    },
                },
                status: NodeStatus::Offline,
                capabilities: Vec::new(),
                tags: pc.tags.clone(),
                addresses: pc.addresses.clone(),
                node_type: String::new(),
            });
            seeded += 1;
        }
        if seeded > 0 {
            logger::log_discovery_info(&format!(
                "state.toml restored: seeded {seeded} known peers (Offline, pending probe/announce)"
            ));
            tracing::info!(
                "[Cluster] state.toml 回读：种入 {seeded} 个已知节点（Offline，待探针/announce 复活）"
            );
        }
        seeded
    }

    /// Load the RPC auth token from `workspace/config/config.cluster.json`
    /// and apply it to the RPC server and client.
    ///
    /// This is called automatically during `start()` so that token auth
    /// works without any manual wiring in gateway.rs or cluster node.
    fn load_rpc_auth_token(&self) {
        // 委托 nemesis-path 唯一拼接点。
        let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.workspace);
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
            Ok(v) => v
                .get("token")
                .and_then(|t| t.as_str())
                .unwrap_or("")
                .to_string(),
            Err(e) => {
                tracing::warn!(path = %cfg_path.display(), error = %e, "[Cluster] Failed to parse cluster config for token");
                return;
            }
        };

        if token.is_empty() {
            tracing::info!("[Cluster] No RPC auth token configured — running without auth");
            return;
        }

        // P0 vault（B3，2026-09-22 计划）：token 支持 vault:/env:/yaml: 引用。
        // 解析失败 = fail-closed：置位 rpc_reference_broken，RPC 绑定点据此
        // 拒绝启动（宁可没有 RPC，不可无认证 RPC）。日志带补救指引。
        let token = match nemesis_config::resolve_secret_field(&token, "config.cluster.json token")
        {
            Ok(t) => t,
            Err(e) => {
                self.rpc_reference_broken.store(true, Ordering::SeqCst);
                tracing::error!(
                    "[Cluster] RPC auth token 引用解析失败: {e} —— fail-closed：本次 RPC 服务不启动（请修复引用或运行 `nemesisbot vault set <alias>`）"
                );
                return;
            }
        };

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

        // P0 vault fail-closed：token 引用解析失败（load_discovery_secret 或
        // 先前的 load_rpc_auth_token 置位）→ 拒绝无加密发现，宁可不发现。
        if self.rpc_reference_broken.load(Ordering::SeqCst) {
            tracing::error!(
                "[Cluster] token 引用解析失败 —— fail-closed：UDP discovery 不启动（拒绝无加密运行；请修复引用或运行 `nemesisbot vault set <alias>`）"
            );
            return;
        }

        // G3: 从 config.cluster.json 读取可配置 announce 过期阈值
        // （announce_expiry_secs，≤0 = 关闭过期丢弃）。
        let app_cfg = crate::config_loader::load_app_config(&self.workspace);
        let mut discovery_config = crate::discovery::DiscoveryConfig::with_encryption(
            self.udp_port,
            self.broadcast_interval,
            &secret,
        );
        discovery_config.announce_expiry_secs = app_cfg.announce_expiry_secs;

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
        // 委托 nemesis-path 唯一拼接点。
        let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.workspace);
        if !cfg_path.exists() {
            return String::new();
        }

        let raw = match std::fs::read_to_string(&cfg_path) {
            Ok(data) => serde_json::from_str::<serde_json::Value>(&data)
                .ok()
                .and_then(|v| v.get("token").and_then(|t| t.as_str()).map(String::from))
                .unwrap_or_default(),
            Err(_) => String::new(),
        };
        if raw.is_empty() {
            return raw;
        }
        // P0 vault（B3）：discovery 加密密钥与 RPC token 同字段，同链路解析。
        // 解析失败 = fail-closed：置位 rpc_reference_broken（start_discovery
        // 据此整体不启动），绝不降级为无加密发现。
        match nemesis_config::resolve_secret_field(&raw, "config.cluster.json token (discovery)") {
            Ok(t) => t,
            Err(e) => {
                self.rpc_reference_broken.store(true, Ordering::SeqCst);
                tracing::error!(
                    "[Cluster] discovery 加密密钥引用解析失败: {e} —— fail-closed：发现层不启动（请修复引用或运行 `nemesisbot vault set <alias>`）"
                );
                String::new()
            }
        }
    }

    /// P0 vault fail-closed：config.cluster.json 的 `token` 是否为解析失败的
    /// 引用。RPC 绑定点（gateway / `commands/cluster.rs`）在 `start()` 之后
    /// 检查此项——为 true 时拒绝 bind，节点以"无 RPC 服务"状态运行而非
    /// 裸奔。空串/字面量 token 恒为 false（不改变既有语义）。
    pub fn rpc_reference_broken(&self) -> bool {
        self.rpc_reference_broken.load(Ordering::SeqCst)
    }

    /// Stop the cluster. Stops discovery (joins threads) and signals shutdown.
    pub fn stop(&self) {
        *self.running.write() = false;

        // Stop RPC server first — reject new connections, existing connections
        // drain naturally via idle_timeout.
        if let Some(ref server) = self.rpc_server
            && let Err(e) = server.stop()
        {
            tracing::warn!(error = %e, "[Cluster] RPC server stop error");
        }

        // Stop discovery service (joins broadcast + receive threads)
        self.discovery_running.store(false, Ordering::SeqCst);
        if let Some(discovery) = self.discovery.lock().take()
            && let Err(e) = discovery.stop()
        {
            tracing::warn!(error = %e, "[Cluster] Discovery stop error");
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
        let bus = self.bus.clone();
        // CD3：恢复交付回调槽与 bus 同款——每 tick 快照读取，兼容 set 在
        // start() 之后的装配顺序。
        let on_recovered_delivery = self.on_recovered_delivery.clone();
        let safety_net = self.task_manager.pending_timeout();

        handle.spawn(async move {
            let mut interval = tokio::time::interval(Duration::from_secs(120));
            // P6b：首 tick 即时（tokio interval 语义），且绕过 2 分钟年龄闸
            // ——重启后立即发起恢复查询，不用白等一个周期。后续 tick 恢复
            // 常规年龄闸（避免把刚派发的任务误查）。
            let mut first_tick = true;
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = interval.tick() => {
                        // G5: 每 tick 重新读 bus —— set_message_bus 可能在
                        // start() 之后才被调用（gateway 装配顺序）。
                        let bus_snapshot: Option<Arc<dyn MessageBus>> = bus.lock().clone();
                        // CD3: 交付回调同款每 tick 快照。
                        let delivery_cb = on_recovered_delivery.lock().clone();
                        poll_stale_pending_tasks(
                            &task_manager,
                            &call_fn,
                            rpc_client.as_deref(),
                            safety_net,
                            bus_snapshot.as_deref(),
                            first_tick,
                            delivery_cb.as_ref(),
                        ).await;
                        first_tick = false;
                    }
                }
            }
        });
    }

    /// G2: 主动健康探针循环。
    ///
    /// 每个 tick 对所有 **Online** 对端发 frame 级 ping（5s 超时）；**Offline**
    /// 对端每 5 个 tick 探一次（自愈检测——一次成功立即翻回 Online）。连续
    /// `failure_threshold` 次失败把 Online 节点翻成 Offline。
    ///
    /// 探针走 AEAD 加密的 RPC 通道：**"TCP 通但握手/解密失败"同样计为失败**
    /// —— 探的是可通信性而非端口存活。跳过本节点；`interval=0` 不进入本循环
    /// （调用方 `start()` 已判断）。
    /// 探针 / 手动 ping 复活对端时触发节点发现回调（单一真相源，两个调用方：
    /// G2 探针循环任务 + `mark_peer_healthy`）。
    ///
    /// 广播被隔离的部署形态（多宿主主机跨网段、AP 隔离）里 announce 永远
    /// 不来，Offline→Online 的探针翻转是唯一的「节点上线」信号——停车场
    /// sweep / 看板收编必须同样感知，否则静态 peer 部署下停车场永不复活。
    /// 幂等由调用方转移门保证（`record_probe_success` 只在翻转时返回 true；
    /// `mark_peer_healthy` 自查翻转前状态）+ gateway 侧 park_sweep_gate 节流。
    fn fire_recovered_callback(
        on_discovered: &Mutex<Option<Arc<dyn Fn(&str, &str, &str) + Send + Sync>>>,
        registry: &PeerRegistry,
        node_id: &str,
    ) {
        if let Some(cb) = on_discovered.lock().clone()
            && let Some(info) = registry.get(node_id)
        {
            cb(node_id, info.base.role.as_role_str(), &info.base.category);
        }
    }

    fn start_health_check_loop(&self, interval: Duration, failure_threshold: u32) {
        let handle = match tokio::runtime::Handle::try_current() {
            Ok(h) => h,
            Err(_) => return, // No runtime available (e.g. in unit tests)
        };

        let mut stop_rx = self.stop_tx.subscribe();
        let registry = self.registry.clone();
        let rpc_client = self.rpc_client.lock().clone();
        let local_node_id = self.node_id.clone();
        // Arc 克隆进任务：探针复活时活读回调槽（gateway 组装晚于 start() 也能读到）
        let on_discovered = self.on_node_discovered.clone();

        handle.spawn(async move {
            // 错过 tick 用 Delay 追赶语义（不补帧），避免卡顿后连发风暴
            let mut tick = tokio::time::interval(interval);
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            let mut tick_count: u64 = 0;
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => {
                        return;
                    }
                    _ = tick.tick() => {
                        tick_count = tick_count.wrapping_add(1);

                        let peers = registry.list_peers();
                        for peer in peers {
                            // 跳过本节点（自 ping 无意义）
                            if peer.base.id == local_node_id {
                                continue;
                            }
                            let online = peer.status == NodeStatus::Online;
                            if !should_probe_peer(online, tick_count) {
                                continue;
                            }

                            let client = match rpc_client.as_ref() {
                                Some(c) => c.clone(),
                                None => return, // RPC client never initialized
                            };
                            let node_id = peer.base.id.clone();
                            let request = crate::rpc_types::RPCRequest {
                                id: uuid::Uuid::new_v4().to_string(),
                                action: crate::rpc_types::ActionType::Custom(
                                    "ping".to_string(),
                                ),
                                payload: serde_json::json!({}),
                                source: local_node_id.clone(),
                                target: Some(node_id.clone()),
                            };
                            // 5s 探针超时：正常 LAN RTT <1ms，5s 足够宽容
                            let result = client
                                .call_probe_with_timeout(&node_id, request, Duration::from_secs(5))
                                .await;

                            match result {
                                Ok(resp) => {
                                    // G2: 交叉校验对端响应 timestamp，漂移过大限频 WARN
                                    // （复用 discovery 的 AnnounceWarnGate，10 分钟冷却）
                                    if let Some(ts) = resp
                                        .result
                                        .as_ref()
                                        .and_then(|r| r.get("timestamp"))
                                        .and_then(|t| t.as_i64())
                                    {
                                        let drift = (chrono::Utc::now().timestamp() - ts).abs();
                                        if drift > crate::discovery::DRIFT_WARN_THRESHOLD_SECS
                                            && probe_drift_gate().admit(&node_id)
                                        {
                                            tracing::warn!(
                                                node_id = %node_id,
                                                drift_secs = drift,
                                                "[Cluster] Health probe response timestamp drift (peer clock not synced?)"
                                            );
                                        }
                                    }
                                    if registry.record_probe_success(&node_id) {
                                        tracing::info!(
                                            node_id = %node_id,
                                            "[Cluster] Health probe: peer recovered -> Online"
                                        );
                                        logger::log_discovery_info(
                                            "health probe recovered, marked Online",
                                        );
                                        // 复活也是「节点发现」：广播被隔离的拓扑（多宿主
                                        // 主机跨网段、AP 隔离）里 announce 永远不来，探针
                                        // 翻转是唯一的 Offline→Online 信号源——停车场
                                        // sweep / 看板收编必须同样感知（T1 真机缺陷修复）。
                                        Self::fire_recovered_callback(
                                            &on_discovered,
                                            &registry,
                                            &node_id,
                                        );
                                    }
                                }
                                Err(e) => {
                                    if registry.record_probe_failure(&node_id, failure_threshold) {
                                        tracing::warn!(
                                            node_id = %node_id,
                                            error = %e,
                                            threshold = failure_threshold,
                                            "[Cluster] Health probe: peer marked Offline after consecutive failures"
                                        );
                                        logger::log_discovery_info(&format!(
                                            "health probe failed {}x, marked Offline",
                                            failure_threshold
                                        ));
                                    }
                                }
                            }
                        }
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
                        let state_path =
                            nemesis_path::resolve_cluster_state_path_in_workspace(&workspace);
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

    /// Register the node-discovered callback (Swarm M2). Called once at
    /// gateway assembly; the closure receives `(node_id, role, category)` on
    /// every non-blacklisted discovery/refresh event.
    pub fn set_on_node_discovered(&self, cb: Arc<dyn Fn(&str, &str, &str) + Send + Sync>) {
        *self.on_node_discovered.lock() = Some(cb);
    }

    /// CD3（2026-09-17）：注册恢复交付回调（语义见字段 doc）。gateway 在
    /// board+cluster 构建下装配时注入；未注入 = 恢复腿保持旧行为（无条件
    /// confirm）。
    pub fn set_on_recovered_delivery(
        &self,
        cb: Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>,
    ) {
        *self.on_recovered_delivery.lock() = Some(cb);
    }

    /// Handle a discovered node (from UDP broadcast or manual config).
    pub fn handle_discovered_node(
        &self,
        node_id: &str,
        name: &str,
        addresses: Vec<String>,
        rpc_port: u16,
        role: &str,
        category: &str,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: &str,
    ) -> bool {
        // Skip blacklisted nodes
        if self.removed_peers.read().contains(node_id) {
            return false;
        }

        // 批次四：撞名后缀收敛（goal 钉死，确定性规则无需协商协议）。
        // 对方 announce 的显示名与我本地名相同且 id 不同 → 本地名追加
        // `-` + 自己 node_id 前 4 位（走 set_node_name 唯一写入点，下一拍
        // announce 自然携带新名；对端同理收敛，双方各自算出同一结论）。
        // 规则只作用于**自动名**（hostname / `Bot {id8}`）——config /
        // peers.toml 显式名免疫（用户意志优先，重名无害，唯一性靠 node_id）。
        if !name.is_empty()
            && name == self.node_name()
            && node_id != self.node_id
            && !self
                .node_name_locked
                .load(std::sync::atomic::Ordering::Relaxed)
        {
            let suffixed = format!(
                "{}-{}",
                self.node_name(),
                name_suffix_from_node_id(&self.node_id)
            );
            tracing::info!(
                "[Cluster] 撞名收敛：显示名「{name}」与节点 {node_id} 重名，本地名改为「{suffixed}」"
            );
            self.set_node_name(suffixed);
        }

        // G14（2026-09-09 双机混跑）：rpc_port=0 的 announce 造不出可用
        // 记录（ip:0 永不可达）。正常路径不会出现 0——静态 peers 装载按
        // udp+10000 派生恒 >0，新版节点 announce 恒带真实 rpc_port；0 只
        // 来自异版本/畸形 announce。未知节点不登记（state.toml 不再存
        // unusable 条目），已知节点不覆盖（静态条目原值保留）。异版本
        // 混合集群靠静态 peers 显式登记，不经 UDP 发现。
        if rpc_port == 0 {
            tracing::debug!(
                "[Cluster] Ignoring announce without rpc_port (legacy/malformed): {} at {:?}",
                node_id,
                addresses
            );
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
                // announce 携带对端自报 role（真相源=对端 peers.toml [node].role）；
                // 曾硬编码 Worker 导致 board.sync 的 coordinator 判定在纯 UDP
                // 发现拓扑下永远失败（补拉死循环），必须走 from_role_str 解析。
                role: nemesis_types::cluster::NodeRole::from_role_str(role),
                address: primary_address.clone(),
                category: category.into(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities,
            tags: tags.clone(),
            // Preserve all addresses for multi-address failover。clone：closure
            // （占位全量比对）与下方 RealNodeInfo 升级构造仍要用 addresses。
            addresses: addresses.clone(),
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
            // Collect placeholder (id, name) pairs. A placeholder is a
            // manually/statically-added entry keyed by the human name or address
            // (e.g. "Node-A") rather than the real node_id. Its `name` field is
            // the human-readable name the operator configured in peers.toml —
            // capture it so we can preserve it across the upgrade.
            //
            // U1-6 补臂（2026-09-18，单机/回环拓扑根修）：自报地址**永不
            // 含回环**（get_all_local_ips 跳过 loopback），而单机多实例的
            // 静态 peer 配的恰是 127.0.0.1——纯地址比对在此拓扑下永不命中，
            // 占位条目永不升级、与真实条目长期并存（占位 id = 人读名），
            // 进而遮蔽 canonical_peer_id 的解析（registry.get 先命中占位 →
            // 返回占位 id 而非运行时 id，D0 账本身份失配，UAT T15 实证）。
            // 补名匹配臂：仅对「id==name」的自键占位条目、且本次 announce
            // 携带非空人读名（≠ node_id）时按名归并。真实条目（id≠name）
            // 不参与名合并——同名异 id 的两个真实节点不会被误并。
            let announce_carries_human_name = !name.is_empty() && name != node_id;
            let placeholders: Vec<(String, String, Vec<String>)> = self
                .registry
                .list_peers()
                .into_iter()
                .filter(|p| {
                    if p.base.id == node_id {
                        return false;
                    }
                    if announce_carries_human_name
                        && p.base.id == p.base.name
                        && p.base.name == name
                    {
                        return true;
                    }
                    // 发现②/B4 强化：占位匹配对自报**全量**地址逐个比对，
                    // 不再只对 primary——placeholder（静态 peers 手写条目）
                    // 配的地址可能不是自报 primary（多网卡枚举序决定），
                    // 单地址比对会永不归一、双条目并存。
                    let self_reported: Vec<String> = addresses
                        .iter()
                        .map(|a| {
                            if a.contains(':') {
                                a.clone()
                            } else {
                                format!("{}:{}", a, rpc_port)
                            }
                        })
                        .collect();
                    self_reported.iter().any(|cand| {
                        addr_eq(&p.base.address, cand)
                            || p.addresses.iter().any(|a| addr_eq(a, cand))
                    })
                })
                .map(|p| {
                    // 占位可继承的拨号候选：地址池原样 + base.address 的
                    // host 部分（静态 loader 装载的条目地址池常为空，拨号
                    // 走 base.address 的 host 回退分支——不并入则升级后
                    // 回环候选直接丢失）。
                    let mut addrs = p.addresses.clone();
                    let host = p.base.address.split(':').next().unwrap_or("").to_string();
                    if !host.is_empty() && !addrs.contains(&host) {
                        addrs.push(host);
                    }
                    (p.base.id.clone(), p.base.name.clone(), addrs)
                })
                .collect();
            let mut inherited_name: Option<String> = None;
            let mut inherited_static = false;
            let mut inherited_addresses: Vec<String> = Vec::new();
            for (placeholder_id, placeholder_name, placeholder_addrs) in &placeholders {
                tracing::info!(
                    real_id = node_id,
                    placeholder_id = %placeholder_id,
                    address = %primary_address,
                    "[Cluster] UDP discovery upgrading placeholder peer to real ID"
                );
                // Capture static-ness BEFORE removing the placeholder so the
                // upgraded real entry inherits it (static peers stay exempt
                // from UDP-staleness expiry).
                if self.registry.is_peer_static(placeholder_id) {
                    inherited_static = true;
                }
                self.registry.remove(placeholder_id);
                // Use the placeholder's human name (e.g. "Node-A") for the
                // persisted entry when available — the announce sometimes
                // carries node_id as name, which would survive into peers.toml
                // and break name-based lookups after a reload.
                let effective_name = if !placeholder_name.is_empty() {
                    placeholder_name.clone()
                } else {
                    name.to_string()
                };
                self.upgrade_peer_in_peers_toml(
                    placeholder_id,
                    node_id,
                    &RealNodeInfo {
                        id: node_id.into(),
                        name: effective_name,
                        address: primary_address.clone(),
                        // announce 携带的真实 RPC 端口随升级落盘（显式
                        // rpc_port 字段），不再留给装载端按约定猜。
                        rpc_port,
                        // 发现①根修：升级路径同样保全自报全量地址。
                        // U1-6：并入占位侧地址（含回环），落盘的候选池
                        // 才是完整选址集合——单机拓扑下回环是唯一可达地址。
                        addresses: {
                            let mut merged = addresses.clone();
                            for a in placeholder_addrs {
                                if !merged.contains(a) {
                                    merged.push(a.clone());
                                }
                            }
                            merged
                        },
                        // 同 handle_discovered_node：role 走对端自报值解析，
                        // 不硬编码（升级后的静态 peer 条目 role 才真实）。
                        role: nemesis_types::cluster::NodeRole::from_role_str(role),
                        category: category.into(),
                        capabilities: Vec::new(),
                        tags: tags.to_vec(),
                        node_type: node_type.into(),
                    },
                );
                if !placeholder_name.is_empty() {
                    inherited_name = Some(placeholder_name.clone());
                }
                for a in placeholder_addrs {
                    if !inherited_addresses.contains(a) {
                        inherited_addresses.push(a.clone());
                    }
                }
            }
            // Preserve the human-readable name across the upgrade. The real
            // entry was inserted above keyed by node_id; if its `name` ended up
            // empty or just the node_id (announce sometimes carries node_id as
            // name), restore the name inherited from the placeholder so that
            // lookups by human name (e.g. cluster_rpc target "Node-A") still
            // resolve via get_peer_info's name fallback.
            if let Some(human_name) = inherited_name
                && let Some(mut info) = self.registry.get(node_id)
                && (info.base.name.is_empty() || info.base.name == node_id)
            {
                info.base.name = human_name;
                self.registry.upsert(info);
            }
            // U1-6：地址池继承。占位条目（静态 peers）配的往往是 operator
            // 手写的可达地址（单机拓扑 = 127.0.0.1），而 announce 自报地址
            // 永不含回环——不继承则升级后的真实条目只剩网卡 IP 候选，单机
            // /防火墙拓扑下 RpcClient 选址全部不可达（send_and_receive 的
            // 多地址 failover 也无从兜底，候选池里根本没有回环）。占位侧
            // 地址去重并入真实条目候选池。
            if !inherited_addresses.is_empty()
                && let Some(mut info) = self.registry.get(node_id)
            {
                let mut changed_addr = false;
                for a in &inherited_addresses {
                    if !info.addresses.contains(a) {
                        info.addresses.push(a.clone());
                        changed_addr = true;
                    }
                }
                if changed_addr {
                    self.registry.upsert(info);
                }
            }
            // Preserve static-ness across the upgrade so a configured peer
            // (loaded from peers.toml) doesn't suddenly become UDP-expirable
            // once its placeholder key is replaced by the real node_id.
            if inherited_static {
                self.registry.mark_static(node_id);
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

        // Swarm M2: notify the discovery callback (gateway folds the node into
        // board channels; first-join semantics are the closure's job — cluster
        // does not depend on board).
        if let Some(cb) = self.on_node_discovered.lock().clone() {
            cb(node_id, role, category);
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
    /// `status` is NOT overwritten from the incoming data: it is a local
    /// observation (we may have just failed a health check). `addresses` is
    /// refreshed only when the payload carries a non-empty list (发现①：保全
    /// 全量选址候选池；空列表不清空已有集合——旧数据不砸新数据).
    /// The caller can separately mark the node online if warranted.
    ///
    /// Returns the canonical node_id that was written (i.e. `real_id`).
    pub fn merge_real_node_info(&self, info: &RealNodeInfo) -> String {
        // 1. Existing entry with real_id → update fields
        if let Some(mut existing) = self.registry.get(&info.id) {
            existing.base.name = info.name.clone();
            existing.base.role = info.role;
            existing.base.category = info.category.clone();
            // 静态 peer（peers.toml 显式配置）的地址不被对端自报覆盖：
            // 对端自报的 primary 是它 addresses[0]（网卡优先级决定），多网卡
            // 跨网段环境下对本地常不可达（真机三节点实证：master primary=
            // 10.103.x，worker 按它拨号必败 → G2 探针失败 → Offline →
            // online 门禁拒 transfer）。静态语义=用户显式配置优先，地址
            // 与 mark_static 的免过期保护同源；动态发现节点仍照常更新。
            if !info.address.is_empty() && !self.registry.is_peer_static(&info.id) {
                existing.base.address = info.address.clone();
            }
            // 发现①根修：全量自报数组非空才刷新（primary 保护同上——静态
            // 语义只锁用户显式配置的 base.address；数组是选址候选池，刷新
            // 让 select_best_address 在多网卡下始终拿到最新的可达集合）。
            if !info.addresses.is_empty() {
                existing.addresses = info.addresses.clone();
            }
            existing.base.last_seen = chrono::Local::now().to_rfc3339();
            existing.capabilities = info.capabilities.clone();
            existing.tags = info.tags.clone();
            existing.node_type = info.node_type.clone();
            self.registry.upsert(existing);
            self.persist_real_peer_to_toml(&info.id, info);
            return info.id.clone();
        }

        // 2. Placeholder by address → remove + insert under real_id.
        // 发现②/B4 强化：占位匹配不再只比对自报 primary——对端多网卡下
        // primary 可能不是用户静态配置的那个地址（真机实证：placeholder
        // 配 192.168.137.x 而自报 primary=10.103.x → 永不归一，双条目
        // 并存）。改为对自报全量地址逐个匹配，任一命中即归一。
        let placeholder_id = self
            .registry
            .find_by_address(&info.address)
            .map(|p| p.base.id.clone())
            .or_else(|| {
                info.addresses
                    .iter()
                    .filter_map(|a| {
                        let candidate = if a.contains(':') {
                            a.clone()
                        } else {
                            let port = info.address.rsplit_once(':').map(|(_, p)| p.to_string());
                            match port {
                                Some(p) => format!("{}:{}", a, p),
                                None => a.clone(),
                            }
                        };
                        self.registry
                            .find_by_address(&candidate)
                            .map(|p| p.base.id.clone())
                    })
                    .next()
            });

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
            tags: info.tags.clone(),
            addresses: info.addresses.clone(),
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

    /// Convert an RPC address (`host:rpc_port`) to the UDP address
    /// (`host:udp_port`) for peers.toml write-back. 委托
    /// [`crate::cluster_config::rpc_to_udp_address`]（单一真相源，pair 配对
    /// 写盘同源消费）。
    fn rpc_to_udp_address(rpc_addr: &str) -> String {
        crate::cluster_config::rpc_to_udp_address(rpc_addr)
    }

    /// Persist the real peer info to peers.toml under `[peers.{real_id}]`.
    fn persist_real_peer_to_toml(&self, real_id: &str, info: &RealNodeInfo) {
        let path = &self.static_config_path;
        let role_str = info.role.as_role_str();
        // peers.toml's `address` field is the UDP host:port — the static loader
        // (gateway.rs) derives rpc_port = udp_port + 10000 from it. info.address
        // is the RPC address (host:rpc_port) used in-memory; convert it back to
        // the UDP address on write so the derivation isn't double-applied on
        // reload (otherwise 21949 → 31949, breaking post-restart callbacks —
        // root cause of cluster-uat T9).
        let toml_address = Self::rpc_to_udp_address(&info.address);
        if let Err(e) = crate::cluster_config::append_peer_to_file_with_name(
            path,
            real_id,
            &toml_address,
            role_str,
            &info.category,
            Some(&info.name),
            info.rpc_port,
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
    fn upgrade_peer_in_peers_toml(&self, placeholder: &str, real_id: &str, info: &RealNodeInfo) {
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
            let content =
                std::fs::read_to_string(path).map_err(|e| format!("read peers.toml: {}", e))?;
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

        // 发现②/B3：优先按字面 placeholder 键删（新写盘路径是字面键）；
        // 找不到再试 sanitize 旧键（旧版代码落盘的有损键存量条目）。
        let placeholder_key = placeholder.to_string();
        let removed = peers_table.remove(&placeholder_key).or_else(|| {
            let legacy = crate::cluster_config::sanitize_peer_key(placeholder);
            if legacy == placeholder_key {
                return None;
            }
            peers_table.remove(&legacy)
        });
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
        let task =
            self.task_manager
                .create_task(action, payload, original_channel, original_chat_id);
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
            tracing::warn!(task_id = task_id, error = error, "[Cluster] Task failed",);
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
                                        Some(val) => serde_json::to_vec(val)
                                            .map_err(|e| format!("serialize response: {}", e)),
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
                                Some(val) => serde_json::to_vec(val)
                                    .map_err(|e| format!("serialize response: {}", e)),
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
            metadata: std::collections::HashMap::new(),
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
            node_name: String::new(),
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

    /// 把调用方给的 target（peer 名或节点 id，人读/机读两种形态都收）归一化
    /// 为注册表权威节点 id（`base.id`，即 worker 侧 `_rpc.from` 的形态）。
    ///
    /// 单一真相源纪律（T37 双身份失配第三处落点，2026-09-13）：派发账本
    /// `issue_dispatch.worker_id` 与 worker 上报的传输层身份必须是同一形态
    /// ——matcher 产出本来就是节点 id，人工指派/兜底路径可能给 peer 名
    /// （如 "Alex"），不归一化则 `task.started`/`delivery.files` 的
    /// worker 校验永远失配（名字 ≠ 节点 id）。解析失败（未知 target）返回
    /// None，调用方保留原值（RPC resolver 自身还有 name/id 兜底扫描）。
    pub fn canonical_peer_id(&self, target: &str) -> Option<String> {
        if let Some(info) = self.registry.get(target) {
            // U1-6 占位遮蔽防护（2026-09-18）：get 命中的是「id==name」自键
            // 占位条目（静态 peers 以人读名键控）时，注册表里可能同时存在
            // 同一节点的真实条目（announce 已到、占位按地址升级未命中的
            // 窗口，单机/回环拓扑下该窗口是常态）——此时必须返回真实运行
            // 时 id（worker 侧 `_rpc.from` 的形态），否则派发账本 worker_id
            // 落占位 id、写回评论作者身份失配（D0 单一真相源被占位击穿，
            // UAT T15 实证：作者落 "Node-B" 而非 node-* 运行时 id）。扫描
            // 按 name 精确匹配、排除占位自身；无真实条目 = 维持占位 id
            //（旧行为，纯静态拓扑下这是唯一可用句柄）。
            if info.base.id == info.base.name
                && let Some(real) = self
                    .registry
                    .list_peers()
                    .into_iter()
                    .find(|p| p.base.id != target && p.base.name == target)
            {
                return Some(real.base.id);
            }
            return Some(info.base.id);
        }
        self.registry
            .list_peers()
            .into_iter()
            .find(|p| p.base.id == target || p.base.name == target)
            .map(|p| p.base.id)
    }

    /// RPC 学到的未知对端登记（真实节点 id 首次出现、UDP announce 尚未
    /// 到达时的身份补齐入口）。
    ///
    /// T26 根修（2026-09-18）：此前 gateway 在 peer_chat handler 里以硬
    /// 编码缺省值（name=id、addresses=["127.0.0.1"]、role="worker"、
    /// category="general"、rpc_port fallback 21949）直接调
    /// handle_discovered_node。RPC 天然比 announce 先到（广播周期
    /// broadcast_interval + 发送线程 0-5s jitter，TCP 恒抢先）：垃圾条目
    /// 触发地址匹配占位升级臂，把占位条目 operator 配置的
    /// role=coordinator/category/udp 地址语义整体覆盖（inherited_name 只
    /// 救回 name），并随 upgrade_peer_in_peers_toml 落盘 peers.toml 永久
    /// 固化——announce 的后续修正只进 registry + state.toml
    /// （sync_to_disk 不写 peers.toml），节点每次重启装载 peers.toml 垃
    /// 圾复活，worker_sync_once 找不到 coordinator（UAT T26 实证：B 视
    /// 角 A 条目 = worker/general/127.0.0.1，而 A 实为
    /// coordinator/development）。
    ///
    /// 正确姿势：优先继承同地址占位（静态 peers 人读名条目）的完整身份
    /// ——role/category/name/地址池原样、rpc_port 用 hint 或占位端口派
    /// 生——再走 handle_discovered_node 正常升级路径。继承不到时才退保
    /// 守缺省（与旧行为一致；纯 UDP 集群无静态配置，announce 数秒内修
    /// 正）。返回 true = 本次实际登记/升级了条目。
    pub fn register_rpc_peer(&self, real_id: &str, rpc_port_hint: u16) -> bool {
        if self.registry.get(real_id).is_some() {
            return false;
        }

        // 自键占位（id==name，静态 peers 的人读名条目）。真实条目（id≠
        // name）不参与继承——那是 announce 学来的运行时身份，不是
        // operator 配置的原始语义。static 标记是占位资格的硬条件：保守
        // 缺省插入的条目（name=id，无 static 标记）不得被当占位继承，
        // 否则垃圾缺省身份会自我扩散。
        let placeholders: Vec<crate::types::ExtendedNodeInfo> = self
            .registry
            .list_peers()
            .into_iter()
            .filter(|p| {
                p.base.id == p.base.name
                    && p.base.id != real_id
                    && self.registry.is_peer_static(&p.base.id)
            })
            .collect();

        // 端口提示匹配：占位地址的端口与 hint 同值（占位地址已被静态
        // loader 转成 rpc 形态）或相差 10000（占位地址仍是 udp 形态）都
        // 算命中。
        let port_of = |addr: &str| -> Option<u16> {
            addr.rsplit(':').next().and_then(|p| p.parse::<u16>().ok())
        };
        let matched = if rpc_port_hint > 0 {
            placeholders.iter().find(|p| {
                port_of(&p.base.address)
                    .map(|port| {
                        port == rpc_port_hint || Some(port) == rpc_port_hint.checked_sub(10000)
                    })
                    .unwrap_or(false)
            })
        } else {
            None
        };
        // 无 hint 时的兜底：恰有一个占位才继承（单机/双节点拓扑几乎必
        // 然正确）；多占位无法判定身份，宁可等待 announce，不猜。
        let chosen = matched.or_else(|| {
            if placeholders.len() == 1 {
                placeholders.first()
            } else {
                None
            }
        });

        let Some(ph) = chosen else {
            // 无占位可继承：保守登记（旧行为，仅 hint 可用时）。announce
            // 到达后由 upsert_if_changed 修正为真实自报身份。G14 纪律：
            // rpc_port 未知（hint=0）登记不可达条目不如不登记。
            if rpc_port_hint == 0 {
                return false;
            }
            return self.handle_discovered_node(
                real_id,
                real_id,
                vec!["127.0.0.1".to_string()],
                rpc_port_hint,
                "worker",
                "general",
                vec![],
                vec![],
                "unknown",
            );
        };

        // 继承占位身份升级。role 走占位的真实角色（T26 死因 = 旧代码在
        // 此硬编码 "worker"，覆盖了 coordinator）；rpc_port：hint 优先，
        // 占位端口已是 rpc 形态（>10000）直接用，udp 形态按 +10000 约定
        // 派生（与静态 loader 同源）。
        let udp_port = port_of(&ph.base.address).unwrap_or(0);
        let rpc_port = if rpc_port_hint > 0 {
            rpc_port_hint
        } else if udp_port > 10000 {
            udp_port
        } else if udp_port > 0 {
            udp_port + 10000
        } else {
            0
        };
        // 占位 base.address 的 host 并入地址池（单机拓扑回环往往是唯一
        // 可达地址，见 handle_discovered_node 升级臂 U1-6 注记）。
        let host = ph.base.address.split(':').next().unwrap_or("").to_string();
        let mut addresses = ph.addresses.clone();
        if !host.is_empty() && !addresses.contains(&host) {
            addresses.push(host);
        }
        let name = if ph.base.name.is_empty() {
            real_id.to_string()
        } else {
            ph.base.name.clone()
        };
        let role_str = ph.base.role.as_role_str();
        let category = ph.base.category.clone();
        let changed = self.handle_discovered_node(
            real_id,
            &name,
            addresses,
            rpc_port,
            role_str,
            &category,
            ph.tags.clone(),
            vec![],
            &ph.node_type,
        );
        if changed {
            tracing::info!(
                real_id = real_id,
                placeholder = %ph.base.id,
                role = role_str,
                category = %category,
                rpc_port,
                "[Cluster] RPC peer registration inherited placeholder identity"
            );
        }
        changed
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
    ///
    /// Offline→Online 翻转同样触发 `on_node_discovered`（复活也是节点发现，
    /// 与 G2 探针复活同语义，见 `fire_recovered_callback`）；已 Online 的
    /// 重复确认不触发（幂等）。
    pub fn mark_peer_healthy(&self, node_id: &str) {
        let was_offline = self
            .registry
            .get(node_id)
            .map(|info| info.status != NodeStatus::Online)
            .unwrap_or(false);
        self.registry.mark_healthy(node_id);
        if was_offline {
            Self::fire_recovered_callback(&self.on_node_discovered, &self.registry, node_id);
        }
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

    /// On-demand frame-level connectivity probe of a single peer（G2 探针
    /// 同款形态：`Custom("ping")` + 5s 超时，`require_online=false`——对端
    /// 被标 Offline 也能探到真话）。返回对端是否帧级应答。
    ///
    /// 与健康检查循环的分工：本方法**不**因单次失败翻转注册表状态（那是
    /// G2 连败累计的职责，单次失败可能是瞬态抖动）；成功侧顺带
    /// [`Self::mark_peer_healthy`]（幂等，Offline→Online 复活广播照常触发
    /// ——看板停车场 sweep 依赖该信号，业务流主动接触到的复活不该被
    /// sweep 晚一轮才看见）。消费方：看板冲突硬解重派前的三轮主动接触
    ///（board conflict_resolver）。
    pub async fn probe_peer(&self, node_id: &str) -> bool {
        let Some(client) = self.rpc_client_arc() else {
            return false;
        };
        let request = crate::rpc_types::RPCRequest {
            id: uuid::Uuid::new_v4().to_string(),
            action: crate::rpc_types::ActionType::Custom("ping".to_string()),
            payload: serde_json::json!({}),
            source: self.node_id.clone(),
            target: Some(node_id.to_string()),
        };
        match client
            .call_probe_with_timeout(node_id, request, std::time::Duration::from_secs(5))
            .await
        {
            Ok(_) => {
                self.mark_peer_healthy(node_id);
                true
            }
            Err(_) => false,
        }
    }

    /// Mark a peer as static/configured (loaded from peers.toml). Static peers
    /// are exempt from UDP-staleness expiry in `check_health` — they have a
    /// known address and stay Online across UDP announce gaps. Call after
    /// loading each static peer at startup.
    pub fn mark_peer_static(&self, node_id: &str) -> bool {
        self.registry.mark_static(node_id)
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

    /// Set the node role ("coordinator" — 旧值 "master"/"manager" 兼容 — or "worker").
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

    /// 本机能力清单快照（`set_capabilities` 注入的原值）。与
    /// [`Self::get_capabilities`]（registry 在线节点能力**并集**）语义不同
    /// ——桥 hello（二期身份交换）携带的是本机能力，用本方法。
    pub fn local_capabilities(&self) -> Vec<String> {
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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
        let existing_address = self
            .registry
            .get(&self.node_id)
            .map(|e| e.base.address.clone())
            .unwrap_or_else(|| self.address.clone());

        let role_str = self.role.read().clone();
        let role = nemesis_types::cluster::NodeRole::from_role_str(&role_str);
        let caps = self
            .capabilities
            .lock()
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
            tags: self.tags.read().clone(),
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
                tags: node.tags.clone(),
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

        crate::cluster_config::save_dynamic_state(&self.dynamic_state_path, &state).map_err(|e| {
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
        if let Err(e) =
            self.register_rpc_handler("peer_chat_callback", self.build_callback_handler())
        {
            logger::log_error(
                "cluster",
                &format!("register peer_chat_callback: {}", e),
                "",
            );
        }

        // Register hello handler
        let node_id = self.node_id.clone();
        if let Err(e) = self.register_rpc_handler(
            "hello",
            Box::new(move |_payload| {
                Ok(serde_json::json!({
                    "node_id": node_id,
                    "status": "online",
                    "message": "hello from cluster node",
                }))
            }),
        ) {
            logger::log_error("cluster", &format!("register hello: {}", e), "");
        }

        // H4: Register query_task_result handler (B-side responds to A's polling)
        if let Err(e) =
            self.register_rpc_handler("query_task_result", self.build_query_task_result_handler())
        {
            logger::log_error("cluster", &format!("register query_task_result: {}", e), "");
        }

        // H4: Register confirm_task_delivery handler
        if let Err(e) = self.register_rpc_handler(
            "confirm_task_delivery",
            self.build_confirm_task_delivery_handler(),
        ) {
            logger::log_error(
                "cluster",
                &format!("register confirm_task_delivery: {}", e),
                "",
            );
        }
    }

    /// Register only the H4 task-recovery handlers (query_task_result +
    /// confirm_task_delivery).
    ///
    /// Gateway mode never calls [`Self::set_rpc_channel`]（该入口在 gateway
    /// 装配流程中无人触发，`register_peer_chat_handlers` 因此不会执行），而
    /// gateway 只自行注册 peer_chat / peer_chat_callback / task_cancel——
    /// 不覆盖本函数注册的两个恢复 handler。缺失时 A 侧
    /// `poll_stale_pending_tasks`（120s 周期）永远收到 "no handler"，B 重启
    /// 丢结果后的 stale 恢复链路断裂（2026-09-01 跨机 E2E 实测发现）。
    /// gateway 在 `Arc<Cluster>` 就绪后调用本函数；与
    /// [`Self::register_peer_chat_handlers`] 共用同一对私有 builder（单一
    /// 真相源），不重复实现。
    pub fn register_task_recovery_handlers(&self) {
        if let Err(e) =
            self.register_rpc_handler("query_task_result", self.build_query_task_result_handler())
        {
            logger::log_error("cluster", &format!("register query_task_result: {}", e), "");
        }
        if let Err(e) = self.register_rpc_handler(
            "confirm_task_delivery",
            self.build_confirm_task_delivery_handler(),
        ) {
            logger::log_error(
                "cluster",
                &format!("register confirm_task_delivery: {}", e),
                "",
            );
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
        self.register_rpc_handler(
            "ping",
            Box::new(move |_payload| {
                Ok(serde_json::json!({
                    "status": "pong",
                    "node_id": node_id,
                    // G2: 供主动探针交叉校验对端时钟漂移（additive 字段，
                    // 旧客户端忽略）。
                    "timestamp": chrono::Utc::now().timestamp(),
                }))
            }),
        )?;

        // get_capabilities — shares Arc with Cluster for real-time reads
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler(
            "get_capabilities",
            Box::new(move |_payload| {
                let caps = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
                Ok(serde_json::json!({
                    "capabilities": caps,
                }))
            }),
        )?;

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
        self.register_rpc_handler(
            "get_info",
            Box::new(move |_payload| {
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
            }),
        )?;

        // list_actions
        let node_id = self.node_id.clone();
        self.register_rpc_handler(
            "list_actions",
            Box::new(move |_payload| {
                let schemas = crate::actions_schema::builtin_schemas();
                let actions: Vec<String> = schemas.iter().map(|s| s.action.to_string()).collect();
                Ok(serde_json::json!({
                    "node_id": node_id,
                    "actions": actions,
                }))
            }),
        )?;

        // hello
        let node_id = self.node_id.clone();
        self.register_rpc_handler(
            "hello",
            Box::new(move |_payload| {
                Ok(serde_json::json!({
                    "node_id": node_id,
                    "status": "online",
                    "message": "hello from cluster node",
                }))
            }),
        )?;

        // diagnostics.system — OS, memory, uptime
        self.register_rpc_handler(
            "diagnostics.system",
            Box::new(move |_payload| {
                let os = std::env::consts::OS.to_string();
                let arch = std::env::consts::ARCH.to_string();
                let hostname = crate::diagnostics::get_hostname();
                let (mem_total, mem_used, uptime_secs) =
                    crate::diagnostics::collect_system_metrics();
                let os_version = crate::diagnostics::collect_os_version();
                Ok(serde_json::json!({
                    "os": os, "os_version": os_version, "arch": arch,
                    "hostname": hostname, "uptime_secs": uptime_secs,
                    "memory_total_bytes": mem_total, "memory_used_bytes": mem_used,
                }))
            }),
        )?;

        // diagnostics.network — network interfaces and IPs
        self.register_rpc_handler(
            "diagnostics.network",
            Box::new(move |_payload| {
                let interfaces = network::get_local_network_interfaces();
                let all_ips = network::get_all_local_ips();
                Ok(serde_json::json!({
                    "interfaces": interfaces,
                    "all_ips": all_ips,
                }))
            }),
        )?;

        // diagnostics.cluster_state — peers this node sees
        let registry_arc = self.registry.clone();
        self.register_rpc_handler(
            "diagnostics.cluster_state",
            Box::new(move |_payload| {
                let all_nodes = registry_arc.list_peers();
                let online: Vec<_> = all_nodes
                    .iter()
                    .filter(|n| n.is_online())
                    .map(|n| {
                        serde_json::json!({
                            "id": n.base.id,
                            "name": n.base.name,
                            "address": n.base.address,
                            "role": n.base.role,
                            "last_seen": n.base.last_seen,
                        })
                    })
                    .collect();
                Ok(serde_json::json!({
                    "node_count": all_nodes.len(),
                    "online_count": online.len(),
                    "nodes": online,
                }))
            }),
        )?;

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
        self.register_rpc_handler(
            "forge_share",
            Box::new(move |payload| {
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
            }),
        )?;

        // forge_get_reflections: list available local reflections
        let provider_list = provider.clone_boxed();
        let node_id_list = node_id.clone();
        self.register_rpc_handler(
            "forge_get_reflections",
            Box::new(move |payload| {
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
                if let Some(filename) = payload.get("filename").and_then(|v| v.as_str())
                    && !filename.is_empty()
                {
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

                result["node_id"] = serde_json::Value::String(node_id_list.clone());

                Ok(result)
            }),
        )?;

        tracing::info!(
            "[Cluster] Registered forge RPC handlers: forge_share, forge_get_reflections"
        );
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
            let error = payload.get("error").and_then(|v| v.as_str()).unwrap_or("");

            tracing::info!(
                task_id = %task_id,
                status = %status,
                "[Cluster] peer_chat_callback received"
            );

            // Check if this callback is for a cluster agent task (B-side forwarding).
            if let (Some(tl), Some(wq)) = (&cluster_task_list, &cluster_work_queue)
                && let Some(parent_id) = tl.find_by_child_task_id(task_id)
            {
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
                    // G5 修复（2026-09-01）：set_running 占位条目
                    // （gateway ClusterResultPersisterAdapter，result 内含
                    // "status":"running"）曾在此被误报为 done+空回复 —— A 端
                    // 恢复轮询会拿着空内容过早完成任务。running 条目必须
                    // 原样上报，让 A 继续等。
                    let inner_status = entry
                        .result
                        .get("status")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if inner_status == "running" {
                        return Ok(serde_json::json!({
                            "status": "running",
                            "task_id": task_id,
                        }));
                    }
                    let result_status = if entry.success { "success" } else { "error" };
                    // 键归一：写端（adapter set_result）现为 "response"；
                    // 保留 "content" 回退兼容修复前落盘的旧结果。
                    let response = entry
                        .result
                        .get("response")
                        .or_else(|| entry.result.get("content"))
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
    /// than the pending safety net (G4: max(24h, 2×LLM timeout)) time out.
    ///
    /// Mirrors Go's `Cluster.pollStalePendingTasks()`.
    pub async fn poll_stale_pending_tasks(&self) {
        let call_fn = self.call_with_context_fn.lock().clone();
        let rpc_client = self.rpc_client.lock().clone();
        let bus_snapshot: Option<Arc<dyn MessageBus>> = self.bus.lock().clone();
        let safety_net = self.task_manager.pending_timeout();
        // CD3: 交付回调快照（set 可发生在 start 之后，同 bus 语义）。
        let delivery_cb = self.on_recovered_delivery.lock().clone();
        poll_stale_pending_tasks(
            &self.task_manager,
            &call_fn,
            rpc_client.as_deref(),
            safety_net,
            bus_snapshot.as_deref(),
            false,
            delivery_cb.as_ref(),
        )
        .await;
    }

    /// Confirm task delivery to the B-node, allowing it to clean up the result.
    ///
    /// Mirrors Go's `Cluster.confirmDelivery(peerID, taskID)`.
    pub fn confirm_delivery(&self, peer_id: &str, task_id: &str) {
        let payload = serde_json::json!({"task_id": task_id});
        // Clone the override OUT of the lock (same pattern as poll_stale_pending_tasks
        // and handle_task_complete). Binding the guard here would hold the
        // non-reentrant parking_lot Mutex across call_with_context, which re-locks
        // the same mutex at its override check → guaranteed self-deadlock on the
        // fallback branch (the production path when no test override is set).
        // (BUG #19, quality-hardening goal 冲刺 S4)
        let call_fn = self.call_with_context_fn.lock().clone();
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

/// G4: A 端 stale-task 安全网超时 = max(24h, 2 × B 端 LLM 有效超时)。
///
/// `llm_timeout_secs == 0`（B 端不设超时）按 peer_chat 协议默认
/// [`crate::rpc::peer_chat_handler::DEFAULT_LLM_TIMEOUT`]（2h）计算。
/// 超时阶梯不变量：provider 单请求超时 < B 端 LLM 处理超时 < **本安全网**
/// < 结果存储 TTL（7 天）—— 安全网触发时 B 端结果尚未被 TTL 清掉，
/// A 端 poll 仍能捞回真实结果。
pub fn stale_task_safety_net(llm_timeout_secs: u64) -> chrono::Duration {
    const FLOOR: chrono::Duration = chrono::Duration::hours(24);
    let effective = if llm_timeout_secs == 0 {
        crate::rpc::peer_chat_handler::DEFAULT_LLM_TIMEOUT
    } else {
        Duration::from_secs(llm_timeout_secs)
    };
    let doubled = effective.saturating_mul(2);
    chrono::Duration::from_std(doubled)
        .unwrap_or(FLOOR)
        .max(FLOOR)
}

/// G2: 探针调度判定（纯函数，便于单测）：Online 对端每 tick 都探；
/// Offline 对端每 5 个 tick 探一次（1/5 降频自愈——不是放弃，是恢复手段，
/// 一次成功立即翻回 Online）。`tick` 为 1-based tick 序号。
fn should_probe_peer(online: bool, tick: u64) -> bool {
    online || tick.is_multiple_of(5)
}

/// G2: 探针 timestamp 漂移 WARN 限频闸门（进程级，每节点 10 分钟冷却）。
fn probe_drift_gate() -> &'static crate::discovery::AnnounceWarnGate {
    static GATE: std::sync::OnceLock<crate::discovery::AnnounceWarnGate> =
        std::sync::OnceLock::new();
    GATE.get_or_init(crate::discovery::AnnounceWarnGate::new)
}

/// Poll stale pending tasks: query the B-node for any task that has been
/// pending for longer than 2 minutes. If the B-node reports it done, complete
/// the task locally; if not found, fail it; if older than the safety net,
/// time it out.
///
/// Uses the real RPC client when available, falling back to the synchronous
/// test override (`call_fn`).  This matches Go's `pollStalePendingTasks`
/// which calls `c.CallWithContext()`.
/// G5: `bus` 非空时，done / not_found / 安全网超时分支会按 gateway Route 2
/// 的形状向总线重发 `cluster_continuation:{task_id}`（含真实响应内容 +
/// metadata），唤醒 A 端主 agent 的续行恢复——生产装配不接
/// TaskManager.on_complete，仅靠 complete_callback 只会更新任务状态、不会
/// 唤醒 agent。（CD2，2026-09-17：安全网分支补齐 publish。）
///
/// CD3: `delivery_cb` 非空时，done 分支先触发交付回调（看板写回 / chat
/// 交付判定，路由在 gateway 注入的闭包里），回调返回 true 才向 worker 发
/// confirm 删除其本地副本；false 跳过 confirm（宁留勿丢）。
async fn poll_stale_pending_tasks(
    task_manager: &Arc<TaskManager>,
    call_fn: &Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    >,
    rpc_client: Option<&RpcClient>,
    safety_net: chrono::Duration,
    bus: Option<&dyn MessageBus>,
    include_young: bool,
    // CD3（2026-09-17）：恢复交付回调——done 分支 confirm 前置闸，见
    // Cluster::set_on_recovered_delivery。None = 旧行为（无条件 confirm）。
    delivery_cb: Option<&Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>>,
) {
    let tasks = task_manager.list_pending_tasks();

    if !tasks.is_empty() {
        tracing::debug!(count = tasks.len(), "[Cluster] Polling stale pending tasks",);
    }

    for task in tasks {
        // Parse created_at (RFC 3339) and compute age.
        let created = match chrono::DateTime::parse_from_rfc3339(&task.created_at) {
            Ok(dt) => dt.with_timezone(&chrono::Local),
            Err(_) => continue,
        };
        let age = chrono::Local::now() - created;

        // Skip tasks younger than 2 minutes. P6b（2026-09-11 日志）：恢复
        // loop 的首 tick 传 include_young=true 绕过此闸——重启后立即查询，
        // 不用白等 2 分钟（tokio interval 首 tick 本就即时，此前被这个
        // 年龄闸架空）。
        if !include_young && age < chrono::Duration::minutes(2) {
            continue;
        }

        // Timeout tasks past the safety net (G4: 可配置，默认 max(24h, 2×LLM超时)).
        if age > safety_net {
            // CD2（2026-09-17）：错误文案先落变量——complete_callback 与
            // publish_continuation_to_bus 两处共用，保证任务状态与 agent
            // 唤醒消息里的错误一致。
            let timeout_error = format!(
                "task timed out: no response within {}s safety net",
                safety_net.num_seconds()
            );
            tracing::warn!(
                task_id = %task.id,
                age_secs = age.num_seconds(),
                safety_net_secs = safety_net.num_seconds(),
                "[Cluster] Timing out stale task after safety-net timeout",
            );
            task_manager.complete_callback(&task.id, "error", "", &timeout_error);
            // CD2（2026-09-17）：安全网超时同样要发 bus 唤醒 agent（error
            // 形态，形状对齐下方 not_found 分支）。此前只 complete_callback
            // 不 publish——任务状态标 Failed 了，但等回复的 agent 永远不知
            // 道，续行快照悬挂到永远。
            if let Some(bus) = bus {
                publish_continuation_to_bus(
                    bus,
                    &task.id,
                    "",
                    "error",
                    &task.peer_id,
                    Some(&timeout_error),
                );
            }
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
                .call_with_timeout(&task.peer_id, request, Duration::from_secs(30))
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

        let status = resp.get("status").and_then(|v| v.as_str()).unwrap_or("");

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
                task_manager.complete_callback(&task.id, &result_status, &response, &error);
                // G5: 唤醒 A 端主 agent（形状对齐 gateway Route 2）。
                if let Some(bus) = bus {
                    publish_continuation_to_bus(
                        bus,
                        &task.id,
                        &response,
                        &result_status,
                        &task.peer_id,
                        if error.is_empty() { None } else { Some(&error) },
                    );
                }
                // CD3（2026-09-17）：交付回调成功（或未注册回调 = 旧行为）才
                // confirm——写回成功才删 worker 端副本，宁留勿丢。回调返回
                // false（如看板写回失败）时跳过 confirm + WARN，副本走 worker
                // 端 7 天 TTL 兜底（TaskManager 已完成，下轮 poll 不会再查）。
                let delivery_ok = match delivery_cb {
                    Some(cb) => {
                        let ok = cb(
                            &task.id,
                            &result_status,
                            &response,
                            if error.is_empty() { None } else { Some(&error) },
                        );
                        if !ok {
                            tracing::warn!(
                                task_id = %task.id,
                                "[Cluster] recovered delivery callback reported failure; \
                                 skipping confirm (worker copy kept until TTL)"
                            );
                        }
                        ok
                    }
                    None => true,
                };
                if delivery_ok {
                    // Best-effort delivery confirmation
                    confirm_delivery_with(call_fn, rpc_client, &task.peer_id, &task.id).await;
                }
            }
            "not_found" => {
                tracing::warn!(
                    task_id = %task.id,
                    peer_id = %task.peer_id,
                    "[Cluster] Stale task not found on remote peer",
                );
                task_manager.complete_callback(&task.id, "error", "", "remote task not found");
                // G5: not_found 同样要唤醒 agent（带 error metadata），
                // 否则续行快照永远悬着直到安全网清理。
                if let Some(bus) = bus {
                    publish_continuation_to_bus(
                        bus,
                        &task.id,
                        "",
                        "error",
                        &task.peer_id,
                        Some("remote task not found"),
                    );
                }
            }
            _ => {
                // Unknown status, skip.
                continue;
            }
        }
    }
}

/// G5: 按 gateway Route 2 的形状把恢复出的任务结果发回总线，唤醒 A 端
/// 主 agent 的 `cluster_continuation` 续行恢复。
///
/// - `sender_id` = `cluster_continuation:{task_id}`（AgentLoop 拦截前缀）
/// - `content` = B 端真实响应（恢复的 LLM 回复）
/// - `metadata` = status / source_node / error（与 Route 2 字段一致）
fn publish_continuation_to_bus(
    bus: &dyn MessageBus,
    task_id: &str,
    content: &str,
    status: &str,
    source_node: &str,
    error: Option<&str>,
) {
    let mut metadata = std::collections::HashMap::new();
    metadata.insert("status".to_string(), status.to_string());
    metadata.insert("source_node".to_string(), source_node.to_string());
    if let Some(err) = error {
        metadata.insert("error".to_string(), err.to_string());
    }
    bus.publish_inbound(BusInboundMessage {
        channel: "system".into(),
        sender_id: format!("cluster_continuation:{}", task_id),
        chat_id: String::new(),
        content: content.to_string(),
        metadata,
    });
}

/// Notify the B-node that the task result was received.
/// Uses the RPC client if available, otherwise the synchronous test override.
async fn confirm_delivery_with(
    call_fn: &Option<
        Arc<dyn Fn(&str, &str, serde_json::Value) -> Result<Vec<u8>, String> + Send + Sync>,
    >,
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
        self.capabilities
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone()
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

    fn peer_udp_endpoints(&self) -> Vec<String> {
        crate::cluster_config::load_peer_udp_endpoints(&self.static_config_path)
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
                if host.is_empty() {
                    Vec::new()
                } else {
                    vec![host]
                }
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
                    if host.is_empty() {
                        Vec::new()
                    } else {
                        vec![host]
                    }
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

/// 批次四：显示名 hostname 链（COMPUTERNAME → HOSTNAME，与
/// `generate_node_id` 同款）。空 / `unknown`（纯容器环境）= None——
/// 调用方落 `Bot {id8}` 兜底。
pub(crate) fn hostname_display_name() -> Option<String> {
    let h = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_default();
    let h = h.trim().to_string();
    if h.is_empty() || h.eq_ignore_ascii_case("unknown") {
        None
    } else {
        Some(h)
    }
}

/// 批次四：显示名自动解析链（goal 钉死优先级）的**纯函数核**：
/// `config.cluster.json node_name` 非空 →（用它，显式免疫撞名后缀）；
/// 否则 hostname 链（由调用方注入 `hostname_display_name()` 结果）；
/// 空 / `unknown` → `Bot {id8}` 兜底（纯容器环境）。
/// 返回 `(名字, 显式锁定)`——锁定名不被撞名后缀收敛（重名无害，唯一性
/// 靠 node_id；后缀只作用于自动名）。
///
/// hostname 作为参数注入而非函数内读 env：env 是进程全局，并行测试
/// 注入 `COMPUTERNAME` 会互相污染（且 Rust 2024 `set_var` 是 unsafe）——
/// 三元纯函数让优先级链可零 env 依赖直测（见 `cluster/tests.rs`）。
pub(crate) fn resolve_auto_node_name(
    config_name: &str,
    hostname: Option<String>,
    node_id: &str,
) -> (String, bool) {
    let cn = config_name.trim();
    if !cn.is_empty() {
        return (cn.to_string(), true);
    }
    if let Some(h) = hostname {
        return (h, false);
    }
    (format!("Bot {}", &node_id[..8.min(node_id.len())]), false)
}

/// 批次四：解析链的 env 薄包装——生产路径（`new` / `with_workspace`）
/// 调这个；测试直接调 `resolve_auto_node_name` 纯函数核。
pub(crate) fn resolve_node_name_from_env(config_name: &str, node_id: &str) -> (String, bool) {
    resolve_auto_node_name(config_name, hostname_display_name(), node_id)
}

/// 批次四：撞名后缀来源——node_id **末段**前 4 字符。
///
/// goal 字面是「node_id 前 4 位」，但生产 id 形如 `node-{hostname}-{uuid}`
/// （`generate_node_id`），前 4 位恒为 `"node"`——真机撞名双方各自算出的
/// 后缀相同，收敛失效（2026-09-19 真机彩排实测）。末段（uuid 段）前 4
/// 保留 goal 意图：稳定（id 不变则后缀不变，重启不变）+ 有区分度
/// （uuid 随机段；4 hex 碰撞 ≈ 1/65536，对「显示名给人看」量级足够）。
pub(crate) fn name_suffix_from_node_id(node_id: &str) -> String {
    let last = node_id.rsplit('-').next().unwrap_or(node_id);
    last.chars().take(4).collect()
}

/// Atomic write helper — REL-002（2026-09-23）起委托统一 helper
/// `nemesis_utils::write_file_atomic`（唯一临时名 + sync_all + 失败清理 +
/// unix 0600 创建即挂）。保留签名兼容：`merge_real_node_info` 重写
/// peers.toml（占位 subtable 移除后回填 real_id）的唯一调用点。
fn write_atomic(path: &Path, data: &[u8]) -> std::io::Result<()> {
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data, 0o600)
        .map_err(std::io::Error::other)
}

/// Real node info obtained from RPC `get_info` or UDP AnnounceMessage.
///
/// Carries the authoritative identity of a remote node. Used by
/// `Cluster::merge_real_node_info` to upgrade placeholder peer entries
/// (created by manual `nodes.add`) to the remote's real ID, and to refresh
/// fields whenever the remote broadcasts a new state.
///
/// `addresses` 是对端自报的**全量**地址列表（网卡枚举序）。发现①根修
/// （2026-09-15 真机三节点）：此前 RPC merge 路径建条目时丢弃该列表
/// （`Vec::new()`），`get_peer_info` 回落单地址后 `select_best_address`
/// 的 `len<=1` 短路使子网匹配智能选址从未运行——多网卡环境下对端只能
/// 拿 primary（枚举运气）盲拨不可达网段。保全全量后多地址 failover
/// 与子网优选恢复工作。
#[derive(Debug, Clone)]
pub struct RealNodeInfo {
    pub id: String,
    pub name: String,
    pub address: String,
    /// 对端真实 RPC 端口（announce 携带）。>0 时随占位升级显式落盘
    /// peers.toml（`rpc_port` 字段），不再依赖 `udp+10000` 约定推导。
    pub rpc_port: u16,
    /// 全量自报地址（host 形态，无端口）；空 = 来源未携带（旧行为回落单地址）。
    pub addresses: Vec<String>,
    pub role: nemesis_types::cluster::NodeRole,
    pub category: String,
    pub capabilities: Vec<String>,
    pub tags: Vec<String>,
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

// 覆盖率补充批次：start 配置臂 / 占位升级 / handler 注册 / 安全网超时。
#[cfg(test)]
mod cov_tests;

// 覆盖率补充批次：RPC 全栈往返（假对端 Frame 协议）。
#[cfg(test)]
mod cov_net_tests;

#[cfg(test)]
mod node_name_tests;
