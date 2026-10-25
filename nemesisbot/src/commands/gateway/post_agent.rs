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
use tracing::{error, info, warn};

use super::{AgentWiring, ClusterWiring, GatewayCtx};
use crate::adapters;

/// P3 T8：roster 条目 `state` 判定（纯函数，单测钉死）。常驻执行体存活
/// → `running`；否则回记录生命周期态（active/provisioning/failed）。
/// 词汇表契约见 `nemesis_cluster::types::AgentAnnouncement::state`。
/// 唯一生产消费者在 cluster 装配块内；tests 直引，故 allow 不迁 cfg。
#[cfg_attr(not(feature = "cluster"), allow(dead_code))]
pub(crate) fn roster_state(
    alive_ids: &std::collections::HashSet<String>,
    agent_id: &str,
    record_status: &str,
) -> String {
    if alive_ids.contains(agent_id) {
        "running".to_string()
    } else {
        record_status.to_string()
    }
}

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
#[cfg_attr(not(feature = "cluster"), allow(unused_mut, unused_variables))]
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
                // M1（2026-10-10 机制加固）：锚点根单一真相源——经
                // workspace_dir() 解析（显式覆盖跟随，canonical 回退），
                // 禁止从 home 手拼（A10：部署形态漂移 → 九锚点全 os error 2 假红）。
                workspace: shared_resources.workspace_dir(),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
                // P4-d：评审结论→交付账本闭环的审计插件槽。
                #[cfg(feature = "security")]
                security_slot: Some(ctx.outbound_dlp_slot.clone()),
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
                // M1：同上——锚点根单一真相源，禁止 home 手拼。
                workspace: shared_resources.workspace_dir(),
                home: home.clone(),
                moderator_loop: board_moderator_loop.clone(),
                cluster: cluster.clone(),
                estop: shared_resources.estop.clone(),
                estop_parked: board_estop_parked.clone(),
                selfcheck: board_selfcheck_registry.clone(),
                // P4-d：同上——项目收口钩子的审计插件槽。
                #[cfg(feature = "security")]
                security_slot: Some(ctx.outbound_dlp_slot.clone()),
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
                    // 任务档案自动导出（2026-09-29 goal）：人工收口路径同源
                    // 触发（自动 PASS 路径在 apply_project_review_outcome）。
                    crate::board_review::spawn_project_dossier((*sum_deps).clone(), project_id);
                }),
            ) {
                warn!("[Gateway] Board project summary hook register failed: {e}");
            } else {
                info!("[Gateway] Board project summary hook armed (archive summary.md)");
            }

            // M5-1（2026-10-10 机制加固）：评审重触发钩子——WSAPI
            // `review.retrigger` 经此回调 spawn 评审（卡 in_review 单子的
            // 人工一键重入）。master 判定同父单钩子（非 master 的 board
            // store 是只读镜像，不跑评审）。
            let retrig_deps = replay_sweep_deps.clone();
            let retrig_cluster = cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_review_retrigger_hook(
                std::sync::Arc::new(move |issue_id: i64| {
                    if !matches!(
                        retrig_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    ) {
                        return;
                    }
                    crate::board_review::spawn_board_review((*retrig_deps).clone(), issue_id);
                }),
            ) {
                warn!("[Gateway] Board review retrigger hook register failed: {e}");
            } else {
                info!("[Gateway] Board review retrigger hook armed (review.retrigger)");
            }

            // 项目收口复评钩子（2026-10-17 NB-101 project_escalate_human 死路
            // 根修）：WSAPI `review.retrigger {project_id}` 的人工一键重入——
            // 项目升级记入 auto_decide 后重放扫描视为已结论，父单又已 done，
            // 没有此钩子升级就是死局。
            let proj_retrig_deps = replay_sweep_deps.clone();
            let proj_retrig_cluster = cluster.clone();
            if let Err(e) = nemesis_web::handlers::board::set_project_review_retrigger_hook(
                std::sync::Arc::new(move |project_id: i64| {
                    if !matches!(
                        proj_retrig_cluster.role().as_str(),
                        "coordinator" | "master" | "manager"
                    ) {
                        return;
                    }
                    crate::board_review::spawn_project_review(
                        (*proj_retrig_deps).clone(),
                        project_id,
                    );
                }),
            ) {
                warn!("[Gateway] Board project review retrigger hook register failed: {e}");
            } else {
                info!(
                    "[Gateway] Board project review retrigger hook armed (review.retrigger project_id)"
                );
            }

            // 启动重放（R4-BUG-2 根修）：评审 spawn 纯内存、master 重启即
            // 丢——in_review 单据/父单/项目在重启后由这里扫描重触发。必须
            // 在三类钩子注册完成后调用（重放验收 PASS 会级联点火上层钩子）。
            crate::board_review::replay_stuck_reviews(&replay_sweep_deps, &[]);
            info!("[Gateway] Board stuck-review replay sweep done");

            // N1（§1.3 无人值守闭环）：拆解中断恢复扫描——RUNNING_PLANS 内存
            // 态、重启杀拆解任务，backlog/todo +「已开始自动拆解」评论态的
            // 单子自动重新发起拆解（开关 board.plan.restart_recover 默认开；
            // estop 挂起跳过；worker 镜像不跑）。评审重放同阶段、moderator
            // 槽已装配（:119 set）。
            if let Some(agent_loop) = replay_sweep_deps.moderator_loop.get().cloned() {
                let recovered = nemesis_web::handlers::board::recover_interrupted_plans(
                    &replay_sweep_deps.store,
                    Some(cluster.clone()),
                    agent_loop,
                    Some(home.clone()),
                    Some(web_server.event_hub().clone()),
                    Some(replay_sweep_deps.estop.as_ref()),
                );
                if recovered > 0 {
                    info!(
                        "[Gateway] Board plan recovery: {recovered} interrupted plan(s) re-armed"
                    );
                }
            } else {
                warn!("[Gateway] Board plan recovery skipped: moderator loop 未装配");
            }

            // U-B（§9.2 无人值守）：LLM 端点不可用恢复探针——封存期周期
            // 最小 completion 探测（TCP 通 ≠ LLM 活），解封边沿读中断档案
            // 逐单续做（复评/续拆/项目收口）。estop 挂起时不探测（优先级
            // 更高）；probe_interval_secs=0 只封存不探测。全部节点装配，
            // loop 内角色闸（worker 只记账不续做）。
            crate::board_unavailable::spawn_unavailable_probe(
                crate::board_unavailable::UnavailableProbeDeps {
                    review: (*replay_sweep_deps).clone(),
                    home: home.clone(),
                    hub: Some(web_server.event_hub().clone()),
                },
            );
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

    // --- P2（2026-10-16）：Agent 持久实体 seed（tasks.db agents 表）---
    // main 身份投影（幂等）+ 项目 agent 镜像 + 半创建 sweep 收敛。三类
    // 失败都 warn 降级不炸 gateway（agents 记录是增强面，不是启动硬依赖；
    // tasks.db 打不开 = agents.* WSAPI 诚实报错）。装进进程槽供 WSAPI 与
    // 集群节点宣告读取。与 P1 审批台账各持独立连接（多连接 WAL 是既定
    // 形态，TaskDb::open 自带并发首开纪律）。
    {
        match nemesis_taskstore::TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
            &shared_resources.workspace_dir(),
        )) {
            Ok(db) => {
                // P4-c：交付账本 bridge 用独立连接（db 本体随 mgr move；
                // 多连接 WAL 是既定形态）。G1：agent loop 管理器同款第三
                // 连接（store 是真相源句柄，不持 manager Arc——无引用环）。
                let deliveries_db = std::sync::Arc::clone(&db);
                let loops_db = std::sync::Arc::clone(&db);
                let mgr = std::sync::Arc::new(crate::agents::AgentManager::from_db(
                    shared_resources.workspace_dir(),
                    db,
                ));
                if let Err(e) = mgr.ensure_main_agent() {
                    warn!("[Gateway] main agent seed 失败（对话组身份投影缺位）: {e}");
                }
                let entries: Vec<(String, String, std::path::PathBuf)> =
                    crate::projects::registry::list_projects(projects_manager.registry_path())
                        .into_iter()
                        .map(|p| (p.id, p.name, p.path))
                        .collect();
                match mgr.sync_project_agents(&entries) {
                    Ok(created) if !created.is_empty() => {
                        info!("[Gateway] 项目 agent 镜像同步: +{} 条", created.len());
                    }
                    Ok(_) => {}
                    Err(e) => warn!("[Gateway] 项目 agent 镜像同步失败: {e}"),
                }
                match mgr.sweep() {
                    Ok(report) if !report.activated.is_empty() || !report.failed.is_empty() => {
                        info!(
                            "[Gateway] agents sweep: 续建 {} 条, 失败 {} 条",
                            report.activated.len(),
                            report.failed.len()
                        );
                    }
                    Ok(_) => {}
                    Err(e) => warn!("[Gateway] agents sweep 失败: {e}"),
                }
                crate::agents::install_manager(std::sync::Arc::clone(&mgr));
                // P6-R2：迁移传输层注入（cluster Arc 已由 init_cluster 回填
                // bridge_cluster_slot——A 端编排 + B 端 commit 的传输面）。
                #[cfg(feature = "cluster")]
                if let Some(cluster) = cluster.bridge_cluster_slot.get() {
                    mgr.set_migrate_transport(std::sync::Arc::new(
                        crate::agents::migrate::ClusterMigrateTransport {
                            cluster: std::sync::Arc::clone(cluster),
                        },
                    ));
                }
                // T9：常驻监督器注入 manager（agents.sleep/wake/slots 生命
                // 周期面经 bridge 走到 supervisor；supervisor 构造在
                // agent_build（早于本块），此刻恒在场）。
                // R4：注册表存在性检查闭包同点注入——看护循环对已删 agent
                // 诚实停手（崩溃槽不重拉），删除联动常驻进程生命周期。
                if let Some(rt) = shared_resources.agent_runtime.as_ref() {
                    let reg_mgr = std::sync::Arc::clone(&mgr);
                    rt.set_registration_check(std::sync::Arc::new(move |agent_id| {
                        reg_mgr.get(agent_id).map(|r| r.is_some()).unwrap_or(false)
                    }));
                    mgr.set_runtime(std::sync::Arc::clone(rt));
                }
                // P5 第 3 步：VM 监督器装配（vm feature 闸；`executor.vm`
                // 总闸在 supervisor 内 config 现读——关 = 全操作诚实不可用，
                // 这里装配的只是纪律面本身，不激活任何 VM 路径）。装配 =
                // 注入 manager + 启动孤儿 sweep（清理不收编）+ watchdog
                // （死槽收敛/周期 sweep/预算告警/idle 自动睡眠）。
                #[cfg(feature = "vm")]
                {
                    let vm_config_path = crate::common::config_path(&shared_resources.home);
                    // 载体缺席 loud warn：VM 工具路径骑 RemoteExecutorTool 桥
                    // （executor.enabled=true 才装配）。vm 总闸开而载体缺席 =
                    // VM 档不可达（管理面/切档可用，工具执行诚实 Unavailable
                    // 回落 per-call）——装配期提醒，不静默。
                    let exec_cfg = std::fs::read_to_string(&vm_config_path)
                        .ok()
                        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
                        .and_then(|v| v.get("executor").cloned());
                    let vm_on = exec_cfg
                        .as_ref()
                        .and_then(|e| e.get("vm"))
                        .and_then(|v| v.get("enabled"))
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    let carrier_on = exec_cfg
                        .as_ref()
                        .and_then(|e| e.get("enabled"))
                        .and_then(|b| b.as_bool())
                        .unwrap_or(false);
                    if vm_on && !carrier_on {
                        warn!(
                            "[Gateway] executor.vm.enabled=true 但 executor.enabled=false：RemoteExecutorTool 桥未装配，VM 档工具执行不可达（调用将诚实 Unavailable 回落 per-call）；如需 VM 执行请同时开启 executor.enabled"
                        );
                    }
                    let vm_sup = std::sync::Arc::new(crate::agents::vm_runtime::VmSupervisor::new(
                        shared_resources.workspace_dir(),
                        // 主配置现行布局在 home 根（`<home>/config.json`，
                        // common::config_path 同源）——不走
                        // resolve_config_path_in_workspace（该函数锚
                        // workspace 根，与现行布局不符且无其他生产调用方）。
                        vm_config_path,
                    ));
                    // 试玩 try.start 后台启动任务的 owned 句柄（发起即返回
                    // 语义依赖此注入；缺席 = 同步回落长请求）。
                    vm_sup.set_self_arc();
                    // P5 第 4 步：VM 后端注入 manager（档位路由执行器每调用
                    // 经 manager 现取——晚注入纪律：loop 构建晚于本块，路由
                    // 侧无需感知装配时序）。
                    mgr.set_vm_backend(vm_sup.clone());
                    // B9（VM 项目工作副本）：concrete supervisor 槽并行注入
                    // ——vm_sync 需要 internal_call（guest 内部方法通道，
                    // trait 对象 VmBackend 上没有），manager 里与 trait 槽
                    // 各持一份 Arc，同一实例。
                    mgr.set_vm_supervisor(vm_sup.clone());
                    // P5 第 4 步：vm.* WSAPI 接线（web 侧 trait + 槽，
                    // 实现落本 crate——依赖方向 nemesisbot→nemesis-web，
                    // agents bridge 同款）。manager 不持 VM 引用——真注入
                    // 面就是这里的桥槽，直接从本地 Arc 装。
                    let vm_bridge: std::sync::Arc<dyn nemesis_web::handlers::vm::VmBridge> =
                        vm_sup.clone();
                    nemesis_web::handlers::vm::install_vm_bridge(vm_bridge);
                    let sup_for_sweep = std::sync::Arc::clone(&vm_sup);
                    tokio::spawn(async move {
                        let swept = sup_for_sweep.sweep_orphans().await;
                        if swept > 0 {
                            warn!("[Gateway] VM 孤儿 sweep: 清理 {swept} 个孤儿 QEMU 进程");
                        }
                    });
                    vm_sup.start_watchdog();
                    // P5-R2 ④：plugin-vm 宿主装载（vm-display feature 门控，
                    // implies vm）。缺席 = 常态静默（display=vnc 时 shim
                    // 端诚实 501）；在场但被拒（签名/ABI）= loud warn 留痕。
                    // 2026-10-23 花名册补正：装载前过统一开关闸
                    // （config.plugin.native.json，plugin_vm 条目）——开关
                    // 缺省 = 启用（与四插件同款「有签名自动装载」），关掉
                    // = 用户主动禁用；enforce 签名策略与四插件同谱。
                    #[cfg(feature = "vm-display")]
                    {
                        let vm_switch_dir = nemesis_path::workspace_config_dir(
                            &crate::common::workspace_path(&shared_resources.home),
                        );
                        match crate::native_plugins::gate_load(
                            &vm_switch_dir,
                            nemesis_config::plugins_native::NativePluginId::PluginVm,
                        ) {
                            crate::native_plugins::GateOutcome::DisabledBySwitch => {
                                info!(
                                    "[Gateway] plugin-vm 显示插件开关关闭（config.plugin.native.json），跳过装载"
                                );
                            }
                            crate::native_plugins::GateOutcome::BlockedBySignature(reason) => {
                                warn!("[Gateway] plugin-vm 显示插件签名策略拦截：{reason}");
                            }
                            crate::native_plugins::GateOutcome::Allow { .. } => {
                                match crate::vm_display_loader::load_plugin_vm(
                                    std::sync::Arc::clone(&vm_sup),
                                    &shared_resources.workspace_dir(),
                                ) {
                                    crate::vm_display_loader::LoadOutcome::Absent => {}
                                    crate::vm_display_loader::LoadOutcome::Loaded {
                                        listen_port,
                                    } => {
                                        info!(
                                            "[Gateway] plugin-vm 显示插件装载成功：127.0.0.1:{listen_port}"
                                        );
                                    }
                                    crate::vm_display_loader::LoadOutcome::Rejected(reason) => {
                                        warn!("[Gateway] plugin-vm 装载被拒：{reason}");
                                    }
                                }
                            }
                        }
                    }
                    // estop 断连挂点：急停触发 → 全部显示会话立即断连
                    // （与 PTY estop kill 同款纪律；watcher 断 = gateway
                    // 停机 → 插件 display_free 收尾——幂等，缺席 no-op）。
                    #[cfg(feature = "vm-display")]
                    {
                        let estop = shared_resources.estop.clone();
                        tokio::spawn(async move {
                            let mut rx = estop.subscribe();
                            while rx.changed().await.is_ok() {
                                if *rx.borrow() {
                                    nemesis_web::vm_display::disconnect_all_for_estop();
                                }
                            }
                            // estop watch sender 全 drop = gateway 停机路径。
                            crate::vm_display_loader::shutdown_plugin_vm();
                        });
                    }
                }
                // WSAPI agents.* 同点接线（同一 Manager 实例；web 侧 trait
                // + 槽，实现落本 crate——依赖方向 nemesisbot→nemesis-web）。
                // G1：先注入 loop manager 再装 bridge（bridge 的
                // loop_for_agent/send_to_agent 委托它；与项目 manager 同款
                // 共享主 loop 的 SessionStore Arc，会话隔离靠 session_key）。
                let loops_mgr =
                    std::sync::Arc::new(crate::agents::loop_manager::AgentLoopManager::new(
                        std::sync::Arc::clone(&shared_resources),
                        loops_db,
                        agent_loop
                            .session_store()
                            .cloned()
                            .expect("main agent loop must carry a session store"),
                    ));
                mgr.set_loop_manager(loops_mgr);
                let bridge: std::sync::Arc<dyn nemesis_web::handlers::agents::AgentsBridge> =
                    mgr.clone();
                nemesis_web::handlers::agents::install_agents_bridge(bridge);
                // P4-c（2026-10-17）：交付账本 WSAPI 同点接线（deliveries.*
                // 查询/裁决面；审计插件槽惰性取法同 cluster_init 回闭——
                // 此刻槽还空着，回调执行时 get() 解析）。
                #[cfg(feature = "security")]
                crate::delivery_ledger::DeliveryLedgerBridge::install(
                    std::sync::Arc::clone(&deliveries_db),
                    Some(ctx.outbound_dlp_slot.clone()),
                );
                #[cfg(not(feature = "security"))]
                crate::delivery_ledger::DeliveryLedgerBridge::install(std::sync::Arc::clone(
                    &deliveries_db,
                ));
                // 项 C（2026-10-22）：通知账本 db 同点注入（sweeper 每 tick
                // 惰性读槽；钩子记账复用同一 Arc<TaskDb>）。
                crate::notify::install_notify_db(std::sync::Arc::clone(&deliveries_db));
                // P3 外循环（PR 发布，2026-10-25）：发布桥同点接线（交付
                // 账本查询面同款依赖反转；缺 board/security/connectors 任一
                // feature = 发布面不装配，WSAPI delivery.publish 诚实报
                // 「桥未装配」）。external_source::spawn 同款三 feature 闸。
                #[cfg(all(feature = "board", feature = "security", feature = "connectors"))]
                crate::publish::install(
                    std::sync::Arc::clone(&deliveries_db),
                    board_store
                        .clone()
                        .expect("board feature 已启用，board_store 必在"),
                    std::sync::Arc::clone(&agent_loop),
                    home.join("workspace"),
                );
                // 节点宣告带 agent 清单（P2）：provider 闭包每次现读实体
                // 注册表（UDP announce / TCP 探针回退 / get_info 三路同源），
                // 运行期建删 agent 无需回写或重启。cluster 关 = 不装（roster
                // 缺位与 professions 同语义：对端看到空清单=未宣告）。
                // P3 T8：state 现读常驻监督器槽位——存活 → `running`，否则
                // 回记录生命周期态（active/provisioning/failed）。闭包捕获
                // supervisor Arc（manager 级单例，与 SharedResources 同寿）。
                #[cfg(feature = "cluster")]
                if let Some(cluster) = cluster_arc_ref.as_ref() {
                    let mgr_for_roster = mgr.clone();
                    let runtime_for_roster = shared_resources.agent_runtime.clone();
                    cluster.set_agent_roster_provider(Some(std::sync::Arc::new(move || {
                        let running: std::collections::HashSet<String> = runtime_for_roster
                            .as_ref()
                            .map(|rt| {
                                rt.slot_states()
                                    .into_iter()
                                    .filter(|s| s.alive)
                                    .map(|s| s.agent_id)
                                    .collect()
                            })
                            .unwrap_or_default();
                        mgr_for_roster
                            .list()
                            .unwrap_or_default()
                            .into_iter()
                            .map(|r| nemesis_cluster::types::AgentAnnouncement {
                                state: roster_state(&running, &r.agent_id, &r.status),
                                agent_id: r.agent_id,
                                name: r.name,
                                kind: r.kind,
                                tier: r.tier,
                            })
                            .collect()
                    })));
                }
                info!("[Gateway] agent 实体注册表 armed (tasks.db agents 表)");
            }
            Err(e) => {
                error!(
                    "[Gateway] tasks.db unavailable, agent 实体注册表关闭（agents.* WSAPI 将诚实报错）: {e}"
                );
            }
        }
    }

    // --- G3（2026-10-18）：运行账本漏斗安装（runs/attempts 进生产调度）---
    // 第五条独立连接（多连接 WAL 是既定形态）。装上后 board 派发 / cron
    // 触发 / workflow 执行经 nemesis-taskstore::ledger 记账（集群任务的
    // runs 行由 SqliteTaskStore 适配器既有落盘，账本只补 attempts 归因）。
    // 装失败 = 账本关闭（生产调度照常，仅丢记账），warn 留痕不炸启动。
    // 顺带启动 sweep：上一进程遗留的未收口 cron 触发诚实标
    // failed("gateway restart")（board/cluster/workflow 在途 run 由各自
    // 生产收口路径负责，重启不等于死亡——见 ledger 模块头「诚实边界」）。
    {
        match nemesis_taskstore::TaskDb::open(&nemesis_path::resolve_tasks_db_path_in_workspace(
            &shared_resources.workspace_dir(),
        )) {
            Ok(db) => {
                if let Err(e) = nemesis_taskstore::ledger::install(db) {
                    warn!("[Gateway] run ledger 重复安装（保持既有实例）: {e}");
                } else {
                    let swept = nemesis_taskstore::ledger::sweep_restart_orphans_installed();
                    if swept > 0 {
                        info!("[Gateway] run ledger 启动 sweep: 归位 {swept} 条重启孤儿");
                    }
                    info!("[Gateway] run ledger armed (tasks.db; board/cron/workflow 记账)");
                }
            }
            Err(e) => {
                warn!("[Gateway] tasks.db unavailable, run ledger 关闭（调度照常，仅丢记账）: {e}");
            }
        }
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
            stream_mode: resolution.stream_mode.clone(),
            cache_breakpoints: resolution.cache_breakpoints,
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
    // 只有 agent loop。board 关 = handlers::board 随 nemesis-board 退出编译
    // （desktop 形态 board 关 cluster 开：闸必须含 board，E0433 实录）。
    #[cfg(feature = "board")]
    {
        if nemesis_web::handlers::board::install_board_estop(shared_resources.estop.clone()) {
            info!("[Gateway] E-stop gate installed for board dispatch family");
        }
    }

    // U-A（2026-10-12 停车现场工程）：派发/拆解侧停车档案的工作区根注入
    // （<workspace>/board/handoffs/；模块级 OnceLock，单测不装配零波及）。
    // board 关 = handlers::board 随 nemesis-board 退出编译，注入跳过。
    #[cfg(feature = "board")]
    if nemesis_web::handlers::board::install_handoff_workspace(home.join("workspace")) {
        info!("[Gateway] Board handoff workspace root installed");
    }

    // P1 外循环（外部来源绑定，2026-10-25）：Gitea issue 轮询导入 + 状态
    // 回写 + 任务副本物化。config `board.external_sources` 空 = spawn 内部
    // 短路（零开销，worker 节点天然不装）。派发面用 cluster_arc_ref（refs
    // take 后仍存活的通用引用，board_discussion_cluster 同款纪律）；无
    // cluster 编译/未启用集群 = 导入照常，自动派发诚实停车（⏸ 评论）。
    // 三 feature 同闸：board（store 面）+ security（vault 凭据）+
    // connectors（connector_invoke 通路）。
    #[cfg(all(feature = "board", feature = "security", feature = "connectors"))]
    {
        crate::external_source::spawn(
            agent_loop.clone(),
            board_store
                .clone()
                .expect("board feature 已启用，board_store 必在"),
            home.join("workspace"),
            home.clone(),
            shared_resources.estop.clone(),
            #[cfg(feature = "cluster")]
            cluster_arc_ref.clone(),
            #[cfg(not(feature = "cluster"))]
            (),
        );
    }

    // C5: inject the shared LSP manager (same Arc the LspTool registered
    // with) — Phase-2 diagnostics loop and future dashboard LSP ops read it.
    #[cfg(feature = "lsp")]
    {
        web_server.set_lsp_manager(shared_resources.lsp_manager.clone());
        info!("[Gateway] LSP manager injected into web server");
    }

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
                        now_ms: Arc::new(|| chrono::Utc::now().timestamp_millis()),
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
        // usage-store 关：watcher 模块随 SQLite 整体退出编译（无账可侦）。
        #[cfg(feature = "usage-store")]
        {
            const USAGE_CHANGE_POLL_SECS: u64 = 2;
            let usage_db_for_watch =
                nemesis_path::workspace_data_dir(&home).join("nemesisbot_data.db");
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

    // QUIC 中继插件句柄注入 web 面（QUIC 插件化 M1）：WSAPI quic.relay.*/
    // registry.members 经同一句柄代理插件调用（2026-10-23 解耦——与集群
    // 注入互不依赖，集群未装配中继面照常可用）。句柄是 fn 指针 Copy——
    // 直接解引用值拷贝。插件未装载（文件缺席/拒载）= web 面保持 None
    // （handler 诚实降级 not loaded，不注入假句柄）。
    if let Some(handle) = crate::roaming_plugin_loader::plugin_handle() {
        web_server.set_roaming_plugin(*handle);
        info!("[Gateway] plugin_roaming handle injected into web server");
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
        let bus_for_wf_lag = std::sync::Arc::clone(&bus);
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
                        // P2-1：接收侧真实丢失计入 bus 指标。
                        bus_for_wf_lag.record_inbound_lagged(n);
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
