// ---------------------------------------------------------------------------
// PB-2 集群初始化（计划 §4.2 B2）：Step 9a（计划口径原 2461–3968，~1,500 行
// 最大自包含块）整体迁入 `init_cluster`——参数表自 12 个上游变量缩到 ctx
// 单参（B1 重排的动机）。原文自 run() 逐字迁入，仅三处机械改写：fn 体包裹、
// ctx 依赖项影子重绑、末尾 ClusterWiring 构造。
//
// ClusterWiring 字段 = 实测逃逸本块、被下游相位消费的完整集合。计划 B2 表
// 列十项的差异如实说明：cluster/task_list/work_queue/persister 随
// cluster_adapter_refs 四元组整体传递（下游仅以元组消费，独立字段会成为
// dead_code 违 clippy -D）；node_id/node_name 消费不出块（§2.3.2 末注，
// ClusterRpcConfig.local_node_id 已携带）。表外机械项按实测下游消费补入：
// bridge_cluster_slot（PB-3 桥身份装配 + PB-7 桥客户端 spawn）、
// cluster_should_start（PB-5 first_start/rebuild_pending/board_role）、
// board_worker_inbox（PB-2 赋值 → PB-5 take）、board_estop_parked +
// board_selfcheck_registry（PB-5 评审依赖集）。
//
// cfg 门纪律（§0-5/C5）：`#[cfg]` 整段摘除形态逐字保留；未启用 cluster 时
// 无门 trio 三 None 语义与原 run() pre-block 前向声明逐字节一致。块内不变
// 量随块内聚：C4 discovery 回调组装期捕获 Handle::current、C2 E3 sweep
// 顺序（exec 残留清扫先于 outbox sweep_startup）、C9 占位→真身两段
// peer_chat_callback 注册、E3 传输栈顺序。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use tracing::{error, info, warn};

#[cfg(all(feature = "cluster", feature = "forge"))]
use super::ClusterForgeBridgeAdapter;
use super::GatewayCtx;
#[cfg(feature = "cluster")]
use super::parse_host_port;
#[cfg(feature = "cluster")]
use super::record_cluster_usage;
#[cfg(feature = "cluster")]
use super::{BusToClusterAdapter, ClusterResultPersisterAdapter};
#[cfg(all(feature = "board", feature = "cluster"))]
use super::{sweep_dispatch_timeouts, write_back_board_dispatch};
use crate::common;

/// PB-2 产物 struct（§4.2 B2）。无门 trio 由 SharedResources（PB-4）无门
/// 消费；cluster 门字段由 PB-3/PB-5/PB-7 消费。未启用 cluster 时整体为
/// None/缺省——与原 pre-block 前向声明语义一致。
pub(crate) struct ClusterWiring {
    pub cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    >,
    pub cluster_rpc_config: Option<nemesis_agent::ClusterRpcConfig>,
    pub cluster_peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>>,
    /// （cluster, task_list, work_queue, persister）四元组——PB-5
    /// ClusterServiceAdapter 构建原料（原 run() 同名前向声明）。
    #[cfg(feature = "cluster")]
    pub cluster_adapter_refs: Option<(
        Arc<nemesis_cluster::cluster::Cluster>,
        Arc<nemesis_cluster::ClusterTaskList>,
        Arc<nemesis_cluster::ClusterWorkQueue>,
        Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
    )>,
    /// §2.3.2：PB-5 first_start / rebuild_pending / board_role 消费。
    #[cfg(feature = "cluster")]
    pub cluster_should_start: bool,
    /// 二期批次五：桥身份 sink 注入（PB-3）+ 桥客户端 hello 身份快照（PB-7）。
    #[cfg(feature = "cluster")]
    pub bridge_cluster_slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>>,
    /// Swarm M3（G4/G8）：worker 侧讨论事件入站箱（本节点不是 board 权威
    /// 时创建；board feature 裁剪下恒 None——adapter 不接讨论通道）。
    #[cfg(feature = "cluster")]
    pub board_worker_inbox: Option<std::sync::Arc<crate::cluster_agent::DiscussionInbox>>,
    /// A2 停车场 estop 挂起队列（PB-5 评审依赖集消费）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_estop_parked:
        std::sync::Arc<std::sync::Mutex<Vec<(crate::board_review::ParkedKind, i64)>>>,
    /// P4/B2b 自检取证路由表（selfcheck 派发不写 issue_dispatch，callback
    /// 凭本表识别取证任务并路由到二段验收；评审任务与回调闭包共享）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_selfcheck_registry: crate::board_review::SelfcheckRegistry,
}

/// Step 9a：集群基础设施装配（计划 §4.2 B2）。Cluster 对象与 adapter refs
/// 恒建（动态 start/stop 支持）；网络组件（RPC server/discovery）仅在
/// master + app config 双开时启动。
pub(crate) async fn init_cluster(ctx: &GatewayCtx) -> Result<ClusterWiring> {
    // ctx 影子重绑（B1 同款）：块内原文引用局部名不变，所有权拓扑不变。
    // 门控取各名字在本函数内的实际消费门（全部落在 cfg(cluster) 块内），
    // 避免 cluster 裁剪组合的 unused 警告。
    #[cfg(feature = "cluster")]
    let home = ctx.home.clone();
    #[cfg(feature = "cluster")]
    let cfg = ctx.cfg.clone();
    #[cfg(feature = "cluster")]
    let estop = ctx.estop.clone();
    #[cfg(feature = "cluster")]
    let bus = ctx.bus.clone();
    #[cfg(feature = "cluster")]
    let data_store = ctx.data_store.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_store = ctx.board_store.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_quota = ctx.board_quota.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_moderator_loop = ctx.board_moderator_loop.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let autopilot_cluster_slot = ctx.autopilot_cluster_slot.clone();
    #[cfg(all(feature = "forge", feature = "cluster"))]
    let forge_for_web = ctx.forge_for_web.clone();

    // Swarm M3（G4/G8）：worker 侧讨论事件入站箱前向声明（原 run() 项随 B2
    // 整体迁入——赋值在本块 nb_bus else 分支、消费在 PB-5；board feature 裁
    // 剪下恒 None——adapter 不接讨论通道。）
    #[cfg(feature = "cluster")]
    #[allow(unused_mut)]
    let mut board_worker_inbox: Option<std::sync::Arc<crate::cluster_agent::DiscussionInbox>> =
        None;

    // Step 9a: Set up cluster.
    // Mirrors Go's bot_service.go initComponents → startCluster.
    // The Cluster object and adapter are always created for dynamic start/stop support.
    // Network components (RPC server, discovery) only start when both config flags are enabled.
    #[cfg(feature = "cluster")]
    let cluster_master_enabled = cfg.cluster.as_ref().map(|c| c.enabled).unwrap_or(false);
    #[cfg(feature = "cluster")]
    let cluster_app_cfg = nemesis_cluster::config_loader::load_app_config(&home.join("workspace"));
    #[cfg(feature = "cluster")]
    let cluster_should_start = cluster_master_enabled && cluster_app_cfg.enabled;

    // 二期批次五（goal：桥集群）：cluster 句柄槽——Arc<Cluster> 构建在下方
    // cfg(feature = "cluster") 块内、块后不可见；桥两处装配（hub 侧身份
    // sink 注入 / 设备侧 hello 集群身份快照）都在块外的 relay/web 装配段，
    // 经 OnceLock 槽位回填写取用（同 sweep_cluster_slot 模式）。
    #[cfg(feature = "cluster")]
    let bridge_cluster_slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>> =
        Arc::new(std::sync::OnceLock::new());

    // Cluster RPC resources — filled inside the cluster block below, consumed by SharedResources.
    #[allow(unused_mut)] // mut only needed when feature="cluster" assigns these in the init block
    let mut cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<dyn std::future::Future<Output = Result<serde_json::Value, String>> + Send>,
                > + Send
                + Sync,
        >,
    > = None;
    #[allow(unused_mut)]
    let mut cluster_rpc_config: Option<nemesis_agent::ClusterRpcConfig> = None;
    #[allow(unused_mut)]
    let mut cluster_peers_fn: Option<
        Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>,
    > = None;
    // Cluster refs saved during init, used to create adapter after SharedResources is built.
    #[cfg(feature = "cluster")]
    #[allow(unused_assignments)] // always overwritten by the cluster init block below
    let mut cluster_adapter_refs: Option<(
        Arc<nemesis_cluster::cluster::Cluster>,
        Arc<nemesis_cluster::ClusterTaskList>,
        Arc<nemesis_cluster::ClusterWorkQueue>,
        Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister>,
    )> = None;
    // Always create cluster infrastructure (Cluster object, handlers, adapter refs).
    // Network components are started below only when cluster_should_start is true.
    // （estop 句柄创建已上移到 cron 装配前——F-U4-5：cron 闭包与评审依赖集
    // / SharedResources 共享同一 Arc。）
    // `#[cfg]` 整段摘除（非 cfg_attr+dead_code）：类型位引用
    // `crate::board_review::`，feature 关闭时必须整体出编译（2026-09-12
    // CI feature-matrix E0433 根修；消费点均挂同闸）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_estop_parked = std::sync::Arc::new(std::sync::Mutex::new(Vec::<(
        crate::board_review::ParkedKind,
        i64,
    )>::new()));
    // P4/B2b 自检取证路由表：selfcheck 派发不写 issue_dispatch，callback
    // 凭本表识别取证任务并路由到二段验收（评审任务与回调闭包共享）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_selfcheck_registry = crate::board_review::SelfcheckRegistry::new();
    #[cfg(feature = "cluster")]
    {
        // Build ClusterConfig — node_id 留空，with_workspace() 会从 peers.toml [node] 段加载真实身份；
        // node_name 传 config.cluster.json 显式显示名（空 = cluster.rs 自动解析链：hostname → Bot {id8}）。
        let cluster_config = nemesis_cluster::types::ClusterConfig {
            node_id: String::new(),
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            peers: vec![],
            node_name: cluster_app_cfg.node_name.clone(),
        };

        let mut cluster = nemesis_cluster::cluster::Cluster::with_workspace(
            cluster_config,
            home.join("workspace"),
        );

        // Set ports and node info from app config
        cluster.set_ports(cluster_app_cfg.port, cluster_app_cfg.rpc_port);
        cluster.set_broadcast_interval(std::time::Duration::from_secs(
            cluster_app_cfg.broadcast_interval.max(1),
        ));
        cluster.set_node_type("agent");

        // Swarm M2: 节点发现 → 自动收编进看板频道（first-join 语义：
        // 成员在 channel_member 全表零行 = 全新节点才自动入；管理员手动
        // 调整过的不被 announce 拉回）。role=worker 且 category 含 qa →
        // #qa，其余 worker → #dev；coordinator / 本节点不入。注册在静态
        // peers 装载之前 —— 启动时静态对端同样收编。
        // A2 停车场 sweep：cluster 句柄经 OnceLock 回填（回调注册早于
        // Arc::new(cluster)，同 autopilot 槽位模式）；10s 节流抗 announce
        // 风暴；estop 挂起时不派发（急停冻结一切自动 agent 活动）。
        // None = 从未跑过（Option 防开机窗口 Instant 下溢）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let (sweep_cluster_slot, sweep_last, estop_for_sweep) = {
            let slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>> =
                Arc::new(std::sync::OnceLock::new());
            let last: Arc<std::sync::Mutex<Option<std::time::Instant>>> =
                Arc::new(std::sync::Mutex::new(None));
            (slot, last, estop.clone())
        };

        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let store_for_hook = board_store.clone();
            let self_node_id = cluster.node_id().to_string();
            let sweep_cluster_slot = sweep_cluster_slot.clone();
            let sweep_last = sweep_last.clone();
            // 真机缺陷（2026-09-11 双端双平台复现）：本闭包在 std 线程
            // discovery-udp-listen 上触发——该线程无 tokio reactor，闭包内
            // 直接 tokio::spawn 会 panic 并炸死监听线程 → announce 接收全哑
            // （节点上线/刷新静默失联，停车场 sweep/看板收编永不触发，且
            // health 探针走 tokio 任务所以状态看起来仍正常——高度迷惑）。
            // 根修：组装期（tokio 上下文内）捕获 Handle，回调里用 Handle::spawn。
            let discovery_rt = tokio::runtime::Handle::current();
            cluster.set_on_node_discovered(Arc::new(move |node_id, role, category| {
                // sweep 先于 auto-join 的 self/非 worker 早退：任何非本节点
                // announce（含身份/tags 变更刷新）都是重试信号；本节点自身
                // 的 announce 不是（派发匹配器本就排除本机）。
                // 触发闸（estop 短路 + 10s 节流）抽为 board_review::
                // park_sweep_gate（可测）：estop 挂起 → 拒且不消耗节流窗口。
                if node_id != self_node_id && sweep_cluster_slot.get().is_some() {
                    let throttle_ok = {
                        let mut last = sweep_last
                            .lock()
                            .unwrap_or_else(|e| e.into_inner());
                        crate::board_review::park_sweep_gate(
                            estop_for_sweep.is_engaged(),
                            &mut last,
                            std::time::Instant::now(),
                            std::time::Duration::from_secs(10),
                        )
                    };
                    if throttle_ok {
                        let store = store_for_hook.clone();
                        let cluster = sweep_cluster_slot.get().unwrap().clone();
                        let node_id = node_id.to_string();
                        discovery_rt.spawn(async move {
                            if let Some(store) = store.as_ref() {
                                let actor = nemesis_board::Actor::system("board");
                                let (cands, dispatched, failed) =
                                    nemesis_web::handlers::board::sweep_parked_dispatches(store, &cluster, &actor);
                                if dispatched > 0 || failed > 0 {
                                    info!(
                                        "[Gateway] 停车场 sweep：候选 {cands} 派出 {dispatched} 失败 {failed}（节点 {} 上线/刷新触发）",
                                        node_id
                                    );
                                }
                                // D0b（goal P2）重平衡：announce 节点有空闲 slot
                                // 时，从超载 worker（在途 > 上限）偷排队单转派
                                // 过来——新设备上线即有活接（准入控制留量的
                                // 承接半环）。
                                let moved = nemesis_web::handlers::board::rebalance_queued_to_worker(
                                    store,
                                    &cluster,
                                    None,
                                    &node_id,
                                    &actor,
                                )
                                .await;
                                if moved > 0 {
                                    info!(
                                        "[Gateway] D0b 重平衡：{moved} 单转派至 {node_id}"
                                    );
                                }
                            }
                        });
                    }
                }
                let Some(store) = store_for_hook.as_ref() else {
                    return;
                };
                if node_id == self_node_id || !role.eq_ignore_ascii_case("worker") {
                    return;
                }
                let channel_name = if category.to_lowercase().contains("qa") {
                    "#qa"
                } else {
                    "#dev"
                };
                let member = nemesis_board::Actor::agent(node_id);
                match store.has_any_channel_membership(&member) {
                    Ok(true) => {} // 已见过的成员：手动调整不被撤销
                    Ok(false) => match store.get_channel_by_name(channel_name) {
                        Ok(Some(ch)) => {
                            if let Err(e) = store.join_channel(ch.id, member) {
                                warn!("[Gateway] Board auto-join failed: {}", e);
                            } else {
                                info!(
                                    "[Gateway] Board: node {} auto-joined {}",
                                    node_id, channel_name
                                );
                            }
                        }
                        Ok(None) => {}
                        Err(e) => warn!("[Gateway] Board auto-join lookup failed: {}", e),
                    },
                    Err(e) => warn!("[Gateway] Board auto-join probe failed: {}", e),
                }
            }));
        }

        // Load static peers from peers.toml into the registry
        // The peers.toml uses [peers.Key] table format (not [[peers]] array),
        // so we parse it manually.
        let peers_toml_path = common::cluster_dir(&home).join("peers.toml");
        if peers_toml_path.exists()
            && let Ok(content) = std::fs::read_to_string(&peers_toml_path)
            && let Ok(doc) = content.parse::<toml::Value>()
            && let Some(peers_table) = doc.get("peers").and_then(|v| v.as_table())
        {
            for (key, val) in peers_table {
                // 表键即 peer_id（发现②/B3 起写盘为字面 id，TOML 引号键保
                // 真；旧版 sanitize 有损键的存量条目靠运行期占位升级归一）。
                let peer_id = key.clone();
                let addr = val.get("address").and_then(|v| v.as_str()).unwrap_or("");
                let name = val.get("name").and_then(|v| v.as_str()).unwrap_or(&peer_id);
                let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
                let cat = val
                    .get("category")
                    .and_then(|v| v.as_str())
                    .unwrap_or("general");
                let tags: Vec<String> = val
                    .get("tags")
                    .and_then(|v| v.as_array())
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|v| v.as_str().map(|s| s.trim().to_string()))
                            .filter(|s| !s.is_empty())
                            .collect()
                    })
                    .unwrap_or_default();
                if addr.is_empty() {
                    continue;
                }
                // The address field contains UDP host:port (e.g., "127.0.0.1:11950").
                // RPC port resolution: explicit `rpc_port` field (written by pair /
                // placeholder upgrade with the *probed* value) wins; fall back to
                // the udp+10000 convention for legacy/hand-written entries.
                let (host, udp_port) = parse_host_port(addr);
                let rpc_port =
                    nemesis_cluster::cluster_config::resolve_peer_rpc_port(val, udp_port);
                let addresses = if host.is_empty() { vec![] } else { vec![host] };
                info!(
                    "[Gateway] Loading static peer: {} ({}) addr={} rpc_port={}",
                    name, peer_id, addr, rpc_port
                );
                cluster.handle_discovered_node(
                    &peer_id,
                    name,
                    addresses,
                    rpc_port,
                    role,
                    cat,
                    tags,
                    vec![],
                    "unknown",
                );
                // Mark as static/configured so flaky UDP discovery
                // (e.g. multi-node on one device) can't take it
                // offline via check_health staleness — the address
                // is known, only explicit removal/RPC-failure takes
                // it down.
                cluster.mark_peer_static(&peer_id);
            }
        }

        // --- Create and set RPC Server (before start, needs &mut self) ---
        let rpc_server_config = nemesis_cluster::rpc::server::RpcServerConfig {
            bind_address: format!("0.0.0.0:{}", cluster_app_cfg.rpc_port),
            ..Default::default()
        };
        cluster.set_rpc_server(Arc::new(nemesis_cluster::rpc::server::RpcServer::new(
            rpc_server_config,
        )));

        // Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
        cluster.start();

        // 节点身份（node_id / node_name）由 with_workspace() 从 peers.toml [node] 段加载，
        // 这里直接从 cluster 拿真值供下游消费（ClusterRpcConfig.local_node_id、Forge 桥、日志）。
        let node_id = cluster.node_id().to_string();
        let node_name = cluster.node_name();
        info!(
            "[Gateway] Cluster started (node_id: {}, name: {}, udp: {}, rpc: {})",
            node_id, node_name, cluster_app_cfg.port, cluster_app_cfg.rpc_port
        );

        // Diagnostic: list registry contents after start
        {
            let all_nodes = cluster.list_nodes();
            for n in &all_nodes {
                info!(
                    "[Gateway] Registry node: {} (id={}) status={:?} addr={}",
                    n.base.name, n.base.id, n.status, n.base.address
                );
            }
        }

        // Register RPC handlers on the server
        if let Err(e) = cluster.register_basic_handlers() {
            warn!("[Gateway] Failed to register basic RPC handlers: {}", e);
        }

        // Start RPC server (network operation — only when cluster is fully enabled).
        if cluster_should_start {
            // P0 vault fail-closed：token 引用解析失败 → 拒绝 bind（宁可没有
            // RPC，不可无认证 RPC）。节点其余功能照常，日志已带补救指引。
            if cluster.rpc_reference_broken() {
                error!(
                    "[Gateway] Cluster RPC auth token 引用解析失败 —— fail-closed：RPC 服务不启动（请修复引用或运行 `nemesisbot vault set <alias>`）"
                );
            } else {
                let rpc_server_ref = cluster.rpc_server().expect("rpc_server just set").clone();
                info!(
                    "[Gateway] Starting RPC server on 0.0.0.0:{}",
                    cluster_app_cfg.rpc_port
                );
                // Await start() synchronously — it binds the TCP listener and spawns the
                // accept loop, then returns. This ensures default handlers are registered
                // before we overwrite them below.
                if let Err(e) = rpc_server_ref.start().await {
                    error!(
                        "[Gateway] RPC server error on port {}: {}",
                        cluster_app_cfg.rpc_port, e
                    );
                }
                info!(
                    "[Gateway] RPC server started on port {}",
                    cluster_app_cfg.rpc_port
                );
            }
        }

        // Now register custom peer_chat handler using PeerChatHandler.
        // NOTE: We create the handler here but register it AFTER Arc::new(cluster)
        // so the closure can capture the Arc and register the remote node in the registry.
        let result_store = cluster.result_store().clone();
        let node_id_for_handler = node_id.clone();
        let _node_name_for_handler = node_name.clone();

        let mut handler = nemesis_cluster::rpc::peer_chat_handler::PeerChatHandler::new(
            node_id_for_handler.clone(),
        );
        let llm_timeout = nemesis_cluster::rpc::peer_chat_handler::llm_timeout_from_config_secs(
            cluster_app_cfg.llm_timeout_secs,
        );
        handler.set_timeout(llm_timeout);
        // P4/E3（看板项目档案 goal）：任务接收钩子——档案管线派发
        // （payload `_baseline_commit`）解包基线工作副本到
        // `<workspace>/cluster/exec/<task_id>/` 并注入 prompt 工作目录段。
        // 非档案 payload 零改动；不依赖 transfer 栈装配（独立成立）。
        #[cfg(feature = "cluster")]
        handler.set_task_receive_hook(std::sync::Arc::new(
            nemesis_cluster::exec_workspace::ExecReceiveHook::new(&home.join("workspace")),
        ));

        // Create cluster agent work queue and task list.
        let cluster_data_dir = nemesis_path::workspace_data_dir(&home);
        let cluster_task_list = Arc::new(nemesis_cluster::ClusterTaskList::new(&cluster_data_dir));
        let cluster_work_queue = Arc::new(nemesis_cluster::ClusterWorkQueue::new(64));
        handler.set_cluster_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // Set RPC client for callbacks (after cluster.start() creates the client).
        let rpc_client = cluster.rpc_client_arc();
        if let Some(client) = rpc_client.clone() {
            handler.set_rpc_client(client);
        }

        // --- P3/D1-D5（看板项目档案 goal）：分块档案传输栈 ---
        // 收件 sink（inbox 落地 + D6 manifest 核验 + D4 master 护栏）与发件
        // outbox（worker 推送循环）。D4 护栏初值读 board.archive.max_transfer_bytes，
        // 热刷新循环在 handler 注册段启动。on_landed/on_overlimit 回调 =
        // board_archive_ingest（无处安置诚实留收件箱，不删不弃）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let (transfer_sink, transfer_outbox): (
            Option<std::sync::Arc<nemesis_cluster::transfer::TransferSink>>,
            Option<std::sync::Arc<nemesis_cluster::outbox::TransferOutbox>>,
        ) = match rpc_client.clone() {
            Some(rc) => {
                let ws_dir = home.join("workspace");
                let limit0 = cfg
                    .board
                    .as_ref()
                    .map(|b| b.archive.max_transfer_bytes)
                    .unwrap_or_else(|| {
                        nemesis_config::BoardArchiveConfig::default().max_transfer_bytes
                    });
                let sink = std::sync::Arc::new(nemesis_cluster::transfer::TransferSink::new(
                    &ws_dir, limit0,
                ));
                if let Some(store) = board_store.clone() {
                    let store_landed = store.clone();
                    sink.set_on_landed(std::sync::Arc::new(move |task_id, dir| {
                        crate::board_archive_ingest::ingest_landed(&store_landed, task_id, dir);
                    }));
                    let store_ol = store;
                    sink.set_on_overlimit(std::sync::Arc::new(move |req| {
                        crate::board_archive_ingest::note_overlimit(&store_ol, req);
                    }));
                }
                let transport = std::sync::Arc::new(
                    nemesis_cluster::outbox::RpcTransferTransport::new(rc.clone(), node_id.clone()),
                );
                let limit_cell = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(limit0));
                let provider: Box<dyn Fn() -> u64 + Send + Sync> = {
                    let c = limit_cell.clone();
                    Box::new(move || c.load(std::sync::atomic::Ordering::Relaxed))
                };
                let outbox = std::sync::Arc::new(nemesis_cluster::outbox::TransferOutbox::new(
                    &ws_dir,
                    node_id.clone(),
                    transport,
                    provider,
                ));
                // CD6（2026-09-17）：健康联动注入——RpcClient 判 Offline 的
                // 对端暂停回传（跳过本轮，不计数不打日志），回 Online 后下
                // 一 tick 自然恢复。节点未知按可推处理（fast-fail 诚实暴露）。
                outbox.set_online_check(Box::new({
                    let rc = rc.clone();
                    move |peer| rc.is_peer_online(peer).unwrap_or(true)
                }));
                // P4/E2（看板项目档案 goal 合并批）：基线推送器装配——board.rs
                // 派发链（dispatch_issue_core）消费；复用既有分块传输通路
                //（begin/chunk/end，AEAD 鉴权）+ 同一 limit 热刷新 cell（D4
                // 护栏基线下发同源）。board store 未装配 = 不装（派发走既有
                // 无基线路径，push_dispatch_baseline Ok(None)）。
                if board_store.is_some()
                    && let Some(rc) = rpc_client.clone()
                {
                    let push_transport = std::sync::Arc::new(
                        nemesis_cluster::outbox::RpcTransferTransport::new(rc, node_id.clone()),
                    );
                    let cell = limit_cell.clone();
                    let pusher =
                        std::sync::Arc::new(nemesis_web::handlers::board::BaselinePusher {
                            transport: push_transport,
                            source_node_id: node_id.clone(),
                            max_bytes: Box::new(move || {
                                cell.load(std::sync::atomic::Ordering::Relaxed)
                            }),
                        });
                    if nemesis_web::handlers::board::install_baseline_pusher(pusher) {
                        info!("[Gateway] Board baseline pusher armed (E2)");
                    }
                }
                // D4 热生效：30s 周期现读 config.json board.archive 段。
                {
                    let sink_ref = sink.clone();
                    let cell = limit_cell.clone();
                    let cfg_path = std::path::Path::new(&home).join("config.json");
                    tokio::spawn(async move {
                        let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                        loop {
                            ticker.tick().await;
                            if let Ok(raw) = std::fs::read_to_string(&cfg_path)
                                && let Ok(live) =
                                    serde_json::from_str::<nemesis_config::Config>(&raw)
                                && let Some(b) = live.board
                            {
                                let v = b.archive.max_transfer_bytes;
                                cell.store(v, std::sync::atomic::Ordering::Relaxed);
                                sink_ref.set_max_bytes(v);
                            }
                        }
                    });
                }
                (Some(sink), Some(outbox))
            }
            None => (None, None),
        };
        #[cfg(not(all(feature = "board", feature = "cluster")))]
        let (transfer_sink, transfer_outbox): (
            Option<std::sync::Arc<nemesis_cluster::transfer::TransferSink>>,
            Option<std::sync::Arc<nemesis_cluster::outbox::TransferOutbox>>,
        ) = (None, None);

        // Set result persister for fallback when callback fails.
        // 2026-09-08 G1 收口：同一份 persister 同时交给 peer_chat_handler
        // （legacy 路径）与 cluster agent work-queue 路径（经
        // cluster_adapter_refs → ClusterServiceAdapter）。此前 work-queue
        // 路径（生产唯一路径）不接 persister：回调失败真结果不落盘 → G5
        // 恢复轮询只能拿到 running 占位；回调成功占位也不清理。
        let persister: Arc<dyn nemesis_cluster::rpc::peer_chat_handler::TaskResultPersister> =
            Arc::new(ClusterResultPersisterAdapter {
                result_store: result_store.clone(),
                node_id: node_id_for_handler.clone(),
                outbox: transfer_outbox.clone(),
                workspace: Some(home.join("workspace")),
            });
        handler.set_result_persister(persister.clone());

        // We'll register the handler after Arc::new(cluster) below.
        let handler_arc = Arc::new(handler);
        // Register callback handler (placeholder — will be replaced after Arc::new below).
        // This placeholder just acknowledges receipt.
        {
            let _ = cluster.register_rpc_handler(
                "peer_chat_callback",
                Box::new(move |payload| {
                    let task_id = payload
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    Ok(serde_json::json!({"status": "placeholder", "task_id": task_id}))
                }),
            );
        }

        let cluster = Arc::new(cluster);

        // 二期批次五：cluster 句柄回填槽（桥身份 sink 注入 / hello 身份快照
        // 构造，均在块外 relay/web 装配段消费）。
        let _ = bridge_cluster_slot.set(cluster.clone());

        // --- P3（看板项目档案 goal）：档案传输 handler 注册 + 循环启动 ---
        // 5 个 handler（begin/chunk/end/overlimit/pull；master 收前四，worker
        // 收 pull——全员同注册按角色自然分流）。启动清扫补崩溃残留（pushing
        // 重置 + cluster_logs 残留补入队）+ 推送循环（15s tick + kick）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        if let (Some(sink), Some(outbox)) = (transfer_sink.clone(), transfer_outbox.clone()) {
            match nemesis_cluster::outbox::register_transfer_handlers(
                &cluster,
                sink,
                Some(outbox.clone()),
            ) {
                Ok(()) => {
                    // E3 顺序强制：exec 残留清扫必须先于 outbox sweep_startup
                    // ——后者会对 cluster_logs 残留补纯记录入队，条目一旦先建，
                    // 带变更集的入队就被幂等挡死（模块头注释钦定）。
                    let swept = nemesis_cluster::exec_workspace::sweep_exec_residual(
                        &home.join("workspace"),
                        &outbox,
                    );
                    if swept > 0 {
                        info!(
                            "[Gateway] Exec workspace residual swept: {swept} task(s) re-enqueued"
                        );
                    }
                    outbox.sweep_startup();
                    outbox.spawn_push_loop();
                    info!("[Gateway] Transfer stack armed (archive inbox+outbox+5 handlers)");
                }
                Err(e) => warn!("[Gateway] Transfer handler registration skipped: {}", e),
            }
            // D5 兜底拉取 sweep（master 侧；60s 周期）。
            if let (Some(store), Some(rc)) = (board_store.clone(), rpc_client.clone()) {
                let transport = std::sync::Arc::new(
                    nemesis_cluster::outbox::RpcTransferTransport::new(rc, node_id.clone()),
                );
                let seen = Arc::new(tokio::sync::Mutex::new(
                    std::collections::HashSet::<String>::new(),
                ));
                let ws_dir = home.join("workspace");
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(60));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    loop {
                        ticker.tick().await;
                        crate::board_archive_ingest::sweep_missing_archives(
                            &store,
                            &ws_dir,
                            transport.as_ref(),
                            &seen,
                        )
                        .await;
                    }
                });
                info!("[Gateway] Board archive D5 sweep armed (interval=60s)");
            }
        }

        // W2 P4: 回填 autopilot 集群槽位（on_job 闭包经 OnceLock 取用）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let _ = autopilot_cluster_slot.set(cluster.clone());
            // A2 停车场 sweep 槽位回填（节点发现闭包经 OnceLock 取用）。
            // 启动期 announce 早于回填也不丢：下一个 announce 周期兜底。
            let _ = sweep_cluster_slot.set(cluster.clone());
        }

        // --- Inject cluster task queue into cluster for callback routing ---
        cluster.set_cluster_task_queue(cluster_task_list.clone(), cluster_work_queue.clone());

        // --- Swarm M3: nb_bus handler（board 信封协议）---
        // master 与 worker 同名注册 `nb_bus`，按信封 ns/op 路由；本节点是
        // master（coordinator 角色 + board_store 在手）才注册上行 op，否则装
        // worker 侧唤醒通道 + 周期 board.sync 补拉。master 判据必须用集群
        // 角色：board_store 是全员 open 的（每个节点都有本地 board.db 作
        // dashboard 视图），拿它当 master 判据会让 worker 注册错 handler、
        // 唤醒通道成死代码（2026-09-09 UAT T23 根因）。cluster 未启动时
        // 注册失败静默（register_rpc_handler 要求 running——与 peer_chat
        // 同款忽略策略），board.sync 兜底语义不受影响。board feature 裁剪
        // 形态不参与讨论。（dashboard 人工发言桥在下文 board_service 装配
        // 处接线。）
        #[cfg(all(feature = "board", feature = "cluster"))]
        let board_master_armed = board_store.is_some()
            && matches!(
                cluster.role().as_str(),
                "coordinator" | "master" | "manager"
            );
        #[cfg(all(feature = "board", feature = "cluster"))]
        if board_master_armed {
            let deps = crate::board_bus::MasterBusDeps {
                store: board_store.clone().expect("board_store checked above"),
                quota: board_quota.clone(),
                cluster: cluster.clone(),
                moderator_loop: board_moderator_loop.clone(),
                workspace: home.join("workspace"),
            };
            match cluster.register_rpc_handler(
                nemesis_cluster::envelope::NB_BUS_ACTION,
                crate::board_bus::build_master_nb_bus_handler(deps),
            ) {
                Ok(()) => info!("[Gateway] Registered nb_bus handler (board envelope protocol)"),
                Err(e) => warn!("[Gateway] nb_bus handler registration skipped: {}", e),
            }
        } else {
            let inbox = Arc::new(crate::cluster_agent::DiscussionInbox::new());
            let deps = crate::board_bus::WorkerBusDeps {
                self_node_id: cluster.node_id().to_string(),
                node_name: cluster.node_name(),
                node_role: cluster.role(),
                node_category: cluster.category(),
                inbox: inbox.clone(),
                wake_state: Arc::new(crate::board_bus::WorkerWakeState::load_or_create(
                    nemesis_path::board_wake_state_path(&home),
                )),
            };
            match cluster.register_rpc_handler(
                nemesis_cluster::envelope::NB_BUS_ACTION,
                crate::board_bus::build_worker_nb_bus_handler(deps.clone()),
            ) {
                Ok(()) => info!("[Gateway] Registered worker nb_bus handler (board wake channel)"),
                Err(e) => warn!("[Gateway] worker nb_bus registration skipped: {}", e),
            }
            crate::board_bus::spawn_worker_sync_loop(deps, cluster.clone());
            board_worker_inbox = Some(inbox);
        }

        // --- Swarm M3 资产 RPC 兜底通路（2026-09-20，提供方侧）---
        // asset.meta / asset.chunk：跨网段 HTTP 直连不可达时，消费方经集群
        // RPC 分块拉取（验证链与 web 下载端点同构——表白名单 + HMAC 验签）。
        // 每节点都注册（任何节点都是潜在提供方，worker 产物反走同一条路）；
        // 集群未启动时注册失败静默（与 peer_chat 同款忽略策略——RPC 通路
        // 是 HTTP 下载的兜底，缺它只降级不致残）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let deps = crate::board_asset_rpc::AssetRpcDeps {
                workspace: home.join("workspace"),
                board_store: board_store.clone(),
            };
            match cluster.register_rpc_handler(
                crate::board_asset_rpc::ACTION_ASSET_META,
                crate::board_asset_rpc::build_meta_handler(deps.clone()),
            ) {
                Ok(()) => info!("[Gateway] Registered asset.meta handler (RPC asset fallback)"),
                Err(e) => warn!("[Gateway] asset.meta handler registration skipped: {}", e),
            }
            match cluster.register_rpc_handler(
                crate::board_asset_rpc::ACTION_ASSET_CHUNK,
                crate::board_asset_rpc::build_chunk_handler(deps),
            ) {
                Ok(()) => info!("[Gateway] Registered asset.chunk handler (RPC asset fallback)"),
                Err(e) => warn!("[Gateway] asset.chunk handler registration skipped: {}", e),
            }
        }

        // --- Register peer_chat handler (needs Arc<Cluster> to register remote nodes) ---
        {
            let handler_ref = handler_arc.clone();
            let cluster_ref = cluster.clone();
            let _ = cluster.register_rpc_handler(
                "peer_chat",
                Box::new(move |payload| {
                    // Extract source node ID from RPC metadata injected by the server.
                    let source_node_id = payload
                        .get("_rpc")
                        .and_then(|r| r.get("from"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();

                    if !source_node_id.is_empty() {
                        // Register the remote node in our registry so we can callback later.
                        // The remote node may not be known via UDP discovery yet (static peers
                        // use peer names, not node_ids). We use the RPC port from the payload
                        // (sent by the remote node's ClusterRpcTool).
                        if cluster_ref.get_peer(&source_node_id).is_none() {
                            // T26 根修（2026-09-18）：不再以硬编码缺省值（name=id、
                            // 127.0.0.1、"worker"、"general"、21949 fallback）直接
                            // 登记——RPC 先于 announce 到达时会触发地址匹配占位
                            // 升级，把 operator 配置的 role=coordinator/category/
                            // udp 地址整体覆盖成缺省值并落盘 peers.toml（重启复
                            // 活，worker_sync_once 找不到 coordinator）。改为
                            // cluster 侧继承占位身份（payload 缺端口提示时传 0，
                            // 由占位端口派生）。
                            let remote_rpc_port = payload
                                .get("_source_rpc_port")
                                .and_then(|v| v.as_u64())
                                .unwrap_or(0)
                                as u16;
                            cluster_ref.register_rpc_peer(&source_node_id, remote_rpc_port);
                        }
                    }

                    // Pass RpcMeta to PeerChatHandler so it can read source_node_id from
                    // `rpc_meta.from` (authoritative wire-level sender ID) and chat_id from
                    // `payload._source.chat_id` (filled by the originating node's tasks_submit).
                    // Together these form the composite session_key `cluster_rpc:{node_id}/{chat_id}`
                    // for LLM conversation isolation.
                    let rpc_meta = nemesis_cluster::rpc::peer_chat_handler::RpcMeta {
                        from: if source_node_id.is_empty() {
                            None
                        } else {
                            Some(source_node_id.clone())
                        },
                    };
                    let h = handler_ref.clone();
                    let ack = h.handle(payload, Some(rpc_meta));
                    Ok(serde_json::to_value(&ack)
                        .unwrap_or_else(|_| serde_json::json!({"status": "error"})))
                }),
            );
            info!("[Gateway] Registered PeerChatHandler (async LLM + callback) for peer_chat");
        }

        // --- H4: 任务恢复 handler（query_task_result / confirm_task_delivery）---
        // gateway 装配不走 set_rpc_channel（该入口无人触发 →
        // register_peer_chat_handlers 不会执行），这里显式补注册，与
        // peer_chat/callback/task_cancel 并列。缺失时 A 侧
        // poll_stale_pending_tasks（120s 周期）永远收到 "no handler"，
        // B 重启丢结果后的 stale 恢复链路断裂（2026-09-01 跨机 E2E 实测）。
        cluster.register_task_recovery_handlers();
        info!(
            "[Gateway] Registered task recovery handlers (query_task_result/confirm_task_delivery)"
        );

        // --- W2 P4: task_cancel handler — per-task cancel ---
        // 取消指定 peer_chat 任务的执行：排队中的直接出队丢弃，运行中的经
        // running_tokens 广播取消（LLM 迭代间隙/工具派发前检查）。这是与
        // estop 正交的细粒度取消（estop 冻结全部 agent 活动，此处只停一个
        // 任务）；board issue.cancel 经此送达 worker。
        {
            let task_list_for_cancel = cluster_task_list.clone();
            let _ = cluster.register_rpc_handler(
                "task_cancel",
                Box::new(move |payload| {
                    let task_id = payload
                        .get("task_id")
                        .and_then(|v| v.as_str())
                        .unwrap_or("");
                    if task_id.is_empty() {
                        return Err("missing field: task_id".to_string());
                    }
                    let outcome = task_list_for_cancel.cancel_task(task_id);
                    info!(
                        "[Gateway] task_cancel: task={} outcome={:?}",
                        task_id, outcome
                    );
                    Ok(serde_json::to_value(&outcome)
                        .unwrap_or_else(|_| serde_json::json!({"outcome": "error"})))
                }),
            );
            info!("[Gateway] Registered task_cancel handler (per-task cancel)");
        }

        // --- W2 P4: 派发超时 sweep（board 派发无人回报的兜底）---
        // board.dispatch_timeout_secs = 0 关闭；扫描间隔
        // dispatch_sweep_interval_secs（下限 1s）。失败处置（⛔ 评论 + 通知）
        // 在 sweep_dispatch_timeouts 内完成，赢 fail_dispatch 竞态才动账。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let sweep_cfg = cfg.board.clone().unwrap_or_default();
            if sweep_cfg.dispatch_timeout_secs > 0 {
                let store_for_sweep = board_store.clone();
                let cluster_for_sweep = cluster.clone();
                let home_for_sweep = home.clone();
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        sweep_cfg.dispatch_sweep_interval_secs.max(1),
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    // CD6（裁决⑫，2026-09-17）：mtime 缓存——config 文件未
                    // 变化不重读，消除派发 sweep 每 20s tick 触发 load_config
                    // 两条 INFO 刷屏；热生效语义保留（R4：改文件 → mtime 变化
                    // → 下个 tick 重新现读）。
                    let mut sweep_timeout_cache: Option<(std::time::SystemTime, u64)> = None;
                    loop {
                        ticker.tick().await;
                        let Some(store) = store_for_sweep.as_ref() else {
                            continue;
                        };
                        // 超时阈值每 tick 现读（2026-09-15 R4 真机实证：
                        // config.set dispatch_timeout_secs 改值后 sweep 仍用
                        // 启动烘焙值跑到底——与旗标类键「每次现读」语义对齐；
                        // 0=关（运行期可关）；读取失败沿用启动值兜底不停摆。
                        let timeout_secs = {
                            let path = home_for_sweep.join("config.json");
                            let mtime = std::fs::metadata(&path).and_then(|m| m.modified()).ok();
                            let cached = match (mtime.as_ref(), sweep_timeout_cache.as_ref()) {
                                (Some(t), Some((cached_t, v))) if t == cached_t => Some(*v),
                                _ => {
                                    let v = nemesis_config::load_config(&path)
                                        .ok()
                                        .and_then(|c| c.board)
                                        .map(|b| b.dispatch_timeout_secs)
                                        .unwrap_or(sweep_cfg.dispatch_timeout_secs);
                                    if let Some(t) = mtime {
                                        sweep_timeout_cache = Some((t, v));
                                    }
                                    Some(v)
                                }
                            };
                            cached.unwrap_or(sweep_cfg.dispatch_timeout_secs)
                        };
                        if timeout_secs == 0 {
                            continue;
                        }
                        sweep_dispatch_timeouts(store, &cluster_for_sweep, timeout_secs);
                    }
                });
                info!(
                    "[Gateway] Board dispatch sweep armed (timeout={}s, interval={}s)",
                    sweep_cfg.dispatch_timeout_secs,
                    sweep_cfg.dispatch_sweep_interval_secs.max(1)
                );
            }
        }

        // --- F-U3-4（2026-09-15 U3 真机）：停车场周期兜底 sweep ticker ---
        // 停车场 sweep 此前只有边沿触发（announce 回调 / 派发落定重估波）。
        // 真机实证：announce 单向不可达（跨子网 UDP/防火墙不对称；G2 探针
        // 走 RPC 让节点照常 Online——高度迷惑）+ 稳态在线（无 Offline→Online
        // 翻转）+ 无派发落定时，停车场**永不重估**——停车单/reopen 单无限期
        // 滞留且零反馈（NB-10 实证：reopen 后 10+ 分钟零动静）。补 30s 周期
        // ticker：候选空时近零成本（一条 SQL）；estop 挂起不派发；
        // notify_park=true 让首次停车落 ⏸ 评论 + 父单 blocked 显形（B4 去重
        // 防刷屏）；三路触发源由 PARK_SWEEP_LOCK 串行防双派。ticker 首拍
        // 立即执行——gateway 重启后存量停车单即时获得一次重估。
        #[cfg(all(feature = "board", feature = "cluster"))]
        {
            let park_sweep_store = board_store.clone();
            let park_sweep_cluster = cluster.clone();
            let park_sweep_estop = estop.clone();
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await;
                    if park_sweep_estop.is_engaged() {
                        continue;
                    }
                    let Some(store) = park_sweep_store.as_ref() else {
                        continue;
                    };
                    let actor = nemesis_board::Actor::system("board");
                    let (cands, dispatched, failed) =
                        nemesis_web::handlers::board::sweep_parked_dispatches_notify(
                            store,
                            &park_sweep_cluster,
                            &actor,
                            true,
                        );
                    if cands > 0 {
                        info!(
                            "[Gateway] 停车场周期重估：候选 {cands} 派出 {dispatched} 失败 {failed}"
                        );
                    }
                }
            });
            info!("[Gateway] Board park sweep ticker armed (interval=30s)");
        }

        // --- Now that Cluster is Arc-wrapped, wire up the real callback handler ---
        // Routes callbacks to the correct destination:
        // 1. If the callback matches a ClusterAgent child task (nested cluster_rpc),
        //    inject it back into the ClusterAgent's work queue.
        // 2. Otherwise, publish to bus as cluster_continuation for the main AgentLoop.
        // 3. Update TaskManager task status (for dashboard-initiated peer_chat).
        {
            let bus_for_cb = bus.clone();
            let task_list_for_cb = cluster_task_list.clone();
            let work_queue_for_cb = cluster_work_queue.clone();
            let cluster_for_cb = cluster.clone();
            // E1 二期 token 回传：worker 回传的 usage 记入 master 用量账本
            //（session_key=cluster_rpc:{worker}/{task_id}，E1 token 预算闸
            // 按派发行精确聚合）。未装配 DataStore = 记账静默跳过。
            let ds_for_usage_cb = data_store.clone();
            // W2 P2 派发写回：board 派发的 task_id 命中 issue_dispatch →
            // 写回看板。board feature 未编译时占位（拦截整体被 cfg 掉）。
            #[cfg(feature = "board")]
            let board_store_for_cb = board_store.clone();
            // 交付线程超限汇报落资产需要 workspace 根（层 2 HTTP 资产目录）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let workspace_for_cb = home.join("workspace");
            // Swarm M4 验收 agent 读 config.json 旗标需要 home 根。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let home_for_cb = home.clone();
            // M4 评审用主 loop 桥槽——闭包外再 clone 一份（原 Arc 稍后
            // set(agent_loop) 还要用）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let moderator_loop_for_cb = board_moderator_loop.clone();
            // P1/T1-6：estop 保险丝随评审依赖集进回调闭包（冻结停车用）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let estop_for_cb = estop.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let estop_parked_for_cb = board_estop_parked.clone();
            // P4/B2b：取证路由表随回调闭包（selfcheck 命中判定）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let selfcheck_for_cb = board_selfcheck_registry.clone();
            #[cfg(not(feature = "board"))]
            #[allow(unused_variables)]
            let board_store_for_cb: Option<()> = None;
            // CD3：恢复交付回调的依赖集——在回调闭包 move 走原值之前克隆
            // （同一批依赖，两份闭包各自持有 Arc）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_store = board_store_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_workspace = workspace_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_home = home_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_moderator = moderator_loop_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_cluster = cluster_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_estop = estop_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_estop_parked = estop_parked_for_cb.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let delivery_selfcheck = selfcheck_for_cb.clone();
            let _ = cluster.register_rpc_handler("peer_chat_callback", Box::new(move |payload| {
                let task_id = payload
                    .get("task_id")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let status = payload
                    .get("status")
                    .and_then(|v| v.as_str())
                    .unwrap_or("success");
                let response = payload
                    .get("response")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                // P1（2026-09-11 真机日志分析）：worker 的 error 回调把错误
                // 文本放在 `error` 字段、`response` 为空（send_callback 的
                // 契约），此前本 handler 从不提取 error 字段——所有路由拿到
                // 空串，错误详情全丢。合并文本：error 非空用 error，否则
                // response（success 回调二者等价）。
                let error_field = payload
                    .get("error")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");
                let fail_text: &str = if response.is_empty() {
                    error_field
                } else {
                    response
                };
                // worker 身份走传输层：RPC server 派发前注入 `_rpc.from`
                //（server.rs enhancePayload 同款），payload 本体没有
                // source_node 字段——T37 真机实证空段。优先 _rpc.from，
                // 显式字段保留为前向兼容 fallback。
                let source_node = payload
                    .get("_rpc")
                    .and_then(|m| m.get("from"))
                    .and_then(|v| v.as_str())
                    .or_else(|| {
                        payload
                            .get("source_node")
                            .and_then(|v| v.as_str())
                    })
                    .unwrap_or("");
                // E1 二期：usage 可选字段（serde 兼容——旧 worker 无此字段
                // 不炸；只认 object 形态）。
                let usage = payload.get("usage").filter(|v| v.is_object());
                record_cluster_usage(
                    ds_for_usage_cb.as_ref(),
                    source_node,
                    task_id,
                    usage,
                );
                // P2A（2026-09-12 NB-15）：结构化失败分类（可选字段，旧
                // worker 无此字段不炸；只认字符串形态）。随写回落到 ⛔
                // 失败评论，验收重派决策据此避免同 worker 同模型盲重派。
                let fail_class = payload
                    .get("fail_class")
                    .and_then(|v| v.as_str())
                    .unwrap_or("");

                info!("[Gateway] peer_chat_callback received: task_id={}, status={}, from={}", task_id, status, source_node);

                // P4/B2b 自检取证拦截（先于一切路由）：命中 SelfcheckRegistry
                // 的 task_id 是取证任务（不写 issue_dispatch——Route 0 的
                // 写回不适用；不进 Route 2 续行 bus——无续行快照只会告警），
                // 路由到二段验收。TaskManager 收口（Route 3）照常——取证
                // 任务也是 submit_peer_chat 登记的，不收口留 ghost pending。
                #[cfg(all(feature = "board", feature = "cluster"))]
                let selfcheck_issue_id = if task_id.is_empty() {
                    None
                } else {
                    selfcheck_for_cb.take(task_id)
                };
                #[cfg(any(not(feature = "board"), not(feature = "cluster")))]
                let selfcheck_issue_id: Option<i64> = None;
                #[cfg(all(feature = "board", feature = "cluster"))]
                if let Some(sc_issue_id) = selfcheck_issue_id {
                    info!(
                        "[Gateway] peer_chat_callback selfcheck branch: task_id={}, issue={}",
                        task_id, sc_issue_id
                    );
                    crate::board_review::spawn_selfcheck_second_stage(
                        crate::board_review::BoardReviewDeps {
                            store: board_store_for_cb.clone().expect(
                                "selfcheck armed implies store present",
                            ),
                            workspace: workspace_for_cb.clone(),
                            home: home_for_cb.clone(),
                            moderator_loop: moderator_loop_for_cb.clone(),
                            cluster: cluster_for_cb.clone(),
                            estop: estop_for_cb.clone(),
                            estop_parked: estop_parked_for_cb.clone(),
                            selfcheck: selfcheck_for_cb.clone(),
                        },
                        sc_issue_id,
                        status.to_string(),
                        fail_text.to_string(),
                    );
                    // TaskManager 状态收口（同 Route 3 语义）。
                    let result_value = serde_json::json!({
                        "status": status,
                        "response": response,
                        "source_node": source_node,
                    });
                    if status == "error" {
                        cluster_for_cb.fail_task(task_id, fail_text);
                    } else {
                        cluster_for_cb.complete_task(task_id, result_value);
                    }
                    return Ok(serde_json::json!({"status": "received", "task_id": task_id}));
                }

                // Route 0: Board 派发写回（W2 P2）——命中 issue_dispatch 的
                // task_id 直接写回看板（结果评论 + 状态推进），且跳过 Route 2
                // 的 agent 续行（board 派发无续行快照）；TaskManager 状态更新
                // （Route 3）照常，board 派发也登记了本地 task。
                // Swarm M4：推进到 in_review 的写回携带评审触发目标，下方
                // spawn 验收 agent。
                #[cfg(all(feature = "board", feature = "cluster"))]
                let board_writeback = write_back_board_dispatch(
                    &board_store_for_cb,
                    &workspace_for_cb,
                    task_id,
                    status,
                    fail_text,
                    fail_class,
                );
                #[cfg(all(feature = "board", feature = "cluster"))]
                let is_board_task = board_writeback.is_board_task;
                #[cfg(any(not(feature = "board"), not(feature = "cluster")))]
                let is_board_task = false;

                // Swarm M4 批作业：in_review 自动验收（三态处置 + FAIL 重派
                // 保险丝）。依赖集在此快照——moderator 桥槽/board store/
                // cluster 均为 Arc，评审任务运行时再解引用。
                #[cfg(all(feature = "board", feature = "cluster"))]
                if let Some(review_issue_id) = board_writeback.issue_for_review {
                    crate::board_review::spawn_board_review(
                        crate::board_review::BoardReviewDeps {
                            store: board_store_for_cb.clone().expect(
                                "board writeback armed implies store present",
                            ),
                            workspace: workspace_for_cb.clone(),
                            home: home_for_cb.clone(),
                            moderator_loop: moderator_loop_for_cb.clone(),
                            cluster: cluster_for_cb.clone(),
                            estop: estop_for_cb.clone(),
                            estop_parked: estop_parked_for_cb.clone(),
                            selfcheck: selfcheck_for_cb.clone(),
                        },
                        review_issue_id,
                    );
                }

                // R-9 互斥释放波（2026-09-13 T-sched-1 实机缺口）：board 派发
                // 落定（done/failed 写回）后重估停车场——touch 互斥延后单的
                // 「下一触发波」此前只有节点事件（上线/刷新），稳态集群中冲突
                // 派发落定后延后单会永久滞留 backlog（NB-18 实证）。复用
                // sweep_parked_dispatches 单一重估波：候选=planner 来源或暂缓
                // 标记。EST-05（2026-09-16 横扫修正）：此前注释谎称「estop/
                // 预算闸都在 dispatch_subissue_auto 内重跑」——实际 estop 闸
                // 现已下沉到 dispatch_issue_core（EST-01/02），准入/touch 互斥
                // 闸也在其内；**预算闸（E1）只在评审侧判定点**（board_review
                // budget_breach），不经本链路。新增触发源按此真实边界补闸。
                #[cfg(all(feature = "board", feature = "cluster"))]
                if board_writeback.settled && !estop_for_cb.is_engaged() {
                    let sweep_store = board_store_for_cb.clone();
                    let sweep_cluster = cluster_for_cb.clone();
                    let sweep_task = task_id.to_string();
                    tokio::spawn(async move {
                        if let Some(store) = sweep_store.as_ref() {
                            let actor = nemesis_board::Actor::system("board");
                            let (cands, dispatched, failed) =
                                nemesis_web::handlers::board::sweep_parked_dispatches(
                                    store,
                                    &sweep_cluster,
                                    &actor,
                                );
                            if dispatched > 0 || failed > 0 {
                                info!(
                                    "[Gateway] 派发落定重估波：候选 {cands} 派出 {dispatched} 失败 {failed}（task {sweep_task} 落定触发）"
                                );
                            }
                        }
                    });
                }

                // Route 1: Check if this callback belongs to a ClusterAgent child task.
                // When the ClusterAgent's LLM generates a nested cluster_rpc, the child
                // task's callback must be routed back to the ClusterAgent work queue,
                // not to the main AgentLoop's continuation system.
                if !task_id.is_empty()
                    && let Some(parent_task_id) = task_list_for_cb.find_by_child_task_id(task_id) {
                        info!(
                            "[Gateway] Callback for child task {} matched ClusterAgent parent task {}, injecting result",
                            task_id, parent_task_id
                        );
                        // P1：error 回调的文本在 error 字段——注入合并文本，
                        // 父任务 LLM 才能看到真实失败原因。
                        task_list_for_cb.inject_callback(&parent_task_id, fail_text);
                        if let Err(e) = work_queue_for_cb.submit(parent_task_id) {
                            warn!("[Gateway] Failed to submit resumed task to work queue: {}", e);
                        }
                        return Ok(serde_json::json!({"status": "received", "task_id": task_id}));
                    }

                // Route 2: Main AgentLoop continuation — publish to bus.
                // （board 派发任务跳过：无续行快照，进 bus 只会告警。）
                if !task_id.is_empty() && !is_board_task {
                    let mut metadata = std::collections::HashMap::new();
                    metadata.insert("status".to_string(), status.to_string());
                    // 集群续行归属（2026-09-23）：回调 source 是节点 ID——
                    // 徽章要人读名字，注册表在线时换名；查不到（对端已逐出/
                    // 老恢复快照）回退原 ID（诚实显示）。G5 恢复发布点在
                    // nemesis-cluster 内无注册表句柄，保持 ID 直传。
                    let source_display = cluster_for_cb
                        .get_peer(source_node)
                        .map(|p| p.base.name)
                        .filter(|n| !n.is_empty())
                        .unwrap_or_else(|| source_node.to_string());
                    metadata.insert("source_node".to_string(), source_display);
                    // P1：content 与 metadata.error 都用合并文本——B 端 error
                    // 回调的 response 为空，续行 tool 结果须携带真实错误。
                    metadata.insert("error".to_string(), fail_text.to_string());

                    let inbound = nemesis_types::channel::InboundMessage {
                        channel: "system".to_string(),
                        sender_id: format!("cluster_continuation:{}", task_id),
                        chat_id: String::new(),
                        content: fail_text.to_string(),
                        media: vec![],
                        session_key: String::new(),
                        correlation_id: String::new(),
                        metadata,
                        voice_playback: None,
                    };
                    bus_for_cb.publish_inbound(inbound);
                    info!("[Gateway] Published cluster_continuation for task_id={}", task_id);
                }

                // Route 3: Update TaskManager task status (for dashboard-initiated peer_chat).
                if !task_id.is_empty() {
                    let result_value = serde_json::json!({
                        "status": status,
                        "response": response,
                        "source_node": source_node,
                    });
                    if status == "error" {
                        cluster_for_cb.fail_task(task_id, fail_text);
                    } else {
                        cluster_for_cb.complete_task(task_id, result_value);
                    }
                }

                Ok(serde_json::json!({"status": "received", "task_id": task_id}))
            }));

            // CD3（2026-09-17）：恢复交付回调——恢复轮询（poll_stale_pending_tasks）
            // 查回 worker 结果后、confirm 删 worker 副本前触发。路由判断留在
            // 本闭包（cluster 不依赖 board）：
            // - issue_dispatch 反查命中 → write_back_board_dispatch 看板写回
            //   （终结派发 + 结果评论 + 状态推进），并补 spawn 评审（对齐
            //   peer_chat_callback Route 0 的完整链路，否则恢复回来的单会
            //   卡 in_review 无人验收）；settled（真实终结）才算交付成功。
            // - 未命中（chat 任务）→ 交付 = 恢复腿已 publish bus 续行帧，恒
            //   true。返回 false 时 cluster 跳过 confirm，worker 副本留 TTL
            //   兜底（宁留勿丢）。
            // fail_class 对齐 peer_chat_callback 语义：恢复腿查询结果不携带
            // 该字段，传空串（⛔ 评论降级为无分类文案）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            {
                cluster.set_on_recovered_delivery(Arc::new(
                    move |task_id: &str, status: &str, response: &str, error: Option<&str>| {
                        // error 非空 = result_status=error——合并文本语义对齐
                        // peer_chat_callback 的 fail_text（error 优先，否则
                        // 用 response）。
                        let text = match error {
                            Some(e) if !e.is_empty() => e,
                            _ => response,
                        };
                        let outcome = write_back_board_dispatch(
                            &delivery_store,
                            &delivery_workspace,
                            task_id,
                            status,
                            text,
                            "",
                        );
                        if !outcome.is_board_task {
                            return true; // chat 任务：交付 = 已发 bus 续行
                        }
                        // 对齐 Route 0：推进到 in_review 的写回补 spawn 验收
                        // agent（spawn_board_review 自带 tokio::spawn；恢复
                        // 腿运行在 tokio 上下文——恢复循环 spawn，可直接调）。
                        if let Some(review_issue_id) = outcome.issue_for_review {
                            crate::board_review::spawn_board_review(
                                crate::board_review::BoardReviewDeps {
                                    store: delivery_store
                                        .clone()
                                        .expect("board writeback armed implies store present"),
                                    workspace: delivery_workspace.clone(),
                                    home: delivery_home.clone(),
                                    moderator_loop: delivery_moderator.clone(),
                                    cluster: delivery_cluster.clone(),
                                    estop: delivery_estop.clone(),
                                    estop_parked: delivery_estop_parked.clone(),
                                    selfcheck: delivery_selfcheck.clone(),
                                },
                                review_issue_id,
                            );
                        }
                        outcome.settled
                    },
                ));
            }
        }

        // --- Inject MessageBus into Cluster for continuation flow ---
        // Cluster.handle_task_complete() publishes cluster_continuation messages
        // on the bus, which AgentLoop intercepts to resume from snapshots.
        {
            let bus_adapter = Arc::new(BusToClusterAdapter { bus: bus.clone() });
            cluster.set_message_bus(bus_adapter);
            info!("[Gateway] Cluster: message bus injected for continuation flow");
        }

        // --- Network components: only start when cluster is fully enabled ---
        if cluster_should_start {
            // Wire Forge-Cluster bridge
            #[cfg(feature = "forge")]
            {
                if let Some(ref forge_arc) = forge_for_web {
                    let cluster_bridge = ClusterForgeBridgeAdapter::new(node_id.clone());
                    forge_arc.set_bridge(Arc::new(cluster_bridge));
                    info!("[Gateway] Forge-Cluster bridge wired (node_id={})", node_id);
                }
            }

            // Start UDP Discovery Service (managed by Cluster)
            cluster.start_discovery(cluster.clone());
            info!(
                "[Gateway] UDP discovery started on port {}",
                cluster_app_cfg.port
            );

            // RPC server was already created and set above before start().
            // RPC client was already created by Cluster::start().

            // Create cluster_rpc config + RPC call function for SharedResources.
            // The factory function will create the ClusterRpcTool and register it.
            let rpc_cfg = nemesis_agent::ClusterRpcConfig {
                local_node_id: node_id.clone(),
                timeout_secs: 3600,
                local_rpc_port: cluster_app_cfg.rpc_port,
            };

            // Wire the RPC call function to use cluster.call_with_context_async
            let cluster_weak_for_rpc = Arc::downgrade(&cluster);
            // CD5：占位续行快照需要 workspace 根（ContinuationStore 单一
            // 真相源路径解析在其内部）。
            let home_for_rpc = home.clone();
            let call_fn = std::sync::Arc::new(
                move |target: &str, action: &str, payload: serde_json::Value| {
                    // 每次调用克隆——async move 块按值捕获，避免把环境里的
                    // 变量移出导致闭包退化为 FnOnce（call_fn 是共享 Arc）。
                    let home_for_rpc = home_for_rpc.clone();
                    let c = match cluster_weak_for_rpc.upgrade() {
                        Some(arc) => arc,
                        None => {
                            return Box::pin(async move {
                                Err("Cluster已关闭，RPC调用不可用".to_string())
                            })
                                as std::pin::Pin<
                                    Box<
                                        dyn std::future::Future<
                                                Output = Result<serde_json::Value, String>,
                                            > + Send,
                                    >,
                                >;
                        }
                    };
                    let t = target.to_string();
                    let a = action.to_string();
                    Box::pin(async move {
                        // CD5-a（2026-09-17）：A 端预生成 task_id——chat 派发
                        // 此前 task_id 由 B 端 ACK 生成，「ACK 已收、正式续行
                        // 快照未落盘」窗口崩溃即无声丢失。预生成后随 payload
                        // 下发（B 端 peer_chat_handler 原样采用外来 id），A 端
                        // 从发起时刻起持有同一凭据。board 路径不走本闭包
                        // （自带登记），按 action 过滤。
                        let mut payload = payload;
                        let pre_task_id = if a == "peer_chat" {
                            match payload.get("task_id").and_then(|v| v.as_str()) {
                                Some(s) if !s.is_empty() => None, // 调用方自带
                                _ => {
                                    let id = format!("chat-{}", uuid::Uuid::new_v4());
                                    if let Some(obj) = payload.as_object_mut() {
                                        obj.insert(
                                            "task_id".to_string(),
                                            serde_json::Value::String(id.clone()),
                                        );
                                    }
                                    Some(id)
                                }
                            }
                        } else {
                            None
                        };

                        // CD5-b：派发前落占位续行快照——崩溃后 G5 重建（扫
                        // rpc_cache 登记 Pending）+ 恢复轮询至少能诚实收口；
                        // peer_id 随行（恢复时才知道 poll 该问谁）。正式快照
                        // （AgentLoop 存续行快照，同 task_id）会覆盖它。
                        if let Some(ref task_id) = pre_task_id {
                            let ws = home_for_rpc.join("workspace");
                            let store = nemesis_agent::ContinuationStore::new(&ws);
                            let placeholder = nemesis_agent::ContinuationSnapshot {
                                task_id: task_id.clone(),
                                messages: "[]".to_string(),
                                tool_call_id: String::new(),
                                channel: String::new(),
                                chat_id: String::new(),
                                session_key: String::new(),
                                peer_id: t.clone(),
                                image_refs: Vec::new(),
                                image_refs_by_user_turn: Vec::new(),
                                created_at: chrono::Local::now().to_rfc3339(),
                                final_persisted: false,
                            };
                            if let Err(e) = store.save(&placeholder) {
                                tracing::warn!(
                                    task_id = %task_id,
                                    error = %e,
                                    "[Gateway] CD5 占位续行快照写入失败（恢复凭据缺失，不影响派发）"
                                );
                            }
                        }

                        let rpc_result = c
                            .call_with_context_async(
                                &t,
                                &a,
                                payload,
                                std::time::Duration::from_secs(3600),
                            )
                            .await
                            .map_err(|e| e.to_string());

                        let bytes = match rpc_result {
                            Ok(b) => b,
                            Err(e) => {
                                // CD5：确定未送达（对端离线 fast-fail）→ 清理
                                // 占位快照（任务在 B 端不存在，凭据无意义）。
                                // 其余失败（超时等）保留——B 端可能已接单执行，
                                // 凭据留给重启后的恢复轮询查询（宁留勿丢）。
                                if let Some(ref task_id) = pre_task_id {
                                    if e.contains("peer is offline") {
                                        let ws = home_for_rpc.join("workspace");
                                        nemesis_agent::ContinuationStore::new(&ws).delete(task_id);
                                    } else {
                                        tracing::warn!(
                                            task_id = %task_id,
                                            error = %e,
                                            "[Gateway] CD5 chat 派发失败（非离线形态），占位快照保留供恢复轮询查询"
                                        );
                                    }
                                }
                                return Err(e);
                            }
                        };

                        let ack: serde_json::Value = serde_json::from_slice(&bytes)
                            .map_err(|e| format!("Failed to parse RPC response: {}", e))?;

                        // CD1（2026-09-17）：ACK accepted → 登记 TaskManager
                        // Pending——恢复轮询从此接管查询（worker 死亡/分区后
                        // 安全网诚实收尾，对话不再永久悬挂）。重复登记由
                        // submit 同 id Err 幂等闸吸收。B 端明确拒绝（非
                        // accepted）→ 清理占位快照（任务不存在）。
                        if a == "peer_chat" {
                            let accepted =
                                ack.get("status").and_then(|v| v.as_str()) == Some("accepted");
                            let ack_task_id =
                                ack.get("task_id").and_then(|v| v.as_str()).unwrap_or("");
                            if accepted && !ack_task_id.is_empty() {
                                let task = nemesis_types::cluster::Task {
                                    id: ack_task_id.to_string(),
                                    status: nemesis_types::cluster::TaskStatus::Pending,
                                    action: "peer_chat".to_string(),
                                    peer_id: t.clone(),
                                    payload: serde_json::json!({}),
                                    result: None,
                                    original_channel: String::new(),
                                    original_chat_id: String::new(),
                                    created_at: chrono::Local::now().to_rfc3339(),
                                    completed_at: None,
                                };
                                if let Err(e) = c.task_manager().submit(task) {
                                    tracing::debug!(
                                        task_id = %ack_task_id,
                                        error = %e,
                                        "[Gateway] chat 派发登记 TaskManager 跳过（同 id 已登记）"
                                    );
                                } else {
                                    tracing::info!(
                                        task_id = %ack_task_id,
                                        peer = %t,
                                        "[Gateway] chat 派发已登记 TaskManager Pending（CD1 恢复接管）"
                                    );
                                }
                            } else if !accepted && let Some(ref task_id) = pre_task_id {
                                let ws = home_for_rpc.join("workspace");
                                nemesis_agent::ContinuationStore::new(&ws).delete(task_id);
                            }
                        }

                        Ok(ack)
                    })
                        as std::pin::Pin<
                            Box<
                                dyn std::future::Future<Output = Result<serde_json::Value, String>>
                                    + Send,
                            >,
                        >
                },
            );

            // Store for SharedResources (factory function will register the tool).
            cluster_rpc_call_fn = Some(call_fn);
            cluster_rpc_config = Some(rpc_cfg);

            // cluster_rpc tool registration is now handled by the factory function.
            // The rpc_call_fn is stored here for SharedResources consumption.
            info!(
                "[Gateway] cluster_rpc tool created (node: {}, peers loaded from peers.toml)",
                node_name
            );

            // Build peers_fn: closure that returns online peers with capabilities
            // from the Cluster registry, EXCLUDING the local node. Used by
            // ClusterRpcTool's dynamic tool description so the LLM never sees
            // itself as a valid cluster_rpc target (prevents self-invocation loops).
            {
                let cluster_weak_for_peers = Arc::downgrade(&cluster);
                cluster_peers_fn = Some(Arc::new(move || match cluster_weak_for_peers.upgrade() {
                    Some(c) => c
                        .get_online_peers_excluding_self()
                        .into_iter()
                        .map(|p| (p.base.id, p.base.name, p.capabilities))
                        .collect(),
                    None => Vec::new(),
                }));
            }
        } else {
            info!(
                "[Gateway] Cluster initialized (inactive) — start via Dashboard or enable in config"
            );
        }

        // ContinuationManager injection into agent_loop is now handled by the factory function.

        // Save references for ClusterServiceAdapter creation (after SharedResources is built).
        // The adapter will be created later in the code where SharedResources is available.
        // For now, save the cluster-related Arc refs needed.
        cluster_adapter_refs = Some((
            cluster.clone(),
            cluster_task_list.clone(),
            cluster_work_queue.clone(),
            persister.clone(),
        ));
    }

    Ok(ClusterWiring {
        cluster_rpc_call_fn,
        cluster_rpc_config,
        cluster_peers_fn,
        #[cfg(feature = "cluster")]
        cluster_adapter_refs,
        #[cfg(feature = "cluster")]
        cluster_should_start,
        #[cfg(feature = "cluster")]
        bridge_cluster_slot,
        #[cfg(feature = "cluster")]
        board_worker_inbox,
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_estop_parked,
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_selfcheck_registry,
    })
}
