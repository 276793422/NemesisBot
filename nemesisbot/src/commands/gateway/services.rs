// ---------------------------------------------------------------------------
// PB-6 服务族（计划 §4.2 B6）：Step 11–17（HealthServer 构建 + Heartbeat
// Service 装配 + 设备监控 + ServiceManager/基础服务 + board autopilot 启动
// 同步 + cron 装配/调度器 + Agent 启动信息 + WebServer 启动/real_port 等待/
// 资产基址落槽与 30s 自愈 + 网关状态落盘 + 反向桥客户端 spawn）整体迁入
// `init_services`。原文自 run() 逐字迁入，仅五处机械改写：fn 体包裹、
// ctx 依赖影子重绑、cluster 三件入参解包、末尾 ServicesWiring 构造。
//
// 签名取 web_server 按可变值（Step 16 set_internal_cmd_tx 需 &mut，Step 17
// `tokio::spawn(async move { ... })` 将其移入 web 任务，调用方此后仅持
// web_handle 句柄）；web_bind_host/web_port/bridge_client_launch/
// initial_tool_count 按值（调用方下游零消费，实测）；agent_adapter/
// web_display_host 取克隆（调用方 Step 18/19/24 仍消费原对象；agent_loop
// 实测非本块消费——Step 18/19 装配走调用方原绑定，不入参）；cluster 三件
//（bridge_cluster_slot/cluster_adapter/cluster_arc_ref）经 `ClusterHandoff`
// 传入——调用点不支持逐参 cfg 属性，
// 以无门结构体 + @cluster 门控字段承载（ClusterWiring 同款先例），体内
// 消费点均在 cluster 门内（含 @not 回退臂），@not(cluster) 传空结构体。
//
// ServicesWiring 字段 = 实测逃逸本块的完整集合（五项）：health_server
//（下游唯一消费 set_ready，@health 门内）+ svc_mgr（Step 19 bot 服务/
// Step 23 停机编排重度消费）+ web_handle（Step 24 abort）+ real_port
//（展示 URL/tray/横幅）+ internal_cmd_rx（web 内部命令接收端，Step 19
// 泵 move 消费——Receiver 非 Clone，体內创建、体外消费，随产物携出）。
// dispatch_handle 为注释死码（创建与 abort 均被注释），不进产物。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use nemesis_services::LifecycleService;
use tracing::{error, info, warn};

use super::GatewayCtx;
use super::print_agent_startup_info;
#[cfg(all(feature = "board", feature = "cluster"))]
use super::{advertise_host_for, select_lan_ip_for_advertisement};
use crate::adapters;
use crate::common;

/// PB-6 cluster 三件入参打包（§4.2 B6）：无门结构体 + @cluster 门控字段，
/// @not(cluster) 下为空结构体（三件消费在 init_services 体内均处 cluster
/// 门内，含 @not 回退臂）。
pub(crate) struct ClusterHandoff {
    #[cfg(feature = "cluster")]
    pub bridge_cluster_slot: Arc<std::sync::OnceLock<Arc<nemesis_cluster::cluster::Cluster>>>,
    #[cfg(feature = "cluster")]
    pub cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>>,
    #[cfg(feature = "cluster")]
    pub cluster_arc_ref: Option<Arc<nemesis_cluster::cluster::Cluster>>,
}

/// PB-6 产物 struct（§4.2 B6）。
pub(crate) struct ServicesWiring {
    #[cfg(feature = "health")]
    pub health_server: Arc<nemesis_health::server::HealthServer>,
    pub svc_mgr: Arc<nemesis_services::ServiceManager>,
    pub web_handle: tokio::task::JoinHandle<()>,
    pub real_port: i64,
    pub internal_cmd_rx: tokio::sync::mpsc::Receiver<nemesis_web::internal::InternalCommand>,
}

/// Step 11–17 服务族装配（计划 §4.2 B6）。
#[cfg_attr(not(feature = "cluster"), allow(unused_variables))]
pub(crate) async fn init_services(
    ctx: &GatewayCtx,
    mut web_server: nemesis_web::server::WebServer,
    web_bind_host: String,
    web_display_host: String,
    web_port: i64,
    bridge_client_launch: Option<nemesis_config::BridgeClientConfig>,
    agent_adapter: Arc<crate::adapters::AgentLoopServiceAdapter>,
    initial_tool_count: usize,
    cluster_handoff: ClusterHandoff,
) -> Result<ServicesWiring> {
    // ctx 影子重绑（B1 同款）：门控取各名字在本函数内的实际消费门。
    let home = ctx.home.clone();
    let config_path = ctx.config_path.clone();
    let cfg = ctx.cfg.clone();
    let bus = ctx.bus.clone();
    let cron_service = ctx.cron_service.clone();
    #[cfg(feature = "board")]
    let board_store = ctx.board_store.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_asset_url_slot = ctx.board_asset_url_slot.clone();
    // cluster 三件解包影子：名与原局部一致（体逐字），门随消费点
    //（bridge_cluster_slot @cluster；adapter/arc_ref 均 @all(board,cluster)）。
    #[cfg(feature = "cluster")]
    let bridge_cluster_slot = cluster_handoff.bridge_cluster_slot;
    #[cfg(all(feature = "board", feature = "cluster"))]
    let cluster_adapter = cluster_handoff.cluster_adapter;
    #[cfg(all(feature = "board", feature = "cluster"))]
    let cluster_arc_ref = cluster_handoff.cluster_arc_ref;
    // Step 11: Create HealthServer
    #[cfg(feature = "health")]
    let health_server = {
        let health_port = cfg.gateway.port;
        let health_config = nemesis_health::server::HealthServerConfig {
            listen_addr: format!("{}:{}", &cfg.gateway.host, health_port),
            version: Some(crate::common::VERSION_INFO.version.to_string()),
        };
        Arc::new(nemesis_health::server::HealthServer::new(health_config))
    };
    #[cfg(feature = "health")]
    info!(
        "[Gateway] Health server created for {}:{}",
        &cfg.gateway.host, cfg.gateway.port
    );

    // Step 12: Create HeartbeatService
    let heartbeat_interval_secs = if cfg.heartbeat.interval > 0 {
        (cfg.heartbeat.interval * 60) as u64
    } else {
        300
    };
    #[cfg(feature = "heartbeat")]
    let heartbeat_service = {
        let heartbeat_config = nemesis_heartbeat::service::HeartbeatConfig {
            interval: std::time::Duration::from_secs(heartbeat_interval_secs),
            enabled: cfg.heartbeat.enabled,
            workspace: Some(common::workspace_path(&home).to_string_lossy().to_string()),
            min_interval_minutes: 5,
            default_interval_minutes: 30,
        };
        Arc::new(nemesis_heartbeat::service::HeartbeatService::new(
            heartbeat_config,
        ))
    };
    #[cfg(feature = "heartbeat")]
    info!(
        "[Gateway] Heartbeat service created (enabled: {})",
        cfg.heartbeat.enabled
    );

    // C2: Wire HeartbeatService — bus + handler + skip file.
    // Mirrors Go's bot_service.go:403-406:
    //   heartbeatSvc.SetBus(msgBus)
    //   heartbeatSvc.SetHandler(createHeartbeatHandler(agentLoop))
    #[cfg(feature = "heartbeat")]
    {
        // Adapter: nemesis_bus::MessageBus → heartbeat::MessageBus
        struct HeartbeatBusAdapter {
            bus: Arc<nemesis_bus::MessageBus>,
        }
        impl nemesis_heartbeat::service::MessageBus for HeartbeatBusAdapter {
            fn publish_outbound(&self, channel: String, chat_id: String, content: String) {
                let msg = nemesis_types::channel::OutboundMessage {
                    channel,
                    chat_id,
                    content,
                    message_type: String::new(),
                    meta: Default::default(),
                };
                self.bus.publish_outbound(msg);
            }
        }
        heartbeat_service.set_bus(Arc::new(HeartbeatBusAdapter { bus: bus.clone() }));

        // Handler: calls agent_loop.process_heartbeat() synchronously via block_in_place.
        // Mirrors Go's `createHeartbeatHandler()` in bot_service.go:
        //   1. Check BOOTSTRAP.md → skip heartbeat
        //   2. Fallback channel = "cli", chat_id = "direct"
        //   3. Call ProcessHeartbeat(prompt, channel, chatID)
        //   4. Always return SilentResult (agent sends messages via tools, not via handler)
        let bootstrap_path = common::workspace_path(&home).join("BOOTSTRAP.md");
        let adapter_for_hb = agent_adapter.clone();
        heartbeat_service.set_handler(Box::new(
            move |prompt: String, mut channel: String, mut chat_id: String| {
                // Check BOOTSTRAP.md — if exists, skip heartbeat entirely.
                if bootstrap_path.exists() {
                    tracing::info!("[Gateway] BOOTSTRAP.md exists, skipping heartbeat LLM call");
                    return Some(nemesis_heartbeat::service::HeartbeatResult {
                        is_error: false,
                        is_async: false,
                        silent: true,
                        for_user: String::new(),
                        for_llm: "HEARTBEAT_OK".to_string(),
                    });
                }

                // Get the current AgentLoop via adapter (may be None if stopped).
                let agent_loop_for_hb = match adapter_for_hb.current() {
                    Some(al) => al,
                    None => {
                        tracing::debug!("[Gateway] Agent not running, skipping heartbeat");
                        return Some(nemesis_heartbeat::service::HeartbeatResult {
                            is_error: false,
                            is_async: false,
                            silent: true,
                            for_user: String::new(),
                            for_llm: "HEARTBEAT_OK".to_string(),
                        });
                    }
                };

                // Use cli:direct as fallback (matching Go).
                if channel.is_empty() || chat_id.is_empty() {
                    channel = "cli".to_string();
                    chat_id = "direct".to_string();
                }

                tokio::task::block_in_place(|| {
                    let rt = tokio::runtime::Handle::current();
                    match rt
                        .block_on(agent_loop_for_hb.process_heartbeat(&prompt, &channel, &chat_id))
                    {
                        Ok(response) if response.is_empty() => None,
                        Ok(response) => {
                            let is_heartbeat_ok = response.trim() == "HEARTBEAT_OK";
                            Some(nemesis_heartbeat::service::HeartbeatResult {
                                is_error: false,
                                is_async: false,
                                silent: true, // Go always returns SilentResult
                                for_user: String::new(),
                                for_llm: if is_heartbeat_ok {
                                    "HEARTBEAT_OK".to_string()
                                } else {
                                    response
                                },
                            })
                        }
                        Err(e) => Some(nemesis_heartbeat::service::HeartbeatResult {
                            is_error: true,
                            is_async: false,
                            silent: false,
                            for_user: String::new(),
                            for_llm: format!("Heartbeat error: {}", e),
                        }),
                    }
                })
            },
        ));

        // Set skip file (BOOTSTRAP.md) — if present, heartbeat is deferred.
        let skip_file = common::workspace_path(&home).join("BOOTSTRAP.md");
        if skip_file.exists() {
            heartbeat_service.set_skip_file(skip_file.to_string_lossy().to_string());
        }

        info!("[Gateway] Heartbeat service wired (bus + handler + skip_file)");
    }

    // M1: Create and wire DeviceService.
    // Mirrors Go's bot_service.go:409-413: devices.NewService(Config{Enabled, MonitorUSB}).
    #[cfg(feature = "devices")]
    {
        if cfg.devices.enabled {
            let device_config = nemesis_devices::service::DeviceServiceConfig {
                enabled: true,
                poll_interval_secs: 30,
                monitor_usb: cfg.devices.monitor_usb,
            };
            let device_service =
                nemesis_devices::service::DeviceService::with_config(device_config);
            // Wire bus sender: device events → outbound messages via bus
            let bus_for_devices = bus.clone();
            device_service.set_bus_sender(Box::new(
                move |channel: &str, chat_id: &str, content: &str| {
                    let msg = nemesis_types::channel::OutboundMessage {
                        channel: channel.to_string(),
                        chat_id: chat_id.to_string(),
                        content: content.to_string(),
                        message_type: String::new(),
                        meta: Default::default(),
                    };
                    bus_for_devices.publish_outbound(msg);
                },
            ));
            // Start monitoring (USB hotplug, etc.) — async, fire-and-forget
            if let Err(e) = device_service.start().await {
                warn!("[Gateway] Device service start note: {} (non-fatal)", e);
            } else {
                info!("[Gateway] Device service started (USB hotplug monitoring)");
            }
        } else {
            info!("[Gateway] Device service disabled (config.json: devices.enabled = false)");
        }
    } // #[cfg(feature = "devices")]

    // Step 13: Create ServiceManager with config
    let bot_config = nemesis_services::BotServiceConfig {
        security_enabled: cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true),
        config_path: config_path.clone(),
        workspace: home.join("workspace"),
        heartbeat_interval_secs,
        heartbeat_enabled: cfg.heartbeat.enabled,
        gateway_host: cfg.gateway.host.clone(),
        gateway_port: cfg.gateway.port as u16,
        llm_logging_enabled: cfg
            .logging
            .as_ref()
            .and_then(|l| l.llm.as_ref())
            .map(|l| l.enabled)
            .unwrap_or(false),
        ..Default::default()
    };
    let svc_mgr = Arc::new(nemesis_services::ServiceManager::with_config(bot_config));

    // Inject adapted services into BotService
    {
        let bot = svc_mgr.get_bot_service();
        #[cfg(feature = "health")]
        {
            bot.inject_health(Arc::new(adapters::HealthServerAdapter::new(
                health_server.clone(),
            )));
        }
        #[cfg(feature = "heartbeat")]
        {
            bot.inject_heartbeat(Arc::new(adapters::HeartbeatServiceAdapter::new(
                heartbeat_service.clone(),
            )));
        }
        #[cfg(not(any(feature = "health", feature = "heartbeat")))]
        let _ = bot;
        // Agent is NOT injected into BotService — its lifecycle is managed directly
        // by AgentLoopServiceAdapter (tray start/stop, gateway shutdown).
    }

    // Step 14: Start basic services
    svc_mgr
        .start_basic_services()
        .map_err(|e| anyhow::anyhow!("Error starting basic services: {}", e))?;

    // W2 P4: board autopilot 启动同步——store 为真相源，删孤儿/补登记/跟随
    // （必须在 cron.start 之前完成，防同步窗口内 job 已开始调度）。
    #[cfg(feature = "board")]
    if let Some(store) = board_store.as_ref() {
        match nemesis_web::handlers::board::sync_autopilot_jobs(&cron_service, store) {
            Ok(n) if n > 0 => {
                info!("[Gateway] Board autopilot sync: {n} rule(s) re-armed from store")
            }
            Ok(_) => {}
            Err(e) => warn!("[Gateway] Board autopilot sync failed: {}", e),
        }
    }

    // Start cron scheduler (after on_job handler is wired).
    // Mirrors Go's bot_service.go:571-579 cronSvc.Start().
    // 提取为嵌套 fn：await_holding_lock 是数据流型 lint，只认函数级 allow，
    // 不认语句级 attribute——函数级 allow 必须挂在这个小 fn 上而非几千行的
    // run() 上。
    #[allow(clippy::await_holding_lock)]
    async fn start_cron_scheduler(
        cron_service: &std::sync::Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    ) {
        // 启动序列唯一持有者：此时 cron handler 未运行、无并发 lock 竞争者，
        // std guard 跨这一次性 start().await 无实际死锁风险（start(&self) 的
        // future 借用锁内数据，结构性无法先放锁；彻底解 = Arc 化 CronService
        // 去掉外层 std::Mutex，见 goal 文档债务记录）。
        let cron = cron_service.lock().unwrap();
        if let Err(e) = cron.start().await {
            warn!("[Gateway] Cron service start note: {}", e);
        } else {
            info!("[Gateway] Cron scheduler started");
        }
    }
    start_cron_scheduler(&cron_service).await;
    // H1 (U12) armed gate：这里只 start 不 arm——arm() 被移到 Step 17 之后
    // （agent 已订阅 + web 已 bind）。此前 arm 挂在本处是 BUG #49（2026-08-28）
    // 的根因：boot 顺序是 arm(Step14) → web bind/state 写盘(Step17) →
    // agent_adapter.start() 才订阅 bus inbound(旧 Step18)，中间没有任何
    // 订阅者。overdue 的持久化 job 在 arm 后第一个 1s tick 即 fire，消息
    // publish 进 tokio broadcast 时零订阅者 = 静默丢弃（cron fire-and-forget
    // 照记 last_status=ok，agent 的 LLM 一次不发）。空载时间隙 <1s 订阅赢，
    // 负载下间隙拉开 >1s 必丢——测试全部通过/失败随负载轮换的根源。
    // tick 调度器在 disarm 状态下空转（见 service.rs H1 gate），晚 arm 无
    // 副作用，只是把 fire 时机推迟到"订阅者就位"之后。

    // Step 14b: Start AgentLoop's bus processing（原 Step 18 上移，BUG #49）
    // 订阅必须在所有 inbound 生产者上线之前完成：
    //   - web server（Step 17 起 accept WS 消息 → bus.publish_inbound）；
    //   - cron.arm()（下移到 Step 17 之后，armed 后第一个 tick 即 fire）。
    // tokio broadcast 零订阅者时 publish 即丢，所以顺序不变量是：
    // agent 订阅 → web 上线 → cron arm。
    if let Err(e) = agent_adapter.start() {
        warn!("[Gateway] Agent adapter start note: {}", e);
    }
    info!("[Gateway] Agent loop started via adapter, listening on bus");

    // Step 15: Print agent startup info
    print_agent_startup_info(&home, initial_tool_count);

    // L3: Bridge logger → SSE EventHub for real-time log streaming to Dashboard.
    // Mirrors Go's bot_service.go:674-688: logger.SetLogHook() → eventHub.
    //
    // Two paths:
    //   1. GlobalSseLogLayer installed in `init_logger_from_config` intercepts every
    //      `tracing::info!` / `tracing::warn!` / etc. across the codebase (~680 sites).
    //      This is the main path — most production logging goes through tracing macros.
    //   2. Legacy `NemesisLogger::set_hook` captures the rare `logger.log()` call (mostly tests
    //      these days). Kept for backwards compatibility.
    {
        let event_hub = web_server.event_hub().clone();
        nemesis_logger::set_global_log_callback(move |ev: nemesis_logger::SseLogEvent| {
            let data = serde_json::json!({
                "seq": ev.seq,
                "level": ev.level,
                "timestamp": ev.timestamp,
                "component": ev.component,
                "target": ev.target,
                "source": ev.source,
                "message": ev.message,
                "fields": ev.fields,
                "file": ev.file,
                "line": ev.line,
            });
            event_hub.publish(nemesis_web::events::EVENT_LOG, data);
        });
        info!("[Gateway] Tracing → SSE EventHub bridge connected (GlobalSseLogLayer)");
    }
    if let Some(logger) = nemesis_logger::global() {
        let event_hub = web_server.event_hub().clone();
        logger.set_hook(Box::new(move |entry: nemesis_logger::logger::LogEntry| {
            let data = serde_json::json!({
                "level": entry.level,
                "timestamp": entry.timestamp,
                "component": entry.component,
                "message": entry.message,
            });
            event_hub.publish(nemesis_web::events::EVENT_LOG, data);
        }));
        info!("[Gateway] Logger → SSE EventHub bridge connected");
    }

    // Step 16: Start outbound dispatch (bus outbound → WebSocket sessions)
    //  MSG: 目前这里暂不需要了，因为我们通过主通道直接来收发 web 消息了
    //let dispatch_bus = bus.clone();
    //let dispatch_session_mgr = web_server.session_manager().clone();
    //let dispatch_handle = tokio::spawn(async move {
    //    nemesis_web::server::dispatch_outbound(dispatch_bus, dispatch_session_mgr).await;
    //});
    //info!("[Gateway] Outbound dispatch started");

    // Step 17: Start WebServer in background
    let web_shutdown_rx = svc_mgr.subscribe_shutdown();
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel::<std::net::SocketAddr>();

    // Create internal command channel (web handler → gateway logic)
    let (internal_cmd_tx, internal_cmd_rx) =
        tokio::sync::mpsc::channel::<nemesis_web::internal::InternalCommand>(16);
    web_server.set_internal_cmd_tx(internal_cmd_tx);

    // 在 web server 接受请求之前，按 config.voice.json 自动初始化已启用的语音引擎
    // （STT/TTS/speaker）。这样 dashboard 查 engine_status 拿到的就是真值，前端 chat
    // 页一次询问即可正确反映按钮可用状态，无需轮询。
    #[cfg(feature = "voice")]
    {
        nemesis_web::handlers::voice::init_engines_from_config(&home.join("workspace")).await;
    }

    let web_handle = tokio::spawn(async move {
        if let Err(e) = web_server
            .start_with_shutdown(web_shutdown_rx, Some(bound_tx))
            .await
        {
            error!("[Gateway] Web server error: {}", e);
        }
    });
    info!(
        "[Gateway] Web server starting on {}:{}",
        web_bind_host, web_port
    );

    // Wait for the actual bound address (sent immediately after TcpListener::bind)
    let real_port: i64 = match bound_rx.await {
        Ok(addr) => {
            info!("[Gateway] Web server bound to {}", addr);
            // Swarm M3（§5.4）：对外资产基址落槽。G9（2026-09-09 结构修复）：
            // ① 只在 socket 真实监听所有网卡（unspecified 绑定）时广告 LAN
            //   IP；回环绑定如实广告 127.0.0.1——回环绑定 + 广告 LAN IP =
            //   承诺不可达 URL（G9 病灶）。
            // ② LAN IP 的选择走集群注册表网段匹配（select_advertised_lan_ip
            //   ，static peers 此时已入表），并挂 30s 自愈任务——UDP 发现的
            //   peer 稍后入表、多重网卡/DHCP 换 IP 都自动跟随，bundle 重签
            //   即走 HTTP 老路。
            #[cfg(all(feature = "board", feature = "cluster"))]
            {
                let lan_ip = cluster_adapter
                    .as_ref()
                    .and_then(|ca| select_lan_ip_for_advertisement(ca.cluster()));
                let host = advertise_host_for(addr.ip(), lan_ip);
                let base_url = format!("http://{host}:{}", addr.port());
                board_asset_url_slot.set(base_url.clone());
                // 落盘一份：board_asset 工具 publish 时读取（跨进程一致）。
                let url_path =
                    nemesis_path::resolve_asset_node_url_path_in_workspace(&home.join("workspace"));
                if let Some(parent) = url_path.parent() {
                    let _ = std::fs::create_dir_all(parent);
                }
                if let Err(e) = std::fs::write(&url_path, &base_url) {
                    warn!("[Gateway] asset node url persist failed: {}", e);
                }
                // 本节点集群 node_id 同拍落盘：publish / delivery 内联存档
                // 签发 bundle 时读入 node_id 字段（RPC 兜底寻址）。取
                // take() 前抢好的 cluster 副本（refs 槽已被消耗恒 None）。
                let id_path =
                    nemesis_path::resolve_asset_node_id_path_in_workspace(&home.join("workspace"));
                let self_node_id = cluster_arc_ref
                    .as_ref()
                    .map(|c| c.node_id().to_string())
                    .unwrap_or_default();
                if let Err(e) = std::fs::write(&id_path, &self_node_id) {
                    warn!("[Gateway] asset node id persist failed: {}", e);
                }

                // G9 自愈：每 30s 按最新注册表重算对外基址，变化才更新槽与
                // 落盘（UDP 发现的 peer 入表 / 本机 IP 变化后 bundle 广告
                // 自动修正）。槽是可更新句柄（AdvertisedUrl），刷新无碍
                // dispatch 签发链。
                let heal_slot = board_asset_url_slot.clone();
                let heal_home = home.clone();
                let heal_port = addr.port();
                let heal_adapter = cluster_adapter.clone();
                tokio::spawn(async move {
                    let mut ticker = tokio::time::interval(std::time::Duration::from_secs(30));
                    ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
                    ticker.tick().await; // 首个 tick 立即返回，跳过（bind 刚算过）
                    loop {
                        ticker.tick().await;
                        let Some(ref ca) = heal_adapter else {
                            continue;
                        };
                        let cluster = ca.cluster();
                        if !cluster.is_running() {
                            continue;
                        }
                        let host = select_lan_ip_for_advertisement(cluster)
                            .unwrap_or_else(|| "127.0.0.1".to_string());
                        let base_url = format!("http://{host}:{heal_port}");
                        if heal_slot.get().as_deref() == Some(base_url.as_str()) {
                            continue;
                        }
                        heal_slot.set(base_url.clone());
                        let url_path = nemesis_path::resolve_asset_node_url_path_in_workspace(
                            &heal_home.join("workspace"),
                        );
                        if let Some(parent) = url_path.parent() {
                            let _ = std::fs::create_dir_all(parent);
                        }
                        let _ = std::fs::write(&url_path, &base_url);
                        info!(
                            "[Gateway] Asset base url healed: {base_url} (cluster registry changed)"
                        );
                    }
                });
            }
            addr.port() as i64
        }
        Err(_) => {
            // oneshot 发送端被 drop = web 任务在报告绑定地址前就退出（典型：
            // build_router panic——panic 只落 stderr，error! 臂都到不了）。
            // 此时 web 完全不可用，绝不是"退回 config 端口还能服务"，
            // 必须 error 级别如实告警（2026-08-31 A3 路由 panic 事故教训）。
            error!(
                "[Gateway] Web server task exited without reporting a bound address; \
                 web UI/API are NOT serving (check stderr for a task panic)"
            );
            web_port
        }
    };

    // Update gateway state with actual web port
    {
        let state_path =
            nemesis_path::resolve_gateway_state_path_in_workspace(&common::workspace_path(&home));
        let state_json = serde_json::json!({
            "pid": std::process::id(),
            "web_host": web_display_host,
            "web_port": real_port,
        });
        if let Err(e) = std::fs::write(&state_path, state_json.to_string()) {
            warn!("[Gateway] Failed to update gateway state: {}", e);
        } else {
            info!("[Gateway] Gateway state updated: port={}", real_port);
        }
    }

    // 反向桥客户端 spawn（goal 批次二）：web server real_port 已确定——
    // conn 泵 dial 127.0.0.1:<real_port>。旁路任务（内部自重连、全部失败
    // 路径只打日志），spawn 即不管，不影响主流程。`--relay` 纯中继不经过
    // 此路径（run_relay 独立启动，无客户端语义）。
    if let Some(client) = bridge_client_launch {
        // 二期批次五（设备侧）：hello 携带本机集群身份（hub 据此把本机
        // 注册进 registry 同权组网）。cluster rpc_port==0（RPC server 未
        // 启动）= None——纯隧道设备语义；feature 关同。
        #[cfg(feature = "cluster")]
        let bridge_cluster_identity = bridge_cluster_slot.get().and_then(|c| {
            let rpc_port = c.rpc_port();
            if rpc_port == 0 {
                None
            } else {
                Some(nemesis_web::relay::BridgeClusterIdentity {
                    node_id: c.node_id().to_string(),
                    name: c.node_name(),
                    role: c.role(),
                    category: c.category(),
                    tags: c.tags(),
                    capabilities: c.local_capabilities(),
                    node_type: c.node_type().to_string(),
                    rpc_port,
                    addresses: c.get_all_local_ips(),
                })
            }
        });
        #[cfg(not(feature = "cluster"))]
        let bridge_cluster_identity: Option<nemesis_web::relay::BridgeClusterIdentity> = None;
        // 二期批次六（设备侧）：桥 RPC 枢纽——RpcClient 桥出口（经桥上行发
        // 请求给 hub）+ 下行 cluster_rpc 分流（response 唤醒 pending / request
        // 喂本地 RPC 链）。与身份快照同闸（rpc_port==0 = 无本地 RPC server，
        // 桥 RPC 无意义）；bridge.client 未启用时 uplink 恒空 = bridge_online
        // false = 仲裁纯直连（一期行为零变化，装配无害）。
        #[cfg(feature = "cluster")]
        let device_bridge_rpc = bridge_cluster_slot.get().and_then(|c| {
            if c.rpc_port() == 0 {
                return None;
            }
            let rpc_server = c.rpc_server()?.clone();
            let rpc_client = c.rpc_client_arc()?;
            let dev = std::sync::Arc::new(crate::bridge_rpc::DeviceBridgeRpc::new(
                rpc_server,
                c.node_id().to_string(),
                // 三期批次八：member_sync 成员合并进本地 registry（桥成员
                // 表的 registry 面）。
                Some(c.clone()),
            ));
            rpc_client.set_bridge_transport(dev.clone());
            Some(dev)
        });
        #[cfg(not(feature = "cluster"))]
        let device_bridge_rpc: Option<crate::bridge_client::BridgeRpcHandle> = None;
        crate::bridge_client::spawn(crate::bridge_client::BridgeClientParams {
            relay_url: client.relay_url,
            token: client.token,
            node_id: crate::bridge_client::hostname_node_id(),
            name: crate::bridge_client::hostname(),
            version: crate::common::VERSION_INFO.version.to_string(),
            web_port: real_port as u16,
            access_token: client.access_token,
            cluster_identity: bridge_cluster_identity,
            bridge_rpc: device_bridge_rpc,
        });
    }

    // Step 17: HealthServer is started by BotService (svc_mgr.start_bot() below)
    // via start_services() → services.health.start(). No separate spawn needed here.
    info!(
        "[Gateway] Health server will be started by bot service on {}:{}",
        &cfg.gateway.host, cfg.gateway.port
    );

    Ok(ServicesWiring {
        #[cfg(feature = "health")]
        health_server,
        svc_mgr,
        web_handle,
        real_port,
        internal_cmd_rx,
    })
}
