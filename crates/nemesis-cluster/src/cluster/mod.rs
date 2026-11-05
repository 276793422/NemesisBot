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
    /// 结构化元数据（如 cluster_continuation 的 status / source_node /
    /// error）。`BusToClusterAdapter` 原样映射到 `InboundMessage.metadata`，
    /// 供 AgentLoop 的续行拦截逻辑读取。空 = 无元数据。
    pub metadata: std::collections::HashMap<String, String>,
}

/// Trait for publishing messages to the message bus.
pub trait MessageBus: Send + Sync {
    fn publish_inbound(&self, msg: BusInboundMessage);
    /// （二层根修）：入站接收者计数。broadcast send 对「无接收
    /// 者」是静默丢弃（publish_inbound 返回），恢复链路 publish 前需
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

/// agent 实体清单提供者：nemesisbot 装配期注入，闭包内
/// 现读 AgentManager（fresh read）。放 Cluster 侧只依赖本 crate 类型，
/// 不反向依赖 nemesisbot。
pub type AgentRosterProvider =
    std::sync::Arc<dyn Fn() -> Vec<crate::types::AgentAnnouncement> + Send + Sync>;

/// The cluster manages a set of nodes and distributes tasks.
pub struct Cluster {
    // -- Identity --
    node_id: String,
    node_name: parking_lot::RwLock<String>,
    /// 显示名为显式配置（config.cluster.json `node_name` 或
    /// peers.toml [node].name）时 = true——撞名后缀免疫（用户意志优先；
    /// 自动名 hostname / `Bot {id8}` 才允许撞名收敛）。
    node_name_locked: std::sync::atomic::AtomicBool,
    node_type: String,
    address: String,
    role: parking_lot::RwLock<String>,
    category: parking_lot::RwLock<String>,
    tags: parking_lot::RwLock<Vec<String>>,
    /// 自报职能清单（集群专业职能框架；peers.toml `[node].professions`
    /// 种入，announce 携带； 起经身份编辑热改）。
    professions: parking_lot::RwLock<Vec<String>>,
    /// 节点档位（role_tier 口径；peers.toml `[node].tier` 种入，手工声明）。
    tier: parking_lot::RwLock<Option<String>>,
    /// 自报运行时后端形态（peers.toml `[node].runtime_backend`
    /// 种入，手工声明；announce 携带。本批只宣告不做 matcher 闸， 上位）。
    runtime_backend: parking_lot::RwLock<Option<String>>,
    /// 自报 agent 实体清单提供者。闭包每次现读实体注册
    /// 表（fresh read，不缓存快照）——announce / get_info 拿到的都是当前
    /// 值，运行期增删 agent 无需回写。`None` = 未宣告（announce 不带
    /// agents，对端 serde default 兼容）。Arc 包装：get_info 的静态捕获
    /// 闭包持 Arc 读活值（后 set 的提供者也能被看到）。
    agent_roster: std::sync::Arc<parking_lot::RwLock<Option<AgentRosterProvider>>>,
    /// Dynamic capabilities reported by the AgentLoop (tool names).
    /// Set via `set_capabilities()` after the agent is built.
    /// Wrapped in Arc for sharing with RPC handler closures (real-time reads).
    capabilities: Arc<std::sync::Mutex<Vec<String>>>,
    /// 环境能力声明（批 ③ 数据面）：peers.toml
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
    /// 统一任务层：tasks.db 连接（None = `Cluster::new` 测试路径内存态；
    /// 打不开/损坏 = 同样 None 但 task_manager 挂 PoisonedTaskStore——
    /// fail-closed 响亮报错，不静默回内存）。
    task_db: Option<Arc<nemesis_taskstore::TaskDb>>,
    cont_store: Arc<ContinuationStore>,
    result_store: Arc<TaskResultStore>,
    rpc_client: Mutex<Option<Arc<RpcClient>>>,
    /// RPC server instance.
    rpc_server: Option<Arc<crate::rpc::server::RpcServer>>,
    /// iroh 漫游叠加层运行旗标（QUIC 插件化
    /// 端点本体搬 plugin_roaming，宿主只记「setup_roaming 装配成功 /
    /// stop() 已请求停机」——`setup_roaming` 置位，`stop()` 复位）。
    roaming_started: AtomicBool,
    /// 本节点 iroh 漫游身份 hex（`setup_roaming` 成功后写入；announce
    /// 携带，对端据此把 `iroh:<hex>` 追加进拨号候选池）。None = 未启用
    /// 漫游叠加层（announce 不带字段，wire 兼容）。
    roaming_id: parking_lot::RwLock<Option<String>>,
    /// 自建中继形态的完整拨号地址（`iroh:<hex>@<relay_url>`；`setup_roaming`
    /// 成功后写入，announce `addresses` 携带）。Minimal 基座零发现层，裸
    /// node_id 无解析路径——对端需要完整形态才能 bridge → `with_relay_url`。
    /// 空 = 形态（裸候选经 `roaming_id` 字段， 发现层解析）。
    roaming_dialable: parking_lot::RwLock<Vec<String>>,
    /// rendezvous 名册客户端状态快照（自动发现；QUIC 插件化
    /// 后由成员回流漏斗（插件名册循环每次推送）覆写；WSAPI roaming.status 读取）。
    /// None = 循环未挂（形态 / 推导失败 / 未启用漫游）。
    rendezvous_status:
        parking_lot::RwLock<Option<crate::roaming::rendezvous::RendezvousClientState>>,
    /// 名册循环装配参数缓存（QUIC 插件化：循环本体在 plugin_roaming，
    /// 宿主持权威参数——身份/宣告字段热改经 `sync_local_node_to_registry`
    /// 尾部钩子重组装重发 `registry_start`（幂等停旧起新））。None = 循环
    /// 未挂（形态 / 未启用漫游 / stop() 已清）。
    rendezvous_params: parking_lot::RwLock<Option<RendezvousParams>>,
    /// 运行时硬化批：本节点构建号（git 短 hash；announce 携带）。由
    /// 宿主（nemesisbot gateway / 轻量节点命令）在构造后经
    /// [`Cluster::set_build_hash`] 注入——编译期常量 `NEMESISBOT_GIT_COMMIT`
    /// 住在 nemesisbot，cluster crate 不重复抽 git。空 = 未宣告。
    build_hash: parking_lot::RwLock<String>,
    /// a（三期）：本节点 mandate 签名公钥（P-256 SEC1 hex；
    /// announce 携带，对端 TOFU 钉存）。构造期从 peers.toml `[node].pubkey`
    /// 静态种入（同 tier/professions 先例）；-b 落地运行时 key 源后由
    /// key 派生接管。None = 未宣告（announce 不带字段）。
    mandate_pubkey: parking_lot::RwLock<Option<String>>,
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
    /// vault fail-closed：config.cluster.json 的 `token` 若是 vault:/env:/yaml:
    /// 引用且解析失败，置位。RPC 绑定点据此拒绝启动（宁可没有 RPC，不可
    /// 无认证 RPC）；start_discovery 据此拒绝无加密发现。空串/字面量/读文件
    /// 失败不置位（见 rpc_token_missing）。
    rpc_reference_broken: AtomicBool,
    /// fail-closed：集群启用但 token 为空（字面量空串/键缺
    /// 失），置位。空 token + bind 0.0.0.0 = 明文无鉴权 RPC，局域网任意主机
    /// 可 peer_chat 驱动本节点 agent 带工具执行——与坏引用同威胁且更劣
    /// （连加密都不协商），同款处理：RPC 与 discovery 均拒绝启动。消费方
    /// 统一走 [`Cluster::rpc_auth_unavailable`]。
    rpc_token_missing: AtomicBool,
    discovery: Mutex<Option<crate::discovery::DiscoveryService>>,
    stop_tx: broadcast::Sender<()>,
    /// Arc 包一层 —— 恢复循环 spawn 时克隆 Arc，之后每 tick 重新
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

    /// 节点发现回调（Swarm 落点）：对端节点被发现/刷新时触发
    /// `(node_id, role, category)`。gateway 用它把新节点自动收编进看板
    /// 频道；first-join 语义（成员零行才入）由闭包实现方裁决——cluster
    /// 不依赖 board。未注册时为零开销 no-op。
    ///
    /// Arc 包裹： 探针循环在 spawn 出去的任务里运行（不持 &self），
    /// 必须能活读这个槽——探针复活也是「节点发现」（见
    /// `fire_recovered_callback`）。
    on_node_discovered: Arc<Mutex<Option<Arc<dyn Fn(&str, &str, &str) + Send + Sync>>>>,

    /// 恢复交付回调。恢复轮询（poll_stale_pending_tasks）
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

    /// 负载感知路由（最小版）：A 端观测的逐 peer 任务结局统计（内存态，
    /// 重启归零——announce wire 不携带、state.toml 不落盘，对端自报负载
    /// 是远期项）。主漏斗 = [`Cluster::complete_task`] / [`Cluster::fail_task`]
    /// （覆盖 gateway peer_chat_callback handler 两臂）；旁路 settle 点
    /// （恢复轮询 sweep 三分支 + 默认回调 handler 兜底臂 + 清理 tick
    /// 的 安全网判死）经 TaskManager 的路由钩子补记账，全部入口共用
    /// 同一去重集与记账核心（[`record_route_outcome_core`]）；消费方
    /// [`Cluster::pick_auto_route_target`]。
    /// Arc 包裹：构造期路由钩子闭包捕获（'static spawn / handler 闭包）。
    route_stats: Arc<Mutex<std::collections::HashMap<String, RoutePeerStats>>>,

    /// 负载感知路由：已记账任务去重集（settle 幂等重放防御）。`settle_task`
    /// 对「已是同一终态」的重复 settle 返回 true（幂等 no-op）——重复回调
    /// （送达重试/B 端重发/恢复轮询重放）会让 Cluster 层再次看到 ok=true，
    /// 无去重则同一任务双计、成功率失真。每任务只计首次 settle。
    /// Arc 包裹：同 route_stats，构造期路由钩子闭包捕获。
    route_recorded: Arc<Mutex<std::collections::HashSet<String>>>,
}

/// 单个对端节点的路由统计（负载感知路由最小版数据面）。胜负与耗时均由
/// A 端本地观测——成功率是「任务在这台节点上办成没有」，不是节点自报。
#[derive(Debug, Clone, Default)]
pub struct RoutePeerStats {
    pub completed: u64,
    pub failed: u64,
    /// 耗时样本数（created_at→completed_at 解析成功才计入——宁缺毋假）。
    pub response_samples: u64,
    pub total_response_ms: u64,
}

impl RoutePeerStats {
    /// 成功率（任务数计）；**零样本 = 1.0 乐观中性**——新节点不被饿死，
    /// 首个样本落定后回归真实排序。
    pub fn success_rate(&self) -> f64 {
        let total = self.completed + self.failed;
        if total == 0 {
            1.0
        } else {
            self.completed as f64 / total as f64
        }
    }

    /// 平均耗时 ms；零样本 = 0（中性，参与平局裁决时新节点优先——
    /// 探索性分流，拿到样本即回归真实排序）。
    pub fn avg_response_ms(&self) -> u64 {
        self.total_response_ms
            .checked_div(self.response_samples)
            .unwrap_or(0)
    }
}

/// 名册循环装配参数缓存（QUIC 插件化，循环本体在 plugin_roaming
/// 自持 runtime，宿主持权威参数）。身份/宣告字段热改经
/// `sync_local_node_to_registry` 尾部钩子重组装 member 载荷重发
/// `registry_start`（插件侧幂等停旧起新）。
#[derive(Debug, Clone)]
pub(crate) struct RendezvousParams {
    /// 名册服务 URL（resolve_registry_url 推导产物）。
    registry_url: String,
    /// 本方对外 relay_url（member.relay_url + 对端拨号兜底）。
    relay_url: String,
    /// 中继/名册共享 token（member 提取自集群 token 槽——热改即走钩子）。
    relay_token: String,
    /// 本节点 iroh 身份 hex（插件侧 identity_mismatch 闸的对账键）。
    iroh_hex: String,
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

        // 显示名解析链（config 显式 → hostname → `Bot {id8}` 兜底）。
        let (node_name_init, name_locked) = resolve_node_name_from_env(&config.node_name, &node_id);

        let cluster = Self {
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
            roaming_started: AtomicBool::new(false),
            roaming_id: parking_lot::RwLock::new(None),
            roaming_dialable: parking_lot::RwLock::new(Vec::new()),
            rendezvous_status: parking_lot::RwLock::new(None),
            rendezvous_params: parking_lot::RwLock::new(None),
            build_hash: parking_lot::RwLock::new(String::new()),
            mandate_pubkey: parking_lot::RwLock::new(None),
            self_weak: Arc::new(Mutex::new(None)),
            rpc_channel: RwLock::new(None),
            udp_port: DEFAULT_UDP_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
            running: RwLock::new(false),
            discovery_running: Arc::new(AtomicBool::new(false)),
            rpc_reference_broken: AtomicBool::new(false),
            rpc_token_missing: AtomicBool::new(false),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Arc::new(Mutex::new(None)),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            route_stats: Arc::new(Mutex::new(std::collections::HashMap::new())),
            route_recorded: Arc::new(Mutex::new(std::collections::HashSet::new())),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
            on_node_discovered: Arc::new(Mutex::new(None)),
            on_recovered_delivery: Arc::new(Mutex::new(None)),
        };
        cluster.install_route_record_hook();
        cluster
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
        // peers.toml 路径解析以 nemesis-path 为准（跨 crate 读写：本 crate /
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
        // peers.toml 存在但损坏（手改出错/历史残留/磁盘问题）
        // 时不得静默——`.ok()` 吞掉后 node_id 走新生成，ensure_node_id 对不可
        // 解析文件整体序列化覆盖，节点身份静默漂移 + [peers.*] 静态表全部丢失
        // 且零日志。NotFound（首次启动）是正常路径不打扰；其余失败 loud WARN
        // 显性化（与 tasks.db fail-closed / ChatSecretStore 熔断同款可观测纪律）。
        let sc = match crate::cluster_config::load_static_config(&peers_path) {
            Ok(sc) => Some(sc),
            Err(ConfigError::Io(ref e)) if e.kind() == std::io::ErrorKind::NotFound => None,
            Err(e) => {
                tracing::warn!(
                    path = %peers_path.display(),
                    error = %e,
                    "[Cluster] peers.toml exists but failed to parse — node identity will \
                     regenerate and the file will be overwritten (static [peers.*] tables \
                     lost). Back up / fix the file before restart if the old identity matters"
                );
                None
            }
        };
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

        // 本节点 id 手改含 `:`/`/` 时，本端发出的
        // announce/续行 marker 同样携带坏身份（对端闸会拒收我们）且 B 端
        // session_key 复合键被切坏。构造器不可 fail——loud WARN 把配置
        // 错误显性化（修复 = 手改 peers.toml id 为合法字符集后重启）。
        if !crate::cluster_config::is_valid_node_identity(&node_id) {
            tracing::warn!(
                node_id = %node_id,
                "[Cluster] 本节点 id 含非法字符（合法集：字母/数字/._-；含 ':' 或 '/' 会切坏 session_key 与续行 marker 协议）——请手改 peers.toml [node].id"
            );
        }

        // Persist runtime-generated node_id to peers.toml [node].id so it
        // remains stable across restarts. No-op if user has already set one.
        if let Err(e) = crate::cluster_config::ensure_node_id(&peers_path, &node_id) {
            tracing::warn!("[Cluster] Failed to persist node_id to peers.toml: {}", e);
        }
        // 显示名解析链。peers.toml [node].name（既有显式持久化位）
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
        // 职能框架：peers.toml [node] 声明种入（announce 携带 + matcher
        // 数据源；身份编辑热改走 setter， 接线）。
        let professions_default = sc
            .as_ref()
            .map(|s| s.node.professions.clone())
            .unwrap_or_default();
        let tier_default = sc.as_ref().and_then(|s| s.node.tier.clone());
        // peers.toml [node] 自报运行时后端形态种入（announce
        // 携带；手工声明不自动推导，同 tier 先例）。
        let runtime_backend_default = sc.as_ref().and_then(|s| s.node.runtime_backend.clone());
        // 批 ③ 数据面：`[node].capabilities` 自报段种入（环境事实
        // 能力词；set_capabilities 时与工具名并集进宣告面）。
        let static_caps_default = sc
            .as_ref()
            .map(|s| s.node.capabilities.clone())
            .unwrap_or_default();
        // a（三期）：`[node].pubkey` mandate 签名公钥种入。
        // 归一（trim/小写）+ 形态校验——非法形态 warn 一次且不进 announce
        // （分发面只放合法形态；错钥在 -b 验签时自然 loud 失败）。
        let mandate_pubkey_default = sc
            .as_ref()
            .and_then(|s| s.node.pubkey.as_deref())
            .filter(|s| !s.trim().is_empty())
            .and_then(|s| match crate::roaming::trust::normalize_mandate_pubkey(s) {
                Some(k) => Some(k),
                None => {
                    tracing::warn!(
                        pubkey = %s,
                        "[Cluster] peers.toml [node].pubkey 形态非法（期待 P-256 SEC1 hex 66/130）——本节点不宣告 mandate 公钥"
                    );
                    None
                }
            });

        // 统一任务层：生产路径任务库换 tasks.db（SQLite）——内存态根除，
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

        let cluster = Self {
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
            mandate_pubkey: parking_lot::RwLock::new(mandate_pubkey_default),
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
            // 磁盘持久化 —— B 端回调失败时结果落盘（rpc_cache/results），
            // A 端重启恢复的 poll（query_task_result）才有真实数据可查。
            // 构造期同步建目录；启动后的清扫+恢复在 ClusterService::first_start。
            result_store: Arc::new(TaskResultStore::with_disk_persistence(
                1000,
                nemesis_path::resolve_cluster_results_dir_in_workspace(&workspace),
            )),
            rpc_client: Mutex::new(None),
            rpc_server: None,
            roaming_started: AtomicBool::new(false),
            roaming_id: parking_lot::RwLock::new(None),
            roaming_dialable: parking_lot::RwLock::new(Vec::new()),
            rendezvous_status: parking_lot::RwLock::new(None),
            rendezvous_params: parking_lot::RwLock::new(None),
            build_hash: parking_lot::RwLock::new(String::new()),
            self_weak: Arc::new(Mutex::new(None)),
            rpc_channel: RwLock::new(None),
            udp_port: DEFAULT_UDP_PORT,
            rpc_port: DEFAULT_RPC_PORT,
            broadcast_interval: DEFAULT_BROADCAST_INTERVAL,
            running: RwLock::new(false),
            discovery_running: Arc::new(AtomicBool::new(false)),
            rpc_reference_broken: AtomicBool::new(false),
            rpc_token_missing: AtomicBool::new(false),
            discovery: Mutex::new(None),
            stop_tx,
            bus: Arc::new(Mutex::new(None)),
            removed_peers: parking_lot::RwLock::new(std::collections::HashSet::new()),
            route_stats: Arc::new(Mutex::new(std::collections::HashMap::new())),
            route_recorded: Arc::new(Mutex::new(std::collections::HashSet::new())),
            cluster_task_list: Mutex::new(None),
            cluster_work_queue: Mutex::new(None),
            call_with_context_fn: Mutex::new(None),
            on_node_discovered: Arc::new(Mutex::new(None)),
            on_recovered_delivery: Arc::new(Mutex::new(None)),
        };
        cluster.install_route_record_hook();
        cluster
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

        // 超时阶梯 —— pending 任务安全网 = max(24h, 2×B端LLM超时)，
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

        // TaskManager 后台清理循环：此前从未在生产接线
        // （旧 &mut self 签名无法经 Arc<TaskManager> 调用）——2h 终态清理
        // 与 pending/running 安全网兜底双双失效，终态任务（含完整
        // payload+result）在 tasks.db 无限累积。接线后 on_complete 生产恒
        // None（gateway 走 with_workspace 不挂回调），清理只落状态不重复
        // 唤醒——pending 唤醒仍由 recovery_loop 独占（settle 幂等，双循环
        // 竞争无害）。
        self.task_manager.start();

        // 主动健康探针循环（health_check_interval_secs=0 关闭）。
        if app_cfg.health_check_interval_secs > 0 {
            self.start_health_check_loop(
                Duration::from_secs(app_cfg.health_check_interval_secs),
                app_cfg.health_check_failure_threshold,
            );
        }

        // state.toml 回读（集群完备性加固）：必须在 start_sync_loop
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
}

impl Cluster {
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

        let data = match nemesis_utils::fsutil::read_config_string(&cfg_path) {
            Ok(d) => d,
            Err(e) => {
                tracing::warn!(path = %cfg_path.display(), error = %e, "[Cluster] Failed to read cluster config for token");
                return;
            }
        };

        let v = match serde_json::from_str::<serde_json::Value>(&data) {
            Ok(v) => v,
            Err(e) => {
                tracing::warn!(path = %cfg_path.display(), error = %e, "[Cluster] Failed to parse cluster config for token");
                return;
            }
        };

        let token = v
            .get("token")
            .and_then(|t| t.as_str())
            .unwrap_or("")
            .to_string();

        if token.is_empty() {
            // fail-closed：集群启用 + 空 token = 明文无鉴权
            // RPC（bind 0.0.0.0，局域网任意主机可 peer_chat 驱动本节点 agent
            // 带工具执行）。坏引用同威胁已 fail-closed（rpc_reference_broken），
            // 空 token 是同类威胁更劣形态（连加密都不协商），同款处理：RPC
            // 拒绝启动。子系统未启用（enabled 非 true）不置位——disabled 的
            // 集群本就不 bind，置位只会污染 Dashboard 闸链快照（抢在「未启
            // 用」前面误报「token 异常」）。
            if v.get("enabled").and_then(|b| b.as_bool()).unwrap_or(false) {
                self.rpc_token_missing.store(true, Ordering::SeqCst);
                tracing::error!(
                    "[Cluster] 集群已启用但 RPC auth token 为空 —— fail-closed：RPC 服务不启动（无认证 RPC = 局域网任意主机可驱动本节点 agent）。补救：运行 `nemesisbot cluster token generate --save`（或重跑 `nemesisbot cluster enable` 自动生成）"
                );
            }
            return;
        }

        // vault（计划）：token 支持 vault:/env:/yaml: 引用。
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
}

impl Cluster {
    /// vault fail-closed：config.cluster.json 的 `token` 是否为解析失败的
    /// 引用。RPC 绑定点（gateway / `commands/cluster.rs`）在 `start()` 之后
    /// 检查此项——为 true 时拒绝 bind，节点以"无 RPC 服务"状态运行而非
    /// 裸奔。空串/字面量 token 恒为 false（见 rpc_token_missing）。
    pub fn rpc_reference_broken(&self) -> bool {
        self.rpc_reference_broken.load(Ordering::SeqCst)
    }

    /// 集群启用但 token 为空。RPC 绑定点与 start_discovery
    /// 据此拒绝无认证/无加密运行（同 rpc_reference_broken 的处置）。
    pub fn rpc_token_missing(&self) -> bool {
        self.rpc_token_missing.load(Ordering::SeqCst)
    }

    /// RPC 认证不可用统一裁决（引用解析失败 **或** token 为空）：true =
    /// RPC 绑定点不 bind、start_discovery 不启动。消费方（gateway /
    /// `cluster node` CLI / Dashboard 闸链快照）一律走此入口；需要区分
    /// 具体原因时分别读 `rpc_reference_broken()` / `rpc_token_missing()`。
    pub fn rpc_auth_unavailable(&self) -> bool {
        self.rpc_reference_broken() || self.rpc_token_missing()
    }

    /// iroh 漫游叠加层是否在跑。供 Dashboard `cluster.roaming.status`
    /// 展示运行态；插件缺席/未装配 = 恒 false。
    /// QUIC 插件化：读装配旗标（端点本体在 plugin_roaming）。
    pub fn roaming_active(&self) -> bool {
        self.roaming_started.load(Ordering::SeqCst)
    }

    /// 从注册表节点条目地址抠 `iroh:<hex>` node_id hex（小写）。None = 非
    /// 漫游节点。纯字符串逻辑，无 feature 门（feature-off 时拨号路径表
    /// 恒空，消费面 `roaming_dial_paths` 查不到即自然降级为「无观测」）。
    pub fn roaming_hex_of(&self, node_id: &str) -> Option<String> {
        let info = self.registry.get(node_id)?;
        info.addresses.iter().find_map(|a| {
            a.strip_prefix("iroh:")
                .map(|rest| rest.split('@').next().unwrap_or(rest).to_ascii_lowercase())
        })
    }

    /// 最近拨号路径观测批量快照（打洞直连 / 经中继转发——Dashboard 节点面
    /// 可观测）：node_id hex → "direct"/"relay"。QUIC 插件化：观测槽在
    /// plugin_roaming（插件侧逐连接观察任务维护），宿主经桥读快照——批量
    /// 一次 FFI 往返，消费面配 [`Self::roaming_hex_of`] 做节点→hex 映射。
    /// 空 map = 插件缺席 / 端点未运行 / 尚无观测。
    pub async fn roaming_dial_paths(&self) -> std::collections::HashMap<String, String> {
        match crate::roaming::plugin_bridge::status_snapshot().await {
            crate::roaming::plugin_bridge::BridgeOutcome::Ok(s) => s.dial_paths,
            crate::roaming::plugin_bridge::BridgeOutcome::Absent => {
                std::collections::HashMap::new()
            }
            crate::roaming::plugin_bridge::BridgeOutcome::Failed(e) => {
                tracing::debug!(error = %e, "[Cluster] roaming 拨号路径快照读取失败（按无观测渲染）");
                std::collections::HashMap::new()
            }
        }
    }

    /// Stop the cluster. Stops discovery (joins threads) and signals shutdown.
    /// iroh 漫游叠加层装配。
    ///
    /// 显式开关（Dashboard/CLI 可控）：config.cluster.json `roaming.enabled`
    /// 为 true 才装配——key 只是身份材料不再是开关（的「key 存在即启用」
    /// 文件 opt-in 语义由本闸取代）；缺 key 时自动生成（身份为本地自证材料，
    /// info 日志诚实记录）。缺省/显式 false = 不装配（Ok(false)），LAN 节点
    /// 零 iroh 面。QUIC 插件化：端点本体（iroh QUIC 栈）在 plugin_roaming
    /// ——本方法只做装配编排：身份经桥 ensure、`endpoint_start` 拉起插件端
    /// 点（accept 侧入站交接经 notify_incoming 漏斗回流）、给 RPC client 挂
    /// [`crate::roaming:plugin_bridge::BridgeDialer`]（dial 按 `iroh:` 前缀
    /// 分派）、名册循环照旧 registry_start。AEAD token 取 server 当前值
    /// （`load_rpc_auth_token` 在 start() 已应用——调用方必须晚于
    /// `cluster.start()`）。
    ///
    /// 返回 Ok(true)=已启用；Ok(false)=开关未开；Err=装配失败（调用方 warn
    /// 不炸集群——漫游是叠加层，TCP 主通路不受影响）。
    pub async fn setup_roaming(&self) -> Result<bool, String> {
        // 显式闸：config.cluster.json `roaming.enabled`（缺文件/缺字段/false
        // 恒不启用——口径统一，key 在场也不再等价于开启）。
        let cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(&self.workspace);
        if !Self::roaming_enabled_in_config(&cfg_path) {
            tracing::debug!(
                "[Cluster] Roaming overlay not enabled (config.cluster.json roaming.enabled is not true)"
            );
            return Ok(false);
        }

        // 身份材料（QUIC 插件化）：身份面在 plugin_roaming——桥
        // identity_ensure（无文件生成+落盘，幂等）等价旧 load_or_create。
        // 插件缺席 = warn 降级 Ok(false)（叠加层整体不启用——CLI 显式路径
        // 才诚实报错，gateway 装配失败不炸启动，与旧「缺 key 正常启动」
        // 降级哲学同源）。 起 seed 不出插件（插件是身份与端点的唯一
        // 消费者——identity_payload 一律不带 secret）。
        let identity = match crate::roaming::plugin_bridge::identity_ensure().await {
            crate::roaming::plugin_bridge::BridgeOutcome::Ok(info) => info,
            crate::roaming::plugin_bridge::BridgeOutcome::Absent => {
                tracing::warn!(
                    "[Cluster] Roaming 开关已启用但 plugin_roaming 未装载——iroh 叠加层不启用\
                     （请随发行包安装 plugins/plugin_roaming/plugin_roaming.dll）"
                );
                return Ok(false);
            }
            crate::roaming::plugin_bridge::BridgeOutcome::Failed(e) => return Err(e),
        };
        if identity.created {
            tracing::info!(
                path = %crate::roaming::roaming_key_path(&self.workspace).display(),
                "[Cluster] Roaming enabled but no identity key — generated one (cluster/roaming.key)"
            );
        }
        let iroh_hex = identity.iroh_id.clone();

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

        // 入站受理臂装配（QUIC 插件化）：server 臂挂受理漏斗——插件
        // accept iroh 连接（trust 闸在插件 accept 侧先行，trust.toml 语义
        // 镜像宿主旧 TrustBook）后 notify_incoming 回流 {conn_id, port,
        // remote_hex}，消费任务逐条 `serve_roaming_loopback`
        // （TcpStream::connect 127.0.0.1:port → 既有 TcpConn 帧管线照跑）。
        // 漏斗随 stop() 摘除（摘后抵达的交接诚实丢弃，插件拨入超时兜底）。
        match self.rpc_server.as_ref() {
            Some(server) => server.spawn_roaming_acceptor()?,
            None => return Err("RPC server 未装配，iroh 入站桥无处挂载".to_string()),
        }

        // 端点装配（QUIC 插件化）：端点本体在 plugin_roaming——桥
        // endpoint_start（幂等停旧起新）拉起插件端点。relay_url 空 = n0
        // 公共中继；非空 = 自建中继回落（Minimal 基座零发现层——裸
        // `iroh:<hex>` 寻址在此形态不可用，对端地址需
        // `iroh:<hex>@<relay_url>` 完整形态）。可选 relay_token 与服务端
        // SharedToken 配对。客户端三键（relay_url/relay_token/rendezvous_url）
        // 自 起迁 config.quic.json `client` 段（QUIC 的配置不丢
        // 在集群里——集群只是 QUIC 的一个消费方）。
        let quic_cfg = crate::roaming::quic_config::resolved_quic(
            &nemesis_path::resolve_quic_config_path_in_workspace(&self.workspace),
        );
        let quic_client = &quic_cfg.client;
        if !quic_client.relay_url.trim().is_empty() {
            tracing::info!(
                relay_url = %quic_client.relay_url,
                "[Cluster] Roaming using self-hosted relay (bare iroh:<hex> addressing unavailable, peers need iroh:<hex>@<relay_url> form)"
            );
        }
        let endpoint_cfg = serde_json::json!({
            "relay_url": quic_client.relay_url.trim(),
            "relay_token": quic_client.relay_token.trim(),
        })
        .to_string();
        match crate::roaming::plugin_bridge::endpoint_start(endpoint_cfg).await {
            crate::roaming::plugin_bridge::BridgeOutcome::Ok(_) => {}
            crate::roaming::plugin_bridge::BridgeOutcome::Absent => {
                // identity_ensure 已确保插件在场——此处 Absent 属装配竞态
                // （理论不可达），诚实 Err 不留半装配态。
                return Err("plugin_roaming 未装载（装配竞态？）——端点未启动".to_string());
            }
            crate::roaming::plugin_bridge::BridgeOutcome::Failed(e) => return Err(e),
        }

        // 出站：RPC client dial 分派按 `iroh:` 前缀走桥拨号（插件 loopback
        // 交接；测试可注入 LocalDialer 同 trait）。：包一层 TrustGateDialer
        // 吊销 veto（簿缺席 = 未启用透传字节不变；revoked 诚实 Err，调用方
        // ⛔ 重试语义照常）。
        if let Some(client) = self.rpc_client.lock().clone() {
            client.set_roaming_dialer(Arc::new(crate::roaming::plugin_bridge::TrustGateDialer {
                workspace: self.workspace.clone(),
                inner: crate::roaming::plugin_bridge::BridgeDialer,
            }));
        }

        // announce 携带本节点 iroh 身份（下一拍广播生效；对端入池拨号候选）。
        *self.roaming_id.write() = Some(iroh_hex.clone());
        // 自建中继形态：自报地址带 `@relay_url` 完整形态（Minimal 基座零
        // 发现层，裸 node_id 对端无解析路径——实测钉死在
        // `bare_node_id_dial_fails_on_self_relay_minimal_base`）； 形态
        // 清空——裸候选经 `roaming_id` 字段， 发现层解析。
        *self.roaming_dialable.write() = if quic_client.relay_url.trim().is_empty() {
            Vec::new()
        } else {
            vec![format!("iroh:{iroh_hex}@{}", quic_client.relay_url.trim())]
        };

        tracing::info!(
            node_id = %iroh_hex,
            "[Cluster] Roaming overlay active (iroh: node reachable via iroh overlay)"
        );
        self.roaming_started.store(true, Ordering::SeqCst);

        // rendezvous 名册同步循环（自动发现，；QUIC 插件化，
        // 循环本体搬 plugin_roaming 自持 runtime——宿主只组装载荷
        // + 挂成员回流漏斗 + 发 registry_start）。仅自建中继形态（relay_url
        // 非空）挂载—— 公共中继无名册，保持既有手工静态条目路径。注册表
        // 地址 = relay_url host + 默认端口推导（rendezvous_url 显式覆盖）；
        // 推导失败 = warn 跳过（名册是便利设施，不阻塞叠加层）。
        if !quic_client.relay_url.trim().is_empty() {
            match crate::roaming::rendezvous::resolve_registry_url(
                &quic_client.rendezvous_url,
                &quic_client.relay_url,
            ) {
                Ok(registry_url) => {
                    *self.rendezvous_params.write() = Some(RendezvousParams {
                        registry_url,
                        relay_url: quic_client.relay_url.trim().to_string(),
                        relay_token: quic_client.relay_token.trim().to_string(),
                        iroh_hex: iroh_hex.clone(),
                    });
                    crate::roaming::plugin_bridge::set_members_sink(self.members_sink());
                    self.restart_registry_from_params();
                }
                Err(e) => tracing::warn!(
                    error = %e,
                    "[Cluster] rendezvous 注册表地址推导失败（自动发现不可用，其余叠加层功能不受影响）"
                ),
            }
        }
        Ok(true)
    }

    /// 成员回流漏斗（QUIC 插件化）：插件名册循环经 vtable
    /// `notify_members` 推来的 `{"members":[...]}` 全量包 →
    /// [`Self::merge_rendezvous_members`] 单一漏斗入册 + `rendezvous_status`
    /// 快照覆写。闭包持 `self_weak` 懒升级（同旧循环——`setup_roaming` 跑在
    /// `Arc::new` 之前）；在插件循环线程同步执行（纯内存入册，无回呼插件，
    /// 无死锁面）。快照语义：本漏斗只写**成功**形态（最后成功推送）；失败态
    /// 由插件运行状态槽承载，`plugin_bridge::effective_rendezvous_state`
    /// 在状态面 merge 出最终形态（观测性）。
    fn members_sink(&self) -> std::sync::Arc<dyn Fn(&str) + Send + Sync> {
        let weak = self.self_weak.clone();
        std::sync::Arc::new(move |json: &str| {
            let Some(cluster) = weak.lock().clone().and_then(|w| w.upgrade()) else {
                tracing::debug!(
                    "[Rendezvous] self_weak 未安装或已释放，成员包丢弃（下 tick 重推）"
                );
                return;
            };
            // 参数缓存已清（stop() 竞态窗口）= 集群已撤名册，本包不合并。
            let Some(p) = cluster.rendezvous_params.read().clone() else {
                return;
            };
            #[derive(serde::Deserialize)]
            struct MembersPacket {
                #[serde(default)]
                members: Vec<crate::roaming::rendezvous::RendezvousMember>,
            }
            let packet: MembersPacket = match serde_json::from_str(json) {
                Ok(p) => p,
                Err(e) => {
                    tracing::debug!(error = %e, "[Rendezvous] 成员包解析失败（忽略本包，下 tick 重推）");
                    return;
                }
            };
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_millis() as u64)
                .unwrap_or(0);
            let merged =
                cluster.merge_rendezvous_members(&packet.members, &p.iroh_hex, &p.relay_url);
            if merged > 0 {
                tracing::info!(
                    members = packet.members.len(),
                    merged,
                    "[Rendezvous] 名册同步：新成员/变更已入册"
                );
            }
            *cluster.rendezvous_status.write() =
                Some(crate::roaming::rendezvous::RendezvousClientState {
                    registry_url: p.registry_url,
                    last_sync_ok: true,
                    last_sync_ms: now_ms,
                    member_count: packet.members.len(),
                    last_error: String::new(),
                });
        })
    }

    /// 从参数缓存组装 `registry_start` 入参 JSON（member 身份字段现读——
    /// identity editor 热改经钩子下一拍生效）。
    fn rendezvous_registry_payload(&self, p: &RendezvousParams) -> String {
        let member = self.rendezvous_self_member(&p.iroh_hex, &p.relay_url);
        serde_json::json!({
            "registry_url": p.registry_url,
            "token": p.relay_token,
            "member": member,
            // interval 缺省 = 插件默认 30s（与旧宿主循环同值，不显式传）。
        })
        .to_string()
    }

    /// （重）发 `registry_start`（插件侧幂等停旧起新；fire-and-forget）。
    /// 参数缓存已清 / 无 runtime 上下文 / 插件缺席 = 跳过或日志降级。
    fn restart_registry_from_params(&self) {
        use crate::roaming::plugin_bridge::{self, BridgeOutcome};
        let Some(p) = self.rendezvous_params.read().clone() else {
            return;
        };
        let Ok(handle) = tokio::runtime::Handle::try_current() else {
            tracing::debug!("[Rendezvous] 无 runtime 上下文，registry_start 跳过");
            return;
        };
        let payload = self.rendezvous_registry_payload(&p);
        handle.spawn(async move {
            match plugin_bridge::registry_start(payload).await {
                BridgeOutcome::Ok(_) => {}
                BridgeOutcome::Absent => {
                    tracing::warn!("[Rendezvous] plugin_roaming 未装载，名册循环未启动");
                }
                BridgeOutcome::Failed(e) => {
                    tracing::warn!(error = %e, "[Rendezvous] registry_start 失败（身份热更钩子重试）");
                }
            }
        });
    }

    /// 本节点 → 名册注册载荷（身份字段现读——identity editor 热改下一拍生效）。
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
            // features 严格过滤（「不兼容」）：名册是全量
            // 登记面，集群视图只收宣告 cluster 的成员——缺宣告（旧版设备
            // /非集群设备）= 本节点集群视图不可见（云端全量名册仍可），
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
    pub fn rendezvous_status_snapshot(
        &self,
    ) -> Option<crate::roaming::rendezvous::RendezvousClientState> {
        self.rendezvous_status.read().clone()
    }

    /// 运行时硬化批：注入本节点构建号（git 短 hash）。宿主（nemesisbot
    /// gateway / 轻量节点命令）在 `Cluster::with_workspace` / `Cluster::new`
    /// 之后调用——编译期常量 `NEMESISBOT_GIT_COMMIT` 住在 nemesisbot，cluster
    /// crate 不能反向依赖。空串 = 未宣告（announce 不带 build 语义）。
    pub fn set_build_hash(&self, h: &str) {
        *self.build_hash.write() = h.to_string();
    }

    /// 漫游开关判定（纯函数）：config.cluster.json 顶层 `roaming.enabled`
    /// 严格为 JSON true 才开。缺文件 / 坏 JSON / 缺字段 / false → false。
    /// 实现委托 `roaming::config::resolved_roaming`（`roaming` 段的统一定义
    /// ——setup_roaming 装配闸与 WSAPI/CLI 状态面同读此形；段内自
    /// 起只剩 enabled 一个键，QUIC 客户端/服务端配置已迁
    /// config.quic.json）。
    pub fn roaming_enabled_in_config(config_path: &std::path::Path) -> bool {
        crate::roaming::config::resolved_roaming(config_path).enabled
    }

    // QUIC 中继生命周期已迁出集群（解耦）：中继启停由宿主
    //（nemesisbot gateway / 独立 `quic relay` 进程）经 plugin_roaming
    // C-ABI 承载（QUIC 插件化）——集群关闭/未启用时中继照起，集群
    // `stop()` 也不再触碰中继。配置以 `config.quic.json` `relay` 段为准。
    pub fn stop(&self) {
        *self.running.write() = false;

        // Stop RPC server first — reject new connections, existing connections
        // drain naturally via idle_timeout.
        if let Some(ref server) = self.rpc_server
            && let Err(e) = server.stop()
        {
            tracing::warn!(error = %e, "[Cluster] RPC server stop error");
        }

        // iroh 漫游叠加层停机（QUIC 插件化：端点本体在 plugin_roaming
        // ——桥 endpoint_stop 幂等收口 + 装配旗标复位 + 入站漏斗摘除；无
        // runtime 时 fire-and-forget 兜底由插件 shutdown 承担）。QUIC 中继
        // 不在本停臂（解耦——中继运行面在 plugin_roaming 插件、
        // 由宿主 gateway 停机链 roaming_stop 收口，集群停/启不影响中继）。
        {
            self.roaming_started.store(false, Ordering::SeqCst);
            crate::roaming::plugin_bridge::clear_incoming_sink();
            if let Ok(h) = tokio::runtime::Handle::try_current() {
                h.spawn(async move {
                    if let crate::roaming::plugin_bridge::BridgeOutcome::Failed(e) =
                        crate::roaming::plugin_bridge::endpoint_stop().await
                    {
                        tracing::debug!(
                            "[Cluster] roaming endpoint_stop 失败（幂等无副作用）: {e}"
                        );
                    }
                });
            } else {
                tracing::debug!(
                    "[Cluster] 无 runtime 上下文，endpoint_stop 跳过（插件 shutdown 兜底）"
                );
            }
        }

        // QUIC 插件化：名册循环在插件内——参数缓存清空（成员回流漏斗
        // 闭包据此丢弃迟到包）+ registry_stop（插件侧幂等；fire-and-forget，
        // 无 runtime 上下文由插件 shutdown 兜底）。中继照旧不动（解耦）。
        {
            *self.rendezvous_params.write() = None;
            crate::roaming::plugin_bridge::registry_stop_fire_and_forget();
        }

        // Stop discovery service (joins broadcast + receive threads)
        self.discovery_running.store(false, Ordering::SeqCst);
        if let Some(discovery) = self.discovery.lock().take()
            && let Err(e) = discovery.stop()
        {
            tracing::warn!(error = %e, "[Cluster] Discovery stop error");
        }

        // Stop TaskManager cleanup loop（start 接线的对称收口）
        self.task_manager.stop();

        // Signal recovery/sync loops to exit
        let _ = self.stop_tx.send(());
        logger::log_lifecycle("stop", &self.node_id, "Cluster stopped");
    }

    /// Check whether the cluster is running.
    pub fn is_running(&self) -> bool {
        *self.running.read()
    }

    // -- Recovery loop ---------------------------------------------------------
}

impl Cluster {
    /// List all tasks.
    pub fn list_tasks(&self) -> Vec<Task> {
        self.task_manager.list_tasks()
    }

    /// Get a reference to the task manager.
    pub fn task_manager(&self) -> &Arc<TaskManager> {
        &self.task_manager
    }

    /// tasks.db 连接（统一任务层）。`None` = `Cluster::new` 测试路径
    /// 内存态（无台账可记）；A 端据此把 worker 交付分支
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
}

impl Cluster {
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

    /// 自报职能清单（集群专业职能框架；以 peers.toml
    /// `[node].professions`，身份编辑热改写此处）。
    pub fn professions(&self) -> Vec<String> {
        self.professions.read().clone()
    }

    /// Set 自报职能清单（身份编辑；下一 announce 拍生效）。
    pub fn set_professions(&self, professions: Vec<String>) {
        *self.professions.write() = professions;
        self.sync_local_node_to_registry();
    }

    /// 注入 agent 实体清单提供者（gateway 装配期一次）。
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

    /// 节点档位（role_tier 口径；以 peers.toml `[node].tier` 为准）。
    pub fn tier(&self) -> Option<String> {
        self.tier.read().clone()
    }

    /// Set 节点档位（身份编辑）。
    pub fn set_tier(&self, tier: Option<String>) {
        *self.tier.write() = tier;
        self.sync_local_node_to_registry();
    }

    /// 自报运行时后端形态（以 peers.toml
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
}

impl Cluster {
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

    /// b（三期）：运行时 mandate 签名公钥覆盖静态声明。
    ///
    /// gateway 装配期 load-or-generate 出节点 mandate 密钥后调用——announce
    /// wire 从此携带运行时真实公钥（peers.toml `[node].pubkey` 只是降级对账
    /// 锚；密钥文件缺失/损坏时本方法不被调用，维持静态声明或 None）。与
    /// `set_node_name` 同族：RwLock 原子换值，不触发 registry 同步（announce
    /// 周期广播自然携带）。
    pub fn set_mandate_pubkey(&self, pubkey_hex: impl Into<String>) {
        let new = pubkey_hex.into();
        // 静态声明降级对账（同 iroh_id 先例）：槽位现值 = Cluster::new 从
        // peers.toml `[node].pubkey` 种入的声明值；与运行时密钥不一致 =
        // 声明过期/手改残留，以运行时为准 + warn（announce 载荷自此携带
        // 运行时真实公钥，对端 TOFU 钉存不受声明漂移影响）。
        if let Some(declared) = self.mandate_pubkey.read().as_ref()
            && declared.to_lowercase() != new.to_lowercase()
        {
            tracing::warn!(
                "[Cluster] peers.toml [node].pubkey 与运行时 mandate 密钥不一致（声明过期或手改残留）——以运行时密钥为准"
            );
        }
        *self.mandate_pubkey.write() = Some(new);
    }

    /// Set the dynamic capabilities for this node (tool names from AgentLoop).
    ///
    /// Called after the AgentLoop is built so the discovery broadcast includes
    /// the actual tool set rather than a hardcoded empty list.
    ///
    /// 批 ③ 数据面：与 `static_capabilities`（peers.toml
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
}

impl Cluster {
    /// Find all online peers that have a specific capability.
    ///
    /// Mirrors Go's `FindPeersByCapability(capability string) []*Node`.
    pub fn find_peers_by_capability(&self, capability: &str) -> Vec<ExtendedNodeInfo> {
        self.registry.find_by_capability(capability)
    }

    // -- RPC channel management -----------------------------------------------
}

impl Cluster {
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
    /// 消费方：`ping` RPC handler（探针响应回传自报身份）与 健康探针
    /// 循环（探针成功时应用对端身份）——两者都在只有 `&self` 的上下文里
    /// 注册/启动，经本槽活读自身。未安装（裸构造的单测）时全部优雅降级：
    /// ping 回裸 pong、探针只记活不应用身份。
    pub fn install_self_weak(self: &Arc<Self>) {
        *self.self_weak.lock() = Some(Arc::downgrade(self));
    }

    /// 探针身份应用：把探针响应里携带的对端自报身份（announce 同构载荷）
    /// 走 [`Self::handle_discovered_node_ex`] 单一漏斗入册。
    ///
    /// 背景（ET U 系九连红根因）：身份（professions/tier）唯一
    /// 载体是 UDP announce；广播被隔离/UDP 端口被幽灵进程劫持的部署形态
    /// （CI 虚拟机静态 peer + TCP 全通、announce 十九分钟零到达）里
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
            msg.mandate_pubkey,
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
        mandate_pubkey: Option<&str>,
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
            mandate_pubkey.map(|s| s.to_string()),
            source_addr,
            build,
        )
    }

    fn build_hash(&self) -> String {
        self.build_hash.read().clone()
    }

    fn mandate_pubkey(&self) -> Option<String> {
        self.mandate_pubkey.read().clone()
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

// ---------------------------------------------------------------------------
// 机械拆分：按域分文件。纯代码搬家，行为零变更；
// 各子模块项经 pub use 保持原 nemesis_cluster::cluster:路径不变。
// ---------------------------------------------------------------------------
mod continuation;
mod discovery;
mod health;
mod sync;
mod tasks;

pub use continuation::*;
pub use discovery::*;
// health/tasks 项全是 crate 内可见（pub(super)/pub(crate)），pub use 重导不出 pub
// → health 降私有 glob（ROUTE_RECORDED_CAP 供兄弟经 super:* 解析）、tasks 降 pub(crate)
use health::*;
pub(crate) use tasks::*;
