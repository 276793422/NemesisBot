// ---------------------------------------------------------------------------
// PB-5 Agent→Web 注入族（计划 §4.2 B5）：Swarm M3 主持人桥填装 + 集群能力
// 注入 + ClusterServiceAdapter 构建（refs 四元组 take）+ AgentLoopService
// Adapter + 项目常驻 loop + WebServer set_* 注入链（bus/过滤链/模型信息/
// streaming/agent 服务/estop/LSP/事件接收器/cron/看板/数据存储/内存/forge/
// cluster/workflow/密钥）整体迁入 `init_post_agent`。原文自 run() 逐字迁入，
// 仅三处机械改写：fn 体包裹、ctx/agent/cluster 依赖项影子重绑、末尾
// PostAgentWiring 构造。
//
// 签名取 `(ctx, &mut web_server, web_bind_host, web_port, AgentWiring,
// ClusterWiring)`：web_server 以 &mut 入参（run() 侧 &mut 绑定传递，B6 起
// 继续持有）；web_bind_host/web_port 按值（Step 9 注入日志单点消费，run()
// 侧保 clone）；agent/cluster 按值——agent_event_rx move 进 set_agent_
// event_rx、cluster 的 refs 四元组 take + worker inbox take 均原地耗尽，
// 两 struct 消费后消亡（run() 侧影子已先行 delete）。
//
// 相应 run() 侧演进（B5 头注记）：B2 的 cluster_should_start/
// board_worker_inbox/board_estop_parked/board_selfcheck_registry/
// cluster_adapter_refs 影子与 cluster_adapter None 前向声明删除（消费
// 全在本函数内）；B4 的 agent_event_rx 影子删除（唯一消费
// set_agent_event_rx 在本函数内）；shared_resources/agent_loop/
// initial_tool_count/security_plugin/bridge_cluster_slot 影子留守（下游
// Step 15–23 消费）。
//
// PostAgentWiring 字段 = 实测逃逸本函数的完整集合（四项）：agent_adapter
// （tray 启停/心跳 handler/Step 14 start，无门）、projects_manager（Step
// 23 stop_all，无门）、cluster_adapter（PB-7 局域 IP/heal/tray/停机 take，
// @cluster）、cluster_arc_ref（PB-7 board_role/资产签发 node_id，
// @cluster）。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use super::{AgentWiring, ClusterWiring, GatewayCtx};
use crate::adapters;

/// PB-5 产物 struct（§4.2 B5）。
pub(crate) struct PostAgentWiring {
    /// Tray 启停 + 心跳 handler + Step 14 start（无门）。
    pub agent_adapter: Arc<adapters::AgentLoopServiceAdapter>,
    /// Step 23 停机 stop_all（无门）。
    pub projects_manager: Arc<crate::projects::manager::ProjectLoopManager>,
    /// PB-7 real_port 局域 IP / heal spawn / tray 菜单 / 停机 take（@cluster）。
    #[cfg(feature = "cluster")]
    pub cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>>,
    /// PB-7 board_role 解析 / 资产签发 node_id（@cluster）。
    #[cfg(feature = "cluster")]
    pub cluster_arc_ref: Option<std::sync::Arc<nemesis_cluster::cluster::Cluster>>,
}

/// Step 9c–10 注入族（计划 §4.2 B5）。
pub(crate) async fn init_post_agent(
    ctx: &GatewayCtx,
    web_server: &mut nemesis_web::server::WebServer,
    web_bind_host: String,
    web_port: i64,
    agent: AgentWiring,
    mut cluster: ClusterWiring,
) -> Result<PostAgentWiring> {
    // ctx 影子重绑（B1 同款）：门控取各名字在本函数内的实际消费门。
    let home = ctx.home.clone();
    let cfg = ctx.cfg.clone();
    let bus = ctx.bus.clone();
    let model_name = ctx.model_name.clone();
    let resolution = ctx.resolution.clone();
    let cron_service = ctx.cron_service.clone();
    let conv_router = ctx.conv_router.clone();
    let data_store = ctx.data_store.clone();
    #[cfg(feature = "board")]
    let board_store = ctx.board_store.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_moderator_loop = ctx.board_moderator_loop.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_asset_url_slot = ctx.board_asset_url_slot.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_quota = ctx.board_quota.clone();
    #[cfg(feature = "memory")]
    let memory_manager_for_web = ctx.memory_manager_for_web.clone();
    #[cfg(feature = "forge")]
    let forge_for_web = ctx.forge_for_web.clone();
    #[cfg(feature = "workflow")]
    let workflow_engine = ctx.workflow_engine.clone();
    #[cfg(feature = "workflow")]
    let chat_secret_store = ctx.chat_secret_store.clone();
    // agent wiring 影子：五项按值拆包（参数按值消费）；initial_tool_count
    // 唯一消费在 @cluster 能力注入块，影子随门收放。
    let shared_resources = agent.shared_resources;
    let agent_loop = agent.agent_loop;
    let agent_event_rx = agent.agent_event_rx;
    #[cfg(feature = "cluster")]
    let initial_tool_count = agent.initial_tool_count;
    // cluster wiring 影子：refs 克隆为本地 mut（take 耗尽的是克隆——四元组
    // 皆 Arc 家族，克隆廉价且与原体共享对象，语义等价于原文耗尽原体；owned
    // 本地令原文 `ref` 模式逐字可用——借用形态会撞隐式借用模式中的显式
    // ref 错误）；worker inbox 以 &mut 借改（take 耗尽），should_start 复制，
    // board 评审两件克隆（hook_deps 构造消费）。
    #[cfg(feature = "cluster")]
    let mut cluster_adapter_refs = cluster.cluster_adapter_refs.clone();
    #[cfg(feature = "cluster")]
    let board_worker_inbox = &mut cluster.board_worker_inbox;
    #[cfg(feature = "cluster")]
    let cluster_should_start = cluster.cluster_should_start;
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_estop_parked = cluster.board_estop_parked.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_selfcheck_registry = cluster.board_selfcheck_registry.clone();
    // Cluster adapter — manages dynamic start/stop of all cluster components.
    // B2 注记：None 前向声明随 B5 迁入本函数——赋值在下方 ClusterService
    // Adapter 构建（四元组消费点）。
    #[cfg(feature = "cluster")]
    let mut cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>> = None;

    // --- Swarm M3: 主持人裁决桥填装（nb_bus 注册早于 agent_loop 构建）---
    #[cfg(all(feature = "board", feature = "cluster"))]
    {
        if board_moderator_loop.set(agent_loop.clone()).is_err() {
            warn!("[Gateway] Board moderator loop already set");
        }
    }

    // --- 全自动流转 P1（A3）：父单收口验收钩子注册 ---
    // sync_parent_status 子单全 done 路径 → 本钩子（读 auto_close_parent
    // 旗标 + master 角色闸）→ spawn_parent_review。旗标每次现读：config
    // 热改即时生效，与 load_board_flags 同语义。worker 节点本地 board.db
    // 是 dashboard 视图，非权威——角色闸保持与 write_back 触发链同级。
    #[cfg(all(feature = "board", feature = "cluster"))]
    {
        let cluster_ok = cluster_adapter_refs
            .as_ref()
            .map(|(c, _, _, _)| matches!(c.role().as_str(), "coordinator" | "master" | "manager"))
            .unwrap_or(false);
        // 装配缺失可观测（2026-09-15 R4 真机实证：coordinator 误配 role=worker
        // 时本链静默不装配，档案管线派发完成后合并/评审/父单+项目收口全死、
        // 单据卡 in_progress 无任何决策动作。worker 角色闸是有意设计，但装配
        // 缺失必须响亮——有集群而角色不符才 warn；无集群单节点看板不噪音）。
        if !cluster_ok && let Some((c, _, _, _)) = cluster_adapter_refs.as_ref() {
            warn!(
                "[Gateway] Board 合并/评审/收口钩子不装配：节点角色非 coordinator（role={}；worker 本地 board.db 仅 dashboard 视图）——档案管线合并+验收链不会运行",
                c.role().as_str()
            );
        }
        if cluster_ok
            && let (Some(store), Some((cluster, _, _, _))) =
                (board_store.clone(), cluster_adapter_refs.as_ref())
        {
            let deps = crate::board_review::BoardReviewDeps {
                store,
                workspace: home.join("workspace"),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
            };
            let hook_deps = std::sync::Arc::new(deps);
            // 启动重放（R4-BUG-2 根修）用快照——hook_deps 本体随后 move 进
            // 父单收口钩子闭包。
            let replay_sweep_deps = hook_deps.clone();
            // P1/T1-6：estop 释放 watcher——把冻结停车的评审逐条复评恢复。
            crate::board_review::spawn_estop_resume_watcher((*hook_deps).clone());
            // P4/E4（看板项目档案 goal 合并批）：合并依赖注入——落地腿
            //（ingest_landed）/写回腿（write_back）合并触发 + estop release
            // 补跑共用同一份 deps。
            if crate::board_archive_ingest::install_merge_deps((*hook_deps).clone()) {
                info!("[Gateway] Board archive merge deps armed (E4)");
            }
            // P5/F4：resume 补合并回放钩子——project.resume 冻结先行段经
            // 此回调进 board_archive_ingest::replay_pending_merges（依赖
            // 倒置：nemesis-web 不反向依赖 nemesisbot）。
            let replay_deps = hook_deps.clone();
            nemesis_web::handlers::board::set_resume_replay_hook(std::sync::Arc::new(
                move |project_id: i64| {
                    crate::board_archive_ingest::replay_pending_merges(
                        replay_deps.as_ref(),
                        project_id,
                    )
                },
            ));
            // S-O1：合并停车人工重试钩子——WSAPI audit.retry_merge 经此回调
            // 进 board_archive_ingest::retry_merge_for_issue（依赖倒置同上）。
            let retry_deps = hook_deps.clone();
            nemesis_web::handlers::board::set_retry_merge_hook(std::sync::Arc::new(
                move |issue: &nemesis_board::models::Issue| {
                    crate::board_archive_ingest::retry_merge_for_issue(retry_deps.as_ref(), issue)
                },
            ));
            let hook_home = home.clone();
            let hook_cluster = cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_parent_review_hook(
                std::sync::Arc::new(move |parent_id: i64| {
                    // 旗标现读（fail-closed：读失败不收口）。master 判定
                    // 与 nb_bus handler 注册同款（board_store 全员 open
                    // ≠ master 身份）。
                    let is_master = matches!(
                        hook_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    );
                    if !is_master {
                        return;
                    }
                    let flags = nemesis_config::load_config(&hook_home.join("config.json"))
                        .map(|c| c.board.unwrap_or_default());
                    match flags {
                        Ok(f) if f.auto_close_parent => {
                            crate::board_review::spawn_parent_review(
                                (*hook_deps).clone(),
                                parent_id,
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                "[Gateway] 父单 {parent_id} 收口旗标读取失败（fail-closed 转人工）：{e}"
                            );
                        }
                    }
                }),
            ) {
                warn!("[Gateway] Board parent review hook register failed: {e}");
            } else {
                info!("[Gateway] Board parent review hook armed (auto_close_parent)");
            }

            // --- 全自动流转 P4（F3）：项目收口验收钩子注册 ---
            // 全部顶层父单 done → notify 聚合预检过了才 fire；旗标
            // `board.review.auto_close_project` 每次现读（同父单钩子语义）。
            let proj_deps = std::sync::Arc::new(crate::board_review::BoardReviewDeps {
                store: board_store
                    .clone()
                    .expect("cluster_ok arm guarantees board store present"),
                workspace: home.join("workspace"),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
            });
            let proj_home = home.clone();
            let proj_cluster = cluster.clone();
            // F9 快照先行——下方 review hook 闭包会 move 同一对 Arc，
            // 总结钩子（更下方）需要自己的副本。
            let sum_deps = proj_deps.clone();
            let sum_cluster = proj_cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_project_review_hook(
                std::sync::Arc::new(move |project_id: i64| {
                    let is_master = matches!(
                        proj_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    );
                    if !is_master {
                        return;
                    }
                    let flags = nemesis_config::load_config(&proj_home.join("config.json"))
                        .map(|c| c.board.unwrap_or_default());
                    match flags {
                        Ok(f) if f.auto_review && f.review.auto_close_project => {
                            crate::board_review::spawn_project_review(
                                (*proj_deps).clone(),
                                project_id,
                            );
                        }
                        Ok(_) => {}
                        Err(e) => {
                            tracing::warn!(
                                "[Gateway] 项目 {project_id} 收口旗标读取失败（fail-closed 转人工）：{e}"
                            );
                        }
                    }
                }),
            ) {
                warn!("[Gateway] Board project review hook register failed: {e}");
            } else {
                info!("[Gateway] Board project review hook armed (review.auto_close_project)");
            }

            // F9（看板项目档案 P6）：人工收口（project.update → completed）
            // 触发收口总结；spawn_project_summary 内部自守门（estop/tier/
            // 目录缺失诚实跳过），生成失败不阻塞收口。master 判定同上
            //（非 master 节点的 board store 是只读镜像，不跑 LLM 收尾）。
            if let Err(e) = nemesis_web::handlers::board::set_project_summary_hook(
                std::sync::Arc::new(move |project_id: i64| {
                    if !matches!(
                        sum_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    ) {
                        return;
                    }
                    crate::board_review::spawn_project_summary((*sum_deps).clone(), project_id);
                }),
            ) {
                warn!("[Gateway] Board project summary hook register failed: {e}");
            } else {
                info!("[Gateway] Board project summary hook armed (archive summary.md)");
            }

            // 启动重放（R4-BUG-2 根修）：评审 spawn 纯内存、master 重启即
            // 丢——in_review 单据/父单/项目在重启后由这里扫描重触发。必须
            // 在三类钩子注册完成后调用（重放验收 PASS 会级联点火上层钩子）。
            crate::board_review::replay_stuck_reviews(&replay_sweep_deps, &[]);
            info!("[Gateway] Board stuck-review replay sweep done");
        }
    }

    // --- Inject tool capabilities into cluster for discovery broadcast ---
    #[cfg(feature = "cluster")]
    {
        if let Some((ref cluster, _, _, _)) = cluster_adapter_refs {
            let tool_names = agent_loop.tool_names();
            cluster.set_capabilities(tool_names);
            info!(
                "[Gateway] Cluster capabilities injected ({} tools)",
                initial_tool_count
            );
        }
    }

    // --- Create ClusterServiceAdapter (always, for dynamic start/stop from Dashboard) ---
    // The adapter manages: cluster.start(), RPC server start, discovery start,
    // task recovery from disk, cluster agent loop spawn, ClusterRpcTool enable.
    // 批次 E：dashboard 发言桥要用的 cluster 引用（下方 take() 会把 Arc 消耗
    // 进 adapter，先抢一份）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_discussion_cluster: Option<std::sync::Arc<nemesis_cluster::cluster::Cluster>> =
        cluster_adapter_refs.as_ref().map(|(c, _, _, _)| c.clone());
    // 同理抢一份通用 cluster 引用：下方 take() 把 refs 消耗进 adapter 后
    // refs 恒为 None——board_role 解析 / 资产签发 node_id / node_id 落盘
    // 等任何「拿本节点 cluster 直读」的装配点都从这里取（2026-09-20 真机
    // 验证发现：refs 在 take 之后读取恒 None，worker 的 bundle node_id
    // 全空、RPC 兜底寻址失效）。
    #[cfg(feature = "cluster")]
    let cluster_arc_ref: Option<std::sync::Arc<nemesis_cluster::cluster::Cluster>> =
        cluster_adapter_refs.as_ref().map(|(c, _, _, _)| c.clone());
    #[cfg(feature = "cluster")]
    {
        if let Some((cluster, task_list, work_queue, result_persister)) =
            cluster_adapter_refs.take()
        {
            let adapter = Arc::new(crate::cluster_service::ClusterServiceAdapter::new(
                cluster,
                shared_resources.clone(),
                tokio::runtime::Handle::current(),
                home.clone(),
                task_list,
                work_queue,
                result_persister,
                board_worker_inbox.take(),
            ));
            // Only perform first start when both config flags are enabled.
            // Otherwise the adapter is created but idle — can be started from Dashboard.
            // ASM-08 复核（2026-09-16）：装配自检失败（关键件未接线=代码回归）
            // 启动即炸（D5 裁决）；运行时故障维持 warn 降级。
            if cluster_should_start && let Err(e) = adapter.first_start() {
                if e.contains("ASM-08") {
                    return Err(anyhow::anyhow!("[Gateway] Cluster loop {}", e));
                }
                warn!("[Gateway] Cluster adapter first start failed: {}", e);
            }
            cluster_adapter = Some(adapter);
            // CD4（2026-09-17）：master 重启后从看板在途派发行重建
            // TaskManager Pending——board 派发无续行快照，G5 只救 chat 任务；
            // worker 已落盘的结果此前永远无人查询（7 天 TTL 蒸发）。重建后
            // 恢复轮询自然接管，查回结果经 CD3 恢复交付回调写回看板。
            #[cfg(all(feature = "board", feature = "cluster"))]
            if cluster_should_start {
                crate::cluster_service::rebuild_pending_from_board_dispatches(
                    cluster_adapter
                        .as_ref()
                        .expect("just assigned above")
                        .cluster(),
                    &board_store,
                );
            }
        }
    }

    // Create shared reference for WebServer model switching
    let agent_loop_ref: Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>> =
        Arc::new(parking_lot::RwLock::new(None));

    // Create AgentLoopServiceAdapter for tray start/stop control.
    // Passes the initial AgentLoop directly — no double construction.
    // The adapter manages the inbound bridge + agent spawn internally.
    let agent_adapter = Arc::new(adapters::AgentLoopServiceAdapter::new(
        agent_loop.clone(),
        shared_resources.clone(),
        bus.clone(),
        agent_loop_ref.clone(),
    ));

    // --- L6++（2026-09-08）：项目常驻 loop 启动（对话/项目双分组）---
    // 共享主 loop 内建的同一 SessionStore Arc（存储全局集中、会话隔离靠
    // session_key）；注册表缺失/损坏走 lenient 空表，目录消失的项目
    // warn + skip 不炸 gateway。G3 起项目调度器经 manager 把项目会话消息
    // 转发进对应项目 loop（主桥 skip 谓词同步接线）。
    let projects_manager = {
        let main_store = agent_loop
            .session_store()
            .cloned()
            .expect("main agent loop must carry a session store");
        let mgr = Arc::new(crate::projects::manager::ProjectLoopManager::new(
            shared_resources.clone(),
            main_store,
            bus.clone(),
        ));
        mgr.start_all();
        mgr
    };
    // G3（2026-09-08）：主桥 skip 谓词（项目会话消息不进主 loop，由项目
    // 调度器转发）+ 全进程唯一 1 个项目调度订阅。时序在 web bind 之前，
    // 满足「loop 订阅 bus → web bind」不变量。
    {
        let mgr_for_pred = projects_manager.clone();
        agent_adapter.set_skip_predicate(Arc::new(move |msg| mgr_for_pred.bridge_should_skip(msg)));
        projects_manager.start_routing();
        // G4（2026-09-08）：ProjectsBridge 接线——projects.* WSAPI 与
        // resolve_session_loop（chat/tools/approval/question/agent/fs 各
        // handler 的归属解析）经此 trait 触达 manager（trait 在
        // ProjectLoopManager 上直接实现，Arc 协同转换装槽）。
        let projects_bridge: std::sync::Arc<dyn nemesis_web::handlers::projects::ProjectsBridge> =
            projects_manager.clone();
        nemesis_web::handlers::projects::install_projects_bridge(projects_bridge);
    }

    // Step 10: Wire up WebServer (created early for WebChannel injection)
    web_server.set_message_bus(bus.clone());
    // 入站过滤链（BUG 2026-09-23 项目会话历史修复）：web 咽喉点在
    // bus.publish_inbound 前过链，链上过滤器可就地拦截应答。首个过滤器
    // = HistoryFilter（history 只读查询不再依赖任何 agent loop 的存亡/
    // 忙闲）。未来谁想拦什么，谁构造 FilterChain 往里注册即可（框架见
    // nemesis-bus filter 模块）。
    {
        let inbound_chain: std::sync::Arc<
            nemesis_bus::FilterChain<nemesis_types::channel::InboundMessage>,
        > = std::sync::Arc::new(nemesis_bus::FilterChain::new());
        inbound_chain.attach(std::sync::Arc::new(
            nemesis_web::history_filter::HistoryFilter::new(bus.clone()),
        ));
        tracing::info!(
            filters = ?inbound_chain
                .list()
                .iter()
                .map(|(n, p)| format!("{n}@{p}"))
                .collect::<Vec<_>>(),
            "[Gateway] 入站过滤链已装配（{} 个过滤器）",
            inbound_chain.list().len()
        );
        web_server.set_inbound_filter_chain(inbound_chain);
    }
    web_server.set_model_info(
        &model_name,
        &resolution.api_base,
        !resolution.api_key.is_empty(),
    );

    // Wire streaming provider for SSE chat endpoint + persona generation.
    //
    // 协议感知装配（B 根修 2026-09-17）：此前这里固定构造裸 HttpProvider（只讲
    // OpenAI wire /chat/completions）——主模型切 anthropic 协议（如 CC Switch +
    // glm-5.3-flash）后，persona 生成与 /api/chat/stream 把请求打到错误端点全灭，
    // 且 CC Switch 对 OpenAI 路径回 200 包装错误 → 空响应静默成功（人格生成
    // 0.4s 假失败根因）。改走与主 loop 同源的 factory（同一 resolution +
    // protocol）：anthropic → AnthropicProvider（含流式）、openai → HttpCompat；
    // CLI 型 provider 流式未实现会诚实报「不支持」。超时不再写死 120s，与 P3A
    // 全 lane 统一口径（per-model timeout_secs，缺省 600s）。
    {
        let streaming_factory_cfg = nemesis_providers::factory::FactoryConfig {
            proxy: resolution.proxy.clone(),
            llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
            api_key: resolution.api_key.clone(),
            api_base: resolution.api_base.clone(),
            workspace: home.join("workspace").to_string_lossy().to_string(),
            connect_mode: resolution.connect_mode.clone(),
            protocol: resolution.protocol.clone(),
            timeout_secs: resolution.timeout_secs,
            account_id: String::new(),
            headers: std::collections::HashMap::new(),
        };
        // 双击直启 goal：装配失败装 NullProvider（SSE 流诚实报「未配置模型」），
        // 不再留空槽——空槽的报错形态对双击新用户是二级谜语。
        let (raw_streaming_provider, streaming_warn) =
            nemesis_providers::factory::create_provider_or_null(&streaming_factory_cfg);
        // 默认跟随 wrapper（2026-09-22 方案A）：SSE/persona 传的是
        // AppState.model_name（活槽文本，热切会更新）——wrapper 判据同时
        // 比对捕获名与当前名（见 default_slot::route），否则换型后的新名
        // 会被误判成钉扎、继续打旧 provider（「旧 provider + 新模型名」
        // 跨厂商错配）。
        let streaming_provider = nemesis_providers::default_slot::default_following(
            raw_streaming_provider,
            &resolution.model_name,
            &streaming_factory_cfg.llm_ref,
        );
        web_server.set_streaming_provider(streaming_provider);
        if let Some(e) = streaming_warn {
            warn!(
                "[Gateway] Streaming provider assembly failed — SSE/persona lane degraded (NullProvider): {}",
                e
            );
        } else {
            info!(
                "[Gateway] Streaming provider configured (protocol-aware) for /api/chat/stream + persona"
            );
        }
    }

    info!(
        "[Gateway] Web server created for {}:{}",
        web_bind_host, web_port
    );

    // Inject agent service into web server for start/stop control
    web_server.set_agent_service(agent_adapter.clone());
    info!("[Gateway] Agent service injected into web server");

    // Inject global e-stop state so /api/internal can trigger/release/query it
    // (EstopState is a thread-safe Arc<AtomicBool+watch>; the web handler
    // mutates it directly — no mpsc round-trip needed, and status returns live).
    web_server.set_estop(shared_resources.estop.clone());
    info!("[Gateway] E-stop state injected into web server");

    // EST-01/02（2026-09-16 横扫加固）：同一 estop 实例注入看板派发族闸
    // （dispatch_issue_core 单一入口）——急停冻结全部自动/手动派发，而非
    // 只有 agent loop。
    #[cfg(feature = "cluster")]
    {
        if nemesis_web::handlers::board::install_board_estop(shared_resources.estop.clone()) {
            info!("[Gateway] E-stop gate installed for board dispatch family");
        }
    }

    // C5: inject the shared LSP manager (same Arc the LspTool registered
    // with) — Phase-2 diagnostics loop and future dashboard LSP ops read it.
    web_server.set_lsp_manager(shared_resources.lsp_manager.clone());
    info!("[Gateway] LSP manager injected into web server");

    // M1a: hand the tool-event receiver to the web server — its pump (spawned
    // in WebServer::start) routes events to Dashboard WS push + EventHub.
    web_server.set_agent_event_rx(agent_event_rx);
    info!("[Gateway] Agent tool-event receiver injected into web server");

    // Inject the runtime CronService (so tasks.cron.* calls the live scheduler)
    // and the ConvRouter (shared with the cron fire handler for Opt 2 live
    // delivery). CronService is cloned because gateway still owns a handle to
    // start() it later; conv_router is moved (its only other reference is the
    // clone already captured in the cron fire closure).
    web_server.set_cron(cron_service.clone());
    web_server.set_conv_router(conv_router);
    info!("[Gateway] CronService + ConvRouter injected into web server");

    // Inject the managed-agent board store (board feature only; None →
    // board.* WSAPI commands report "board service not available").
    #[cfg(feature = "board")]
    if let Some(ref store) = board_store {
        // 角色解析（goal 硬约束①：复用 NodeRole，无平行 role 字段）：
        // - cluster 启用 → 取 cluster.role()（peers.toml [node].role；
        //   from_role_str 兼容旧值 master/manager）；
        // - cluster 关闭 / cluster feature 未编译 → Coordinator。
        // role 2026-08-31 起仅为元数据（日志/诊断展示），不门控 board 写——
        // board.db 是节点本地数据，写权限与 role 无关（见 BoardService 文档）。
        #[cfg(feature = "cluster")]
        let board_role = if cluster_should_start {
            cluster_arc_ref
                .as_ref()
                .map(|c| nemesis_types::cluster::NodeRole::from_role_str(&c.role()))
                .unwrap_or(nemesis_types::cluster::NodeRole::Coordinator)
        } else {
            nemesis_types::cluster::NodeRole::Coordinator
        };
        #[cfg(not(feature = "cluster"))]
        let board_role = nemesis_types::cluster::NodeRole::Coordinator;
        // Swarm M3（§5.4/D6）：资产服务装配——密钥 load-or-create
        // （<workspace>/config/asset_secret.key；损坏 loud 报错不重置，
        // 重置=作废全部已签发 token）+ 资产目录 <workspace>/board/assets。
        // 齐备后本节点就是资产提供方（公开端点 /api/board/asset/{ref} 验
        // 自己的 secret），worker 产物反走同一条路。签发上下文挂 store
        // （dispatch 链全部函数持有 store，零参数蔓延）；对外基址槽 bind
        // 后 set（见下方 real_port 解析处）。
        let mut board_service = nemesis_board::BoardService::new(store.clone(), board_role);
        #[cfg(feature = "cluster")]
        match nemesis_board::asset_token::load_or_create_secret(
            &nemesis_path::resolve_asset_secret_path_in_workspace(&home.join("workspace")),
        ) {
            Ok(secret) => {
                store.set_asset_signing(nemesis_board::AssetSignContext {
                    secret: secret.clone(),
                    node_url: board_asset_url_slot.clone(),
                    // 进 bundle 的 node_id 字段（RPC 兜底寻址）。取 take()
                    // 前抢好的 cluster 副本（refs 槽已被消耗恒 None）；
                    // 缺失即非集群形态 → 空串。
                    node_id: cluster_arc_ref
                        .as_ref()
                        .map(|c| c.node_id().to_string())
                        .unwrap_or_default(),
                });
                board_service = board_service.with_asset_secret(secret).with_assets_dir(
                    nemesis_path::resolve_board_assets_dir_in_workspace(&home.join("workspace")),
                );
                info!("[Gateway] Board asset serving armed (HMAC token endpoint)");
            }
            Err(e) => warn!("[Gateway] Board asset serving disabled: {}", e),
        }
        #[cfg(not(feature = "cluster"))]
        let _ = &mut board_service;
        // Swarm M3 批次 E：dashboard 人工发言桥（board.channel.post → 讨论管
        // 线）。board+cluster 齐备且本节点持有 board_store（master 形态）时
        // 注入——幂等/额度/裁决与 worker 上行同一条管线（单一真相）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        if let (Some(store), Some(discussion_cluster)) =
            (board_store.as_ref(), board_discussion_cluster.clone())
        {
            board_service =
                board_service.with_discussion(Arc::new(crate::board_bus::LocalDiscussionIngress {
                    deps: crate::board_bus::MasterBusDeps {
                        store: store.clone(),
                        quota: board_quota.clone(),
                        cluster: discussion_cluster,
                        moderator_loop: board_moderator_loop.clone(),
                        workspace: home.join("workspace"),
                    },
                }));
            info!(
                "[Gateway] Board discussion ingress armed (dashboard channel.post → nb_bus pipeline)"
            );
        }
        web_server.set_board(board_service);
        info!(
            "[Gateway] Board service injected into web server (role={})",
            board_role.as_role_str()
        );

        // --- W2.5: board 数据变化 watcher → SSE 广播 ---
        // 独立零写入连接轮询 `PRAGMA data_version`（只对其他连接的写敏感）：
        // WSAPI / 集群写回 / autopilot / sweep（BoardStore 自己的连接）与
        // CLI 子命令（跨进程）的每次落库都会被看见——写路径零埋点覆盖全部
        // 写入方。变化 → SSE `board-changed` → 前端各面板 200ms 防抖刷新。
        // 无 SSE 订阅者（dashboard 未开）时跳过轮询读数，空闲零成本。
        const BOARD_CHANGE_POLL_SECS: u64 = 2;
        let board_db_for_watch = home.join("workspace").join("board").join("board.db");
        let event_hub_for_watch = web_server.event_hub().clone();
        match nemesis_board::watcher::open_conn(&board_db_for_watch) {
            Ok(watch_conn) => {
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        BOARD_CHANGE_POLL_SECS,
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut last = nemesis_board::watcher::data_version(&watch_conn).ok();
                    loop {
                        ticker.tick().await;
                        if event_hub_for_watch.subscriber_count() == 0 {
                            continue;
                        }
                        let Ok(v) = nemesis_board::watcher::data_version(&watch_conn) else {
                            continue;
                        };
                        if last != Some(v) {
                            last = Some(v);
                            event_hub_for_watch.publish(
                                "board-changed",
                                serde_json::json!({ "ts": chrono::Utc::now().to_rfc3339() }),
                            );
                            tracing::debug!(
                                "[Board] change detected (data_version={v}) → board-changed"
                            );
                        }
                    }
                });
            }
            Err(e) => {
                tracing::warn!("[Gateway] board change watcher disabled: {e}");
            }
        }
    }

    // Inject DataStore into web server for usage statistics API
    if let Some(ref ds) = data_store {
        web_server.set_data_store(ds.clone());
        info!("[Gateway] DataStore injected into web server");

        // --- A3: usage 明细变化 watcher → SSE `usage-changed` ---
        // 独立零写入连接轮询 `PRAGMA data_version`（board 同款原语）：
        // AgentLoop / workflow LLM 节点经 DataStore 自己连接的每次落库都
        // 会被看见，写路径零埋点。变化 → SSE `usage-changed` → 前端请求
        // 明细 tab 200ms 防抖静默刷新。无 SSE 订阅者（dashboard 未开）
        // 时跳过轮询读数，空闲零成本。
        const USAGE_CHANGE_POLL_SECS: u64 = 2;
        let usage_db_for_watch = nemesis_path::workspace_data_dir(&home).join("nemesisbot_data.db");
        let event_hub_for_usage = web_server.event_hub().clone();
        match nemesis_data::watcher::open_conn(&usage_db_for_watch) {
            Ok(watch_conn) => {
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(
                        USAGE_CHANGE_POLL_SECS,
                    ));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    let mut last = nemesis_data::watcher::data_version(&watch_conn).ok();
                    loop {
                        ticker.tick().await;
                        if event_hub_for_usage.subscriber_count() == 0 {
                            continue;
                        }
                        let Ok(v) = nemesis_data::watcher::data_version(&watch_conn) else {
                            continue;
                        };
                        if last != Some(v) {
                            last = Some(v);
                            event_hub_for_usage.publish(
                                "usage-changed",
                                serde_json::json!({ "ts": chrono::Utc::now().to_rfc3339() }),
                            );
                            tracing::debug!(
                                "[Gateway] usage change detected (data_version={v}) → usage-changed"
                            );
                        }
                    }
                });
                info!("[Gateway] usage change watcher armed (poll={USAGE_CHANGE_POLL_SECS}s)");
            }
            Err(e) => {
                warn!("[Gateway] usage change watcher disabled: {e}");
            }
        }

        // --- A3: 保留策略 sweep（启动时 + 每 6h）---
        // config `usage` 段：retention_days=0 关闭按天清理（明细只增到
        // max_rows 上限为止）；max_rows=0 无上限。此前 rollup 逻辑从未
        // 被生产调用（只有测试），本次一并接上。
        let usage_cfg = cfg.usage.clone().unwrap_or_default();
        let ds_for_sweep = ds.clone();
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(std::time::Duration::from_secs(6 * 3600));
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
            // interval 首个 tick 立即返回 = 启动时先跑一次。
            loop {
                ticker.tick().await;
                let max_rows = (usage_cfg.max_rows > 0).then_some(usage_cfg.max_rows);
                if let Err(e) = ds_for_sweep.retention_sweep(
                    (usage_cfg.retention_days > 0).then_some(usage_cfg.retention_days),
                    max_rows,
                ) {
                    warn!("[Gateway] usage retention sweep failed: {e}");
                }
            }
        });
        info!(
            "[Gateway] usage retention sweep armed (retention_days={}, max_rows={})",
            usage_cfg.retention_days, usage_cfg.max_rows
        );
    }

    // Inject MemoryManager into web server for runtime vector store control
    #[cfg(feature = "memory")]
    {
        if let Some(mgr) = memory_manager_for_web {
            web_server.set_memory_manager(mgr);
            info!("[Gateway] MemoryManager injected into web server");
        }
    }

    // Inject Forge into web server for runtime start/stop control
    #[cfg(feature = "forge")]
    {
        if let Some(forge) = forge_for_web.as_ref() {
            web_server.set_forge(forge.clone());
            info!("[Gateway] Forge instance injected into web server");
        }
    }

    // Inject Cluster into web server for dashboard data queries
    #[cfg(feature = "cluster")]
    {
        if let Some(ref adapter) = cluster_adapter {
            web_server.set_cluster(adapter.cluster().clone());
            info!("[Gateway] Cluster instance injected into web server");

            // Initialize cluster log writer for structured JSONL logging.
            // try_ 幂等变体：同一进程内第二次 run()（in-process 测试复跑网关）
            // 不再 panic "called more than once"；生产单 gateway 每进程只走一次，
            // 行为不变。
            let cluster_log_dir = home.join("workspace/logs/cluster_logs");
            if nemesis_cluster::cluster_log::try_init_cluster_log(&cluster_log_dir) {
                info!(
                    dir = %cluster_log_dir.display(),
                    "[ClusterLog] Initialized"
                );
            } else {
                info!("[ClusterLog] Already initialized in this process — reusing existing writer");
            }

            // Inject cluster lifecycle service for start/stop control
            web_server.set_cluster_service(
                adapter.clone() as Arc<dyn nemesis_services::bot_service::LifecycleService>
            );
            // Inject cluster log directory for JSONL log reader
            web_server.set_cluster_log_dir(cluster_log_dir.to_string_lossy().to_string());
            info!("[Gateway] Cluster service and log dir injected into web server");

            // Phase 4: Bridge cluster log events → SSE EventHub for real-time Dashboard updates.
            // Every cluster log entry (task_submitted, rpc_call, node_online, etc.) is forwarded
            // to connected SSE clients via the EVENT_CLUSTER_EVENT channel.
            let event_hub = web_server.event_hub().clone();
            nemesis_cluster::cluster_log::set_cluster_log_hook(Arc::new(move |event, data| {
                event_hub.publish(
                    nemesis_web::events::EVENT_CLUSTER_EVENT,
                    serde_json::json!({
                        "event": event,
                        "data": data,
                    }),
                );
            }));
            info!("[Gateway] Cluster log → SSE EventHub bridge connected");
        }
    }

    // Inject AgentLoop ref into web server for runtime model switching
    web_server.set_agent_loop(agent_loop_ref.clone());
    info!("[Gateway] AgentLoop ref injected into web server for model switching");

    // Inject WorkflowEngine into web server for /api/workflow/* endpoints
    #[cfg(feature = "workflow")]
    {
        web_server.set_workflow_engine(workflow_engine.clone());
        web_server.set_chat_secret_store(chat_secret_store.clone());
        info!("[Gateway] Workflow engine injected into web server");
    }

    #[cfg(feature = "workflow")]
    {
        // --- Workflow trigger drivers (event + message) ---
        // Two subscription tasks wire trigger configs to their data sources:
        //
        // 1. Inbound bus → message triggers:
        //    Every InboundMessage published by any channel (web, telegram, discord,
        //    etc.) is matched against each workflow's `message` trigger configs
        //    (channel/content/sender_id/chat_id glob match). Matches start a
        //    background execution.
        //
        // 2. EventDispatcher → event triggers:
        //    TriggerEvents (workflow.completed/failed, forge.pattern_created, or
        //    manual fire_event via WSAPI) match each workflow's `event` trigger
        //    configs (event_type glob + data field matchers). Matches start a
        //    background execution.
        //
        // Without these, `message` and `event` triggers never fire — the warning
        // "trigger type X has no runtime driver" no longer applies as of P3.
        let msg_engine = workflow_engine.clone();
        let mut inbound_rx_for_wf = bus.subscribe_inbound();
        let _inbound_wf_handle = tokio::spawn(async move {
            loop {
                match inbound_rx_for_wf.recv().await {
                    Ok(msg) => {
                        let channel = msg.channel.clone();
                        let sender = msg.sender_id.clone();
                        let chat = msg.chat_id.clone();
                        let content = msg.content.clone();
                        let session_key = msg.session_key.clone();
                        let matched = msg_engine
                            .workflows_matching_message(&channel, &sender, &chat, &content);
                        if matched.is_empty() {
                            continue;
                        }
                        for wf_name in matched {
                            let engine = msg_engine.clone();
                            let ch = channel.clone();
                            let sd = sender.clone();
                            let ct = chat.clone();
                            let cn = content.clone();
                            let sk = session_key.clone();
                            tokio::spawn(async move {
                                let trigger = nemesis_workflow::types::TriggerSource::Message {
                                    channel: ch.clone(),
                                    chat_id: ct.clone(),
                                    sender_id: sd.clone(),
                                    content: cn.clone(),
                                };
                                let mut input = std::collections::HashMap::new();
                                input.insert("channel".to_string(), serde_json::json!(ch));
                                input.insert("sender_id".to_string(), serde_json::json!(sd));
                                input.insert("chat_id".to_string(), serde_json::json!(ct));
                                input.insert("content".to_string(), serde_json::json!(cn));
                                // Unified `input` field: the message content is
                                // the natural main input for `message` triggers.
                                input.insert("input".to_string(), serde_json::json!(cn));
                                input.insert("session_key".to_string(), serde_json::json!(sk));
                                match engine.start_async(&wf_name, input, Some(trigger)).await {
                                    Ok(id) => {
                                        info!(
                                            workflow = %wf_name,
                                            execution_id = %id,
                                            channel = %ch,
                                            "[Workflow] message-triggered execution started"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            workflow = %wf_name,
                                            error = %e,
                                            "[Workflow] message-triggered execution failed to start"
                                        );
                                    }
                                }
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            n,
                            "[Workflow] message-trigger subscriber lagged (some triggerable messages dropped)"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("[Workflow] inbound bus closed, message-trigger task exiting");
                        break;
                    }
                }
            }
        });

        let evt_engine = workflow_engine.clone();
        let mut event_rx = evt_engine.event_dispatcher().subscribe();
        let _event_wf_handle = tokio::spawn(async move {
            loop {
                match event_rx.recv().await {
                    Ok(event) => {
                        let matched = evt_engine.workflows_matching_event(&event);
                        if matched.is_empty() {
                            continue;
                        }
                        for wf_name in matched {
                            let engine = evt_engine.clone();
                            let ev = event.clone();
                            tokio::spawn(async move {
                                let trigger = nemesis_workflow::types::TriggerSource::Event {
                                    event_type: ev.event_type.clone(),
                                    data: serde_json::Value::Object(
                                        ev.data.clone().into_iter().collect(),
                                    ),
                                };
                                let mut input = std::collections::HashMap::new();
                                input.insert(
                                    "event_type".to_string(),
                                    serde_json::json!(ev.event_type),
                                );
                                for (k, v) in &ev.data {
                                    input.insert(k.clone(), v.clone());
                                }
                                // Unified `input` field: prefer `ev.data.input`
                                // if present (caller can set it explicitly);
                                // otherwise JSON-serialise the whole data object.
                                if !input.contains_key("input") {
                                    let serialized = serde_json::Value::Object(
                                        ev.data.clone().into_iter().collect(),
                                    )
                                    .to_string();
                                    input
                                        .insert("input".to_string(), serde_json::json!(serialized));
                                }
                                if let Some(src) = &ev.source_execution_id {
                                    input.insert(
                                        "source_execution_id".to_string(),
                                        serde_json::json!(src),
                                    );
                                }
                                match engine.start_async(&wf_name, input, Some(trigger)).await {
                                    Ok(id) => {
                                        info!(
                                            workflow = %wf_name,
                                            execution_id = %id,
                                            event_type = %ev.event_type,
                                            "[Workflow] event-triggered execution started"
                                        );
                                    }
                                    Err(e) => {
                                        warn!(
                                            workflow = %wf_name,
                                            event_type = %ev.event_type,
                                            error = %e,
                                            "[Workflow] event-triggered execution failed to start"
                                        );
                                    }
                                }
                            });
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        warn!(
                            n,
                            "[Workflow] event-trigger subscriber lagged (some triggerable events dropped)"
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        info!("[Workflow] event dispatcher closed, event-trigger task exiting");
                        break;
                    }
                }
            }
        });

        info!("[Gateway] Workflow trigger drivers spawned (event + message)");

        // Register the WorkflowChatReplyObserver so `/workflow/chat/<index>`
        // pages get a reply broadcast when their execution finishes. The observer
        // filters by `TriggerSource::WorkflowChat` so non-chat executions are
        // ignored. Per-workflow serialization guards are released here too.
        {
            let observer = Arc::new(
                nemesis_web::workflow_chat_reply_observer::WorkflowChatReplyObserver::new(
                    web_server.session_manager().clone(),
                    workflow_engine.clone(),
                ),
            );
            workflow_engine
                .event_manager()
                .register(observer as Arc<dyn nemesis_workflow::events::WorkflowObserver>)
                .await;
            info!("[Gateway] WorkflowChatReplyObserver registered");
        }
    }

    info!("[Gateway] Web server components injected");

    Ok(PostAgentWiring {
        agent_adapter,
        projects_manager,
        #[cfg(feature = "cluster")]
        cluster_adapter,
        #[cfg(feature = "cluster")]
        cluster_arc_ref,
    })
}
