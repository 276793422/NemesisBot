// ---------------------------------------------------------------------------
// PB-8/PB-9 运行期与关停（计划 §4.2 B7）：Step 18–24（cron arm（BUG #49 调序
// 后）/bot 服务/展示 URL/启动横幅/审批接线/question broker/内部命令泵/
// uploads sweeper/Ctrl+C/托盘装配/wait_for_shutdown/统一善后 teardown）整体
// 迁入 `run_runtime`。原文自 run() 逐字迁入，仅三处机械改写：fn 体包裹、
// ctx 依赖影子重绑、门控三件入参解包。
//
// 终相无产物：run() 在本调用后即 Ok(()) 返回，运行期对象全部按值移入
//（agent_loop/agent_adapter/projects_manager/shared_resources/web_display_
// host/real_port/svc_mgr/web_handle/internal_cmd_rx 无门）；security_plugin
// @security/health_server@health/cluster_adapter@cluster(mut，Step 24 take)
// 经 `RuntimeHandoff` 传入——调用点不支持逐参 cfg 属性，字段双臂声明
//（AgentWiring.security_plugin 同款：真身随门 + 空桩保全组合可构造）。
// ctx 侧 home/cfg/bus/cron_service/model_name 克隆 + bridge_outbound_handle
// 取引用（JoinHandle 非 Clone，仅 Step 24 .abort()）+ forge_for_web@forge/
// llm_provider@any(workflow,security) 克隆。注释死码 dispatch_handle.abort()
// 原样随迁（计划 §8：是否删除另行拍板）。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use nemesis_services::LifecycleService;
// `error!` 唯一调用点在 security 门控块内（tasks.db 审计日志装配失败分支）
// ——security 关态下随块退出编译，导入同门（trim 态 unused-import 清零）。
#[cfg(feature = "security")]
use tracing::error;
use tracing::{info, warn};

// M5（2026-10-10 集群无人值守硬化）：RPC 下行 handler 闭包内直调 trait 方法
// （approval.respond/pending 捕获 web_mgr）。
#[cfg(all(feature = "cluster", feature = "security"))]
use nemesis_types::agent::ApprovalResponder;

use super::GatewayCtx;
#[cfg(feature = "security")]
use super::GatewayLlmJudge;
#[cfg(all(feature = "memory", feature = "security"))]
use super::GatewayMemoryGate;
use super::count_enabled_channels;
use super::print_gateway_banner;
#[cfg(not(target_os = "android"))]
use super::trigger_global_shutdown;
#[cfg(all(feature = "desktop", not(target_os = "android")))]
use super::{open_browser, open_plugin_window};
use crate::common;

/// B7（2026-10-13）：guardian judge 模型通道三级阶梯（单一真相源，原内联
/// match 抽取 + guardian_model 前置级）。优先级：`agents.guardian_model`
/// （审计专用——审计点独立于 /compact 杂务，可单独选档）→
/// `agents.small_model`（杂务通道，/compact 先例）→ 主模型。各级未配置 /
/// 解析失败 / provider 构造失败 = 提示 + 落下一级，绝不阻断启动。返回
/// (provider, model, 来源描述)。
#[cfg(feature = "security")]
fn resolve_guardian_judge_lane(
    cfg: &nemesis_config::Config,
    home: &std::path::Path,
    main_provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    main_model: &str,
) -> (
    Arc<dyn nemesis_providers::router::LLMProvider>,
    String,
    String,
) {
    // 单级尝试：model_ref → resolve → FactoryConfig → create_provider。
    // Err = 该级不可用的原因（调用方落下一级；两类失败消息在级内分别 warn）。
    fn try_rung(
        cfg: &nemesis_config::Config,
        home: &std::path::Path,
        model_ref: &str,
    ) -> std::result::Result<(Arc<dyn nemesis_providers::router::LLMProvider>, String), String>
    {
        let resolution = nemesis_config::resolve_model_config(cfg, model_ref)
            .map_err(|e| format!("not resolvable ({e})"))?;
        let judge_factory_cfg = nemesis_providers::factory::FactoryConfig {
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
        let provider = nemesis_providers::factory::create_provider(&judge_factory_cfg)
            .map_err(|e| format!("provider create failed ({e})"))?;
        Ok((provider, resolution.model_name))
    }

    // 级 1：guardian_model 专用通道。
    if let Some(guardian_ref) = cfg
        .agents
        .guardian_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match try_rung(cfg, home, guardian_ref) {
            Ok((provider, model)) => {
                return (provider, model, format!("guardian model '{guardian_ref}'"));
            }
            Err(e) => warn!(
                "[Gateway] Guardian judge: agents.guardian_model '{}' {e}; falling to agents.small_model ladder",
                guardian_ref
            ),
        }
    }

    // 级 2：small_model 杂务通道。
    if let Some(small_ref) = cfg
        .agents
        .small_model
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        match try_rung(cfg, home, small_ref) {
            Ok((provider, model)) => {
                return (provider, model, format!("small model '{small_ref}'"));
            }
            Err(e) => warn!(
                "[Gateway] Guardian judge: agents.small_model '{}' {e}; falling back to the main model",
                small_ref
            ),
        }
    }

    // 级 3：主模型回落（两级均未配置才 info 提示；失败落下的已有各自 warn）。
    if cfg.agents.guardian_model.is_none() && cfg.agents.small_model.is_none() {
        info!(
            "[Gateway] Guardian judge: agents.guardian_model/agents.small_model not configured; using the main model (set agents.guardian_model to keep audit cost off the main model)"
        );
    }
    (
        main_provider,
        main_model.to_string(),
        "main model".to_string(),
    )
}

/// PB-8/PB-9 门控三件入参打包（§4.2 B7）：字段双臂声明——真身随 feature
/// 门，@not 臂空桩（Option<()> / ()）保持结构体全组合可构造；体内消费点
/// 均处同门块内（含 Step 24 善后）。
pub(crate) struct RuntimeHandoff {
    #[cfg(feature = "security")]
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub security_plugin: Option<()>,
    #[cfg(feature = "health")]
    pub health_server: Arc<nemesis_health::server::HealthServer>,
    #[cfg(not(feature = "health"))]
    #[allow(dead_code)]
    pub health_server: (),
    #[cfg(feature = "cluster")]
    pub cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>>,
    #[cfg(not(feature = "cluster"))]
    #[allow(dead_code)]
    pub cluster_adapter: (),
    /// WS4（P13）：技能装前审批门（LateWebSkillsGate 槽；run_runtime 审批块
    /// bind 真身 WebApprovalManager）。@not(security) 臂空桩——无 security
    /// 构建无审批面，AppState 槽保持 None（= AlwaysAllow 语义）。
    #[cfg(feature = "security")]
    pub skills_install_gate: Option<Arc<crate::web_approval::LateWebSkillsGate>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub skills_install_gate: (),
    /// W5-3：插件安装审批门（LateInstallApprover 槽；init_agent 建、installer
    /// 持有；run_runtime 审批块 bind 真身）。@not 臂空桩同上。
    #[cfg(feature = "plugins-wasm")]
    pub plugin_install_gate: Option<Arc<crate::plugin_bridge::LateInstallApprover>>,
    #[cfg(not(feature = "plugins-wasm"))]
    #[allow(dead_code)]
    pub plugin_install_gate: (),
}

/// Step 18–24 运行期与关停（计划 §4.2 B7）。
#[cfg_attr(
    not(any(feature = "security", feature = "health", feature = "cluster")),
    allow(unused_variables)
)]
pub(crate) async fn run_runtime(
    ctx: &GatewayCtx,
    agent_loop: Arc<nemesis_agent::r#loop::AgentLoop>,
    agent_adapter: Arc<crate::adapters::AgentLoopServiceAdapter>,
    projects_manager: Arc<crate::projects::manager::ProjectLoopManager>,
    shared_resources: Arc<crate::agent_factory::SharedResources>,
    web_display_host: String,
    real_port: i64,
    svc_mgr: Arc<nemesis_services::ServiceManager>,
    web_handle: tokio::task::JoinHandle<()>,
    internal_cmd_rx: tokio::sync::mpsc::Receiver<nemesis_web::internal::InternalCommand>,
    runtime_handoff: RuntimeHandoff,
) -> Result<()> {
    // ctx 影子重绑（B1 同款）：门控取各名字在本函数内的实际消费门。
    let home = ctx.home.clone();
    let cfg = ctx.cfg.clone();
    let bus = ctx.bus.clone();
    let cron_service = ctx.cron_service.clone();
    #[cfg(feature = "security")]
    let model_name = ctx.model_name.clone();
    let bridge_outbound_handle = &ctx.bridge_outbound_handle;
    #[cfg(feature = "forge")]
    let forge_for_web = ctx.forge_for_web.clone();
    #[cfg(any(feature = "workflow", feature = "security"))]
    let llm_provider = ctx.llm_provider.clone();
    // 门控三件解包影子：名与原局部一致（体逐字），门随消费点。
    #[cfg(feature = "security")]
    let security_plugin = runtime_handoff.security_plugin;
    #[cfg(feature = "security")]
    let skills_install_gate = runtime_handoff.skills_install_gate;
    #[cfg(feature = "health")]
    let health_server = runtime_handoff.health_server;
    #[cfg(feature = "cluster")]
    let mut cluster_adapter = runtime_handoff.cluster_adapter;

    // 签名验证启动自验 → 审计链一条（接入计划 §4；Layer 8 Merkle
    // append-only——事后改日志藏不住）。warn 失败进程仍活着，此处如实记
    // denied；enforce 失败活不到这里（main 阶段已 exit 86）。
    #[cfg(feature = "security")]
    if let Some(plugin) = security_plugin.as_ref()
        && let Some(chain) = plugin.audit_chain()
        && let Some(sc) = crate::verify_policy::start_check()
    {
        let (state, detail, key_fp) = match &sc.outcome {
            Some(o) => (
                o.state.as_str().to_string(),
                o.detail.as_str().to_string(),
                o.key_fp.clone(),
            ),
            None => match (sc.degraded, sc.anchor_fp) {
                (true, _) => (
                    "NotRun".to_string(),
                    "无信任锚——验证降级 off".to_string(),
                    None,
                ),
                (false, _) => (
                    "NotRun".to_string(),
                    "security.signature_verify=off——验签跳过".to_string(),
                    None,
                ),
            },
        };
        let exe = std::env::current_exe()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_else(|_| "current_exe".into());
        let decision = if state == "Valid" {
            "allowed"
        } else {
            "denied"
        };
        let reason = format!(
            "mode={} locked={} anchor={} {}",
            sc.mode.as_str(),
            sc.locked,
            sc.anchor_fp.unwrap_or("none"),
            detail
        );
        if let Err(e) = chain.append(
            "startup_self_verify",
            "self_verify",
            "system",
            "local",
            &exe,
            decision,
            &reason,
        ) {
            warn!("[Gateway] 签名验证审计链记录失败（非致命）: {}", e);
        }
        // 正式日志补发：自验发生在 lazy logger 装配前（main 阶段只有
        // eprintln），这里给 gateway 日志一份带模式的汇总。
        info!(
            "[Gateway] 签名验证启动自验: mode={} locked={} anchor={} result={}{}",
            sc.mode.as_str(),
            sc.locked,
            sc.anchor_fp.unwrap_or("none"),
            state,
            key_fp
                .as_deref()
                .map(|k| format!(" key_fp={k}"))
                .unwrap_or_default(),
        );
    }

    // Step 18: Arm cron（原 agent_adapter.start() 位置，BUG #49 调序后）
    // 此时序不变量全部就位：agent 已订阅 bus（Step 14b）+ web 已 bind 且
    // gateway state 已写盘（上方 Step 17 尾部）——armed 后第一个 tick fire
    // 的 overdue job，其消息有订阅者接、deliver=true 的回复有 web channel 投。
    // fresh-process 保护语义不变：arm 之前的一切启动流程仍在 disarm 下跑。
    {
        let cron = cron_service.lock().unwrap();
        cron.arm();
    }

    // Step 19: Start bot service (for state tracking)
    if let Err(e) = svc_mgr.start_bot() {
        warn!("[Gateway] Bot service start note: {}", e);
        // Non-fatal: the real services are already started above
    }

    // Step 20: Compute display URLs (real_port already resolved via oneshot in Step 17)
    let _web_url = format!("http://{}:{}", web_display_host, real_port);
    let _chat_url = format!("http://{}:{}/chat/", web_display_host, real_port);

    // Step 21: Print startup banner
    let enabled_channels = count_enabled_channels(&cfg);
    print_gateway_banner(
        &web_display_host,
        real_port,
        &cfg.channels.web.auth_token,
        enabled_channels,
        &cfg.gateway.host,
        cfg.gateway.port,
    );

    // Verify web server is listening
    let listen_addr = format!("{}:{}", web_display_host, real_port);
    println!("  Checking web server on {}...", listen_addr);
    match tokio::net::TcpStream::connect(&listen_addr).await {
        Ok(_) => println!("  OK Web server is listening"),
        Err(e) => println!("  WARNING: Web server not yet listening: {}", e),
    }

    // Mark as ready (mirrors Go's automatic readiness after HTTP server starts)
    #[cfg(feature = "health")]
    {
        health_server.set_ready(true);
    }

    // Create and start ProcessManager for plugin window lifecycle + dedup
    #[cfg(feature = "desktop")]
    let process_manager = Arc::new(nemesis_desktop::process::ProcessManager::new());
    #[cfg(feature = "desktop")]
    {
        if let Err(e) = process_manager.start().await {
            warn!(
                "[Gateway] ProcessManager start note: {} (non-fatal, plugin windows will use fallback)",
                e
            );
        } else {
            info!(
                "[Gateway] ProcessManager started (WS server on port {})",
                process_manager.ws_port()
            );
        }
    }

    // Wire up ApprovalManager: WebApprovalManager → SecurityPlugin auditor
    // M7（devtool-upgrade 阶段 5）：审批交互同构替换为 Dashboard 审批卡——
    // auditor "ask" 规则触发时广播 SSE `approval-requested`，用户在
    // Dashboard 点批准/拒绝 → WSAPI `approval.respond` → mpsc 解除阻塞。
    // 全平台可用（desktop WebView 内嵌同一 Dashboard，天然生效），因此
    // 装配移出 desktop cfg 门。恢复弹窗方案：ApprovalPopupAdapter 保留
    // （#[allow(dead_code)]，见其头注释）。
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = security_plugin {
            let auditor = plugin.auditor();
            // F3: 审批记忆规则表热载器（auditor 查询侧 + 审批卡写入侧共用
            // 同一磁盘文件 `<workspace>/config/approval_rules.json`）。
            let approval_rules_path = nemesis_path::resolve_approval_rules_path_in_workspace(
                &shared_resources.workspace_dir(),
            );
            let approval_rules_hot = Arc::new(nemesis_config::HotReloader::new(
                approval_rules_path.clone(),
                nemesis_security::approval_rules::load_rules,
            ));
            auditor.set_approval_rules(approval_rules_hot);
            let web_mgr = Arc::new(crate::web_approval::WebApprovalManager::new(
                shared_resources.agent_event_tx.clone(),
                Some(approval_rules_path),
            ));
            // K4 (b)（devtool-upgrade 阶段 7）：IM 通道审批卡 + 组合分流——
            // web/无上下文 → web_mgr（Dashboard 卡片，现状不变）；IM 通道
            // → ChannelApprovalManager（审批卡回发起对话 + /approve|/deny
            // 回执）。skill_manage / memory gate / responder 桥仍指 web_mgr
            // （那三处是 dashboard 语义），只有 auditor 的 manager 换组合。
            let channel_mgr = Arc::new(crate::channel_approval::ChannelApprovalManager::new(
                bus.clone(),
            ));
            // watcher 装配走 spawn_watcher（订阅先于 spawn）：任务内订阅
            // 存在回执丢失窗口——窗口内的 /approve 被 broadcast 静默丢弃，
            // 用户批复等满超时被误拒（2026-09-08 全量实证根因）。
            channel_mgr.spawn_watcher();
            // P1 审批台账：tasks.db 打开一次，web + IM 两个审批 broker 共享
            // 同库同幂等契约（重启补裁「可裁决且只生效一次」）。开失败 =
            // 台账关闭（审批照常内存态工作，仅丢重启补裁能力）——响亮
            // error 留痕，不静默假装持久化。
            match nemesis_taskstore::TaskDb::open(
                &nemesis_path::resolve_tasks_db_path_in_workspace(
                    &shared_resources.workspace_dir(),
                ),
            ) {
                Ok(db) => {
                    let journal: Arc<dyn nemesis_types::approval_journal::ApprovalJournal> =
                        Arc::new(nemesis_taskstore::SqliteApprovalJournal::from_db(db));
                    web_mgr.set_journal(journal.clone());
                    channel_mgr.set_journal(journal);
                    info!("[Gateway] approval journal armed (tasks.db; restart-resume enabled)");
                }
                Err(e) => {
                    error!(
                        "[Gateway] tasks.db unavailable, approval journal disabled (approvals stay memory-only, restart-resume lost): {e}"
                    );
                }
            }
            let composite = crate::channel_approval::CompositeApprovalManager::new(
                web_mgr.clone(),
                channel_mgr,
            );
            // M5-3（2026-10-10 机制加固）：cluster 通道 ask 看板注记——
            // worker 执行看板任务的审批等待向 master 权威 issue 线程投系统
            // 评论（路由侧已在 Composite 内改走默认管理器=本节点 Dashboard
            // SSE 卡）。无集群 / adapter 缺席 = 不装（路由修复仍生效）。
            #[cfg(feature = "cluster")]
            if let Some(ref ca) = cluster_adapter {
                let ask_cluster = ca.cluster().clone();
                composite.set_board_notifier(std::sync::Arc::new(
                    move |notice: &crate::channel_approval::BoardAskNotice| {
                        crate::cluster_agent::spawn_board_ask_notice(
                            ask_cluster.clone(),
                            &notice.chat_id,
                            &notice.request_id,
                            &notice.operation,
                            &notice.target,
                            &notice.risk_level,
                            &notice.reason,
                            notice.timeout_secs,
                        );
                        // M5（2026-10-10 集群无人值守硬化）：跨节点批复桥——
                        // ask 上行登记 master（远程批复入口的数据源）。
                        crate::remote_approval::spawn_remote_ask_register(
                            ask_cluster.clone(),
                            notice,
                        );
                    },
                ));
                info!("[Gateway] Board ask notifier armed (cluster approval visibility)");
            }
            let adapter: Arc<dyn nemesis_security::auditor::ApprovalManager> = Arc::new(composite);
            // M5（2026-10-10 集群无人值守硬化）：跨节点批复桥 worker 侧装配
            // ——注册 RPC 下行 handler（master 转发的批复直达本节点 pending，
            // always 兜底钉死 false：规则记忆只允许本地 Dashboard 显式落盘）
            // + Dashboard 卡通路换装饰器（本节点裁决生效后上行 ask_closed
            // 同步 master 关卡）。无集群 / adapter 缺席 = 不装（本地 Dashboard
            // 批复不受影响）。
            #[cfg(feature = "cluster")]
            let responder: Arc<dyn nemesis_types::agent::ApprovalResponder> = if let Some(ref ca) =
                cluster_adapter
            {
                let ask_cluster = ca.cluster().clone();
                let respond_mgr = web_mgr.clone();
                match ask_cluster.register_rpc_handler(
                    nemesis_web::remote_approval::ACTION_APPROVAL_RESPOND,
                    Box::new(move |payload| {
                        let request_id = payload
                            .get("request_id")
                            .and_then(|v| v.as_str())
                            .unwrap_or("")
                            .to_string();
                        if request_id.is_empty() {
                            return Err("missing request_id".to_string());
                        }
                        let approved = payload
                            .get("approved")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let note = payload
                            .get("note")
                            .and_then(|v| v.as_str())
                            .map(String::from);
                        let delivered = respond_mgr.respond(&request_id, approved, false, note)?;
                        Ok(serde_json::json!({
                            "delivered": true,
                            "approved": delivered,
                        }))
                    }),
                ) {
                    Ok(()) => {
                        info!("[Gateway] approval.respond handler armed (remote verdict downlink)")
                    }
                    Err(e) => warn!("[Gateway] approval.respond handler note: {e}"),
                }
                let pending_mgr = web_mgr.clone();
                match ask_cluster.register_rpc_handler(
                    nemesis_web::remote_approval::ACTION_APPROVAL_PENDING,
                    Box::new(move |_payload| {
                        Ok(serde_json::json!({ "pending": pending_mgr.pending() }))
                    }),
                ) {
                    Ok(()) => {
                        info!("[Gateway] approval.pending handler armed (remote surface)")
                    }
                    Err(e) => warn!("[Gateway] approval.pending handler note: {e}"),
                }
                // connectors 关 = 远程执行面注册段整体退出编译（E1 收官批
                // 2026-10-15 的 worker handler 随 nemesis-connectors 进出）。
                #[cfg(feature = "connectors")]
                {
                    // E1（收官批 2026-10-15）：远程执行面 worker handler——master
                    // 派发的 connector_invoke 到达本节点后走本地 handle_tool_call
                    // 完整 dispatch 瀑布（estop/hidden/Plan/安全 8 层/审批卡；
                    // confirm=required 资金单在审批点阻塞、超时自动拒——远程
                    // 调用不可能静默放行）。凭据铁律：payload 无凭据（master 已
                    // 剥离且永不上 RPC 线），凭据注入由本节点 vault 在管线内
                    // 完成。同步×异步桥：dispatch_wire_request 的 handler 同步
                    // 内联执行于 runtime worker——注册时捕获 Handle +
                    // block_in_place（gateway multi-thread runtime；acp.rs
                    // AcpApprovalManager 同款样板）。
                    let invoke_loop = agent_loop.clone();
                    let invoke_ws = shared_resources.workspace_dir();
                    let invoke_home = home.clone();
                    let invoke_handle = tokio::runtime::Handle::current();
                    match ask_cluster.register_rpc_handler(
                        crate::connector_tools::INVOKE_TOOL_NAME,
                        Box::new(move |payload| {
                            // ① 运行闸：config 现读（缺省 true；false = 诚实拒，
                            // 不做静默放行）。
                            let cfg_path = invoke_home.join("config.json");
                            let enabled = nemesis_config::load_config(&cfg_path)
                                .ok()
                                .and_then(|c| c.connectors)
                                .map(|c| c.remote_invoke_enabled)
                                .unwrap_or(true);
                            if !enabled {
                                return Err(
                                    "远程执行面已关闭（connectors.remote_invoke_enabled=false）"
                                        .to_string(),
                                );
                            }
                            // ② 载荷解析（缺键诚实拒）。
                            let connector = payload
                                .get("connector")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let op = payload
                                .get("op")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let op_args = payload
                                .get("args")
                                .cloned()
                                .unwrap_or(serde_json::json!({}));
                            let session_key = payload
                                .get("session_key")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if connector.is_empty() || op.is_empty() {
                                return Err("payload 缺 connector/op".to_string());
                            }
                            let connector_for_readback = connector.clone();
                            let op_for_readback = op.clone();
                            let from = payload
                                .get("_rpc")
                                .and_then(|r| r.get("from"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            let started = chrono::Utc::now().to_rfc3339();
                            let digest = nemesis_connectors::receipt::args_digest(&op_args);
                            // ③ 完整 dispatch 瀑布。返回值恒为文本（错误以
                            // "Tool error: "/⛔ 前缀形态携带）：有凭条时分类由
                            // master 端凭条 outcome 驱动；dispatch 层拒绝（estop/
                            // hidden/Plan/tier 闸）不落凭条（回执面诚实不造假 =
                            // null），master 按 ⛔ 前缀识别「未执行」镜像 error。
                            let result_text = tokio::task::block_in_place(|| {
                                invoke_handle.block_on(async {
                                    let call = nemesis_agent::types::ToolCallInfo {
                                        id: format!("remote-{}", uuid::Uuid::new_v4()),
                                        name: crate::connector_tools::INVOKE_TOOL_NAME.to_string(),
                                        arguments: serde_json::json!({
                                            "connector": connector,
                                            "op": op,
                                            "args": op_args,
                                        })
                                        .to_string(),
                                    };
                                    let tool_ctx = nemesis_agent::context::RequestContext::new(
                                        "cluster-connector",
                                        &format!("cluster:{from}"),
                                        "remote-master",
                                        &session_key,
                                    );
                                    invoke_loop.handle_tool_call(&call, &tool_ctx).await
                                })
                            });
                            // ④ 凭条回读：op + args_digest + ts 下界匹配——只认
                            // 本次执行新落的凭条；缺失诚实回 null（回执面不
                            // 造假）。ts 均为 chrono RFC3339（UTC +00:00 同格
                            // 式），字符串比较即时间比较。
                            let receipt = nemesis_connectors::receipt::list_receipts(
                                &invoke_ws,
                                20,
                                Some(&connector_for_readback),
                            )
                            .ok()
                            .and_then(|rs| {
                                rs.into_iter().find(|r| {
                                    r.op == op_for_readback
                                        && r.args_digest == digest
                                        && r.ts >= started
                                })
                            });
                            Ok(serde_json::json!({
                                "receipt": receipt,
                                "result_text": result_text,
                            }))
                        }),
                    ) {
                        Ok(()) => {
                            info!(
                                "[Gateway] connector_invoke handler armed (remote execution surface)"
                            )
                        }
                        Err(e) => warn!("[Gateway] connector_invoke handler note: {e}"),
                    }
                }
                Arc::new(crate::remote_approval::RemoteNotifyResponder::new(
                    web_mgr.clone(),
                    ask_cluster,
                ))
            } else {
                web_mgr.clone()
            };
            #[cfg(not(feature = "cluster"))]
            let responder: Arc<dyn nemesis_types::agent::ApprovalResponder> = web_mgr.clone();
            auditor.set_approval_manager(adapter.clone());
            // Bridge the same approval manager to `skill_manage` write approval.
            *shared_resources.approval_slot.write() = Some(adapter.clone());
            // P2: bridge the same approval manager to the agent's memory write/forget
            // gate — agent memory_store/forget now pop up for approval, never
            // bypassed by YOLO/auto. No-op if no memory executor was stashed.
            #[cfg(feature = "memory")]
            {
                // clone：adapter 的最后消费方在此之后（L9 selfupdate 装配块
                // 也要一份——memory feature 开/关两种形态下都取 owned）。
                agent_loop
                    .set_memory_approval_gate(Arc::new(GatewayMemoryGate::new(adapter.clone())));
            }
            // M7: dashboard 审批卡的响应端点经 AgentLoop 的 responder 槽触达
            // （nemesis-web approval handler 读 agent_loop.approval_responder()）。
            agent_loop.set_approval_responder(responder);
            // WS4（P13）：技能装前审批门 bind 真身——web_server AppState 里的
            // LateWebSkillsGate（gateway.rs set_skills_install_gate 注入）经此
            // 挂上同一 web_mgr，技能安装卡与 auditor 审批共用 respond 通路。
            if let Some(gate) = skills_install_gate.as_ref() {
                gate.bind(web_mgr.clone());
                info!("[Gateway] skills install approval gate bound (WS4 P13)");
            }
            // W5-3：插件安装审批门 bind 真身——installer（init_agent 建）里的
            // LateInstallApprover 挂上同一 web_mgr，插件安装卡与 auditor 审批
            // 共用 respond 通路。
            #[cfg(feature = "plugins-wasm")]
            if let Some(gate) = runtime_handoff.plugin_install_gate.as_ref() {
                gate.bind(web_mgr.clone());
                info!("[Gateway] plugin install approval gate bound (W5-3)");
            }
            // X2 (U8 refinement): reflect interactive-approval reachability
            // in the merged context snapshot's `# Runtime Policy` section.
            agent_loop.set_interactive_approval(true);
            info!("[Gateway] Approval manager wired (dashboard web approval, M7)");
            // L9 selfupdate：九步 apply 编排器装配（selfupdate implies
            // security）。审批走同一 adapter（CRITICAL 卡全平台生效，「总是
            // 允许」对 CRITICAL 恒忽略）；审计链走 security_plugin；estop
            // 同源。WSAPI selfupdate.* 与 /api/internal selfupdate_apply 都
            // 从 handlers::selfupdate 的 OnceLock 槽取真身；未装配 = 诚实报。
            #[cfg(feature = "selfupdate")]
            {
                let mgr = Arc::new(nemesis_web::selfupdate::SelfUpdateManager {
                    workspace: shared_resources.workspace_dir(),
                    estop: shared_resources.estop.clone(),
                    approval: adapter.clone(),
                    audit: Some(plugin.clone()),
                    shutdown: Arc::new(|| {
                        // android 无此导入（cfg not(android)）；该平台 apply
                        // 本身运行期诚实拒绝，闭包退化为空操作即可。
                        #[cfg(not(target_os = "android"))]
                        trigger_global_shutdown();
                    }),
                    // R2 进程内单飞闸（与安装目录跨进程锁两层互补）。
                    flight: tokio::sync::Mutex::new(()),
                });
                nemesis_web::handlers::selfupdate::set_manager(mgr);
                info!("[Gateway] selfupdate manager wired (L9)");
            }
            // P5: guardian judge attach — `guardian_mode` 闸（2026-09-16
            // 无上下文 LLM 命令审计，用户拍板默认 off：LLM 审计耗时且贵，
            // 是双刃剑）。off/空/未知值 = 不装配 judge（零 LLM 成本，连旧
            // CRITICAL 审也不跑）；critical = 旧 CRITICAL 全审；high =
            // HIGH+CRITICAL 破坏形态预筛 + LLM 审（消费闸在
            // SecurityPlugin::guardian_should_review 单一决策点）。
            // 模型通道：B7（2026-10-13）三级阶梯——`agents.guardian_model`
            // 审计专用优先 → `agents.small_model` 杂务通道（/compact 先例
            // ——审计点独立于主对话，不烧主模型）→ 主模型回落，绝不阻断
            // 启动（见 resolve_guardian_judge_lane）。
            let guardian_mode = plugin.guardian_mode();
            match guardian_mode.as_str() {
                "critical" | "high" => {
                    let (judge_provider, judge_model, judge_source) =
                        resolve_guardian_judge_lane(&cfg, &home, llm_provider.clone(), &model_name);
                    plugin.set_judge(Arc::new(GatewayLlmJudge {
                        provider: judge_provider,
                        model: judge_model,
                    }));
                    info!(
                        "[Gateway] Guardian LLM judge attached (guardian_mode={}, model={})",
                        guardian_mode, judge_source
                    );
                }
                other => {
                    info!(
                        "[Gateway] Guardian LLM judge NOT attached (guardian_mode={:?}, default off); set \"guardian_mode\": \"critical\"|\"high\" in config.security.json to enable the context-free LLM command audit",
                        other
                    );
                }
            }
        }
    }

    // W7：插件工具面热装 hook——WSAPI wasm.install/uninstall/enable/disable
    // 成功后经此把工具同步进**当前** AgentLoop 注册表（启动装载走
    // agent_factory 8c 的 register_plugin_tools，两路共用同一桥类型与操作
    // 类型映射）。捕获 loop 持有器（与 AppState 同一 Arc，agent 重启自动
    // 跟进）而非裸 Weak；None = 停机窗口，重启路径会从注册表重新拉起，
    // 事件丢弃是诚实降级。同步一律按 `plugin.<slug>.` 前缀整面摘除再按需
    // 注册（2026-09-29 交付审查 1.3：upgrade 换名时只摘同名会留旧名僵尸
    // 工具 + declare 泄漏）。放在 security 装配块之外：hook 注入不依赖
    // 审批 manager，security.enabled=false 只是跳过审批装配，工具面热
    // 同步必须照常工作（2026-09-29 交付审查 3.1）；plugins-wasm feature
    // implies security，nemesis_security 引用编译期恒可用。
    #[cfg(feature = "plugins-wasm")]
    if let Some(ref pm) = shared_resources.plugin_manager {
        let loop_ref = agent_adapter.loop_ref_handle();
        let pm_for_hook = pm.clone();
        // 摘旧-插新不是原子序列（tool_names → remove ×N → register），notify
        // 是同步调用、并发 WSAPI 任务可同时进 hook——交错时 register 后的
        // 另一线程 remove 会把新工具摘掉（或双注册同名），`_2` 僵尸后缀
        // 由此而来（2026-09-30 复查 #5）。hook 体无 await，std Mutex 串行
        // 足够；poison 容忍（持锁线程 panic 后锁仍要用，into_inner 放行）。
        let hook_serial = Arc::new(std::sync::Mutex::<()>::new(()));
        let serial_for_hook = hook_serial.clone();
        nemesis_web::handlers::plugins_wasm::set_tool_sync_hook(Arc::new(
            move |ev: nemesis_web::handlers::plugins_wasm::ToolSyncEvent| {
                let _serial = serial_for_hook
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let Some(loop_arc) = loop_ref.read().clone() else {
                    // 停机窗口：事件丢弃是设计语义（重启路径从注册表拉起），
                    // 但要可见——有人在此窗口装了插件却没看到工具时得有线索。
                    tracing::warn!(
                        "[WasmPlugin] tool sync skipped (loop stopped): slug={}",
                        ev.slug
                    );
                    return;
                };
                let prefix = format!("plugin.{}.", ev.slug);
                let stale: Vec<String> = loop_arc
                    .tool_names()
                    .into_iter()
                    .filter(|n| n.starts_with(&prefix))
                    .collect();
                for name in &stale {
                    loop_arc.remove_plugin_tool(name);
                    nemesis_security::types::undeclare_tool_operation(name);
                }
                if !ev.add {
                    if !stale.is_empty() {
                        info!(
                            "[WasmPlugin] tools hot-unregistered ({}): {:?}",
                            stale.len(),
                            stale
                        );
                    }
                    return;
                }
                let Some(meta) = pm_for_hook
                    .get(&ev.slug)
                    .and_then(|reg| reg.tool_meta.clone())
                else {
                    // 决策面判了 add=true（tool+meta 在场）这里却拿不到
                    // meta = 等待/调度窗口内注册表被动了（uninstall 或升级
                    // 中间态）——静默 return 会留下「说装了但没有」的悬案，
                    // warn 留痕（前缀摘除已在上面完成，不会留僵尸）。
                    tracing::warn!(
                        "[WasmPlugin] tool sync: meta gone for slug={} (uninstalled mid-flight?)",
                        ev.slug
                    );
                    return;
                };
                let bridge = Arc::new(crate::plugin_bridge::PluginToolBridge::new(
                    meta,
                    pm_for_hook.clone(),
                ));
                let actual = loop_arc.register_plugin_tool(ev.tool_name.clone(), bridge);
                // 空/未知 operation-type → None = 不声明（dispatch 时未注册
                // 名 fail-closed CRITICAL，与启动装载同一映射同一语义）。
                if let Some(op) =
                    crate::plugin_bridge::PluginToolBridge::map_operation(&ev.operation_type)
                {
                    nemesis_security::types::declare_tool_operation(&actual, op);
                }
                info!("[WasmPlugin] tool hot-registered: {actual}");
            },
        ));
    }

    // F7（devtool-upgrade 阶段 5）：question 工具的 Dashboard 提问 broker。
    // 与审批不同，提问是交互动作不是安全动作——不依赖 security feature，
    // 无 cfg 门（gateway 跑起来就有 Dashboard，提问天然可答）。broker 同时
    // 扮演两个角色：question 工具的阻塞端（SharedResources.question_slot，
    // SharedToolConfig 建槽时已克隆同一 Arc，此处晚填即生效）+ WSAPI
    // question.respond/pending 的响应端（AgentLoop responder 槽）。
    // J5（devtool-upgrade 阶段 6）：同一 Arc 再挂 AgentLoop asker 槽——
    // doom-loop escalation 审批卡（agents.doom_loop_approval，默认关）与
    // question 工具共用同一提问通路与作答 UI，不新造审批协议。
    {
        let broker = Arc::new(crate::question_broker::WebQuestionBroker::new(
            shared_resources.agent_event_tx.clone(),
        ));
        *shared_resources.question_slot.write() = Some(broker.clone());
        agent_loop.set_question_responder(broker.clone());
        agent_loop.set_question_asker(broker);
        info!(
            "[Gateway] Question broker wired (dashboard question card, F7 + doom-loop approval, J5)"
        );
    }

    // Internal command loop: /api/internal → open_plugin_window / open_browser
    {
        #[cfg(all(feature = "desktop", not(target_os = "android")))]
        let pm = Arc::clone(&process_manager);
        #[cfg(all(feature = "desktop", not(target_os = "android")))]
        let open_dash_cfg_dir = nemesis_path::workspace_config_dir(&common::workspace_path(&home));
        let url = format!("http://{}:{}", web_display_host, real_port);
        let token = cfg.channels.web.auth_token.clone();
        let mut rx = internal_cmd_rx;
        // (BUG #31, quality-hardening goal 冲刺 S11e) `nemesisbot shutdown`
        // 的 HTTP 臂经 POST /api/internal {"cmd":"shutdown"} 落到这里的
        // Shutdown 变体：与托盘 Quit 同源的优雅停机——置全局标志后调用
        // ServiceManager.shutdown()，其 broadcast 让 Step 23 的
        // wait_for_shutdown 返回，进入 Step 24 统一善 teardown。
        let shutdown_svc_internal = Arc::clone(&svc_mgr);
        tokio::spawn(async move {
            while let Some(cmd) = rx.recv().await {
                match cmd {
                    nemesis_web::internal::InternalCommand::OpenDashboard => {
                        #[cfg(all(feature = "desktop", not(target_os = "android")))]
                        {
                            // 原生插件闸：plugin_ui（WebView 窗宿主）。事件驱动
                            // 裁决——开关翻动对本入口即时生效（托盘面仍以启动
                            // 装配为准）。停用/拦截 = 诚实日志 + 不开窗（浏览器
                            // 访问 Dashboard 不受影响）。Allow.path 允许丢弃：
                            // DLL 由子进程 child_mode 以同源
                            // find_plugin_library_in 查找加载（gateway 侧无从
                            // 定点），闸验对象与装载对象恒同文件（native_plugins
                            // 模块文档两档契约第二档）。
                            match crate::native_plugins::gate_load(
                                &open_dash_cfg_dir,
                                nemesis_config::plugins_native::NativePluginId::PluginUi,
                            ) {
                                crate::native_plugins::GateOutcome::Allow { .. } => {
                                    info!("[Gateway] Internal command: open_dashboard");
                                    let _ = open_plugin_window(&pm, "dashboard", &url, &token);
                                }
                                crate::native_plugins::GateOutcome::DisabledBySwitch => info!(
                                    "[Gateway] open_dashboard 忽略：plugin_ui 已停用（本地插件库开关）"
                                ),
                                crate::native_plugins::GateOutcome::BlockedBySignature(d) => {
                                    tracing::warn!(
                                        "[Gateway] open_dashboard 忽略：plugin_ui 未通过签名策略（{d}）"
                                    )
                                }
                            }
                        }
                        #[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
                        {
                            let _ = (&url, &token);
                            info!(
                                "[Gateway] Internal command: open_dashboard (no desktop / android)"
                            );
                        }
                    }
                    nemesis_web::internal::InternalCommand::Shutdown => {
                        info!("[Gateway] Internal command: shutdown via /api/internal (BUG #31)");
                        #[cfg(not(target_os = "android"))]
                        trigger_global_shutdown();
                        shutdown_svc_internal.shutdown();
                    }
                    nemesis_web::internal::InternalCommand::SelfUpdateApply { allow_downgrade } => {
                        // L9：CLI `nemesisbot selfupdate apply` 经 /api/internal
                        // 落到这里——受理后 spawn 九步编排（下载 + 审批卡 +
                        // staging 全程分钟级，不能占住监听）。成功路径 apply
                        // 内部已 spawn 新版本并自带优雅停机线程，此处不再动
                        // svc_mgr（避免双重停机竞态）。
                        #[cfg(feature = "selfupdate")]
                        if let Some(mgr) = nemesis_web::handlers::selfupdate::manager() {
                            let version = crate::common::VERSION_INFO.version;
                            let commit = crate::common::VERSION_INFO.git_commit;
                            tokio::spawn(async move {
                                match mgr.apply(version, commit, allow_downgrade).await {
                                    Ok(msg) => info!("[Gateway] selfupdate apply: {msg}"),
                                    Err(e) => warn!("[Gateway] selfupdate apply 失败：{e}"),
                                }
                            });
                        } else {
                            warn!(
                                "[Gateway] selfupdate apply：manager 未装配（security 未启用？），忽略"
                            );
                        }
                        #[cfg(not(feature = "selfupdate"))]
                        {
                            let _ = allow_downgrade;
                            warn!("[Gateway] selfupdate apply：构建未带 selfupdate feature，忽略");
                        }
                    }
                }
            }
        });
        info!("[Gateway] Internal command listener started");
    }

    // 双击直启（2026-09-17）：无参启动（BARE_LAUNCH env，run_command 归一化
    // 时设置）→ 启动完成自动打开 Dashboard（plugin-ui webview 窗口带 token，
    // 缺 dll 回落浏览器；托盘图标由 Step 22 装配，与本块正交）。显式
    // `nemesisbot gateway` 不带标记——server 语义，不弹窗口。web 已 bind
    // （real_port 已知）；sleep 片刻给前端资源一点启动余量。
    if std::env::var(crate::common::BARE_LAUNCH_ENV).is_ok() {
        #[cfg(all(feature = "desktop", not(target_os = "android")))]
        {
            let pm = Arc::clone(&process_manager);
            let url = format!("http://{}:{}", web_display_host, real_port);
            let token = cfg.channels.web.auth_token.clone();
            tokio::spawn(async move {
                tokio::time::sleep(std::time::Duration::from_millis(800)).await;
                info!("[Gateway] Bare launch: opening dashboard window");
                let _ = open_plugin_window(&pm, "dashboard", &url, &token);
            });
        }
        #[cfg(not(all(feature = "desktop", not(target_os = "android"))))]
        {
            info!("[Gateway] Bare launch detected (no desktop feature: skip dashboard auto-open)");
        }
    }

    // T8（多模态 goal 2026-09-03）：uploads 暂存目录 TTL 清扫（启动扫一次 +
    // 每 6 小时一次，7 天 TTL）。uploads 是 temp 语义；被清扫文件的历史引用
    // 由 T6 水合层诚实降级 `[图片已失效]`，不依赖此处。路径唯一真相源
    // nemesis-path resolve_uploads_dir_in_workspace。
    nemesis_web::handlers::upload::spawn_uploads_sweeper();
    info!("[Gateway] uploads TTL sweeper started (7d TTL, 6h interval)");

    // B11（2026-10-18 snooze）：暂停会话到期唤醒 sweeper（每 10s 扫 meta
    // sidecar；到期摘标记 + 注入恢复 system 行 + 事件广播）。
    nemesis_web::handlers::sessions::spawn_snooze_sweeper();
    info!("[Gateway] snooze sweeper started (10s interval)");

    // Ctrl+C / SIGINT 优雅停机臂（2026-08-31 web-dead 停机事故教训）：
    // 此前 gateway 停机只有两条路——/api/internal（骑 web，web 任务一死即全断）
    // 和托盘 Quit（要求桌面托盘存活）。控制台 Ctrl+C 落 std 默认处置=硬终止，
    // Step 24 善后全部跳过。补一条与托盘 Quit 同源的信号臂。
    // 注意：msys/bash `&` 起的后台子进程继承 SIG_IGN(SIGINT)，该启动方式下
    // 本臂不会触发（Start-Process / 前台控制台启动则有效）。
    #[cfg(not(target_os = "android"))]
    {
        let svc_mgr_signal = Arc::clone(&svc_mgr);
        tokio::spawn(async move {
            if tokio::signal::ctrl_c().await.is_ok() {
                info!("[Gateway] Ctrl+C received, initiating graceful shutdown");
                trigger_global_shutdown();
                svc_mgr_signal.shutdown();
            }
        });
    }

    // Step 22: Configure system tray (desktop only)
    #[cfg(all(feature = "desktop", not(target_os = "android")))]
    'tray: {
        use nemesis_desktop::PlatformTray;

        // 原生插件闸：plugin_ui（托盘 + 审批弹窗 + WebView 窗宿主 DLL）。
        // 停用/签名拦 = 诚实日志 + 整块跳过（装配期一次性消费——开关改动
        // 重启 gateway 生效）。Allow.path 允许丢弃：tray 的 run_via_plugin_ui
        // 与闸共用 find_plugin_library 单一查找（nemesis-desktop systray），
        // 闸验对象与装载对象恒同文件（native_plugins 模块文档两档契约第二档）。
        match crate::native_plugins::gate_load(
            &nemesis_path::workspace_config_dir(&common::workspace_path(&home)),
            nemesis_config::plugins_native::NativePluginId::PluginUi,
        ) {
            crate::native_plugins::GateOutcome::Allow { .. } => {}
            crate::native_plugins::GateOutcome::DisabledBySwitch => {
                info!("[Gateway] plugin_ui 已停用（本地插件库开关）——系统托盘不装配（重启后生效）");
                break 'tray;
            }
            crate::native_plugins::GateOutcome::BlockedBySignature(d) => {
                tracing::warn!("[Gateway] plugin_ui 未通过签名策略（{d}）——系统托盘不装配");
                break 'tray;
            }
        }

        let mut tray = PlatformTray::new();
        // Menu callbacks run on the tray thread (no tokio context). Service
        // callbacks (agent start/stop, cluster start/stop, quit) internally
        // tokio::spawn — hand the tray our runtime so dispatch enters it
        // (Handle::block_on) instead of panicking and killing the tray.
        tray.set_runtime_handle(tokio::runtime::Handle::current());

        // Set cluster callbacks — tray controls both config files + runtime
        #[cfg(feature = "cluster")]
        {
            if let Some(ref ca) = cluster_adapter {
                let home_for_start = home.clone();
                let ca_start = ca.clone();
                tray.set_on_cluster_start(Box::new(move || {
                    // 写串行锁罩两个 config RMW（同类横扫四期，2026-10-13）：
                    // 本回调与 Dashboard 任意 config 写臂（update_config/
                    // update_raw_config/update_json_file 同族）互斥——裸 RMW
                    // 的整份快照写回会冲掉并发写臂的字段，反之亦然。锁块只
                    // 罩文件写段，cluster start() 在锁外（不得长时间占锁）。
                    {
                        let _cfg_serial = nemesis_config::config_write_lock();
                        // Write config.json cluster.enabled = true
                        let cfg_path = home_for_start.join("config.json");
                        if let Ok(content) = std::fs::read_to_string(&cfg_path)
                            && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                        {
                            if cfg.get("cluster").is_none() {
                                cfg["cluster"] = serde_json::json!({});
                            }
                            if let Some(obj) =
                                cfg.get_mut("cluster").and_then(|c| c.as_object_mut())
                            {
                                obj.insert("enabled".to_string(), serde_json::json!(true));
                                if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                                    // REL-002：统一原子写入（cluster enable 开关写主配置）。
                                    let _ = nemesis_utils::write_file_atomic(
                                        &cfg_path.to_string_lossy(),
                                        updated.as_bytes(),
                                        0o600,
                                    );
                                }
                            }
                        }
                        // Write config.cluster.json enabled = true
                        let cluster_cfg_path =
                            nemesis_path::resolve_cluster_config_path_in_workspace(
                                &common::workspace_path(&home_for_start),
                            );
                        if let Ok(content) = std::fs::read_to_string(&cluster_cfg_path)
                            && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                            && let Some(obj) = cfg.as_object_mut()
                        {
                            obj.insert("enabled".to_string(), serde_json::json!(true));
                            if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                                // REL-002：统一原子写入（config.cluster.json 含 token）。
                                let _ = nemesis_utils::write_file_atomic(
                                    &cluster_cfg_path.to_string_lossy(),
                                    updated.as_bytes(),
                                    0o600,
                                );
                            }
                        }
                    }
                    if let Err(e) = ca_start.start() {
                        tracing::warn!("[Gateway] Tray: failed to start cluster: {}", e);
                    }
                }));

                let home_for_stop = home.clone();
                let ca_stop = ca.clone();
                tray.set_on_cluster_stop(Box::new(move || {
                    if let Err(e) = ca_stop.stop() {
                        tracing::warn!("[Gateway] Tray: failed to stop cluster: {}", e);
                    }
                    // 写串行锁罩两个 config RMW（同 start 回调，2026-10-13）。
                    {
                        let _cfg_serial = nemesis_config::config_write_lock();
                        // Write config.cluster.json enabled = false
                        let cluster_cfg_path =
                            nemesis_path::resolve_cluster_config_path_in_workspace(
                                &common::workspace_path(&home_for_stop),
                            );
                        if let Ok(content) = std::fs::read_to_string(&cluster_cfg_path)
                            && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                            && let Some(obj) = cfg.as_object_mut()
                        {
                            obj.insert("enabled".to_string(), serde_json::json!(false));
                            if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                                // REL-002：统一原子写入（config.cluster.json 含 token）。
                                let _ = nemesis_utils::write_file_atomic(
                                    &cluster_cfg_path.to_string_lossy(),
                                    updated.as_bytes(),
                                    0o600,
                                );
                            }
                        }
                        // Write config.json cluster.enabled = false
                        let cfg_path = home_for_stop.join("config.json");
                        if let Ok(content) = std::fs::read_to_string(&cfg_path)
                            && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                            && let Some(obj) =
                                cfg.get_mut("cluster").and_then(|c| c.as_object_mut())
                        {
                            obj.insert("enabled".to_string(), serde_json::json!(false));
                            if let Ok(updated) = serde_json::to_string_pretty(&cfg) {
                                // REL-002：统一原子写入。
                                let _ = nemesis_utils::write_file_atomic(
                                    &cfg_path.to_string_lossy(),
                                    updated.as_bytes(),
                                    0o600,
                                );
                            }
                        }
                    }
                }));
            }
        }

        let start_adapter = Arc::clone(&agent_adapter);
        tray.set_on_start(Box::new(move || {
            if let Err(e) = start_adapter.start() {
                tracing::warn!("[Gateway] Tray: failed to start agent: {}", e);
            }
        }));

        let stop_adapter = Arc::clone(&agent_adapter);
        tray.set_on_stop(Box::new(move || {
            if let Err(e) = stop_adapter.stop() {
                tracing::warn!("[Gateway] Tray: failed to stop agent: {}", e);
            }
        }));

        // E-stop / release: tray is in-process, capture the same EstopState Arc
        // the agent loop reads (shared_resources.estop). trigger()/release() are
        // &self on a thread-safe AtomicBool+watch, safe to call from the tray thread.
        let estop_engage = Arc::clone(&shared_resources.estop);
        tray.set_on_estop(Box::new(move || {
            estop_engage.trigger();
            tracing::info!("[Gateway] Tray: e-stop engaged");
        }));

        let estop_release = Arc::clone(&shared_resources.estop);
        tray.set_on_release(Box::new(move || {
            estop_release.release();
            tracing::info!("[Gateway] Tray: e-stop released");
        }));

        let pm = Arc::clone(&process_manager);
        let dashboard_url = _web_url.clone();
        let dashboard_token = cfg.channels.web.auth_token.clone();
        tray.set_on_open_dashboard(Box::new(move || {
            let _ = open_plugin_window(&pm, "dashboard", &dashboard_url, &dashboard_token);
        }));

        let chat_url = _chat_url.clone();
        tray.set_on_open_chat(Box::new(move || {
            let _ = open_browser(&chat_url);
        }));

        let shutdown_svc = Arc::clone(&svc_mgr);
        tray.set_on_quit(Box::new(move || {
            trigger_global_shutdown();
            shutdown_svc.shutdown();
        }));

        // Start the tray.
        //
        // Windows: runs on a dedicated thread (winit allows off-main-thread via
        //          with_any_thread). macOS: winit's EventLoop MUST run on the
        //          main thread, so hand the configured tray to the main thread
        //          (see nemesis_desktop::main_thread_handoff) which runs the
        //          event loop there. The gateway itself continues on this worker.
        #[cfg(target_os = "macos")]
        {
            nemesis_desktop::main_thread_handoff::deliver(tray);
        }
        #[cfg(not(target_os = "macos"))]
        {
            let _tray_handle = tray.run();
        }
        info!("[Gateway] System tray started");
        println!("  OK System tray started");
    }

    // Step 23: Wait for shutdown signal
    svc_mgr.wait_for_shutdown().await;

    // Step 24: Graceful shutdown
    println!();
    println!("Shutting down...");
    svc_mgr.shutdown();

    // F-L2: graceful Forge shutdown — flush buffered aggregation + stop the
    // background loops cleanly. Previously the spawn handle was dropped and the
    // loops were killed by runtime teardown (losing unflushed data / mid-write).
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = forge_for_web {
            forge.stop().await;
            info!("[Gateway] Forge stopped cleanly");
        }
    }

    // Cancel active voice sessions and release ONNX engines
    // so spawn_blocking tasks exit before Runtime drop.
    #[cfg(feature = "voice")]
    {
        nemesis_web::handlers::voice::voice_shutdown().await;
    }

    // Stop ProcessManager (terminates all child processes)
    #[cfg(feature = "desktop")]
    {
        if let Err(e) = process_manager.stop() {
            warn!("[Gateway] ProcessManager stop note: {}", e);
        }
    }

    // Stop scanner chain — kills clamd so it doesn't orphan (holds port 3310,
    // breaks next gateway start). SecurityService trait has no stop hook, so we
    // call SecurityPlugin.stop_scanner directly here. scanner feature 关 = 无
    // scan chain（第 7 层诚实短路），无需停。
    #[cfg(feature = "scanner")]
    {
        if let Some(ref plugin) = security_plugin {
            plugin.stop_scanner().await;
            info!("[Gateway] Scanner chain stopped (clamd killed)");
        }
    }

    // Sandbox: leave SbieSvc RESIDENT on exit — do NOT stop it. The gateway
    // runs non-elevated; stopping the privileged SbieSvc shells out to
    // `KmdUtil.exe stop SbieSvc`, which is denied (needs admin) and pops a GUI
    // "no permission" dialog that blocks shutdown. A running SbieSvc is
    // harmless: `ensure_sandbox_ready` reuses it on next start (see
    // commands/sandbox.rs), and the kernel driver is resident-by-design. Use
    // the elevated `sandbox stop` CLI / dashboard button to fully uninstall.
    //
    // DISABLED — original per-run stop call kept here for reference. To
    // re-enable, the stop MUST go through an ELEVATED path (elevation.rs /
    // runas); a non-elevated stop re-introduces the KmdUtil permission popup.
    // #[cfg(feature = "sandbox")]
    // {
    //     crate::commands::sandbox::stop_service_if_ours(&home);
    // }

    // Close the message bus
    bus.close();

    // C5: gracefully close every LSP session (shutdown → exit → kill) so
    // language-server child processes never outlive the gateway. Previously
    // the tool's sessions were only reaped lazily (idle timeout) or via
    // kill_on_drop at process exit — an abrupt teardown could orphan them.
    #[cfg(feature = "lsp")]
    let lsp_closed = shared_resources.lsp_manager.shutdown_all().await;
    #[cfg(feature = "lsp")]
    info!(
        "[Gateway] LSP shutdown: {} language-server session(s) closed",
        lsp_closed
    );

    // Abort background tasks
    web_handle.abort();
    agent_adapter.stop().ok();
    // L6++：项目常驻 loop 收尾（镜像主 agent stop：摘表 + stop + abort 任务）。
    projects_manager.stop_all();
    // G1：自建 agent 常驻 loop 收尾（同款摘表 + stop + abort；manager 槽
    // 现取——teardown 点无局部句柄，未装配形态自然跳过）。
    if let Some(lm) = crate::agents::manager().and_then(|m| m.loop_manager()) {
        lm.stop_all();
    }
    bridge_outbound_handle.abort();
    //  MSG: 同 step 16 ，目前暂时不用，所以注释掉了
    //dispatch_handle.abort();

    // Stop cluster (adapter handles: agent abort, RPC server, discovery, recovery/sync loops)
    #[cfg(feature = "cluster")]
    {
        if let Some(adapter) = cluster_adapter.take() {
            let _ = adapter.stop();
        }
    }

    // Clean up gateway state file
    let _ = std::fs::remove_file(nemesis_path::resolve_gateway_state_path_in_workspace(
        &common::workspace_path(&home),
    ));

    println!("  OK Gateway stopped");

    // macOS: tell the main-thread tray loop to exit now that cleanup is done.
    // Covers shutdown paths that don't go through the tray "Quit" menu item
    // (e.g. Ctrl+C) — without this the main thread would block in the tray
    // event loop forever while the gateway worker had already finished.
    #[cfg(target_os = "macos")]
    nemesis_desktop::main_thread_handoff::request_exit();

    Ok(())
}
