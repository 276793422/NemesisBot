// ---------------------------------------------------------------------------
// GatewayCtx（计划 §4.1）：跨相位状态收编容器 + `GatewayCtx::assemble`。
//
// B1（§4.2）：PB-0 bootstrap + PB-1 单例预装配（计划口径原 1561–2459 行，
// Steps 1–9b）整体迁入 assemble；run() 前段收缩为一次 assemble 调用 + 影子
// 重绑。`--relay` 纯中继早退（Step 5b）在 assemble 内部处理——返回 None 时
// run() 直接 Ok 退出；run_relay 语义不变；channel_guard 仍护住 run() 全部
// 早退路径（C10，留在 run() 骨架首部）。
//
// 字段清单 = PB-0/PB-1 产物中被下游相位（PB-2..PB-9）消费的完整集合：
// §2.3.1 基础清单中本区间产物 + §2.3.2 十二变量表中产于本区间的条目
// （resolution / chat_secret_store / bridge_outbound_handle）。表外机械
// 必需项（agent_outbound_tx / model_name / llm_provider / mcp_enabled /
// skills_* / board_quota / config_path——SharedResources 字面量与 PB-8
// judge、PB-6 svc_mgr 的直接消费点）按 §4.1 收编规则补入，实施报告如实
// 记录。行号注记均为计划口径（拆解前 6,735 行版）。
// ---------------------------------------------------------------------------

use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use crate::common;

#[cfg(feature = "board")]
use super::fire_board_autopilot;
#[cfg(feature = "workflow")]
use super::migrate_legacy_workflow_dir;
use super::run_relay;

/// 跨相位状态容器（§4.1）。字段名与原 run() 局部名逐字一致——下游以影子
/// 重绑（owned clone）沿用原名，所有权拓扑与原单绑定等价。晚绑定槽按 §0
/// 原则 6 只收编不改模式；feature 门字段沿用原局部声明门，逐字段保留。
pub(crate) struct GatewayCtx {
    // --- 身份与配置（§2.3.1）---
    pub home: PathBuf,
    pub config_path: PathBuf,
    pub config_store: Arc<nemesis_config::ConfigStore>,
    /// 启动快照（一次性读；live 消费者走 config_store.handle()）。
    pub cfg: nemesis_config::Config,
    /// ProviderResolution 整体跨相位（原 1700 → PB-5 set_model_info/streaming、
    /// §2.3.2）。
    pub resolution: nemesis_config::ProviderResolution,
    /// `resolution.model_name` 快照（SharedResources/web model_info 消费）。
    pub model_name: String,
    // --- 消息主干（§2.3.1）---
    pub bus: Arc<nemesis_bus::MessageBus>,
    pub cron_service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    pub conv_router: nemesis_web::SharedConvRouter,
    /// estop 单例（C8：集群装配前创建，四方共享同一 Arc）。
    pub estop: Arc<nemesis_agent::estop::EstopState>,
    pub data_store: Option<Arc<nemesis_data::DataStore>>,
    /// Step 8 出站桥的 agent 端 mpsc 发送半（PB-4 装入 SharedResources）。
    pub agent_outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,
    /// Step 8 出站桥任务句柄（§2.3.2：PB-9 teardown .abort()）。JoinHandle
    /// 非 Clone，run() 侧以引用重绑。
    pub bridge_outbound_handle: tokio::task::JoinHandle<()>,
    pub mcp_enabled: bool,
    // --- PB-1 建栈子系统单例（§2.3.1，feature 门逐字沿用原局部声明）---
    pub skills_loader_arc: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    pub skills_registry_arc: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    #[cfg(feature = "board")]
    pub board_store: Option<Arc<nemesis_board::BoardStore>>,
    #[cfg(feature = "memory")]
    pub memory_manager_for_web: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    #[cfg(feature = "forge")]
    pub forge_for_web: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(feature = "forge")]
    pub forge_executor_for_tools: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    #[cfg(feature = "workflow")]
    pub workflow_engine: Arc<nemesis_workflow::engine::WorkflowEngine>,
    /// 与 workflow 引擎共享的工具注册表（PB-4 AgentToolAdapter 桥接填装）。
    #[cfg(feature = "workflow")]
    pub workflow_tool_registry: Arc<nemesis_tools::registry::ToolRegistry>,
    /// §2.3.2：原 1772 → set_chat_secret_store@5317（PB-5）。
    #[cfg(feature = "workflow")]
    pub chat_secret_store: Arc<nemesis_workflow::chat_secrets::ChatSecretStore>,
    // --- 晚绑定槽（§0 原则 6：模式不变，集中收编）---
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_moderator_loop: Arc<std::sync::OnceLock<Arc<nemesis_agent::r#loop::AgentLoop>>>,
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub autopilot_cluster_slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>>,
    /// Swarm M3 本节点对外 web 基址槽（PB-7 落槽、PB-5 资产签名读）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_asset_url_slot: nemesis_board::AdvertisedUrl,
    /// Swarm M3 master 侧讨论额度台账（PB-2 讨论桥 + PB-5 board service）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_quota: Arc<nemesis_board::quota::QuotaLedger>,
    /// 默认跟随 wrapper 快照（workflow 引擎 / PB-8 guardian judge 消费）。
    #[cfg(any(feature = "workflow", feature = "security"))]
    pub llm_provider: Arc<dyn nemesis_providers::router::LLMProvider>,
}

impl GatewayCtx {
    /// Steps 1–9b：home/config/logger/relay 早退/state file 占位/LLM 解析
    /// （PB-0）+ workflow 引擎/bus/出站泵/cron/board/estop/槽位/forge/skills/
    /// memory/DataStore（PB-1）。原文自 run() 前段逐字迁入（B1），仅两处机
    /// 械改写：relay 早退改 `?` + `Ok(None)`（结果等价）；末尾以字段简写构
    /// 造本结构体。
    pub(crate) async fn assemble(
        local: bool,
        relay: bool,
        extra_args: &[String],
    ) -> Result<Option<GatewayCtx>> {
        // Step 1: Resolve home directory
        let home = common::resolve_home(local);

        // Step 2: Check configuration file exists
        let config_path = common::config_path(&home);
        if !config_path.exists() {
            // 双击直启 goal（2026-09-17）：config 缺失不再硬退——auto-init
            // （Seed 种子语义：一切 only-if-absent，用户已有的 workspace/人格/
            // 子系统配置绝不被 clobber）后继续启动。显式 `nemesisbot gateway`
            // 同样走此路径：onboard CLI 保留（老用户 re-onboard 覆盖语义），
            // 但不再是新用户的强制前置步骤。
            println!(
                "[Gateway] Configuration file not found at {} — auto-initializing (seed mode)...",
                config_path.display()
            );
            if let Err(e) = crate::commands::onboard::onboard_default(
                &home,
                local,
                crate::commands::onboard::OnboardMode::Seed,
            ) {
                eprintln!("Error: auto-init failed: {}", e);
                eprintln!("  Run 'nemesisbot onboard default' to initialize manually.");
                std::process::exit(1);
            }
        }

        // Step 3: Check home directory exists
        if !home.exists() {
            eprintln!(
                "Error: Configuration directory not found: {}",
                home.display()
            );
            eprintln!("  Run 'nemesisbot onboard default' to create configuration.");
            std::process::exit(1);
        }

        // Step 3-2（SAN-01/D4）：旧「只替换 `:`」文件名映射的嵌套 session 目录
        // 平化（B 端复合键 `{node}/{chat}` 旧写 `<logs>/{node}/{chat}.jsonl`，
        // 白名单消毒后写平面 `{node}_{chat}.jsonl`）。幂等 + best-effort；
        // 必须先于任何会话读写执行（放 Step 3 后、agent/web 装配前）。
        nemesis_agent::chat_log::migrate_nested_session_logs();

        // Step 3a: Ensure exe directory is in PATH so LLM shell tools can find nemesisbot
        if common::ensure_exe_in_path() {
            tracing::info!("[Gateway] Added exe directory to PATH for LLM shell access");
        }

        // Step 4: Load configuration into the runtime cache (single source of
        // truth). `cfg` is a startup snapshot for one-time reads below; live
        // consumers (executor.sandbox, …) read `config_store.handle()` so toggles
        // flip without a gateway restart.
        let config_store = std::sync::Arc::new(
            nemesis_config::ConfigStore::load(&config_path)
                .map_err(|e| anyhow::anyhow!("Error loading config: {}", e))?,
        );
        // Install the process-wide singleton so WSAPI handlers (sandbox/config/
        // channels…) reach the same live config without AppState wiring. A
        // dashboard write through the store is visible to every consumer —
        // including the executor's sandbox probe — on the next read, no restart.
        nemesis_config::set_global(config_store.clone());
        let cfg = config_store.handle().read().clone();

        // Step 4b: Ensure the Sandboxie engine is ready (Route A) — driver and
        // service are judged SEPARATELY, so this never triggers the per-run
        // driver-install UAC. Steady-state (both resident) is a reuse no-op; only
        // the lightweight "start service if stopped" path may run.
        #[cfg(feature = "sandbox")]
        {
            let sandbox_enabled = cfg.executor.as_ref().is_some_and(|ec| ec.sandbox);
            crate::commands::sandbox::ensure_sandbox_ready(&home, sandbox_enabled);
        }

        // [capture] Initialize the diagnostic capture sink — failure-triggered
        // only (zero happy-path overhead). Reads `debug.capture.enabled`
        // (defaults to true when unset). Evidence lands in
        // `{workspace}/logs/capture/{session_key}/{ts}_{signal}/` only when a
        // failure signal fires (LLM retry exhausted / context overflow / session
        // overwrite / agent error funnel). Diagnostic only — does not change any
        // control flow or business logic.
        {
            let capture_enabled = cfg.debug.as_ref().is_none_or(|d| d.capture.enabled);
            nemesis_agent::capture_sink::CaptureSink::init(home.join("workspace"), capture_enabled);
            if capture_enabled {
                info!("[Gateway] Diagnostic capture armed (failure-triggered → logs/capture/)");
            }
        }

        // Step 5: Initialize logger from config
        let mut args: Vec<String> = std::env::args().skip(2).collect();
        args.extend(extra_args.iter().cloned());
        let _log_flags = common::init_logger_from_config(&config_path, &args);

        // Step 5b（goal：反向桥与多设备汇聚，一期批次一）：`--relay` 纯中继
        // 模式早退——只起 web server（状态页 `/relay` + `/bridge` 接入 +
        // `/d/<node_id>/` 转发），不起本地 agent/board/集群/discovery，状态页
        // 即全部 UI。bridge.server.token 空 → 拒绝启动（fail-closed）。
        if relay {
            run_relay(&home, &cfg).await?;
            return Ok(None);
        }

        // Step 6: Write gateway state file (PID only; web_port updated after bind)
        let pid = std::process::id();
        {
            let state_dir =
                nemesis_path::resolve_state_dir_in_workspace(&common::workspace_path(&home));
            if let Err(e) = std::fs::create_dir_all(&state_dir) {
                warn!("[Gateway] Failed to create state dir: {}", e);
            }
            let state_path = state_dir.join("gateway.json");
            let state_json = serde_json::json!({
                "pid": pid,
                "web_host": "",
                "web_port": 0,
            });
            if let Err(e) = std::fs::write(&state_path, state_json.to_string()) {
                warn!("[Gateway] Failed to write gateway state: {}", e);
            } else {
                info!(
                    "[Gateway] Gateway state written: {} (PID: {})",
                    state_path.display(),
                    pid
                );
            }
        }

        // Step 7: Resolve the default LLM model and create provider
        // 双击直启 goal（2026-09-17）：resolve/create 失败不再硬退——warn + 降级
        //（NullProvider，对话诚实报「未配置模型」），Dashboard 配好并设默认后
        // set_default 热切恢复。一次性 CLI 入口保持严格失败（见 agent_factory 注）。
        let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
        let resolution = match nemesis_config::resolve_model_config(&cfg, &llm_ref) {
            Ok(r) => r,
            Err(e) => {
                warn!(
                    "[Gateway] Failed to resolve model '{}': {} — 无 LLM 降级启动（NullProvider）",
                    llm_ref, e
                );
                nemesis_config::ProviderResolution {
                    model_name: llm_ref.clone(),
                    ..Default::default()
                }
            }
        };

        // Build the LLM provider once. The same Arc<dyn LLMProvider> is reused by
        // the workflow engine (milestone 1a-E1, so workflow `llm` nodes route to
        // the same model) and the security guardian judge. The main agent loop
        // builds its own provider, so this is only needed when workflow or security
        // is enabled.
        #[cfg(any(feature = "workflow", feature = "security"))]
        let factory_cfg = nemesis_providers::factory::FactoryConfig {
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
        #[cfg(any(feature = "workflow", feature = "security"))]
        let (raw_llm_provider, provider_assembly_warn): (
            Arc<dyn nemesis_providers::router::LLMProvider>,
            Option<String>,
        ) = nemesis_providers::factory::create_provider_or_null(&factory_cfg);
        // 默认跟随 wrapper（2026-09-22 方案A）：workflow 引擎与 guardian judge
        // 持有的这份快照不再烘焙——热切默认模型后经槽委派自动跟随，引擎无
        // set_provider 也能换模型（捕获名判定语义见 default_slot 模块 doc）。
        #[cfg(any(feature = "workflow", feature = "security"))]
        let llm_provider: Arc<dyn nemesis_providers::router::LLMProvider> =
            nemesis_providers::default_slot::default_following(
                raw_llm_provider,
                &resolution.model_name,
                &factory_cfg.llm_ref,
            );
        #[cfg(any(feature = "workflow", feature = "security"))]
        if let Some(ref e) = provider_assembly_warn {
            warn!(
                "[Gateway] Provider create failed: {} — workflow/security lane 走 NullProvider 降级",
                e
            );
        } else {
            info!("[Gateway] Provider config validated for {}", llm_ref);
        }

        let model_name = resolution.model_name.clone();

        // --- Workflow Engine (milestone 1a-E1) ---
        // Build an integrated engine that wires RealLLMNodeExecutor (so llm nodes
        // invoke the same provider as the agent) and RealToolNodeExecutor (so tool
        // nodes can dispatch to any later-registered tools). All workflow files
        // live under {home}/workspace/workflow/ with four subdirs:
        //   definitions/  - YAML workflow definitions (loaded at startup)
        //   templates/    - starter templates for the CLI workflow command
        //   checkpoints/  - resume snapshots for in-flight recovery
        //   executions/   - JSONL execution logs
        // Migrate any pre-refactor data from {home}/workflow/ first.
        #[cfg(feature = "workflow")]
        let workflow_engine: std::sync::Arc<nemesis_workflow::engine::WorkflowEngine>;
        #[cfg(feature = "workflow")]
        let chat_secret_store: std::sync::Arc<
            nemesis_workflow::chat_secrets::ChatSecretStore,
        >;
        // Tool registry shared with the workflow engine. Declared in the outer
        // scope (not inside the workflow-init block below) so we can populate it
        // *after* the agent loop is built — the agent's tools must be bridged in
        // via `AgentToolAdapter` (the two `Tool` traits are incompatible, so the
        // workflow registry cannot share the agent's map directly).
        #[cfg(feature = "workflow")]
        let workflow_tool_registry: std::sync::Arc<nemesis_tools::registry::ToolRegistry>;
        #[cfg(feature = "workflow")]
        {
            let workflow_root = home.join("workspace").join("workflow");
            let workflow_executions_dir = workflow_root.join("executions");
            let workflow_checkpoints_dir = workflow_root.join("checkpoints");
            let workflow_defs_dir = workflow_root.join("definitions");
            for d in [
                &workflow_executions_dir,
                &workflow_checkpoints_dir,
                &workflow_defs_dir,
            ] {
                if let Err(e) = std::fs::create_dir_all(d) {
                    warn!(
                        "[Gateway] Failed to create workflow subdir {}: {}",
                        d.display(),
                        e
                    );
                }
            }
            migrate_legacy_workflow_dir(&home, &workflow_executions_dir, &workflow_checkpoints_dir);

            workflow_tool_registry = Arc::new(nemesis_tools::registry::ToolRegistry::new());
            let engine = nemesis_workflow::engine::WorkflowEngine::new_integrated_with_dirs(
                llm_provider.clone(),
                workflow_tool_registry.clone(),
                Some(workflow_executions_dir.clone()),
                Some(workflow_checkpoints_dir.clone()),
            );

            // Load all workflow definitions from {home}/workspace/workflow/definitions/.
            engine.set_workflow_defs_dir(workflow_defs_dir.clone());

            // U10 统一执行世界：workflow script 节点（无 registry 的装配路径）、
            // per-node `sandbox: false` 的受守卫直跑、引擎控制面写盘（persist/
            // delete 根外拒绝）都经同一个 ExecutionWorld。executor 分离开（默认）
            // → 无 world（行为不变）；开 → 与 agent 工具层同一条开关链
            // （executor.enabled / executor.sandbox，live probe）。
            // 注意：gateway 的 script 节点主路径仍走注册表车道（AgentToolAdapter
            // 桥接的 agent 工具 —— 已是 RemoteExecutorTool 包装，Layer 1/2 生效）；
            // world 提供的是 CLI 侧同能力 + 引擎 IO 守卫 + spawn 车道。
            #[cfg(feature = "sandbox")]
            {
                let workspace_dir = home.join("workspace");
                let spawn_roots = vec![workspace_dir.clone()];
                match crate::exec_world::build_workflow_world(
                    &home,
                    &workspace_dir,
                    vec![
                        workflow_defs_dir.clone(),
                        workflow_checkpoints_dir.clone(),
                        workflow_executions_dir.clone(),
                    ],
                    spawn_roots,
                    config_store.handle(),
                ) {
                    Ok(Some(world)) => {
                        engine.set_execution_world(world);
                    }
                    Ok(None) => {
                        info!(
                            "[Gateway] executor separation off — workflow engine runs without an \
                             execution world (script nodes via tool registry, engine IO unguarded; \
                             pre-U10 behaviour)"
                        );
                    }
                    Err(e) => {
                        warn!(
                            "[Gateway] execution world build failed (engine continues without it): {}",
                            e
                        );
                    }
                }
            }

            match engine.load_workflows_from_dir(&workflow_defs_dir) {
                Ok(n) => {
                    info!(
                        "[Gateway] Workflow engine loaded {} definition(s) from {}",
                        n,
                        workflow_defs_dir.display()
                    );
                }
                Err(e) => {
                    warn!(
                        "[Gateway] Workflow engine load failed: {} (dir={})",
                        e,
                        workflow_defs_dir.display()
                    );
                }
            }

            // Spawn cron-triggered workflows (milestone 1a-E2).
            let _workflow_cron_handles = engine.spawn_cron_triggers();
            let cron_wf_count = _workflow_cron_handles.len();
            if cron_wf_count > 0 {
                info!(
                    "[Gateway] Workflow cron triggers registered: {}",
                    cron_wf_count
                );
            }

            // Restore any in-flight executions paused at human_review nodes or
            // interrupted by a previous crash (milestone 1b-A1 step 7). The checkpoint
            // store lives under {home}/workspace/workflow/checkpoints/.
            match engine.restore_incomplete_executions().await {
                Ok(n) if n > 0 => {
                    info!(
                        "[Gateway] Workflow engine restored {} in-flight execution(s) from checkpoints",
                        n
                    );
                }
                Ok(_) => {}
                Err(e) => {
                    warn!(
                        "[Gateway] Workflow checkpoint restore failed: {} (continuing with fresh state)",
                        e
                    );
                }
            }
            workflow_engine = engine;

            // Per-workflow chat password store for the standalone workflow-chat page.
            // Loaded from {home}/workspace/workflow/chat_secrets.json — created on
            // first set_password call. Lives outside the workflow engine because
            // secrets shouldn't ride along with workflow YAML (which is shareable).
            let chat_secrets_path = workflow_root.join("chat_secrets.json");
            chat_secret_store = Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::open(
                chat_secrets_path,
            ));
        }

        // Step 8: Create MessageBus
        let bus = Arc::new(nemesis_bus::MessageBus::new());
        info!("[Gateway] Message bus created");

        // Step 9: Create AgentLoop with mpsc channels (bridge to broadcast bus)
        // The AgentLoop uses mpsc channels, while the bus uses broadcast.
        // We bridge: bus inbound (broadcast) → mpsc inbound → AgentLoop
        //            AgentLoop → mpsc outbound → bus outbound (broadcast)
        //
        // Capacity is 1024 (up from 256) to reduce message loss under load.
        // The inbound bridge is created inside AgentLoopServiceAdapter::start().
        let (agent_outbound_tx, mut agent_outbound_rx) =
            tokio::sync::mpsc::channel::<nemesis_types::channel::OutboundMessage>(1024);

        // Bridge: agent outbound mpsc → bus outbound broadcast
        let bus_out = bus.clone();
        let bridge_outbound_handle = tokio::spawn(async move {
            while let Some(msg) = agent_outbound_rx.recv().await {
                bus_out.publish_outbound(msg);
            }
        });

        // The AgentLoop is now created by the factory function (agent_factory.rs).
        // provider, system prompt, AgentConfig, AgentLoop::new_bus, session store,
        // state manager, SharedToolConfig, tool registration, MCP, cluster_rpc,
        // continuation manager — all handled inside build_agent_loop().
        // agent_outbound_tx will be stored in SharedResources later.

        // agent_outbound_tx is moved into SharedResources below.
        // For now, keep it as a local variable.
        // State manager injection into agent_loop is now handled by the factory function.

        // Register all tools (mirrors Go's bot_service.go initComponents):
        //   default tools + web + cluster + spawn + memory + skills + hardware + exec + cron
        let cron_store_path = common::cron_store_path(&home);
        let cron_service = std::sync::Arc::new(std::sync::Mutex::new(
            nemesis_cron::service::CronService::new(&cron_store_path.to_string_lossy()),
        ));

        // Swarm M3（§5.4）：本节点对外 web 基址槽（bind 后 set；G9 起可由
        // 自愈任务随集群注册表知识更新——多重网卡选对 NIC、DHCP 换 IP 自动
        // 跟随。dispatch 签发资产 bundle 时经 store 的 AssetSignContext 读取；
        // 未 set = 基址未解析，签发诚实跳过）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let board_asset_url_slot: nemesis_board::AdvertisedUrl =
            nemesis_board::AdvertisedUrl::default();

        // W2 P1: Managed-agent board store — open (or create) {workspace}/board/board.db.
        // Injected into the web server below; failure logs a warning and leaves the
        // board unavailable (board.* WSAPI commands return "board service not
        // available") instead of blocking gateway startup.
        #[cfg(feature = "board")]
        let board_store = {
            let board_db = home.join("workspace").join("board").join("board.db");
            match nemesis_board::BoardStore::open(&board_db, "NB") {
                Ok(store) => {
                    // Swarm M2: 默认频道（#dev/#qa/#general）幂等种子——首启建，
                    // 已有则 no-op。
                    if let Err(e) = store.ensure_default_channels() {
                        warn!("[Gateway] Board default channels seed failed: {}", e);
                    }
                    info!("[Gateway] Board store opened at {}", board_db.display());
                    Some(std::sync::Arc::new(store))
                }
                Err(e) => {
                    warn!(
                        "[Gateway] Board store open failed: {} (board.* disabled)",
                        e
                    );
                    None
                }
            }
        };

        // Swarm M2: board 维护循环 —— 启动即清扫一次频道消息 + 备份一次
        // board.db，之后每 24h 重复。频道消息按 `board.discussion.retention_days`
        // 清扫（0=永久）；备份经 VACUUM INTO 快照到 workspace/backups/
        // （`board.backup.keep`=0 关闭备份）。
        #[cfg(feature = "board")]
        {
            let maint_cfg = cfg.board.clone().unwrap_or_default();
            let store_for_maint = board_store.clone();
            let workspace_dir = home.join("workspace");
            let db_path_for_maint = workspace_dir.join("board").join("board.db");
            let backups_dir = nemesis_path::resolve_board_backups_dir_in_workspace(&workspace_dir);
            tokio::spawn(async move {
                let mut ticker = tokio::time::interval(std::time::Duration::from_secs(86_400));
                ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                loop {
                    ticker.tick().await; // 首 tick 立即返回 = 启动即维护
                    let Some(store) = store_for_maint.as_ref() else {
                        continue;
                    };
                    let retention = maint_cfg.discussion.retention_days;
                    match store.sweep_channel_messages(retention) {
                        Ok(0) => {}
                        Ok(n) => info!(
                            "[Gateway] Board channel sweep removed {} messages (retention={}d)",
                            n, retention
                        ),
                        Err(e) => warn!("[Gateway] Board channel sweep failed: {}", e),
                    }
                    match nemesis_board::backup::backup_database(
                        &db_path_for_maint,
                        &backups_dir,
                        maint_cfg.backup.keep as usize,
                    ) {
                        Ok(Some(p)) => info!("[Gateway] Board backup written: {}", p.display()),
                        Ok(None) => {}
                        Err(e) => warn!("[Gateway] Board backup failed: {}", e),
                    }
                }
            });
            info!(
                "[Gateway] Board maintenance loop armed (retention={}d, backup keep={})",
                maint_cfg.discussion.retention_days, maint_cfg.backup.keep
            );
        }

        // Swarm M3: master 侧讨论额度台账（进程内存态，重启清零=诚实边界）。
        // 配置三闸来自 board.discussion（0=该闸关闭）；经 live 句柄每次判定
        // 现读——config.json 热改额度即时生效（与 tier/hidden_tools 同语义）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let board_quota: std::sync::Arc<nemesis_board::quota::QuotaLedger> = {
            let config_handle = config_store.handle();
            std::sync::Arc::new(nemesis_board::quota::QuotaLedger::with_provider(
                move || {
                    let guard = config_handle.read();
                    let disc = guard.board.clone().unwrap_or_default().discussion;
                    nemesis_board::quota::QuotaConfig {
                        max_agent_turns_per_thread: disc.max_agent_turns_per_thread,
                        hourly_budget_per_node: disc.hourly_budget_per_node,
                        rate_limit_per_min: disc.rate_limit_per_min,
                    }
                },
            ))
        };

        // Swarm M3: 主持人裁决用主 AgentLoop 后置装配桥（nb_bus 注册早于
        // agent_loop 构建；build 完成后 set()）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let board_moderator_loop: std::sync::Arc<
            std::sync::OnceLock<std::sync::Arc<nemesis_agent::r#loop::AgentLoop>>,
        > = std::sync::Arc::new(std::sync::OnceLock::new());

        // Opt 2: conversation→WS router, shared between the cron fire handler
        // (lookup, here) and process_messages (bind, in the web server). Built
        // early so the cron closure can capture a clone before set_on_job; the
        // original Arc is later moved into the web server via set_conv_router.
        let conv_router: nemesis_web::SharedConvRouter =
            std::sync::Arc::new(nemesis_web::ConvRouter::new());

        // P1/T1-6 estop 保险丝：句柄在集群装配前创建——peer_chat_callback 里的
        // board 评审依赖集与下方 SharedResources 共享同一 Arc（跨 agent 重启存活）。
        // F-U4-5（2026-09-15）：创建点从集群装配段上移到 cron 装配前——autopilot
        // cron 闭包也捕获同一 Arc（急停中定时触发诚实跳过）。
        let estop = std::sync::Arc::new(nemesis_agent::estop::EstopState::new());
        info!("[Gateway] Global e-stop (kill switch) initialized (released)");

        // W2 P4: board autopilot 的集群槽位。on_job 闭包在 cluster 创建之前
        // 装配（cron 服务先于 cluster 就绪），用 OnceLock 让闭包在 cluster 建
        // 好后取用；未启用集群时保持 None（target 为空的 autopilot 规则仍可
        // 纯本地建单）。
        #[cfg(all(feature = "board", feature = "cluster"))]
        let autopilot_cluster_slot: Arc<
            std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>,
        > = Arc::new(std::sync::OnceLock::new());

        // C3: Wire CronService — set_on_job handler + start.
        // Mirrors Go's bot_service.go:392-399, 571-579.
        {
            let bus_for_cron = bus.clone();
            let router_for_cron = conv_router.clone();
            // W2 P4: autopilot 分支捕获（board store + 集群槽位）。
            #[cfg(feature = "board")]
            let store_for_ap = board_store.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let slot_for_ap = autopilot_cluster_slot.clone();
            // 全自动流转 D2：auto_plan 的 moderator 槽（board_moderator_loop 在
            // 本闭包装配前创建、agent_loop 建成后 set——同 OnceLock 模式）。
            #[cfg(all(feature = "board", feature = "cluster"))]
            let mod_slot_for_ap = board_moderator_loop.clone();
            #[cfg(all(feature = "board", feature = "cluster"))]
            let home_for_ap = home.clone();
            // F-U4-5：急停中 autopilot 定时触发诚实跳过（与手动 autopilot.run
            // 同一面；释放后下个周期自然恢复）。
            #[cfg(feature = "board")]
            let estop_for_ap = estop.clone();
            cron_service
                .lock()
                .unwrap()
                .set_on_job(move |job: &nemesis_cron::service::CronJob| {
                    // W2 P4: board autopilot job（名 `board-ap:{id}`）→ 模板建单
                    // +（可选）派发，不走消息总线。必须放在 message 判空前——
                    // autopilot job 的 message 恒为空，否则落 "No message to
                    // deliver"。
                    #[cfg(feature = "board")]
                    if job.name.starts_with("board-ap:") {
                        // F-U4-5：急停中定时触发跳过（记入 job run 历史，诚实
                        // 可见；非 Err——到点跳过与 disabled 规则同语义）。
                        if estop_for_ap.is_engaged() {
                            return Ok(
                                "⛔ 急停（E-STOP）生效中，autopilot 跳过本次触发（释放后自动恢复）"
                                    .to_string(),
                            );
                        }
                        #[cfg(feature = "cluster")]
                        {
                            // auto_plan 上下文在触发时现构（槽引用 + home + 集群；
                            // hub 传 None——闭包装配早于 web server，SSE 诚实降级）。
                            let ap_ctx = nemesis_web::handlers::board::AutoPlanContext {
                                moderator_slot: mod_slot_for_ap.clone(),
                                home: home_for_ap.clone(),
                                hub: None,
                                cluster: slot_for_ap.get().cloned(),
                            };
                            return fire_board_autopilot(
                                &job.name,
                                store_for_ap.as_ref(),
                                slot_for_ap.get(),
                                Some(&ap_ctx),
                            );
                        }
                        #[cfg(not(feature = "cluster"))]
                        return fire_board_autopilot(&job.name, store_for_ap.as_ref());
                    }
                    if !job.payload.message.is_empty() {
                        let channel = job
                            .payload
                            .channel
                            .clone()
                            .unwrap_or_else(|| "web".to_string());
                        // Phase 2: target the named conversation so the exchange is
                        // persisted into its history (loop.rs adopts `agent:`-prefixed
                        // session_key verbatim). Opt 2: if a live WS tab is bound for
                        // this conversation, set chat_id = web:<ws_id> so the reply
                        // also live-pushes; otherwise chat_id falls back to `to`
                        // (delivery may fail-soft, but history is still saved).
                        let session_key = job.payload.session_key.clone().unwrap_or_default();
                        let chat_id = if !session_key.is_empty() {
                            router_for_cron
                                .target(&session_key)
                                .unwrap_or_else(|| job.payload.to.clone().unwrap_or_default())
                        } else {
                            job.payload.to.clone().unwrap_or_default()
                        };
                        let inbound = nemesis_types::channel::InboundMessage {
                            channel,
                            sender_id: format!("cron:{}", job.id),
                            chat_id,
                            content: job.payload.message.clone(),
                            media: vec![],
                            session_key,
                            correlation_id: String::new(),
                            metadata: {
                                let mut m = std::collections::HashMap::new();
                                m.insert("cron_job_id".to_string(), job.id.clone());
                                m.insert("cron_job_name".to_string(), job.name.clone());
                                // T3 (U12): per-fire tool-round budget — the agent
                                // loop reads this and caps the continuation turn's
                                // tool iterations at this value (graceful stop via
                                // the grace-round path; job survives).
                                if let Some(mr) = job.payload.max_rounds {
                                    m.insert("cron_max_rounds".to_string(), mr.to_string());
                                }
                                m
                            },
                            voice_playback: None,
                        };
                        bus_for_cron.publish_inbound(inbound);
                        Ok(format!("Cron job '{}' triggered", job.name))
                    } else {
                        Ok("No message to deliver".to_string())
                    }
                });
            info!(
                "[Gateway] Cron service handler wired (publishes to bus; Opt2 conv_router attached)"
            );
        }

        // Create Forge executor (always create instance for runtime toggle support).
        // M2 + M3 + L1 + L2 + M4 all wired here.
        #[cfg(feature = "forge")]
        let forge_enabled = cfg.forge.as_ref().map(|f| f.enabled).unwrap_or(false);
        #[cfg(feature = "forge")]
        let forge_for_web: Option<std::sync::Arc<nemesis_forge::forge::Forge>>;
        #[cfg(feature = "forge")]
        let forge_executor_for_tools: Option<
            std::sync::Arc<nemesis_forge::forge_tools::ForgeToolExecutor>,
        >;
        #[cfg(feature = "forge")]
        {
            // Load forge config from file, fall back to defaults if missing.
            // 委托 nemesis-path 唯一拼接点（与 web forge handler / CLI 同源）。
            let forge_config_path = nemesis_path::resolve_forge_config_path_in_workspace(
                &common::workspace_path(&home),
            );
            let mut forge_config = if forge_config_path.exists() {
                nemesis_forge::config::load_forge_config(&forge_config_path)
            } else {
                nemesis_forge::config::ForgeConfig::default()
            };
            // F-P1 truth-source fix: the master switch is config.json's `forge.enabled`
            // (-> `forge_enabled`, which gates the background tasks). Mirror it into
            // forge_config so `Forge::is_enabled()` (the per-tool-call recording
            // gate) reflects the real runtime state. config.forge.json is often
            // absent (default enabled=false) — without this, recording would be
            // silently blocked even when forge is on.
            forge_config.enabled = forge_enabled;
            let forge_workspace = home.join("workspace");
            let forge_dir = forge_workspace.join("forge");
            let mut forge = nemesis_forge::forge::Forge::new(forge_config.clone(), forge_workspace);

            // Initialize Reflector (statistical analysis + report writing).
            forge.init_reflector(nemesis_forge::reflector::Reflector::with_reflections_dir(
                forge_dir.join("reflections"),
            ));
            info!("[Gateway] Forge reflector initialized");

            // ONE shared registry for the Phase 6 closed loop (pipeline + monitor +
            // learning engine). Previously each got its own empty Registry (default
            // relative index_path), so deploy/monitor/feedback operated on disjoint
            // stores. (F-C3) Dedicated path so it persists + reloads; separate from
            // Forge's manual-create registry.json to avoid a two-instance collision.
            let forge_shared_registry = std::sync::Arc::new(
                nemesis_forge::registry::Registry::new(nemesis_forge::types::RegistryConfig {
                    index_path: forge_dir
                        .join("learning_registry.json")
                        .to_string_lossy()
                        .to_string(),
                }),
            );
            // F-D3: reload prior learned artifacts so they survive restart.
            let _ = forge_shared_registry.load().await;

            // Initialize Pipeline (3-stage validation). Built as Arc + sharing the
            // closed-loop registry so it can be injected into the LearningEngine (F-C1).
            let forge_pipeline = std::sync::Arc::new(nemesis_forge::pipeline::Pipeline::new(
                forge_config.clone(),
                forge_shared_registry.clone(),
            ));
            forge.init_pipeline(forge_pipeline.clone());
            info!("[Gateway] Forge pipeline initialized");

            // Initialize trace collection (TraceCollector + TraceStore).
            {
                let trace_collector = nemesis_forge::trace::TraceCollector::new();
                let trace_store =
                    nemesis_forge::trace_store::TraceStore::new(forge_dir.join("traces"));
                forge.init_trace(trace_collector, trace_store);
                info!("[Gateway] Forge trace collection initialized");
            }

            // Initialize learning engine (Phase 6 closed-loop). Shares the same
            // registry as pipeline + monitor (F-C3); init_learning injects the
            // pipeline + monitor into the engine so the loop runs (F-C1).
            let cycle_store = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
            let learning_engine = nemesis_forge::learning_engine::LearningEngine::with_forge_dir(
                forge_config.clone(),
                forge_dir.clone(),
                forge_shared_registry.clone(),
                cycle_store,
            );
            let cycle_store_for_init = nemesis_forge::cycle_store::CycleStore::new(&forge_dir);
            let forge_monitor =
                std::sync::Arc::new(nemesis_forge::monitor::DeploymentMonitor::new(
                    forge_config.clone(),
                    forge_shared_registry.clone(),
                ));
            forge.init_learning(learning_engine, forge_monitor, cycle_store_for_init);
            info!(
                "[Gateway] Forge learning engine initialized (Phase 6; pipeline+monitor injected)"
            );

            // Set bridge → init syncer.
            forge.set_bridge(std::sync::Arc::new(nemesis_forge::bridge::NoOpBridge::new(
                "local".to_string(),
            )));
            forge.init_syncer();
            info!("[Gateway] Forge syncer initialized");

            // Set LLM provider — now handled by the factory function (agent_factory.rs).

            let forge = std::sync::Arc::new(forge);

            // F-D3: reload Forge's manual-create registry so prior artifacts survive restart.
            let _ = forge.registry().load().await;
            // F-C1: inject skill_creator into the learning engine (needs Arc<Forge>,
            // which implements SkillCreator). pipeline+monitor were injected in
            // init_learning; this completes the Phase 6 wiring.
            if let Some(le) = forge.learning_engine() {
                le.set_skill_creator(forge.clone());
                info!("[Gateway] Forge learning engine wired: skill_creator injected");
            }

            // LearningEngine dependency injection is now handled by the factory function.

            let executor = std::sync::Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(
                forge.clone(),
            ));
            info!("[Gateway] Forge executor created (8 tools will be registered)");

            // Forge injection into agent_loop is now handled by the factory function.

            // Start background tasks only if enabled in config.
            if forge_enabled {
                let forge_for_start = forge.clone();
                tokio::spawn(async move {
                    forge_for_start.start().await;
                });
                info!("[Gateway] Forge started (background tasks running)");
            } else {
                info!("[Gateway] Forge created but not started (enabled=false in config)");
            }

            // Store for web server injection.
            forge_for_web = Some(forge);
            forge_executor_for_tools = Some(executor);
        }

        let mcp_enabled = cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false);

        #[cfg(feature = "memory")]
        let mut memory_manager_for_web: Option<
            std::sync::Arc<nemesis_memory::manager::MemoryManager>,
        > = None;

        let skills_loader_arc: Option<std::sync::Arc<nemesis_skills::loader::SkillsLoader>> = {
            let workspace_str = home.join("workspace").to_string_lossy().to_string();
            let global_skills_str = home
                .join("workspace")
                .join("skills")
                .to_string_lossy()
                .to_string();
            let loader =
                nemesis_skills::loader::SkillsLoader::new(&workspace_str, &global_skills_str, "");
            info!(
                "[Gateway] Skills loader created (workspace={}, global_skills={})",
                workspace_str, global_skills_str
            );
            Some(std::sync::Arc::new(loader))
        };

        let skills_registry_arc: Option<std::sync::Arc<nemesis_skills::registry::RegistryManager>> = {
            // 委托 nemesis-path 唯一拼接点。
            let skills_config_path = nemesis_path::resolve_skills_config_path_in_workspace(
                &common::workspace_path(&home),
            );
            if skills_config_path.exists() {
                match std::fs::read_to_string(&skills_config_path) {
                    Ok(content) => {
                        match serde_json::from_str::<nemesis_skills::types::RegistryConfig>(
                            &content,
                        ) {
                            Ok(reg_config) => {
                                let rm = nemesis_skills::registry::RegistryManager::from_config(
                                    reg_config,
                                );
                                info!(
                                    "[Gateway] Skills registry loaded from {}",
                                    skills_config_path.display()
                                );
                                Some(std::sync::Arc::new(rm))
                            }
                            Err(e) => {
                                warn!(
                                    "[Gateway] Failed to parse skills config: {} — skills search/install disabled",
                                    e
                                );
                                None
                            }
                        }
                    }
                    Err(e) => {
                        warn!(
                            "[Gateway] Failed to read skills config: {} — skills search/install disabled",
                            e
                        );
                        None
                    }
                }
            } else {
                info!(
                    "[Gateway] No skills config found at {} — skills search/install disabled",
                    skills_config_path.display()
                );
                None
            }
        };

        // Create MemoryManager (still needed for web server injection).
        // Memory tool executor creation is now handled by the factory function.
        #[cfg(feature = "memory")]
        {
            if cfg.memory.as_ref().map(|m| m.enabled).unwrap_or(false) {
                let memory_data_dir = home.join("workspace").join("memory_vector");
                let config_dir = home.join("workspace").join("config");
                let mgr =
                    std::sync::Arc::new(nemesis_memory::manager::MemoryManager::with_config_dir(
                        &memory_data_dir,
                        &config_dir,
                    ));
                info!(
                    "[Gateway] Memory manager created (data_dir={})",
                    memory_data_dir.display()
                );
                memory_manager_for_web = Some(mgr);
            } else {
                info!("[Gateway] Enhanced memory disabled (config.json: memory.enabled = false)");
            }
        }

        // Web search config: compute for reference, but tool registration is handled by factory.
        {
            let web = &cfg.tools.web;
            let any_enabled = web.brave.enabled || web.duckduckgo.enabled || web.perplexity.enabled;
            if any_enabled {
                info!(
                    "[Gateway] Web search enabled (brave={}, duckduckgo={}, perplexity={})",
                    web.brave.enabled, web.duckduckgo.enabled, web.perplexity.enabled
                );
            } else {
                info!(
                    "[Gateway] Web search disabled (no provider enabled in config.json: tools.web)"
                );
            }
        }

        // SharedToolConfig construction, tool registration, and MCP reload are now handled
        // by the factory function (agent_factory.rs build_agent_loop()).

        if !mcp_enabled {
            info!("[Gateway] MCP disabled in config.json (mcp.enabled = false), skipping");
        }
        info!(
            "[Gateway] Agent loop tools configured (default + memory + skills + hardware + exec + cron{})",
            if mcp_enabled { " + MCP" } else { "" }
        );

        // Step 9b: Create DataStore for usage statistics（E1 二期：前移到集群回调
        // 装配点之前——peer_chat_callback 闭包要捕获它，把 worker 回传的 usage
        // 记入 master 用量账本）
        let data_store = {
            let data_dir = nemesis_path::workspace_data_dir(&home);
            let db_path = data_dir.join("nemesisbot_data.db");
            match nemesis_data::DataStore::open(&db_path) {
                Ok(store) => {
                    info!("[Gateway] DataStore opened at {}", db_path.display());
                    Some(Arc::new(store))
                }
                Err(e) => {
                    warn!("[Gateway] Failed to open DataStore: {e}, usage statistics disabled");
                    None
                }
            }
        };

        Ok(Some(GatewayCtx {
            // 身份与配置
            home,
            config_path,
            config_store,
            cfg,
            resolution,
            model_name,
            // 消息主干
            bus,
            cron_service,
            conv_router,
            estop,
            data_store,
            agent_outbound_tx,
            bridge_outbound_handle,
            mcp_enabled,
            // PB-1 建栈单例
            skills_loader_arc,
            skills_registry_arc,
            #[cfg(feature = "board")]
            board_store,
            #[cfg(feature = "memory")]
            memory_manager_for_web,
            #[cfg(feature = "forge")]
            forge_for_web,
            #[cfg(feature = "forge")]
            forge_executor_for_tools,
            #[cfg(feature = "workflow")]
            workflow_engine,
            #[cfg(feature = "workflow")]
            workflow_tool_registry,
            #[cfg(feature = "workflow")]
            chat_secret_store,
            // 晚绑定槽
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_moderator_loop,
            #[cfg(all(feature = "board", feature = "cluster"))]
            autopilot_cluster_slot,
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_asset_url_slot,
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_quota,
            #[cfg(any(feature = "workflow", feature = "security"))]
            llm_provider,
        }))
    }
}
