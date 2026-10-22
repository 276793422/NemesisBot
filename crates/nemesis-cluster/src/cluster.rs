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
    /// G5+P1（T19 二层根修）：入站接收者计数。broadcast send 对「无接收
    /// 者」是静默丢弃（publish_inbound 返回 ()），恢复链路 publish 前需
    /// 探测订阅是否就位（gateway 装配顺序里 AgentLoop 订阅晚于
    /// cluster.start() 的首 tick，实测晚 ~131ms）。
    fn inbound_receiver_count(&self) -> usize;
}

// ---------------------------------------------------------------------------
// Cluster
// ---------------------------------------------------------------------------

/// Default ports and intervals (matching Go implementation).
pub const DEFAULT_UDP_PORT: u16 = 11949;
pub const DEFAULT_RPC_PORT: u16 = 21949;
pub const DEFAULT_BROADCAST_INTERVAL: Duration = Duration::from_secs(30);

/// agent 实体清单提供者（P2 2026-10-16）：nemesisbot 装配期注入，闭包内
/// 现读 AgentManager（fresh read）。放 Cluster 侧只依赖本 crate 类型，
/// 不反向依赖 nemesisbot。
pub type AgentRosterProvider =
    std::sync::Arc<dyn Fn() -> Vec<crate::types::AgentAnnouncement> + Send + Sync>;

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
    /// 自报职能清单（集群专业职能框架 M2；peers.toml `[node].professions`
    /// 种入，announce 携带；M6 起经身份编辑热改）。
    professions: parking_lot::RwLock<Vec<String>>,
    /// 节点档位（role_tier 口径；peers.toml `[node].tier` 种入，手工声明）。
    tier: parking_lot::RwLock<Option<String>>,
    /// 自报运行时后端形态（P5 第 4 步 ③；peers.toml `[node].runtime_backend`
    /// 种入，手工声明；announce 携带。本批只宣告不做 matcher 闸，P6 上位）。
    runtime_backend: parking_lot::RwLock<Option<String>>,
    /// 自报 agent 实体清单提供者（P2 2026-10-16）。闭包每次现读实体注册
    /// 表（fresh read，不缓存快照）——announce / get_info 拿到的都是当前
    /// 值，运行期增删 agent 无需回写。`None` = 未宣告（announce 不带
    /// agents，对端 serde default 兼容）。Arc 包装：get_info 的静态捕获
    /// 闭包持 Arc 读活值（后 set 的提供者也能被看到）。
    agent_roster: std::sync::Arc<parking_lot::RwLock<Option<AgentRosterProvider>>>,
    /// Dynamic capabilities reported by the AgentLoop (tool names).
    /// Set via `set_capabilities()` after the agent is built.
    /// Wrapped in Arc for sharing with RPC handler closures (real-time reads).
    capabilities: Arc<std::sync::Mutex<Vec<String>>>,
    /// 环境能力声明（NB-122 批 ③ 数据面 2026-10-16）：peers.toml
    /// `[node].capabilities` 自报段种入；`set_capabilities` 时与工具名
    /// 并集进宣告面（`[peers.X]` 静态段不消费——那是当年因「配置了也
    /// 不生效」删除的字段，本字段只认自报段）。
    static_capabilities: parking_lot::RwLock<Vec<String>>,

    // -- Paths --
    workspace: PathBuf,
    #[allow(dead_code)]
    static_config_path: PathBuf,
    dynamic_state_path: PathBuf,

    // -- Components --
    registry: Arc<PeerRegistry>,
    task_manager: Arc<TaskManager>,
    /// P1 统一任务层：tasks.db 连接（None = `Cluster::new` 测试路径内存态；
    /// 打不开/损坏 = 同样 None 但 task_manager 挂 PoisonedTaskStore——
    /// fail-closed 响亮报错，不静默回内存）。
    task_db: Option<Arc<nemesis_taskstore::TaskDb>>,
    cont_store: Arc<ContinuationStore>,
    result_store: Arc<TaskResultStore>,
    rpc_client: Mutex<Option<Arc<RpcClient>>>,
    /// RPC server instance.
    rpc_server: Option<Arc<crate::rpc::server::RpcServer>>,
    /// A1：iroh 漫游端点（feature=cluster-roaming；`setup_roaming` 装配，
    /// `stop()` 关停）。None = 未启用（无 roaming.key / 装配失败 / 未编译）。
    #[cfg(feature = "cluster-roaming")]
    roaming_endpoint: Mutex<Option<Arc<crate::roaming::endpoint::RoamingEndpoint>>>,
    /// A1：本节点 iroh 漫游身份 hex（`setup_roaming` 成功后写入；announce
    /// 携带，对端据此把 `iroh:<hex>` 追加进拨号候选池）。None = 未启用
    /// 漫游叠加层（announce 不带字段，wire 兼容）。
    roaming_id: parking_lot::RwLock<Option<String>>,
    /// 自建中继形态的完整拨号地址（`iroh:<hex>@<relay_url>`；`setup_roaming`
    /// 成功后写入，announce `addresses` 携带）。Minimal 基座零发现层，裸
    /// node_id 无解析路径——对端需要完整形态才能 bridge → `with_relay_url`。
    /// 空 = N0 形态（裸候选经 `roaming_id` 字段，N0 发现层解析）。
    roaming_dialable: parking_lot::RwLock<Vec<String>>,
    /// rendezvous 名册客户端状态快照（2026-10-22 自动发现；`setup_roaming`
    /// 自建中继形态 spawn 的同步循环每 tick 覆写；WSAPI roaming.status 读取）。
    /// None = 循环未挂（N0 形态 / 推导失败 / 未启用漫游）。
    #[cfg(feature = "cluster-roaming")]
    rendezvous_status:
        parking_lot::RwLock<Option<crate::roaming::rendezvous::RendezvousClientState>>,
    /// A1 运行时硬化批：本节点构建号（git 短 hash；announce 携带）。由
    /// 宿主（nemesisbot gateway / 轻量节点命令）在构造后经
    /// [`Cluster::set_build_hash`] 注入——编译期常量 `NEMESISBOT_GIT_COMMIT`
    /// 住在 nemesisbot，cluster crate 不重复抽 git。空 = 未宣告。
    build_hash: parking_lot::RwLock<String>,
    /// Arc 自根槽（弱引用；gateway 装配在 `Arc::new(cluster)` 后经
    /// [`Cluster::install_self_weak`] 安装一次）。RPC handler 闭包与后台
    /// 循环经它活读自身身份——构造期只有 `&self`（无 Arc），安装先后无关
    /// （闭包捕获的是共享槽 Arc，后装也能看到）。
    self_weak: Arc<Mutex<Option<std::sync::Weak<Cluster>>>>,
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
            professions: parking_lot::RwLock::new(Vec::new()),
            tier: parking_lot::RwLock::new(None),
            runtime_backend: parking_lot::RwLock::new(None),
            agent_roster: std::sync::Arc::new(parking_lot::RwLock::new(None)),
            capabilities: Arc::new(std::sync::Mutex::new(Vec::new())),
            static_capabilities: parking_lot::RwLock::new(Vec::new()),
            workspace: workspace.clone(),
            static_config_path: cluster_dir_path.join("peers.toml"),
            dynamic_state_path: cluster_dir_path.join("state.toml"),
            registry: Arc::new(PeerRegistry::new(HealthConfig::default())),
            task_manager: Arc::new(TaskManager::new()),
            // 测试路径（cwd 工作区）：内存态保留——tasks.db 属生产路径换装。
            task_db: None,
            cont_store: Arc::new(ContinuationStore::new(cluster_dir_path.join("rpc_cache"))),
            result_store: Arc::new(TaskResultStore::new(1000)),
            rpc_client: Mutex::new(None),
            rpc_server: None,
            #[cfg(feature = "cluster-roaming")]
            roaming_endpoint: Mutex::new(None),
            roaming_id: parking_lot::RwLock::new(None),
            roaming_dialable: parking_lot::RwLock::new(Vec::new()),
            #[cfg(feature = "cluster-roaming")]
            rendezvous_status: parking_lot::RwLock::new(None),
            build_hash: parking_lot::RwLock::new(String::new()),
            self_weak: Arc::new(Mutex::new(None)),
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
        // 职能框架 M2：peers.toml [node] 声明种入（announce 携带 + matcher
        // 数据源；身份编辑热改走 setter，M6 接线）。
        let professions_default = sc
            .as_ref()
            .map(|s| s.node.professions.clone())
            .unwrap_or_default();
        let tier_default = sc.as_ref().and_then(|s| s.node.tier.clone());
        // P5 第 4 步 ③：peers.toml [node] 自报运行时后端形态种入（announce
        // 携带；手工声明不自动推导，同 tier 先例）。
        let runtime_backend_default = sc.as_ref().and_then(|s| s.node.runtime_backend.clone());
        // NB-122 批 ③ 数据面：`[node].capabilities` 自报段种入（环境事实
        // 能力词；set_capabilities 时与工具名并集进宣告面）。
        let static_caps_default = sc
            .as_ref()
            .map(|s| s.node.capabilities.clone())
            .unwrap_or_default();

        // P1 统一任务层：生产路径任务真相源换 tasks.db（SQLite）——内存态根除，
        // gateway 重启后在途任务/审批可恢复。打不开/损坏 = fail-closed（挂
        // PoisonedTaskStore，任务操作响亮报错，不静默退回内存假装持久化；
        // ChatSecretStore 损坏熔断同款纪律）。`Cluster::new` 测试路径保持内存态。
        let tasks_db_path = nemesis_path::resolve_tasks_db_path_in_workspace(&workspace);
        let (task_db, task_manager) = match nemesis_taskstore::TaskDb::open(&tasks_db_path) {
            Ok(db) => {
                let store = nemesis_taskstore::SqliteTaskStore::from_db(db.clone());
                (Some(db), Arc::new(TaskManager::with_store(Arc::new(store))))
            }
            Err(e) => {
                tracing::error!(
                    "[Cluster] tasks.db unavailable at {}, task persistence fail-closed \
                     (task ops will error honestly): {e}",
                    tasks_db_path.display()
                );
                (
                    None,
                    Arc::new(TaskManager::with_store(Arc::new(
                        crate::task_manager::PoisonedTaskStore::new(e.clone()),
                    ))),
                )
            }
        };

        Self {
            node_id: node_id.clone(),
            node_name: parking_lot::RwLock::new(node_name_default),
            node_name_locked: std::sync::atomic::AtomicBool::new(name_locked),
            node_type: "agent".into(),
            address: config.bind_address.clone(),
            role: parking_lot::RwLock::new(role_default),
            category: parking_lot::RwLock::new(category_default),
            tags: parking_lot::RwLock::new(tags_default),
            professions: parking_lot::RwLock::new(professions_default),
            tier: parking_lot::RwLock::new(tier_default),
            runtime_backend: parking_lot::RwLock::new(runtime_backend_default),
            agent_roster: std::sync::Arc::new(parking_lot::RwLock::new(None)),
            capabilities: Arc::new(std::sync::Mutex::new(Vec::new())),
            static_capabilities: parking_lot::RwLock::new(static_caps_default),
            workspace: workspace.clone(),
            static_config_path: nemesis_path::resolve_cluster_peers_path_in_workspace(&workspace),
            dynamic_state_path: nemesis_path::resolve_cluster_state_path_in_workspace(&workspace),
            registry: Arc::new(PeerRegistry::new(HealthConfig::default())),
            task_manager,
            task_db,
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
            #[cfg(feature = "cluster-roaming")]
            roaming_endpoint: Mutex::new(None),
            roaming_id: parking_lot::RwLock::new(None),
            roaming_dialable: parking_lot::RwLock::new(Vec::new()),
            #[cfg(feature = "cluster-roaming")]
            rendezvous_status: parking_lot::RwLock::new(None),
            build_hash: parking_lot::RwLock::new(String::new()),
            self_weak: Arc::new(Mutex::new(None)),
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
            professions: self.professions.read().clone(),
            tier: self.tier.read().clone(),
            agents: self.current_agent_roster(),
            runtime_backend: self.runtime_backend.read().clone(),
            build: self.build_hash.read().clone(),
            node_type: self.node_type.clone(),
        };
        self.registry.upsert(local_node);

        // Initialize RPC client with peer resolver backed by our registry.
        // If a client was already set via set_rpc_client(), this is a no-op.
        if self.rpc_client.lock().is_none() {
            let resolver = Arc::new(ClusterPeerResolver {
                registry: self.registry.clone(),
                node_id: self.node_id.clone(),
                rpc_port: self.rpc_port,
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
                professions: pc.professions.clone(),
                tier: pc.tier.clone(),
                // agents 不持久化到 state.toml（运行时清单，周期 announce
                // 自会重学——空 = 未宣告，绝不因回读假造清单）。
                agents: Vec::new(),
                // runtime_backend 与 agents 相反是静态身份——持久化并回读
                // （P5 第 4 步 ③，同 professions/tier）。
                runtime_backend: pc.runtime_backend.clone(),
                // build 同纪律：回读种入的 Offline 占位不带构建号（空 =
                // 未宣告），下次 announce/探针到达自会补齐。
                build: String::new(),
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

        // L3 语义矩阵（2026-09-27）：enabled + token = 加密广播（主路径）；
        // enabled + token 空 = **明文广播运行**（局域网内任何人都可侦听/伪造
        // announce）——这是既有的向后兼容行为，不拒绝启动，但必须一次性
        // WARN 讲清楚，避免"配了 cluster.enabled 就以为有加密"的误解。
        if secret.is_empty() {
            tracing::warn!(
                "[Cluster] 未配置 discovery token —— UDP 发现以**明文**模式运行 \
                （announce 可被局域网内任意主机侦听/伪造）。生产部署建议在 \
                 config.cluster.json 配置 token（CLI: `nemesisbot cluster token set`）。"
            );
        }

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

    /// 解密失败丢弃摘要（token 失配来源账本；discovery 未启动 = None）。
    ///
    /// L2（2026-09-27）：异 token 来源互不发现是预期安全行为，日常零日志；
    /// 诊断走此处——`cluster.status` WSAPI 的 `discovery_drops` 字段。
    pub fn discovery_decrypt_drops(&self) -> Option<crate::discovery::DecryptDropSummary> {
        let guard = self.discovery.lock();
        guard.as_ref().map(|d| d.decrypt_drop_summary())
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

    /// iroh 漫游叠加层是否在跑（feature=cluster-roaming）。供 Dashboard
    /// `cluster.roaming.status` 展示运行态；feature 关 = 恒 false。
    #[cfg(feature = "cluster-roaming")]
    pub fn roaming_active(&self) -> bool {
        self.roaming_endpoint.lock().is_some()
    }

    /// feature-off 形态：漫游层未编译，恒 false。
    #[cfg(not(feature = "cluster-roaming"))]
    pub fn roaming_active(&self) -> bool {
        false
    }

    /// 最近一次拨号路径观测（打洞直连 / 经中继转发——Dashboard 节点面
    /// 可观测）：从注册表地址抠 `iroh:<hex>` node_id，查漫游端点的拨号
    /// 路径观察槽（`"direct"` / `"relay"` / `"unknown"`）。None = 非漫游
    /// 节点 / 端点未装配 / 尚无观测（连接未发起或观察已退出）。实时读槽
    /// 不落注册表——路径是连接瞬时状态，非节点身份属性。
    #[cfg(feature = "cluster-roaming")]
    pub fn roaming_path_of(&self, node_id: &str) -> Option<String> {
        let info = self.registry.get(node_id)?;
        let hex = info.addresses.iter().find_map(|a| {
            a.strip_prefix("iroh:")
                .map(|rest| rest.split('@').next().unwrap_or(rest).to_ascii_lowercase())
        })?;
        let endpoint = self.roaming_endpoint.lock().clone()?;
        endpoint.dial_path_of(&hex)
    }

    /// feature-off 形态：漫游层未编译，恒 None。
    #[cfg(not(feature = "cluster-roaming"))]
    pub fn roaming_path_of(&self, node_id: &str) -> Option<String> {
        let _ = node_id;
        None
    }

    /// Stop the cluster. Stops discovery (joins threads) and signals shutdown.
    /// A1 P2：iroh 漫游叠加层装配（feature=cluster-roaming）。
    ///
    /// 显式开关（Dashboard/CLI 可控）：config.cluster.json `roaming.enabled`
    /// 为 true 才装配——key 只是身份材料不再是开关（P0 的「key 存在即启用」
    /// 文件 opt-in 语义由本闸取代）；缺 key 时自动生成（身份为本地自证材料，
    /// info 日志诚实记录）。缺省/显式 false = 不装配（Ok(false)），LAN 节点
    /// 零 iroh 面。装配三件事：bind iroh endpoint（N0 预设，可收可拨）、把
    /// RPC server 的入站桥挂上 ALPN accept、把端点注入 RPC client（dial
    /// 按 `iroh:` 前缀分派）。AEAD token 取 server 当前值（`load_rpc_auth_token`
    /// 在 start() 已应用——调用方必须晚于 `cluster.start()`）。
    ///
    /// 返回 Ok(true)=已启用；Ok(false)=开关未开；Err=装配失败（调用方 warn
    /// 不炸集群——漫游是叠加层，TCP 主通路不受影响）。
    #[cfg(feature = "cluster-roaming")]
    pub async fn setup_roaming(&self) -> Result<bool, String> {
        use crate::roaming::endpoint::{RoamingEndpoint, RoamingProtocolHandler};

        // 显式闸：config.cluster.json `roaming.enabled`（缺文件/缺字段/false
        // 恒不启用——单一真相源，key 在场也不再等价于开启）。
        let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.workspace);
        if !Self::roaming_enabled_in_config(&cfg_path) {
            tracing::debug!(
                "[Cluster] Roaming overlay not enabled (config.cluster.json roaming.enabled is not true)"
            );
            return Ok(false);
        }

        // 身份材料：缺 key 自动生成（load_or_create 幂等——已有则原样加载）。
        let (identity, created) =
            crate::roaming::endpoint::RoamingIdentity::load_or_create(&self.workspace)?;
        if created {
            tracing::info!(
                path = %crate::roaming::endpoint::roaming_key_path(&self.workspace).display(),
                "[Cluster] Roaming enabled but no identity key — generated one (cluster/roaming.key)"
            );
        }
        let iroh_hex = identity.node_id_hex();

        // 信任簿裁决（P3 accept 侧闸）：trust.toml 存在 = 启用 allowlist
        // 语义（revoked 一票否决 → trusted 放行 → 默认拒）；不存在 =
        // 未启用，走 token 单层语义。加载失败 = Err（信任簿坏了宁可不起
        // 叠加层，不静默放行）。
        let trust_book = crate::roaming::trust::TrustBook::load(&self.workspace)?;

        // peers.toml [node].iroh_id 静态声明对账：声明与 roaming.key 派生
        // 不一致 = 配置漂移 warn（身份仍以 key 派生为准——声明只是运营者
        // 显式记录的锚）。
        if let Ok(sc) = crate::cluster_config::load_static_config(&self.static_config_path)
            && let Some(declared) = sc
                .node
                .iroh_id
                .as_deref()
                .map(str::trim)
                .filter(|s| !s.is_empty())
        {
            let declared_norm = declared
                .strip_prefix("iroh:")
                .unwrap_or(declared)
                .to_lowercase();
            if declared_norm != iroh_hex {
                tracing::warn!(
                    declared = %declared_norm,
                    actual = %iroh_hex,
                    "[Cluster] peers.toml [node].iroh_id 与 roaming.key 派生身份不一致（配置漂移；以 key 派生为准）"
                );
            }
        }

        let bridge_handler = match self.rpc_server.as_ref() {
            Some(server) => server.roaming_conn_handler(trust_book),
            None => return Err("RPC server 未装配，iroh 入站桥无处挂载".to_string()),
        };

        // 端点装配：relay_url 空 = n0 公共中继（现行为字节不变）；非空 =
        // 自建中继回落（Minimal 基座 + Custom 中继地图——裸 `iroh:<hex>` 寻址
        // 在此形态不可用，对端地址需 `iroh:<hex>@<relay_url>` 形态，bridge
        // 解析 → with_relay_url）。可选 relay_token 与服务端 SharedToken 配对。
        // 客户端三键（relay_url/relay_token/rendezvous_url）自 2026-10-23 起
        // 迁 config.quic.json `client` 段（QUIC 的配置不丢在集群里——集群
        // 只是 QUIC 的一个消费方）。
        let quic_cfg = crate::roaming::quic_config::resolved_quic(
            &nemesis_path::resolve_quic_config_path_in_workspace(&self.workspace),
        );
        let quic_client = &quic_cfg.client;
        let endpoint = if quic_client.relay_url.trim().is_empty() {
            RoamingEndpoint::bind(&identity, true).await?
        } else {
            let trimmed_token = quic_client.relay_token.trim();
            let token = (!trimmed_token.is_empty()).then_some(trimmed_token);
            tracing::info!(
                relay_url = %quic_client.relay_url,
                "[Cluster] Roaming using self-hosted relay (Minimal base — bare iroh:<hex> addressing unavailable, peers need iroh:<hex>@<relay_url> form)"
            );
            RoamingEndpoint::bind_custom_relay(&identity, true, &quic_client.relay_url, token)
                .await?
        };
        let endpoint = Arc::new(endpoint);
        endpoint
            .serve(RoamingProtocolHandler::bridge(bridge_handler))
            .await?;

        // 出站：RPC client dial 分派按 `iroh:` 前缀走本端点。
        if let Some(client) = self.rpc_client.lock().clone() {
            client.set_roaming_endpoint(endpoint.clone());
        }

        // announce 携带本节点 iroh 身份（下一拍广播生效；对端入池拨号候选）。
        *self.roaming_id.write() = Some(iroh_hex.clone());
        // 自建中继形态：自报地址带 `@relay_url` 完整形态（Minimal 基座零
        // 发现层，裸 node_id 对端无解析路径——实测钉死在
        // `bare_node_id_dial_fails_on_self_relay_minimal_base`）；N0 形态
        // 清空——裸候选经 `roaming_id` 字段，N0 发现层解析。
        *self.roaming_dialable.write() = if quic_client.relay_url.trim().is_empty() {
            Vec::new()
        } else {
            vec![format!("iroh:{iroh_hex}@{}", quic_client.relay_url.trim())]
        };

        tracing::info!(
            node_id = %iroh_hex,
            "[Cluster] Roaming overlay active (iroh: node reachable via iroh overlay)"
        );
        *self.roaming_endpoint.lock() = Some(endpoint);

        // rendezvous 名册同步循环（自动发现，2026-10-22）：仅自建中继形态
        // （relay_url 非空）挂载——N0 公共中继无名册，保持既有手工静态条目
        // 路径。注册表地址 = relay_url host + 默认端口推导（rendezvous_url
        // 显式覆盖）；推导失败 = warn 跳过（名册是便利设施，不阻塞叠加层）。
        if !quic_client.relay_url.trim().is_empty() {
            match crate::roaming::rendezvous::resolve_registry_url(
                &quic_client.rendezvous_url,
                &quic_client.relay_url,
            ) {
                Ok(registry_url) => self.start_rendezvous_loop(
                    registry_url,
                    quic_client.relay_url.trim().to_string(),
                    &quic_client.relay_token,
                    &iroh_hex,
                ),
                Err(e) => tracing::warn!(
                    error = %e,
                    "[Cluster] rendezvous 注册表地址推导失败（自动发现不可用，其余叠加层功能不受影响）"
                ),
            }
        }
        Ok(true)
    }

    /// rendezvous 名册同步循环（30s tick；`stop_tx` 停机对称——与恢复循环
    /// 同款 subscribe；`self_weak` 懒升级——`setup_roaming` 跑在 `Arc::new`
    /// 之前，循环持 Weak 每 tick 现升级，未安装/已释放跳过本 tick）。
    ///
    /// 每 tick：register 自己 + 拉全量成员 → 非己成员经
    /// [`Self::merge_rendezvous_members`] 单一漏斗入册；成败写
    /// `rendezvous_status` 快照（观测面）。失败限频告警：首败 + 每 10 连败。
    #[cfg(feature = "cluster-roaming")]
    fn start_rendezvous_loop(
        &self,
        registry_url: String,
        relay_url: String,
        relay_token: &str,
        iroh_hex: &str,
    ) {
        let mut stop_rx = self.stop_tx.subscribe();
        let weak = self.self_weak.clone();
        let token = relay_token.trim().to_string();
        let iroh_hex = iroh_hex.to_string();
        tokio::spawn(async move {
            let client = match reqwest::Client::builder()
                .timeout(Duration::from_secs(15))
                .build()
            {
                Ok(c) => c,
                Err(e) => {
                    tracing::warn!(error = %e, "[Rendezvous] HTTP 客户端构建失败，名册同步不可用");
                    return;
                }
            };
            tracing::info!(
                registry_url = %registry_url,
                "[Rendezvous] 名册同步循环已挂载（30s tick；接入即报到、周期拉名册）"
            );
            let mut interval = tokio::time::interval(Duration::from_secs(30));
            let mut fail_streak: u64 = 0;
            loop {
                tokio::select! {
                    _ = stop_rx.recv() => return,
                    _ = interval.tick() => {}
                }
                // self_weak 懒升级：setup_roaming 时 Arc 可能尚未构造完成
                //（cluster_init.rs 先 setup_roaming 后 Arc::new + install_self_weak
                //——首 tick 之前必就绪，此处只防极端竞态）。
                let Some(cluster) = weak.lock().clone().and_then(|w| w.upgrade()) else {
                    tracing::debug!("[Rendezvous] self_weak 未安装或已释放，跳过本 tick");
                    continue;
                };
                let self_member = cluster.rendezvous_self_member(&iroh_hex, &relay_url);
                let sync_result = crate::roaming::rendezvous::sync_once(
                    &client,
                    &registry_url,
                    &token,
                    &self_member,
                )
                .await;
                let now_ms = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_millis() as u64)
                    .unwrap_or(0);
                match sync_result {
                    Ok(members) => {
                        fail_streak = 0;
                        let merged =
                            cluster.merge_rendezvous_members(&members, &iroh_hex, &relay_url);
                        if merged > 0 {
                            tracing::info!(
                                members = members.len(),
                                merged,
                                "[Rendezvous] 名册同步：新成员/变更已入册"
                            );
                        }
                        *cluster.rendezvous_status.write() =
                            Some(crate::roaming::rendezvous::RendezvousClientState {
                                registry_url: registry_url.clone(),
                                last_sync_ok: true,
                                last_sync_ms: now_ms,
                                member_count: members.len(),
                                last_error: String::new(),
                            });
                    }
                    Err(e) => {
                        fail_streak += 1;
                        if fail_streak == 1 || fail_streak.is_multiple_of(10) {
                            tracing::warn!(
                                streak = fail_streak,
                                error = %e,
                                "[Rendezvous] 名册同步失败（限频告警；静态 peers 不受影响）"
                            );
                        }
                        let prev_count = cluster
                            .rendezvous_status
                            .read()
                            .as_ref()
                            .map(|s| s.member_count)
                            .unwrap_or(0);
                        *cluster.rendezvous_status.write() =
                            Some(crate::roaming::rendezvous::RendezvousClientState {
                                registry_url: registry_url.clone(),
                                last_sync_ok: false,
                                last_sync_ms: now_ms,
                                member_count: prev_count,
                                last_error: e,
                            });
                    }
                }
            }
        });
    }

    /// 本节点 → 名册注册载荷（身份字段现读——identity editor 热改下一拍生效）。
    #[cfg(feature = "cluster-roaming")]
    fn rendezvous_self_member(
        &self,
        iroh_hex: &str,
        relay_url: &str,
    ) -> crate::roaming::rendezvous::RendezvousMember {
        crate::roaming::rendezvous::RendezvousMember {
            node_id: self.node_id.clone(),
            name: self.node_name(),
            role: self.role(),
            category: self.category(),
            node_type: self.node_type.clone(),
            tags: self.tags(),
            professions: self.professions(),
            tier: self.tier(),
            iroh_id: iroh_hex.to_string(),
            build: self.build_hash.read().clone(),
            relay_url: relay_url.to_string(),
            rpc_port: self.rpc_port,
            // 本节点功能宣告：能走集群同步循环 = 集群能力在（消费侧
            // features.contains("cluster") 严格过滤的配对面）。
            features: vec!["cluster".to_string()],
            last_seen_ms: 0,
        }
    }

    /// 名册成员合并（单一漏斗 [`Self::handle_discovered_node_ex`]，announce/
    /// 探针身份同款入口）：过滤自己；tags 加 `via-relay` 哨兵（镜像
    /// via-bridge 先例）；地址 = `iroh:<hex>@<relay_url>`（成员未自报时以
    /// 本方 relay_url 兜底）。返回 upsert 判变为 true 的条数（首见/真变更
    /// ——content_eq 不含 last_seen，稳态每 tick 恒 false 不刷日志）。
    #[cfg(feature = "cluster-roaming")]
    fn merge_rendezvous_members(
        &self,
        members: &[crate::roaming::rendezvous::RendezvousMember],
        self_iroh_hex: &str,
        fallback_relay_url: &str,
    ) -> usize {
        let mut merged = 0;
        for m in members {
            if m.iroh_id == self_iroh_hex {
                continue; // 自己
            }
            // features 严格过滤（2026-10-23 拍板「不兼容」）：名册是全量
            // 登记面，集群视图只收宣告 cluster 的成员——缺宣告（旧版设备
            // /非集群设备）= 本节点集群视图不可见（云端全量名册仍可见），
            // 不猜不兜底。
            if !m.features.iter().any(|f| f == "cluster") {
                continue;
            }
            let mut tags = m.tags.clone();
            if !tags
                .iter()
                .any(|t| t == crate::roaming::rendezvous::VIA_RELAY_TAG)
            {
                tags.push(crate::roaming::rendezvous::VIA_RELAY_TAG.to_string());
            }
            let address = crate::roaming::rendezvous::member_dial_address(m, fallback_relay_url);
            let accepted = self.handle_discovered_node_ex(
                &m.node_id,
                &m.name,
                vec![address],
                m.rpc_port,
                &m.role,
                &m.category,
                tags,
                Vec::new(),
                &m.node_type,
                m.professions.clone(),
                m.tier.clone(),
                Vec::new(),
                Some(m.iroh_id.clone()),
                None,
                None,
                &m.build,
            );
            if accepted {
                merged += 1;
                tracing::info!(
                    node_id = %m.node_id,
                    name = %m.name,
                    "[Rendezvous] 名册成员入册（via-relay）"
                );
            }
        }
        merged
    }

    /// rendezvous 客户端状态快照（WSAPI roaming.status 的 rendezvous 段数据源）。
    #[cfg(feature = "cluster-roaming")]
    pub fn rendezvous_status_snapshot(
        &self,
    ) -> Option<crate::roaming::rendezvous::RendezvousClientState> {
        self.rendezvous_status.read().clone()
    }

    /// A1 运行时硬化批：注入本节点构建号（git 短 hash）。宿主（nemesisbot
    /// gateway / 轻量节点命令）在 `Cluster::with_workspace` / `Cluster::new`
    /// 之后调用——编译期常量 `NEMESISBOT_GIT_COMMIT` 住在 nemesisbot，cluster
    /// crate 不能反向依赖。空串 = 未宣告（announce 不带 build 语义）。
    pub fn set_build_hash(&self, h: &str) {
        *self.build_hash.write() = h.to_string();
    }

    /// 漫游开关判定（纯函数）：config.cluster.json 顶层 `roaming.enabled`
    /// 严格为 JSON true 才开。缺文件 / 坏 JSON / 缺字段 / false → false。
    /// 实现委托 `roaming::config::resolved_roaming`（`roaming` 段的单一
    /// 真相源——setup_roaming 装配闸与 WSAPI/CLI 状态面同读此形；段内自
    /// 2026-10-23 起只剩 enabled 一个键，QUIC 客户端/服务端配置已迁
    /// config.quic.json）。
    #[cfg(feature = "cluster-roaming")]
    pub fn roaming_enabled_in_config(config_path: &std::path::Path) -> bool {
        crate::roaming::config::resolved_roaming(config_path).enabled
    }

    // QUIC 中继生命周期已迁出集群（2026-10-23 解耦）：中继句柄与启停由
    // [`crate::roaming::relay_manager::QuicRelayManager`] 承载、宿主
    //（nemesisbot gateway / 独立 `quic relay` 进程）直接持有——集群关闭/
    // 未启用时中继照起，集群 `stop()` 也不再触碰中继。配置真相源 =
    // `config.quic.json` `relay` 段。
    pub fn stop(&self) {
        *self.running.write() = false;

        // Stop RPC server first — reject new connections, existing connections
        // drain naturally via idle_timeout.
        if let Some(ref server) = self.rpc_server
            && let Err(e) = server.stop()
        {
            tracing::warn!(error = %e, "[Cluster] RPC server stop error");
        }

        // A1：iroh 漫游端点关停（router 收流 + endpoint 关闭；无 runtime 时
        // 句柄即弃——没有 runtime 就没有在跑的 QUIC 任务，无需显式 close）。
        // QUIC 中继不在本停臂（2026-10-23 解耦——中继宿主 QuicRelayManager
        // 自行停机，集群停/启不影响中继）。
        #[cfg(feature = "cluster-roaming")]
        if let Some(endpoint) = self.roaming_endpoint.lock().take()
            && let Ok(h) = tokio::runtime::Handle::try_current()
        {
            h.spawn(async move { endpoint.shutdown().await });
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
                        // G5+P1（T19 根修）：bus / delivery_cb 以句柄传入，
                        // poll 内部分支取用时现读——不再做 tick 级快照
                        //（首 tick 快照必早于 set_message_bus 注入窗口，
                        // 见 poll_stale_pending_tasks 签名注释）。
                        poll_stale_pending_tasks(
                            &task_manager,
                            &call_fn,
                            rpc_client.as_deref(),
                            safety_net,
                            &bus,
                            first_tick,
                            &on_recovered_delivery,
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
        // 自根弱引用槽：探针成功时把响应携带的 announce 身份落册（TCP fallback）
        let self_weak_slot = self.self_weak.clone();

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
                                    // 身份回传（TCP fallback，2026-10-10）：ping 响应携带
                                    // announce（与 UDP announce 同构的 additive 字段）时，
                                    // 经 apply_probe_announce 走 handle_discovered_node_ex
                                    // 同一入册漏斗。UDP 静默拓扑（CI ghost 进程占端口、
                                    // AP 隔离、跨网段多宿主）里这是身份传播的唯一可靠
                                    // 通道——否则静态 peers.toml 入册的条目会带着创建期
                                    // 的空身份冻结到永远（U 家族 9 连红根因）。
                                    if let Some(ann) = resp.result.as_ref().and_then(|r| r.get("announce"))
                                        && let Some(c) = self_weak_slot.lock().clone().and_then(|w| w.upgrade())
                                    {
                                        c.apply_probe_announce(ann);
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
    /// 是否 iroh 形态地址——**形态判定是纯字符串语义**（`iroh:` 前缀），
    /// 不依赖漫游栈编译：feature 关的节点同样会**收到**漫游节点的 announce
    /// （混合版本农场），按 TCP 口径拼 `:rpc_port` 会把完整地址损坏成
    /// `iroh:<hex>@http://...:50591`（`test_discovered_node_relay_qualified_
    /// overlay_address_pooling` 形态一钉死）。识别后原样收录即可——拨号
    /// 由候选分派裁决，本节点未编译漫游栈时该候选自然失败，与地址损坏
    /// 是两回事。
    fn addr_is_roaming(a: &str) -> bool {
        crate::is_roaming_address(a)
    }

    /// A1 双形态：iroh 漫游身份 hex 校验归一（64 位 hex，大小写不敏感，
    /// 统一小写）。feature 关时恒 `None`——未编译时入池 iroh 候选只会
    /// 浪费拨号预算（诚实裁剪）。
    #[cfg(feature = "cluster-roaming")]
    fn valid_iroh_id(s: Option<String>) -> Option<String> {
        let s = s?;
        let trimmed = s.trim();
        if trimmed.is_empty() {
            return None;
        }
        match crate::roaming::endpoint::node_id_from_hex(trimmed) {
            Ok(_) => Some(trimmed.to_lowercase()),
            Err(e) => {
                tracing::debug!(value = %trimmed, error = %e,
                    "[Cluster] Ignoring invalid iroh_id in announce");
                None
            }
        }
    }
    #[cfg(not(feature = "cluster-roaming"))]
    fn valid_iroh_id(_s: Option<String>) -> Option<String> {
        None
    }

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
        self.handle_discovered_node_impl(
            node_id,
            name,
            addresses,
            rpc_port,
            role,
            category,
            tags,
            capabilities,
            node_type,
            Vec::new(),
            None,
            Vec::new(),
            // 旧 9 参回调无 runtime_backend → None = 未宣告（诚实，不猜）。
            None,
            None,
            None,
            "",
        )
    }

    /// [`Self::handle_discovered_node`] 扩展形态：带对端自报职能、档位与
    /// agent 实体清单（集群专业职能框架 M2 + P2 agent roster）。生产 UDP
    /// 发现路径（announce 接收）走本方法；其余路径（桥接/静态）无扩展
    /// 数据，走旧签名。
    ///
    /// `source_addr`：R5 源地址收割——UDP announce 的包源 IP（发现层亲眼
    /// 可见的可达性证据；载荷地址可能因对端枚举缺口缺失，如 if_addrs 的
    /// APIPA 缺口）。作为最低优先级拨号候选并入地址池；非 UDP 路径
    /// （TCP 探针身份回传/静态装载）传 `None`。
    ///
    /// `iroh_id`：对端自报 iroh 漫游身份 hex（A1；hex 校验合法且 feature
    /// 编译时才入池——拼 `iroh:<hex>` 作拨号候选，池尾追加不抢直连优先级；
    /// 非法值/未编译 = 忽略，debug 留痕）。
    #[allow(clippy::too_many_arguments)]
    pub fn handle_discovered_node_ex(
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
        professions: Vec<String>,
        tier: Option<String>,
        agents: Vec<crate::types::AgentAnnouncement>,
        iroh_id: Option<String>,
        runtime_backend: Option<String>,
        source_addr: Option<&str>,
        build: &str,
    ) -> bool {
        self.handle_discovered_node_impl(
            node_id,
            name,
            addresses,
            rpc_port,
            role,
            category,
            tags,
            capabilities,
            node_type,
            professions,
            tier,
            agents,
            iroh_id,
            runtime_backend,
            source_addr,
            build,
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn handle_discovered_node_impl(
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
        professions: Vec<String>,
        tier: Option<String>,
        agents: Vec<crate::types::AgentAnnouncement>,
        iroh_id: Option<String>,
        runtime_backend: Option<String>,
        source_addr: Option<&str>,
        build: &str,
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

        // G14 同族（2026-10-13 本机实录）：addresses 为空时下方构造出的记录
        // address=""（直连不可达形态）。**已知节点**只合并身份字段
        // （professions/tier/capabilities/tags/role/name 照常更新），地址保留
        // 原值——身份传播（探针回传在 UDP 隔离拓扑的 purpose）不得因对端 IP
        // 枚举暂空而把可用地址抹成空（实录：静态 peer 的 127.0.0.1:<rpc> 被
        // 探针身份回传覆盖为 ""，后续 peer_chat 全部
        // `connect to : invalid socket address`）。
        // **未知节点**照常入册：空地址记录是桥接拓扑的合法形态——bridge
        // member_sync 广播的成员本就经 via-bridge 可达而非直连（新到空地址
        // 成员若被拒，桥接设备的成员注册就断了，bridge_rpc/member_sync
        // 契约测试钉此语义）。
        if addresses.is_empty()
            && let Some(existing) = self.registry.get(node_id)
        {
            let mut merged = existing;
            merged.base.name = name.into();
            merged.base.role = nemesis_types::cluster::NodeRole::from_role_str(role);
            merged.base.category = category.into();
            merged.base.last_seen = chrono::Local::now().to_rfc3339();
            merged.status = NodeStatus::Online;
            merged.capabilities = capabilities;
            merged.tags = tags.clone();
            merged.professions = professions.clone();
            merged.tier = tier.clone();
            merged.agents = agents.clone();
            merged.runtime_backend = runtime_backend;
            merged.node_type = node_type.to_string();
            merged.build = build.to_string();
            // A1：iroh 漫游身份入池（新增非覆盖——空地址分支保留原地址的
            // 纪律不变；iroh 候选是新通路证据，照常追加）。
            if let Some(hex) = Self::valid_iroh_id(iroh_id) {
                let cand = format!("iroh:{hex}");
                if !merged.addresses.iter().any(|a| a == &cand) {
                    merged.addresses.push(cand);
                }
            }
            return self.registry.upsert_if_changed(merged);
        }

        // A1：iroh 形态地址自带完整寻址（node_id[+直连 socket]），拼
        // `:rpc_port` 会造出 `iroh:<hex>:<port>` 非法地址——原样作
        // primary。TCP 地址照旧拼 port。
        let primary_address = match addresses.first() {
            Some(a) if Self::addr_is_roaming(a) => a.clone(),
            Some(a) => format!("{}:{}", a, rpc_port),
            None => String::new(),
        };

        let was_known = self.registry.get(node_id).is_some();

        // R2 静态 pin 合并保护：peers.toml 显式配置的对端地址不因 announce
        // 刷新被逐出（静态配置层与发现池层此前无合并保护——Win↔Win 验收
        // 实测：可达 pin 被对端自报的不可达地址挤掉后派发即断）。池序 =
        // pinned（用户显式配置优先）→ 载荷（announce 自报序）→ 源地址。
        // **pin 前置**（多角度审计批中-2，2026-10-13）：拨号预算 = best + 3
        // 个 fallback（rpc/client.rs），载荷地址 ≥4 个且无一同网段时 pin
        // 排尾永远轮不到——「静态语义 = 用户显式配置优先」本就该排在自报
        // 载荷前面（merge_real_node_info 内的静态地址保护注释同口径）。
        // 静态语义同时锁 base.address（与 merge_real_node_info 的静态保护
        // 同源）。
        let mut pool: Vec<String> = Vec::new();
        let mut static_base_address: Option<String> = None;
        if self.registry.is_peer_static(node_id)
            && let Some(existing) = self.registry.get(node_id)
        {
            let mut pinned = existing.addresses.clone();
            // A1：iroh 形态 base.address 是原子（node_id+可选 socket），
            // 按冒号拆 host 会拆出 "iroh" 垃圾候选——iroh 形态整条入 pin。
            let host = if Self::addr_is_roaming(&existing.base.address) {
                existing.base.address.clone()
            } else {
                existing
                    .base
                    .address
                    .split(':')
                    .next()
                    .unwrap_or("")
                    .to_string()
            };
            if !host.is_empty() && !pinned.contains(&host) {
                pinned.push(host);
            }
            for a in pinned {
                if !pool.contains(&a) {
                    pool.push(a);
                }
            }
            if !existing.base.address.is_empty() {
                static_base_address = Some(existing.base.address.clone());
            }
        }
        for a in &addresses {
            if !pool.contains(a) {
                pool.push(a.clone());
            }
        }
        // R5 源地址收割：UDP 包源 IP 是「发现层亲眼可见」的可达性证据，作
        // 为最低优先级候选入池（池序 pin > 载荷 > 源地址，中-2 pin 前置见
        // 上方静态块）——对端唯一可达地址是 link-local（被对端 if_addrs 缺
        // 口排除在载荷外）时的最后一根自愈稻草。
        if let Some(src) = source_addr
            && !src.is_empty()
            && !pool.iter().any(|a| a.as_str() == src)
        {
            pool.push(src.to_string());
        }
        // A1 池尾追加：iroh 候选是跨网兜底通路（N0 发现层打洞），不抢
        // 局域网直连优先级——池序 pin > 载荷 > 源地址 > iroh。hex 校验
        // 合法（feature 编译）才入池，杜绝垃圾地址每次拨号白撞。
        if let Some(hex) = Self::valid_iroh_id(iroh_id) {
            let cand = format!("iroh:{hex}");
            if !pool.contains(&cand) {
                pool.push(cand);
            }
        }

        let node = ExtendedNodeInfo {
            base: nemesis_types::cluster::NodeInfo {
                id: node_id.into(),
                name: name.into(),
                // announce 携带对端自报 role（真相源=对端 peers.toml [node].role）；
                // 曾硬编码 Worker 导致 board.sync 的 coordinator 判定在纯 UDP
                // 发现拓扑下永远失败（补拉死循环），必须走 from_role_str 解析。
                role: nemesis_types::cluster::NodeRole::from_role_str(role),
                address: static_base_address.unwrap_or(primary_address.clone()),
                category: category.into(),
                last_seen: chrono::Local::now().to_rfc3339(),
            },
            status: NodeStatus::Online,
            capabilities,
            tags: tags.clone(),
            // Preserve all addresses for multi-address failover。
            addresses: pool,
            // clone：下方占位升级块（RealNodeInfo）仍要用 professions/tier/agents。
            professions: professions.clone(),
            tier: tier.clone(),
            agents: agents.clone(),
            runtime_backend: runtime_backend.clone(),
            build: build.to_string(),
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
            // 本机自条目永不作占位消费（2026-10-21 同机双节点实测缺陷）：
            // 对端自报主地址与本机地址相同（同机多实例同 rpc_port / NAT 共
            // 出口 IP+端口）时，地址比对臂会把 registry 的本机自条目当
            // stale 占位吞掉——自条目被删、peers.toml [peers.{对端}] 被写成
            // 本机 name + UDP 派生端口、对端单播从此打回本机 socket（发现
            // 自环，G2 探针只能走 overlay）。自条目不参与占位匹配。
            let local_node_id = self.node_id.clone();
            let placeholders: Vec<(String, String, Vec<String>)> = self
                .registry
                .list_peers()
                .into_iter()
                .filter(|p| {
                    if p.base.id == node_id {
                        return false;
                    }
                    if p.base.id == local_node_id {
                        return false;
                    }
                    if announce_carries_human_name
                        && p.base.id == p.base.name
                        && p.base.name == name
                    {
                        return true;
                    }
                    // 地址比对臂只消费「真占位」：静态 peers 手写条目 /
                    // id==name（占位键、保守缺省条目）/ 空 name（无身份
                    // 条目）。运行时学来的真实远端条目（announce 人类名、
                    // id≠name、非静态）不得被地址吞并——同机同 rpc_port /
                    // 共享 NAT 出口拓扑下两个真实节点地址相同，吞错 =
                    // 注册表 flapping + peers.toml 反复改写（2026-10-21
                    // 交付后复查，自条目缺陷的姊妹洞）。
                    if !(self.registry.is_peer_static(&p.base.id)
                        || p.base.id == p.base.name
                        || p.base.name.is_empty())
                    {
                        return false;
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
                        professions: professions.clone(),
                        tier: tier.clone(),
                        agents: agents.clone(),
                        runtime_backend: runtime_backend.clone(),
                        build: build.to_string(),
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
            // R2 静态 pin 合并保护：静态对端的候选池走**合并**而非整组替换
            // ——自报数组不含用户 pin 的可达地址（枚举缺口/网卡优先级）时，
            // 整组替换会把 pin 逐出（与 handle_discovered_node_impl 同纪律）。
            if !info.addresses.is_empty() {
                if self.registry.is_peer_static(&info.id) {
                    for a in &info.addresses {
                        if !existing.addresses.contains(a) {
                            existing.addresses.push(a.clone());
                        }
                    }
                } else {
                    existing.addresses = info.addresses.clone();
                }
            }
            existing.base.last_seen = chrono::Local::now().to_rfc3339();
            existing.capabilities = info.capabilities.clone();
            existing.tags = info.tags.clone();
            existing.professions = info.professions.clone();
            existing.tier = info.tier.clone();
            existing.agents = info.agents.clone();
            existing.runtime_backend = info.runtime_backend.clone();
            existing.build = info.build.clone();
            existing.node_type = info.node_type.clone();
            let static_pinned_address = if self.registry.is_peer_static(&info.id) {
                Some(existing.base.address.clone())
            } else {
                None
            };
            self.registry.upsert(existing);
            // R2 盘面臂（2026-10-21 横扫姊妹洞）：静态对端的 persist 地址与
            // 内存同纪律——本函数唯一生产调用方（web nodes.ping）每次都会走
            // 到这里，若用入参 info.address 写盘，peers.toml 手写的 pin 地址
            // 被 announce 自报 primary 覆盖（append 路径对已存在键=有意覆
            // 盖），重启后静态语义丢失——内存 2111 保住的 pin 只活到下次重
            // 启。动态条目照旧随自报刷新（T9 语义不动）。
            match static_pinned_address {
                Some(addr) if !addr.is_empty() => {
                    let mut pinned = info.clone();
                    pinned.address = addr;
                    self.persist_real_peer_to_toml(&info.id, &pinned);
                }
                _ => self.persist_real_peer_to_toml(&info.id, info),
            }
            return info.id.clone();
        }

        // 2. Placeholder by address → remove + insert under real_id.
        // 发现②/B4 强化：占位匹配不再只比对自报 primary——对端多网卡下
        // primary 可能不是用户静态配置的那个地址（真机实证：placeholder
        // 配 192.168.137.x 而自报 primary=10.103.x → 永不归一，双条目
        // 并存）。改为对自报全量地址逐个匹配，任一命中即归一。
        // 本机自条目豁免（同 announce 漏斗纪律）：对端自报地址与本机地址
        // 相同时不得把自条目当占位消费。地址匹配同样只消费「真占位」
        // （静态 / id==name / 空 name）——运行时学来的真实远端条目不得被
        // 地址吞并（同机同 rpc_port / 共享 NAT 出口拓扑防 flapping）。
        let local_node_id = self.node_id.clone();
        let placeholder_id = self
            .registry
            .find_by_address(&info.address)
            .filter(|p| {
                (self.registry.is_peer_static(&p.base.id)
                    || p.base.id == p.base.name
                    || p.base.name.is_empty())
                    && p.base.id != local_node_id
            })
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
                            .filter(|p| {
                                (self.registry.is_peer_static(&p.base.id)
                                    || p.base.id == p.base.name
                                    || p.base.name.is_empty())
                                    && p.base.id != local_node_id
                            })
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
            professions: info.professions.clone(),
            tier: info.tier.clone(),
            agents: info.agents.clone(),
            runtime_backend: info.runtime_backend.clone(),
            build: info.build.clone(),
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
    /// `persist_real_peer_to_toml`. Otherwise the placeholder is stripped AND
    /// the real entry inserted into the same in-memory doc, then persisted with
    /// **one** atomic write（T2 TOCTOU 根修：两段写之间的「两头皆无」窗口是
    /// 并发读盘方假红的根因——见函数尾注）。条目形态走
    /// [`crate::cluster_config::build_peer_entry_table`] 与追加路径同源。
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

        // T2 TOCTOU 根修（2026-10-10）：真实条目**并入同一份 doc 后单次原子
        // 写**。旧两段式（先删占位写一次，再追加真实条目又写一次）在两次写
        // 之间留出「既无占位也无真实条目」的窗口——G2 探针身份化（probe-identity
        // 改造）后升级路径触发频率大增，并发读盘的 T2 检查恰好落窗即假红。
        // 四审 #6 同族：占位条目（静态 peers 手写）的扩展字段（professions/
        // tier/tags/…）随键迁移**搬运**到新键——removed 旧 table 的非五字段
        // 并入新 entry，operator 手写职能/档位不因升级丢失。
        let mut real_entry = crate::cluster_config::build_peer_entry_table(
            &Self::rpc_to_udp_address(&info.address),
            info.role.as_role_str(),
            &info.category,
            Some(&info.name),
            info.rpc_port,
        );
        if let Some(prev) = removed.as_ref().and_then(|v| v.as_table()) {
            crate::cluster_config::merge_peer_entry_extras(&mut real_entry, prev);
        }
        peers_table.insert(real_id.to_string(), toml::Value::Table(real_entry));

        // Single atomic write of placeholder-strip + real-entry insert.
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

    /// tasks.db 连接（P1 统一任务层）。`None` = `Cluster::new` 测试路径
    /// 内存态（无台账可记）；交付门（P4）A 端据此把 worker 交付分支
    /// 证据入 deliveries 表。
    pub fn task_db(&self) -> Option<Arc<nemesis_taskstore::TaskDb>> {
        self.task_db.clone()
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

        // P1 统一任务层：结果回传 delivery 记账（deliveries 表首个消费者；
        // idempotency_key=task_id 天然去重——G5 恢复轮询/重复回调不重复记账）。
        // 记账失败只留痕不阻断回传（记账是辅助台账，回传是主链路）。
        // 项 C：任务有原始来源通道（original_channel 非空）才有出站通知
        // 义务（notify_expected）——无源任务（纯 RPC/内部）记 none，不进
        // 重试调度。
        if let Some(db) = &self.task_db {
            let payload = task.result.clone().unwrap_or(serde_json::Value::Null);
            let notify_expected = !task.original_channel.is_empty();
            match db.record_delivery(
                "cluster_result",
                Some(task_id),
                task_id,
                &payload,
                notify_expected,
            ) {
                Ok(true) => {}
                Ok(false) => {
                    tracing::debug!("[Cluster] delivery already recorded for task {task_id}")
                }
                Err(e) => {
                    tracing::warn!("[Cluster] delivery record failed for task {task_id}: {e}")
                }
            }
        }

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

    // 注：config() 已删除（LOW-19，2026-10-17 四轮审计）——返回 OnceLock
    // 静态 DEFAULT 的死 API，与实例真实配置（config.cluster.json / peers.toml
    // 装载态）零关联，唯一「调用方」是断言其返回假默认值的测试；留着只会
    // 误导读者以为能拿到运行时配置。

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

    /// 自报职能清单（集群专业职能框架 M2；真相源 peers.toml
    /// `[node].professions`，身份编辑热改写此处）。
    pub fn professions(&self) -> Vec<String> {
        self.professions.read().clone()
    }

    /// Set 自报职能清单（身份编辑；下一 announce 拍生效）。
    pub fn set_professions(&self, professions: Vec<String>) {
        *self.professions.write() = professions;
        self.sync_local_node_to_registry();
    }

    /// 注入 agent 实体清单提供者（P2 2026-10-16；gateway 装配期一次）。
    /// 闭包每次现读 AgentManager——announce / get_info 恒拿当前清单。
    /// 注入 `None` = 回到未宣告（测试形态）。
    pub fn set_agent_roster_provider(&self, provider: Option<AgentRosterProvider>) {
        *self.agent_roster.write() = provider;
    }

    /// 当前 agent 清单（未注入提供者 = 空清单 = 未宣告）。
    fn current_agent_roster(&self) -> Vec<crate::types::AgentAnnouncement> {
        self.agent_roster
            .read()
            .as_ref()
            .map(|p| p())
            .unwrap_or_default()
    }

    /// 节点档位（role_tier 口径；真相源 peers.toml `[node].tier`）。
    pub fn tier(&self) -> Option<String> {
        self.tier.read().clone()
    }

    /// Set 节点档位（身份编辑）。
    pub fn set_tier(&self, tier: Option<String>) {
        *self.tier.write() = tier;
        self.sync_local_node_to_registry();
    }

    /// 自报运行时后端形态（P5 第 4 步 ③；真相源 peers.toml
    /// `[node].runtime_backend`，手工声明不自动推导；None = 未宣告）。
    pub fn runtime_backend(&self) -> Option<String> {
        self.runtime_backend.read().clone()
    }

    /// Set 自报运行时后端形态（身份编辑；下一 announce 拍生效）。
    pub fn set_runtime_backend(&self, backend: Option<String>) {
        *self.runtime_backend.write() = backend;
        self.sync_local_node_to_registry();
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
            .or_else(|| {
                // 大小写不敏感回落（2026-09-30 F1：派发目标校验前置——人读
                // 名手工输入大小写失配是常态，文档承诺「大小写不敏感」；
                // 精确未命中再扫一轮 ASCII 折叠，歧义（两节点仅大小写异）
                // 取先匹配，实际拓扑不存在。
                self.registry
                    .list_peers()
                    .into_iter()
                    .find(|p| {
                        p.base.id.eq_ignore_ascii_case(target)
                            || p.base.name.eq_ignore_ascii_case(target)
                    })
                    .map(|p| p.base.id)
            })
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
    ///
    /// NB-122 批 ③ 数据面：与 `static_capabilities`（peers.toml
    /// `[node].capabilities` 环境能力自报）取并集——matcher 的
    /// required_capabilities 硬条件需要工具名之外的环境能力词。并集
    /// 以自报段在前（顺序稳定，重调幂等），工具名去重追加在后。
    pub fn set_capabilities(&self, caps: Vec<String>) {
        if let Ok(mut guard) = self.capabilities.lock() {
            let mut merged = self.static_capabilities.read().clone();
            for c in caps {
                if !merged.contains(&c) {
                    merged.push(c);
                }
            }
            *guard = merged;
        }
        self.sync_local_node_to_registry();
    }

    /// 本机能力清单快照（`set_capabilities` 注入的值 = `[node].capabilities`
    /// 自报段 ∪ AgentLoop 工具名）。与
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
            professions: self.professions.read().clone(),
            tier: self.tier.read().clone(),
            agents: self.current_agent_roster(),
            runtime_backend: self.runtime_backend.read().clone(),
            build: self.build_hash.read().clone(),
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
                professions: node.professions.clone(),
                tier: node.tier.clone(),
                runtime_backend: node.runtime_backend.clone(),
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
        // 共享槽 Arc：注册可能早于 install_self_weak（gateway 装配顺序），
        // 捕获槽本身而非快照——后装也能被升级到。
        let self_weak_slot = self.self_weak.clone();
        self.register_rpc_handler(
            "ping",
            Box::new(move |_payload| {
                let mut resp = serde_json::json!({
                    "status": "pong",
                    "node_id": node_id,
                    // G2: 供主动探针交叉校验对端时钟漂移（additive 字段，
                    // 旧客户端忽略）。
                    "timestamp": chrono::Utc::now().timestamp(),
                });
                // 探针身份回传（additive 字段，旧客户端忽略）：自报身份与
                // UDP announce 同构（DiscoveryMessage），对端探针成功时经
                // apply_probe_announce 走同一入册漏斗——UDP 隔离拓扑里这是
                // 身份传播的唯一可靠通道。
                let upgraded = self_weak_slot.lock().clone().and_then(|w| w.upgrade());
                if let Some(c) = upgraded {
                    match serde_json::to_value(crate::discovery::build_announce_from_callbacks(c.as_ref())) {
                        Ok(ann) => resp["announce"] = ann,
                        Err(e) => tracing::debug!(error = %e, "[Cluster] announce payload serialize failed"),
                    }
                }
                Ok(resp)
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
        let professions = self.professions.read().clone();
        let tier = self.tier.read().clone();
        // P5 第 4 步 ③：自报运行时后端形态（静态身份，构造期捕获同 tier）。
        let runtime_backend = self.runtime_backend.read().clone();
        // P2 agent roster：Arc 过闭包读活值（提供者每次现读 AgentManager，
        // 运行期增删 agent 无需重启即反映）。
        let roster_slot = self.agent_roster.clone();
        let node_type = self.node_type.clone();
        let rpc_port = self.rpc_port;
        // Dynamic fields (real-time):
        let caps_arc = self.capabilities.clone();
        self.register_rpc_handler(
            "get_info",
            Box::new(move |_payload| {
                let addresses = network::get_all_local_ips();
                let capabilities = caps_arc.lock().unwrap_or_else(|e| e.into_inner()).clone();
                let agents = roster_slot.read().as_ref().map(|p| p()).unwrap_or_default();
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
                    // 职能框架 M2：自报职能/档位（旧对端不读未知键，兼容）。
                    "professions": professions,
                    "tier": tier,
                    // P5 第 4 步 ③：自报运行时后端形态（None = 未宣告，
                    // 旧对端不读未知键，兼容）。
                    "runtime_backend": runtime_backend,
                    // P2：自报 agent 实体清单（announce 子对象同源携带）。
                    "agents": agents,
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
                if let Err(e) = tl.inject_callback(&parent_id, response) {
                    tracing::error!(
                        parent_task_id = %parent_id,
                        error = %e,
                        "[Cluster] inject_callback persisted to memory but disk persist failed — crash before resume loses the callback result"
                    );
                }
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
        let safety_net = self.task_manager.pending_timeout();
        // CD3: 交付回调与 bus 均以句柄传入（T19 根修：分支内现读，
        // set 可发生在 start 之后，同 gateway 装配顺序语义）。
        poll_stale_pending_tasks(
            &self.task_manager,
            &call_fn,
            rpc_client.as_deref(),
            safety_net,
            &self.bus,
            false,
            &self.on_recovered_delivery,
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

    /// 安装 Arc 自根弱引用（gateway 装配在 `Arc::new(cluster)` 之后调用一次）。
    ///
    /// 消费方：`ping` RPC handler（探针响应回传自报身份）与 G2 健康探针
    /// 循环（探针成功时应用对端身份）——两者都在只有 `&self` 的上下文里
    /// 注册/启动，经本槽活读自身。未安装（裸构造的单测）时全部优雅降级：
    /// ping 回裸 pong、探针只记活不应用身份。
    pub fn install_self_weak(self: &Arc<Self>) {
        *self.self_weak.lock() = Some(Arc::downgrade(self));
    }

    /// 探针身份应用：把探针响应里携带的对端自报身份（announce 同构载荷）
    /// 走 [`Self::handle_discovered_node_ex`] 单一漏斗入册。
    ///
    /// 背景（2026-10-10 ET U 系九连红根因）：身份（professions/tier）唯一
    /// 载体是 UDP announce；广播被隔离/UDP 端口被幽灵进程劫持的部署形态
    /// （CI 虚拟机实录：静态 peer + TCP 全通、announce 十九分钟零到达）里
    /// 身份永远不落地，而 TCP 探针全程健康。本方法让可靠 TCP 探针成为
    /// 身份传播的兜底通道。旧对端（响应无 announce 字段）静默跳过。
    pub(crate) fn apply_probe_announce(&self, ann: &serde_json::Value) {
        let msg: crate::discovery::DiscoveryMessage = match serde_json::from_value(ann.clone()) {
            Ok(m) => m,
            Err(e) => {
                tracing::debug!(error = %e, "[Cluster] Probe announce payload malformed, skipped");
                return;
            }
        };
        // 自报来源守卫：空 id / 自身 id（announce 同款 self-check 语义）。
        if msg.node_id.is_empty() || msg.node_id == self.node_id {
            return;
        }
        let changed = self.handle_discovered_node_ex(
            &msg.node_id,
            &msg.name,
            msg.addresses,
            msg.rpc_port,
            &msg.role,
            &msg.category,
            msg.tags,
            msg.capabilities,
            &msg.node_type,
            msg.professions,
            msg.tier,
            msg.agents,
            msg.iroh_id,
            msg.runtime_backend,
            None,
            &msg.build,
        );
        if changed {
            tracing::info!(
                node_id = %msg.node_id,
                "[Cluster] Probe carried identity update (UDP-isolated topology fallback)"
            );
            if let Err(e) = self.sync_to_disk() {
                tracing::error!(error = %e, "[Cluster] Failed to sync config after probe identity apply");
            }
        }
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
/// Offline 对端**前 12 个 tick 全探**（启动冲刺——纯中继/UDP 静默拓扑
/// （跨网自建中继，announce 永远不来）里静态 peer 的 Offline→Online 只能
/// 靠探针，降频 1/5 × 60s interval = 首次上线最长 5 分钟，用户视角等同
/// 「组网失败」；冲刺窗口内全探把首拨号提前到秒级。窗口取 12 = 默认 60s
/// 间隔下覆盖 12 分钟的人尺度启动偏差——用户先起 A 再去配 B 是常态，B 晚
/// 到 5~10 分钟仍在窗口内秒级互见），之后每 5 个 tick 探一次（1/5 降频
/// 自愈——不是放弃，是恢复手段，一次成功立即翻回 Online）。
/// `tick` 为 1-based tick 序号。
fn should_probe_peer(online: bool, tick: u64) -> bool {
    online || tick <= 12 || tick.is_multiple_of(5)
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
    // G5+P1（T19 根修）：bus / delivery_cb 传句柄不传快照——publish 与交付
    // 回调在分支内**取用时现读**。此前每 tick 开头快照一次，而首 tick 快照
    // 必早于 set_message_bus / set_on_recovered_delivery（gateway 装配顺序
    // cluster.start() 先于二者），秒级 RPC 查询跨越注入窗口后 done 分支拿
    // 过期 None 静默跳过 publish。P1 sqlite 持久化使在途任务直接出现在首
    // tick（此前 in-memory store 重启为空、G5 重建注册只能赶上第二 tick），
    // 该竞态被 T19 实证暴露。
    bus: &Arc<Mutex<Option<Arc<dyn MessageBus>>>>,
    include_young: bool,
    // CD3（2026-09-17）：恢复交付回调——done 分支 confirm 前置闸，见
    // Cluster::set_on_recovered_delivery。未注册 = 旧行为（无条件 confirm）。
    delivery_cb: &Arc<
        Mutex<Option<Arc<dyn Fn(&str, &str, &str, Option<&str>) -> bool + Send + Sync>>>,
    >,
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
            // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
            let bus_snapshot = bus.lock().clone();
            if let Some(bus) = bus_snapshot {
                publish_continuation_to_bus(
                    bus.as_ref(),
                    &task.id,
                    "",
                    "error",
                    &task.peer_id,
                    Some(&timeout_error),
                )
                .await;
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
                // 句柄现读（T19 根修）：RPC 查询跨越 set_message_bus 注入
                // 窗口时，tick 开头的旧快照是 None——必须在 publish 前重读。
                // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
                let bus_snapshot = bus.lock().clone();
                if let Some(bus) = bus_snapshot {
                    publish_continuation_to_bus(
                        bus.as_ref(),
                        &task.id,
                        &response,
                        &result_status,
                        &task.peer_id,
                        if error.is_empty() { None } else { Some(&error) },
                    )
                    .await;
                }
                // CD3（2026-09-17）：交付回调成功（或未注册回调 = 旧行为）才
                // confirm——写回成功才删 worker 端副本，宁留勿丢。回调返回
                // false（如看板写回失败）时跳过 confirm + WARN，副本走 worker
                // 端 7 天 TTL 兜底（TaskManager 已完成，下轮 poll 不会再查）。
                let delivery_ok = match delivery_cb.lock().clone() {
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
                // N3c（2026-10-12）：not_found = worker 明确说「没这任务」
                //（典型：worker 重启丢内存任务表）——确定性判死信号，board
                // 派发必须立即走交付回调（write_back FAILED + ⛔ 评论 +
                // in_review + spawn 评审），不等 dispatch_timeout sweep
                //（否则任务已死、派发行悬 dispatched、评审链最长空转 3600s）。
                // worker 端已无副本，无需 confirm；回调失败（写回竞态）时
                // 派发行仍 dispatched，sweep 照常兜底，宁留勿丢语义不变。
                if let Some(cb) = delivery_cb.lock().clone() {
                    let ok = cb(&task.id, "error", "", Some("remote task not found"));
                    if !ok {
                        tracing::warn!(
                            task_id = %task.id,
                            "[Cluster] recovered delivery callback reported failure \
                             (not_found); dispatch row stays for sweep fallback"
                        );
                    }
                }
                // G5: not_found 同样要唤醒 agent（带 error metadata），
                // 否则续行快照永远悬着直到安全网清理。句柄现读（T19 根修）。
                // 先落绑再判：MutexGuard 临时值不得跨 await（非 Send）。
                let bus_snapshot = bus.lock().clone();
                if let Some(bus) = bus_snapshot {
                    publish_continuation_to_bus(
                        bus.as_ref(),
                        &task.id,
                        "",
                        "error",
                        &task.peer_id,
                        Some("remote task not found"),
                    )
                    .await;
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
///
/// T19 二层根修（2026-10-16）：恢复首 tick 的 publish 可能早于 AgentLoop
/// 订阅总线（gateway 装配顺序 cluster.start() → … → AgentLoop，实测订阅
/// 晚 ~131ms；P1 sqlite 持久化把恢复动作从第二 tick 提到首 tick 后暴露
/// ——此前动作落 120s 后的第二 tick，订阅早已就位，两层竞态同被掩盖）。
/// broadcast send 对无接收者是**静默丢弃**（publish_inbound 返回 ()），
/// 故发布前等待接收者就位，上界 15s（150×100ms 轮询）——超时照发不吞
/// （宁试勿丢；总线侧仍有 no-receiver WARN 可观测）。
async fn publish_continuation_to_bus(
    bus: &dyn MessageBus,
    task_id: &str,
    content: &str,
    status: &str,
    source_node: &str,
    error: Option<&str>,
) {
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    while bus.inbound_receiver_count() == 0 && std::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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

    fn professions(&self) -> Vec<String> {
        self.professions.read().clone()
    }

    fn tier(&self) -> Option<String> {
        self.tier.read().clone()
    }

    fn runtime_backend(&self) -> Option<String> {
        self.runtime_backend.read().clone()
    }

    fn agent_roster(&self) -> Vec<crate::types::AgentAnnouncement> {
        self.current_agent_roster()
    }

    fn roaming_id(&self) -> Option<String> {
        self.roaming_id.read().clone()
    }

    fn roaming_overlay_addresses(&self) -> Vec<String> {
        self.roaming_dialable.read().clone()
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

    fn handle_discovered_node_ex(
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
        professions: &[String],
        tier: Option<&str>,
        agents: &[crate::types::AgentAnnouncement],
        iroh_id: Option<&str>,
        runtime_backend: Option<&str>,
        source_addr: Option<&str>,
        build: &str,
    ) -> bool {
        self.handle_discovered_node_ex(
            node_id,
            name,
            addresses.to_vec(),
            rpc_port,
            role,
            category,
            tags.to_vec(),
            capabilities.to_vec(),
            node_type,
            professions.to_vec(),
            tier.map(|s| s.to_string()),
            agents.to_vec(),
            iroh_id.map(|s| s.to_string()),
            runtime_backend.map(|s| s.to_string()),
            source_addr,
            build,
        )
    }

    fn build_hash(&self) -> String {
        self.build_hash.read().clone()
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
    /// 本节点 RPC 端口（R4 自投递防护：候选端点 ip:port 与自身 endpoint
    /// 全匹配 = 注册表混池污染面，剔除——只按裸 IP 判定会误杀同机多节点
    /// 拓扑，端口是它们的唯一区分维度）。
    rpc_port: u16,
}

/// R4 纯函数核：候选端点是否落在本节点自身 endpoint 上。
///
/// 判定 = **端口全匹配** 且（主机为回环 或 主机在本机 IP 集合内）。
/// 同机多节点（cluster-uat / 本地双实例）靠端口区分——裸 IP 判定会把
/// 合法同机对端误杀。`host` 允许携带端口（取 host 部分）。
pub(crate) fn is_self_endpoint(host: &str, port: u16, own_port: u16, own_ips: &[String]) -> bool {
    if port != own_port {
        return false;
    }
    let host_only = host.split(':').next().unwrap_or(host);
    if let Ok(ip) = host_only.parse::<std::net::IpAddr>()
        && ip.is_loopback()
    {
        return true;
    }
    own_ips.iter().any(|o| o.eq_ignore_ascii_case(host_only))
}

impl PeerResolver for ClusterPeerResolver {
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)> {
        // 1. Direct lookup by key (e.g. "Node-A" or a node_id)
        if let Some(info) = self.registry.get(peer_id) {
            return self.resolve_dial_candidates(&info);
        }

        // 2. Fallback: scan all peers for matching node_id or name.
        //    This handles cases where the caller uses a node_id (e.g. "node-laptop-xxx")
        //    but the registry key is a peer name (e.g. "Node-A").
        let all = self.registry.list_peers();
        for info in &all {
            if info.base.id == peer_id || info.base.name == peer_id {
                return self.resolve_dial_candidates(info);
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

impl ClusterPeerResolver {
    /// 由注册表条目推导拨号候选 `(addresses, rpc_port, is_online)`。
    ///
    /// R3 拨号兜底：`base.address`（静态 peers.toml 的地址 / announce
    /// primary）**始终参与拨号**——池非空也不丢（旧实现池非空时完全不
    /// 消费 base.address，静态 pin 被池刷新挤掉后即彻底失联）。host 去重
    /// 后追加在池尾兜底。
    ///
    /// R4 自投递防护：候选端点与本节点自身 endpoint（本机 IP × 本节点
    /// RPC 端口，含回环）全匹配 = 注册表把本节点历史上报地址混进对端池
    /// （Win↔Win 验收实证的自投递劫持面：同端口布局下拨回自己 RPC
    /// server，dispatch 报成功实际本节点执行、交付作者身份漂移）。剔除
    /// 后全空 = 该对端在非自端点意义上不可解析，返回 `None`（诚实失败，
    /// 上层派发失败路径照常留痕），同时 WARN 留诊断现场。
    ///
    /// 自派豁免：条目 id 与本节点 id **全等** = 显式自投递（file: 锚点单
    /// 「只能自派」拓扑约束的合法形态），端点自碰撞是预期而非池污染——
    /// 原样放行全部候选，拨回本节点 RPC server 走正常 peer_chat 语义
    /// （作者=执行者，无身份漂移；任务完成回调按 source_node_id 回程，
    /// 同一豁免覆盖）。防护的真实目标是「别人的单子误投给自己」（id
    /// 不等但候选全撞本地端点），豁免只钉住 id 全等这一条路。
    fn resolve_dial_candidates(&self, info: &ExtendedNodeInfo) -> Option<(Vec<String>, u16, bool)> {
        let is_online = info.status == NodeStatus::Online;
        // A1（QUIC 中继钉址实测 2026-10-21）：iroh 形态 base.address 是原子
        // （node_id+可选 relay_url），按冒号拆 host 会拆出
        // `iroh:<hex>@http://127.0.0.1` 截断垃圾候选（全址 best 失败后 junk
        // 白撞一次拨号 + 失败链 last error 用垃圾地址误导排障）——与
        // handle_discovered_node_impl pin 路径（:1739 同病同修「整条入 pin」）
        // 同族。iroh 形态整条作 host 候选参与去重，port=0（iroh 地址内无
        // RPC 端口语义；`:33440` 是中继端口，绝不能当 rpc_port 派生给
        // host-only 候选拼对）。
        let (host, port) = if crate::is_roaming_address(&info.base.address) {
            (info.base.address.clone(), 0)
        } else {
            parse_host_port(&info.base.address)
        };
        let mut addresses = info.addresses.clone();
        // iroh 原子 host 按整串相等去重（`split(':').next()` 对 iroh 形态
        // 恒得 "iroh"，会把整条重复候选拱进池）。
        let host_in_pool = if crate::is_roaming_address(&host) {
            addresses.iter().any(|a| a == &host)
        } else {
            addresses
                .iter()
                .any(|a| a.split(':').next() == Some(host.as_str()))
        };
        if !host.is_empty() && !host_in_pool {
            addresses.push(host);
        }

        if info.base.id == self.node_id {
            tracing::info!(
                peer_id = %info.base.id,
                own_rpc_port = self.rpc_port,
                candidates = ?addresses,
                "[Cluster] Self-entry dial exemption: explicit self-dispatch, keeping all candidates"
            );
            return Some((addresses, port, is_online));
        }

        let own_ips = network::get_all_local_ips();
        let filtered: Vec<String> = addresses
            .iter()
            .filter(|a| {
                // 裸 IP 候选的端口配对与本 resolver 返回的 rpc_port 一致
                // （dispatch 侧按同语义拼接 ip:port）；带端口条目按自带端口。
                let (h, p) = parse_host_port(a);
                let effective_port = if a.contains(':') { p } else { port };
                !is_self_endpoint(&h, effective_port, self.rpc_port, &own_ips)
            })
            .cloned()
            .collect();
        // 全滤空 WARN 覆盖对称（多角度审计批低-4 + 闭环批低-4 补全）：判据
        // = 「过滤后为空」。三形态诚实失败（返回 `None`，上层派发失败路径
        // 照常留痕）：① 过滤前有候选但全为自端点（池污染）；② 池原本为空、
        // 唯一候选是 base.host 且恰为自端点（返回 Some 空池 = 客户端拿 ""
        // 拨号一次才失败，错误文案误导为连接问题而非自碰撞）；③ 连候选都
        // 没有（announce/静态 pin 全缺）——Some 空池同样是拿 "" 空拨一次，
        // 与①②同罪，统一 None。
        if filtered.is_empty() {
            if addresses.is_empty() {
                tracing::warn!(
                    peer_id = %info.base.id,
                    "[Cluster] No dial candidates at all for peer (no address pool, \
                     no usable base address); cannot resolve"
                );
            } else {
                tracing::warn!(
                    peer_id = %info.base.id,
                    raw_candidates = ?addresses,
                    own_rpc_port = self.rpc_port,
                    "[Cluster] All dial candidates for peer collide with the local endpoint \
                     (registry address-pool pollution?); refusing to dial self"
                );
            }
            return None;
        }
        Some((filtered, port, is_online))
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
    // node_id 可能含多字节文本（中文主机名等），裸字节截断会 panic
    // （2026-10-11 横扫修复），走边界安全版。
    (
        format!(
            "Bot {}",
            nemesis_utils::string_utils::truncate_bytes_at_boundary(node_id, 8)
        ),
        false,
    )
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
    /// 对端自报职能（集群专业职能框架 M2；get_info/announce 携带）。
    pub professions: Vec<String>,
    /// 对端自报档位（role_tier 口径；None = 未宣告）。
    pub tier: Option<String>,
    /// 对端自报 agent 实体清单（P2 2026-10-16；get_info 携带；merge 进
    /// 注册表 ExtendedNodeInfo）。
    pub agents: Vec<crate::types::AgentAnnouncement>,
    /// 对端自报运行时后端形态（P5 第 4 步 ③；get_info/announce 携带；
    /// None = 未宣告）。本批只宣告不做 matcher 闸（P6 上位）。
    pub runtime_backend: Option<String>,
    /// 对端自报构建号（A1 运行时硬化批；announce/探针/get_info 携带；
    /// merge 进注册表 ExtendedNodeInfo）。不落 peers.toml（写盘手选字段，
    /// 下次 announce 自会重学）。
    pub build: String,
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
