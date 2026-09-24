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
use tracing::{info, warn};

use super::GatewayCtx;
#[cfg(feature = "security")]
use super::GatewayLlmJudge;
#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
use super::GatewayMemoryGate;
use super::count_enabled_channels;
use super::print_gateway_banner;
#[cfg(not(target_os = "android"))]
use super::trigger_global_shutdown;
#[cfg(all(feature = "desktop", not(target_os = "android")))]
use super::{open_browser, open_plugin_window};
use crate::common;

/// PB-8/PB-9 门控三件入参打包（§4.2 B7）：字段双臂声明——真身随 feature
/// 门，@not 臂空桩（Option<()> / ()）保持结构体全组合可构造；体内消费点
/// 均处同门块内（含 Step 24 善后）。
pub(crate) struct RuntimeHandoff {
    #[cfg(feature = "security")]
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    pub security_plugin: Option<()>,
    #[cfg(feature = "health")]
    pub health_server: Arc<nemesis_health::server::HealthServer>,
    #[cfg(not(feature = "health"))]
    pub health_server: (),
    #[cfg(feature = "cluster")]
    pub cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>>,
    #[cfg(not(feature = "cluster"))]
    pub cluster_adapter: (),
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
    let model_name = ctx.model_name.clone();
    let bridge_outbound_handle = &ctx.bridge_outbound_handle;
    #[cfg(feature = "forge")]
    let forge_for_web = ctx.forge_for_web.clone();
    #[cfg(any(feature = "workflow", feature = "security"))]
    let llm_provider = ctx.llm_provider.clone();
    // 门控三件解包影子：名与原局部一致（体逐字），门随消费点。
    #[cfg(feature = "security")]
    let security_plugin = runtime_handoff.security_plugin;
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
            let adapter: Arc<dyn nemesis_security::auditor::ApprovalManager> =
                Arc::new(crate::channel_approval::CompositeApprovalManager::new(
                    web_mgr.clone(),
                    channel_mgr,
                ));
            let responder: Arc<dyn nemesis_types::agent::ApprovalResponder> = web_mgr.clone();
            auditor.set_approval_manager(adapter.clone());
            // Bridge the same approval manager to `skill_manage` write approval.
            *shared_resources.approval_slot.write() = Some(adapter.clone());
            // P2: bridge the same approval manager to the agent's memory write/forget
            // gate — agent memory_store/forget now pop up for approval, never
            // bypassed by YOLO/auto. No-op if no memory executor was stashed.
            #[cfg(feature = "memory")]
            {
                agent_loop.set_memory_approval_gate(Arc::new(GatewayMemoryGate::new(adapter)));
            }
            // M7: dashboard 审批卡的响应端点经 AgentLoop 的 responder 槽触达
            // （nemesis-web approval handler 读 agent_loop.approval_responder()）。
            agent_loop.set_approval_responder(responder);
            // X2 (U8 refinement): reflect interactive-approval reachability
            // in the merged context snapshot's `# Runtime Policy` section.
            agent_loop.set_interactive_approval(true);
            info!("[Gateway] Approval manager wired (dashboard web approval, M7)");
            // P5: guardian judge attach — `guardian_mode` 闸（2026-09-16
            // 无上下文 LLM 命令审计，用户拍板默认 off：LLM 审计耗时且贵，
            // 是双刃剑）。off/空/未知值 = 不装配 judge（零 LLM 成本，连旧
            // CRITICAL 审也不跑）；critical = 旧 CRITICAL 全审；high =
            // HIGH+CRITICAL 破坏形态预筛 + LLM 审（消费闸在
            // SecurityPlugin::guardian_should_review 单一决策点）。
            // 模型通道：`agents.small_model` 杂务通道优先（同 /compact 先
            // 例——审计点独立于主对话，不烧主模型），未配置/解析失败 =
            // 回落主模型 + warn，绝不阻断启动。
            let guardian_mode = plugin.guardian_mode();
            match guardian_mode.as_str() {
                "critical" | "high" => {
                    let (judge_provider, judge_model, judge_source) = match cfg
                        .agents
                        .small_model
                        .as_deref()
                        .map(str::trim)
                        .filter(|s| !s.is_empty())
                    {
                        Some(small_ref) => {
                            match nemesis_config::resolve_model_config(&cfg, small_ref) {
                                Ok(resolution) => {
                                    let judge_factory_cfg =
                                        nemesis_providers::factory::FactoryConfig {
                                            proxy: resolution.proxy.clone(),
                                            llm_ref: format!(
                                                "{}/{}",
                                                resolution.provider_name, resolution.model_name
                                            ),
                                            api_key: resolution.api_key.clone(),
                                            api_base: resolution.api_base.clone(),
                                            workspace: home
                                                .join("workspace")
                                                .to_string_lossy()
                                                .to_string(),
                                            connect_mode: resolution.connect_mode.clone(),
                                            protocol: resolution.protocol.clone(),
                                            timeout_secs: resolution.timeout_secs,
                                            account_id: String::new(),
                                            headers: std::collections::HashMap::new(),
                                        };
                                    match nemesis_providers::factory::create_provider(
                                        &judge_factory_cfg,
                                    ) {
                                        Ok(p) => (
                                            p,
                                            resolution.model_name,
                                            format!("small model '{}'", small_ref),
                                        ),
                                        Err(e) => {
                                            warn!(
                                                "[Gateway] Guardian judge: agents.small_model '{}' provider create failed ({}); falling back to the main model",
                                                small_ref, e
                                            );
                                            (
                                                llm_provider.clone(),
                                                model_name.clone(),
                                                "main model".to_string(),
                                            )
                                        }
                                    }
                                }
                                Err(e) => {
                                    warn!(
                                        "[Gateway] Guardian judge: agents.small_model '{}' not resolvable ({}); falling back to the main model",
                                        small_ref, e
                                    );
                                    (
                                        llm_provider.clone(),
                                        model_name.clone(),
                                        "main model".to_string(),
                                    )
                                }
                            }
                        }
                        None => {
                            info!(
                                "[Gateway] Guardian judge: agents.small_model not configured; using the main model (set agents.small_model to keep audit cost off the main model)"
                            );
                            (
                                llm_provider.clone(),
                                model_name.clone(),
                                "main model".to_string(),
                            )
                        }
                    };
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
                            info!("[Gateway] Internal command: open_dashboard");
                            let _ = open_plugin_window(&pm, "dashboard", &url, &token);
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
    {
        use nemesis_desktop::PlatformTray;

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
                    // Write config.json cluster.enabled = true
                    let cfg_path = home_for_start.join("config.json");
                    if let Ok(content) = std::fs::read_to_string(&cfg_path)
                        && let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&content)
                    {
                        if cfg.get("cluster").is_none() {
                            cfg["cluster"] = serde_json::json!({});
                        }
                        if let Some(obj) = cfg.get_mut("cluster").and_then(|c| c.as_object_mut()) {
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
                    let cluster_cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(
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
                    // Write config.cluster.json enabled = false
                    let cluster_cfg_path = nemesis_path::resolve_cluster_config_path_in_workspace(
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
                        && let Some(obj) = cfg.get_mut("cluster").and_then(|c| c.as_object_mut())
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
    // call SecurityPlugin.stop_scanner directly here.
    #[cfg(feature = "security")]
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
    let lsp_closed = shared_resources.lsp_manager.shutdown_all().await;
    info!(
        "[Gateway] LSP shutdown: {} language-server session(s) closed",
        lsp_closed
    );

    // Abort background tasks
    web_handle.abort();
    agent_adapter.stop().ok();
    // L6++：项目常驻 loop 收尾（镜像主 agent stop：摘表 + stop + abort 任务）。
    projects_manager.stop_all();
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
