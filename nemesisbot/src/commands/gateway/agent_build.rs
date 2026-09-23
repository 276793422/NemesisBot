// ---------------------------------------------------------------------------
// PB-4 Agent 构建（计划 §4.2 B4）：Step 9b 安全插件 + Step 9d 观察者 +
// LspManager/事件通道/后台进程注册表 + SharedResources + build_agent_loop +
// workflow 工具桥/agent runner/usage store 注入（计划口径原 4418–4623）整体
// 迁入 `init_agent`。原文自 run() 逐字迁入，仅三处机械改写：fn 体包裹、
// ctx/web/cluster 依赖项影子重绑、末尾 AgentWiring 构造。
//
// 签名取 `(&GatewayCtx, &WebWiring, &ClusterWiring)`：全引用——cluster 结构
// 体须完整存活到 B5（init_post_agent 按值消费 refs 四元组 take + worker
// inbox take，结构体 Clone 不成立），本块对 trio 取 .clone()（Arc 家族，
// 廉价）、refs/web_server 取共享借用（body 仅 as_ref 读 + event_hub 读）。
// 相应 run() 侧演进（B4 头注记）：B2 的 trio 影子删除（SharedResources 在
// 本函数内取自 cluster 字段克隆）；cluster_adapter_refs / bridge_cluster_
// slot / board_worker_inbox / board_estop_parked / board_selfcheck_registry
// 影子转 .clone()（部分移动后结构体不可整体再借——本函数借读全结构体）；
// web_server 移动推迟到本调用之后，其余 web 影子转 clone（同因）。
//
// AgentWiring 字段 = 实测逃逸本块的完整集合（五项）：shared_resources /
// agent_loop（PB-5–PB-7 重度消费）、agent_event_rx（PB-6 set_agent_event_rx）、
// security_plugin（PB-6 审批装配 + Step 22 scanner 停机，两处均
// cfg(security) 门内）、initial_tool_count（PB-5 能力注入日志 + Step 15
// 启动信息）。observer_manager 实测消费不出块（仅 SharedResources 引用）
// 留内。security_plugin 字段随 build_security_plugin 的 cfg 双形态
// （security 真身 / 无 security 空桩 Option<()>）双臂声明。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use tracing::info;

#[cfg(feature = "workflow")]
use super::GatewayAgentRunner;
use super::{ClusterWiring, GatewayCtx, WebWiring};
use crate::common;

/// PB-4 产物 struct（§4.2 B4）。
pub(crate) struct AgentWiring {
    pub shared_resources: Arc<crate::agent_factory::SharedResources>,
    pub agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    pub agent_event_rx: tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
    #[cfg(feature = "security")]
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    pub security_plugin: Option<()>,
    pub initial_tool_count: usize,
}

/// Step 9b/9d + SharedResources + AgentLoop 构建（计划 §4.2 B4）。
pub(crate) async fn init_agent(
    ctx: &GatewayCtx,
    web: &WebWiring,
    cluster: &ClusterWiring,
) -> Result<AgentWiring> {
    // ctx 影子重绑（B1 同款）：门控取各名字在本函数内的实际消费门。
    let home = ctx.home.clone();
    let cfg = ctx.cfg.clone();
    let bus = ctx.bus.clone();
    let agent_outbound_tx = ctx.agent_outbound_tx.clone();
    let cron_service = ctx.cron_service.clone();
    let estop = ctx.estop.clone();
    let data_store = ctx.data_store.clone();
    let config_store = ctx.config_store.clone();
    let mcp_enabled = ctx.mcp_enabled;
    let skills_loader_arc = ctx.skills_loader_arc.clone();
    let skills_registry_arc = ctx.skills_registry_arc.clone();
    #[cfg(feature = "forge")]
    let forge_for_web = ctx.forge_for_web.clone();
    #[cfg(feature = "forge")]
    let forge_executor_for_tools = ctx.forge_executor_for_tools.clone();
    #[cfg(feature = "memory")]
    let memory_manager_for_web = ctx.memory_manager_for_web.clone();
    #[cfg(feature = "workflow")]
    let workflow_engine = ctx.workflow_engine.clone();
    #[cfg(feature = "workflow")]
    let workflow_tool_registry = ctx.workflow_tool_registry.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_store = ctx.board_store.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_moderator_loop = ctx.board_moderator_loop.clone();
    // web wiring 影子：event_hub 读借用（门=唯一消费点 board_event_hub）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    let web_server = &web.web_server;
    let enabled_channels = web.enabled_channels.clone();
    // cluster wiring 影子：trio 克隆（参数按引用，Arc 家族廉价）——原文
    // 以移动填 SharedResources，拓扑等价（原体此后无 trio 消费）；refs
    // 借读（body 仅 as_ref 取 Arc 副本填 board_cluster）。
    let cluster_rpc_call_fn = cluster.cluster_rpc_call_fn.clone();
    let cluster_rpc_config = cluster.cluster_rpc_config.clone();
    let cluster_peers_fn = cluster.cluster_peers_fn.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let cluster_adapter_refs = &cluster.cluster_adapter_refs;

    // Step 9b: Create and inject SecurityPlugin if enabled.
    // Mirrors Go's SecurityPlugin registered via PluginManager in instance.go.
    // Keep a reference to the auditor so we can wire up the approval manager later.
    // K1（devtool-upgrade 阶段 4）：装配逻辑原样迁往 `crate::security_setup`
    // （layer 开关 + DLP + 规则 + 审计日志 + scanner 链）——headless `run`
    // 与 gateway 共用同一构造，安全 9 层在无端口形态不降级。
    let security_plugin = crate::security_setup::build_security_plugin(
        &home,
        cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true),
    )
    .await;

    // Step 9d: Setup Observer Manager for conversation lifecycle events.
    // Mirrors Go's bot_service.go Phase 5: observerMgr creation + RequestLogger registration.
    let observer_manager: Option<Arc<nemesis_observer::Manager>> = {
        let observer_mgr = Arc::new(nemesis_observer::Manager::new());

        // Register RequestLogger as Observer (if logging.llm.enabled)
        // （ASM-05：配置→LoggingConfig 映射 + 注册收敛到 agent_factory 单一
        // 真相源，与 CLI `nemesisbot agent` 共用）。
        if crate::agent_factory::register_request_logger_observer(&observer_mgr, &cfg, &home) {
            info!("[Gateway] RequestLoggerObserver registered (logging.llm.enabled = true)");
        }

        // Check if any observers were registered.
        let mgr_check = observer_mgr.clone();
        let has_observers = tokio::task::block_in_place(|| {
            tokio::runtime::Handle::current().block_on(async { mgr_check.has_observers().await })
        });
        if has_observers {
            info!("[Gateway] Observer manager initialized (injection handled by factory)");
            Some(observer_mgr)
        } else {
            None
        }
    };

    // Note: DataStore injection into agent_loop is now handled by the factory function.

    // Note: Forge injection into agent_loop is now handled at creation time above.

    // Build SharedResources and use the factory to create the AgentLoop.
    // （estop 句柄已在集群装配块前创建——board 评审依赖集共用同一 Arc。）
    // C5 (2026-09-04): ONE LspManager for the whole gateway — the LspTool
    // registers with it (via SharedToolConfig.lsp_manager), the web server
    // holds the same Arc, and Step-24 teardown calls shutdown_all() so
    // language-server child processes don't outlive the gateway.
    let lsp_manager = std::sync::Arc::new(nemesis_lsp::LspManager::new(
        cfg.agents
            .lsp_tool
            .timeout_secs
            .map(std::time::Duration::from_secs),
        cfg.agents
            .lsp_tool
            .idle_secs
            .map(std::time::Duration::from_secs),
    ));
    // C6（devtool-upgrade 阶段 6）：agents.lsp_tool.auto_install=true → 网关
    // 启动期静默自举缺失的语言服务器（官方安装通道白名单目录，非交互命令
    // 串行后台执行，结果进日志）。信任级=用户显式配置的 standing consent
    // （同 A6 formatter / LSP spawn——基础设施装配不走 8 层管线；dashboard
    // 一键安装按钮那条路才走）。装完需重启 Agent 重新探测注册。
    if cfg.agents.lsp_tool.auto_install {
        tokio::spawn(nemesis_lsp::install::auto_install_missing(
            std::time::Duration::from_secs(600),
        ));
    }
    // M1a (2026-09-05): ONE tool-event broadcast channel for the gateway —
    // sender side goes into SharedResources (each AgentLoop gets a
    // ToolEventHook), receiving side goes to the web server (pump routes
    // events to Dashboard WS push + EventHub).
    let (agent_event_tx, agent_event_rx) =
        tokio::sync::broadcast::channel::<nemesis_types::agent::AgentEvent>(256);
    // B4 (2026-09-05): gateway-level background-process registry singleton —
    // construction is process-free (children spawn lazily via
    // background_start); Drop raises kill flags so no spawned job outlives
    // the gateway.
    let background_registry = std::sync::Arc::new(nemesis_agent::BackgroundProcessRegistry::new());
    let shared_resources = crate::agent_factory::SharedResources {
        home: home.clone(),
        // K1：gateway 固定 canonical 布局（显式写出，不依赖 fallback）。
        workspace: home.join("workspace"),
        bus: bus.clone(),
        agent_outbound_tx,
        #[cfg(feature = "forge")]
        forge: forge_for_web.clone(),
        #[cfg(not(feature = "forge"))]
        forge: None,
        #[cfg(feature = "forge")]
        forge_executor: forge_executor_for_tools.clone(),
        #[cfg(not(feature = "forge"))]
        forge_executor: None,
        cron_service: cron_service.clone(),
        security_plugin: security_plugin.clone(),
        observer_manager: observer_manager.clone(),
        data_store: data_store.clone(),
        skills_loader: skills_loader_arc.clone(),
        skills_registry: skills_registry_arc.clone(),
        #[cfg(feature = "memory")]
        memory_manager: memory_manager_for_web.clone(),
        #[cfg(not(feature = "memory"))]
        memory_manager: None,
        enabled_channels: enabled_channels.clone(),
        #[cfg(feature = "workflow")]
        workflow_engine: Some(workflow_engine.clone()),
        #[cfg(not(feature = "workflow"))]
        workflow_engine: None,
        cluster_rpc_call_fn,
        cluster_rpc_config,
        cluster_peers_fn,
        cluster_rpc_enabled: parking_lot::RwLock::new(None::<Arc<std::sync::atomic::AtomicBool>>),
        mcp_config_path: common::mcp_config_path(&home),
        mcp_enabled,
        estop,
        config_store: config_store.clone(),
        lsp_manager,
        agent_event_tx: Some(agent_event_tx),
        background_registry,
        // 全自动流转 P3/D1：board_issue 工具依赖（注册点在 build_agent_loop
        // 主 agent；store=None 时工具不注册）。moderator 槽此刻还空，agent
        // 建成后 :board_moderator_loop.set 填充——工具调用时读槽即得。
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_store: board_store.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_cluster: cluster_adapter_refs.as_ref().map(|(c, _, _, _)| c.clone()),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_moderator_slot: board_moderator_loop.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_home: home.clone(),
        #[cfg(all(feature = "board", feature = "cluster"))]
        board_event_hub: Some(web_server.event_hub().clone()),
        #[cfg(feature = "security")]
        approval_slot: std::sync::Arc::new(parking_lot::RwLock::new(
            None::<Arc<dyn nemesis_security::auditor::ApprovalManager>>,
        )),
        #[cfg(not(feature = "security"))]
        approval_slot: (),
        // F7（2026-09-06）：question 工具 broker 槽（先建空槽，装配块晚填
        // WebQuestionBroker；重启重建的 AgentLoop 共享同一槽 Arc）。
        question_slot: std::sync::Arc::new(parking_lot::RwLock::new(
            None::<Arc<dyn nemesis_types::agent::QuestionAsker>>,
        )),
    };

    let shared_resources = Arc::new(shared_resources);
    let agent_loop = crate::agent_factory::build_agent_loop(&shared_resources)
        .map_err(|e| anyhow::anyhow!("Failed to build agent loop: {}", e))?;
    let initial_tool_count = agent_loop.tool_count();
    info!(
        "[Gateway] AgentLoop built via factory ({} tools)",
        initial_tool_count
    );

    // Bridge the agent's tools into the workflow engine's tool registry so the
    // workflow `tool` node can invoke them. The registry was created empty
    // during workflow init above; here we wrap each agent tool in an
    // `AgentToolAdapter` (the two `Tool` traits are incompatible) and register
    // it. Each adapted tool runs the 8-layer security rule pipeline per call —
    // no interactive approval popup, no guardian LLM judge — so batch
    // workflows run unattended while still respecting workspace isolation and
    // the other rule layers.
    #[cfg(feature = "workflow")]
    {
        let tools_guard = agent_loop.tools();
        let mut bridged = 0usize;
        for (name, tool) in tools_guard.iter() {
            #[cfg(feature = "security")]
            let adapted = nemesis_agent::tool_adapter::AgentToolAdapter::new(
                name.clone(),
                Arc::clone(tool),
                security_plugin.clone(),
            );
            #[cfg(not(feature = "security"))]
            let adapted =
                nemesis_agent::tool_adapter::AgentToolAdapter::new(name.clone(), Arc::clone(tool));
            workflow_tool_registry.register(adapted);
            bridged += 1;
        }
        info!(
            "[Gateway] Bridged {} agent tools into the workflow tool registry",
            bridged
        );
    }

    // Wire up `agent` workflow nodes (milestone 1b-D2). Each workflow run
    // that hits an `agent` node will route through this runner, which
    // namespaces session keys under `workflow:{agent_id}` so workflow
    // sessions don't collide with human user sessions.
    #[cfg(feature = "workflow")]
    {
        workflow_engine
            .register_agent_runner(Arc::new(GatewayAgentRunner::new(agent_loop.clone())));
        info!("[Gateway] Workflow agent runner registered");

        // Wire DataStore into workflow engine so llm/question_classifier/parameter_extractor
        // node executors record a RequestLog per LLM call. Agent nodes are already
        // tracked via the agent_loop's own data_store wiring.
        if let Some(ref ds) = data_store {
            workflow_engine.set_usage_store(ds.clone());
            info!("[Gateway] Workflow usage store wired");
        }
    }

    Ok(AgentWiring {
        shared_resources,
        agent_loop,
        agent_event_rx,
        security_plugin,
        initial_tool_count,
    })
}
