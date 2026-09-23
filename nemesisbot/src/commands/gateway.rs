//! Gateway command - start the NemesisBot gateway server.
//!
//! Mirrors Go CmdGateway:
//! 1. Check config file exists
//! 2. Check home directory exists
//! 3. Load configuration
//! 4. Initialize logger from config
//! 5. Write PID file
//! 6. Create MessageBus
//! 7. Create LLM Provider via factory
//! 8. Create AgentLoop with bus integration
//! 9. Create WebServer with bus
//! 10. Create HealthServer
//! 11. Create HeartbeatService
//! 12. Start all services
//! 13. Print gateway banner
//! 14. Wait for shutdown signal
//! 15. Graceful shutdown

use std::sync::Arc;

use anyhow::Result;
use nemesis_services::LifecycleService;
use tracing::{error, info, warn};

use crate::adapters;
use crate::common;

// K1（devtool-upgrade 阶段 4）：apply_security_layer_switches /
// load_security_rules / load_scanner_full_config 与 Step 9b 装配块整体迁往
// `crate::security_setup`（单一真相源，headless `run` 共用）。

// ---------------------------------------------------------------------------
// Phase A 顶层装配件拆解（2026-09-23 gateway god-object 计划）：按职责迁入
// gateway/ 子模块；此处 cfg 门与原定义一致的 `pub(crate) use` 再导出保持
// `gateway::` 消费面不变——run() 主体与姊妹测试的 `use super::*` 按原名
// 解析，外部两处引用（main.rs run / agent_factory GatewayMemoryGate）不动。
// 仅 use/可见性调整：跨模块消费项 pub(crate) 化（结构体字面量构造的字段
// 随之），仅测试消费的再导出挂 cfg(test)。
//
// Phase B run() 相位化（计划 §4.2）：B1 起 Steps 1–9b 收敛为
// GatewayCtx::assemble（gateway/ctx.rs），下游相位按 PR 逐个提取为独立 fn
// + 产物 struct；run() 终态为编排骨架（<600 行），Step 编号注释保留。
// ---------------------------------------------------------------------------

mod autopilot;
mod board_dispatch;
mod bridges;
mod cluster_init;
mod cluster_support;
mod ctx;
mod display;
mod migrate;
mod relay;
mod shutdown;

#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) use self::autopilot::fire_board_autopilot;
#[cfg(all(feature = "board", not(feature = "cluster")))]
pub(crate) use self::autopilot::fire_board_autopilot;
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) use self::autopilot::sweep_dispatch_timeouts;
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) use self::board_dispatch::write_back_board_dispatch;
#[cfg(all(test, feature = "desktop", feature = "security"))]
pub(crate) use self::bridges::ApprovalPopupAdapter;
#[cfg(all(feature = "cluster", feature = "forge"))]
pub(crate) use self::bridges::ClusterForgeBridgeAdapter;
#[cfg(feature = "workflow")]
pub(crate) use self::bridges::GatewayAgentRunner;
#[cfg(feature = "security")]
pub(crate) use self::bridges::GatewayLlmJudge;
#[cfg(all(feature = "desktop", feature = "memory", feature = "security"))]
pub(crate) use self::bridges::GatewayMemoryGate;
#[cfg(all(test, feature = "desktop", feature = "security"))]
pub(crate) use self::bridges::plugin_ui_library_exists;
#[cfg(feature = "cluster")]
pub(crate) use self::bridges::{BusToClusterAdapter, ClusterResultPersisterAdapter};
pub(crate) use self::cluster_init::init_cluster;
#[cfg(any(feature = "cluster", test))]
pub(crate) use self::cluster_support::parse_host_port;
#[cfg(feature = "cluster")]
pub(crate) use self::cluster_support::record_cluster_usage;
pub(crate) use self::ctx::GatewayCtx;
#[cfg(all(test, feature = "board", feature = "cluster"))]
pub(crate) use self::display::select_advertised_lan_ip;
#[cfg(all(feature = "board", feature = "cluster"))]
pub(crate) use self::display::{advertise_host_for, select_lan_ip_for_advertisement};
pub(crate) use self::display::{
    count_enabled_channels, print_agent_startup_info, print_gateway_banner,
    web_bind_and_display_hosts,
};
#[cfg(all(feature = "desktop", not(target_os = "android")))]
pub(crate) use self::display::{open_browser, open_plugin_window};
#[cfg(feature = "workflow")]
pub(crate) use self::migrate::migrate_legacy_workflow_dir;
pub(crate) use self::relay::run_relay;
#[cfg(test)]
pub(crate) use self::shutdown::SHUTDOWN_REQUESTED;
#[cfg(test)]
pub(crate) use self::shutdown::is_shutdown_requested;
#[cfg(not(target_os = "android"))]
pub(crate) use self::shutdown::trigger_global_shutdown;

#[cfg(test)]
mod tests;

/// R9 补测批：gateway 活动场景组（live 双节点/心跳/审批/工作流，见模块头注释）。
/// 整文件 Windows 形态（11/11 live 场景走 Windows CLI 进程边界），随测试一并门控。
/// `pub(crate)` 仅为姊妹模块 scenario_tests 复用互斥闸与启动夹具。
#[cfg(all(test, windows))]
pub(crate) mod r9_live_tests;

/// 场景级真机 E2E（2026-09-23 多会话并行清账批）：起真 gateway 子进程 +
/// 可控延时 mock LLM，经持久 WS 连接逐条复现四联 BUG 的原始场景并断言修复
/// 生效（跨会话并行 / 同会话排队不丢 / Reject 忙弹回留痕 / 绑定注册表全
/// 生命周期）。同样 Windows 进程边界形态，与 R9 组同门控。
#[cfg(all(test, windows))]
mod scenario_tests;

/// Run the gateway command.
pub async fn run(local: bool, relay: bool, extra_args: &[String]) -> Result<()> {
    // macOS: acquire the tray-handoff channel guard first, so that ANY return
    // path from this function (early `?` errors, normal completion) closes the
    // channel and unblocks the main thread waiting for the tray. See
    // nemesis_desktop::main_thread_handoff. (process::exit paths terminate the
    // whole process, so they need no special handling here.)
    #[cfg(target_os = "macos")]
    let _tray_channel_guard = nemesis_desktop::main_thread_handoff::channel_guard();

    // Phase B1（计划 §4.2）：Steps 1–9b（PB-0 bootstrap + PB-1 单例预装配）
    // 收敛为 GatewayCtx::assemble（gateway/ctx.rs）。`--relay` 纯中继早退在
    // assemble 内部处理（None → 直接 Ok 返回；run_relay 语义不变）；
    // channel_guard（上方）仍护住 run() 全部早退路径（C10）。
    let Some(ctx) = GatewayCtx::assemble(local, relay, extra_args).await? else {
        return Ok(());
    };
    // 影子重绑：下游（Step 9a 起）沿用原局部名——逐字段 owned clone，所有权
    // 拓扑与原单绑定等价（下游 .clone()/move 语义逐字节不变）。JoinHandle 非
    // Clone，以引用重绑（下游仅 .abort()）。
    let home = ctx.home.clone();
    let config_path = ctx.config_path.clone();
    let config_store = ctx.config_store.clone();
    let cfg = ctx.cfg.clone();
    let resolution = ctx.resolution.clone();
    let model_name = ctx.model_name.clone();
    let bus = ctx.bus.clone();
    let cron_service = ctx.cron_service.clone();
    let conv_router = ctx.conv_router.clone();
    let estop = ctx.estop.clone();
    let data_store = ctx.data_store.clone();
    let agent_outbound_tx = ctx.agent_outbound_tx.clone();
    let bridge_outbound_handle = &ctx.bridge_outbound_handle;
    let mcp_enabled = ctx.mcp_enabled;
    let skills_loader_arc = ctx.skills_loader_arc.clone();
    let skills_registry_arc = ctx.skills_registry_arc.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_moderator_loop = ctx.board_moderator_loop.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_asset_url_slot = ctx.board_asset_url_slot.clone();
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_quota = ctx.board_quota.clone();
    #[cfg(feature = "board")]
    let board_store = ctx.board_store.clone();
    #[cfg(feature = "memory")]
    let memory_manager_for_web = ctx.memory_manager_for_web.clone();
    #[cfg(feature = "forge")]
    let forge_for_web = ctx.forge_for_web.clone();
    #[cfg(feature = "forge")]
    let forge_executor_for_tools = ctx.forge_executor_for_tools.clone();
    #[cfg(feature = "workflow")]
    let workflow_engine = ctx.workflow_engine.clone();
    #[cfg(feature = "workflow")]
    let workflow_tool_registry = ctx.workflow_tool_registry.clone();
    #[cfg(feature = "workflow")]
    let chat_secret_store = ctx.chat_secret_store.clone();
    #[cfg(any(feature = "workflow", feature = "security"))]
    let llm_provider = ctx.llm_provider.clone();

    // Phase B2（计划 §4.2）：PB-2 集群初始化（Step 9a）提取为 init_cluster
    //（gateway/cluster_init.rs）——参数表自 12 个上游变量缩到 ctx 单参；
    // 产物 ClusterWiring 只携带实测逃逸项（计划表列 cluster/task_list/
    // work_queue/persister 随 cluster_adapter_refs 四元组传递、node_id/
    // node_name 消费不出块，见该模块头注记）。C4 discovery Handle 捕获、
    // C2 E3 sweep 顺序、C9 占位→真身两段注册随块原样内聚。
    let cluster_wiring = init_cluster(&ctx).await?;
    // wiring 影子重绑：下游沿用原局部名（B1 同款，所有权拓扑不变）。
    let cluster_rpc_call_fn = cluster_wiring.cluster_rpc_call_fn;
    let cluster_rpc_config = cluster_wiring.cluster_rpc_config;
    let cluster_peers_fn = cluster_wiring.cluster_peers_fn;
    #[cfg(feature = "cluster")]
    let cluster_should_start = cluster_wiring.cluster_should_start;
    #[cfg(feature = "cluster")]
    let bridge_cluster_slot = cluster_wiring.bridge_cluster_slot;
    #[cfg(feature = "cluster")]
    #[allow(unused_mut)]
    let mut board_worker_inbox = cluster_wiring.board_worker_inbox;
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_estop_parked = cluster_wiring.board_estop_parked;
    #[cfg(all(feature = "board", feature = "cluster"))]
    let board_selfcheck_registry = cluster_wiring.board_selfcheck_registry;
    #[cfg(feature = "cluster")]
    let mut cluster_adapter_refs = cluster_wiring.cluster_adapter_refs;

    // Cluster adapter — manages dynamic start/stop of all cluster components.
    // B2 注记：None 前向声明留守 run()——赋值在 PB-5 ClusterServiceAdapter
    // 构建（四元组消费点）；B5 提取时随其构建段整体迁入。
    #[cfg(feature = "cluster")]
    let mut cluster_adapter: Option<Arc<crate::cluster_service::ClusterServiceAdapter>> = None;

    // C1: Create ChannelManager and wire it.
    // Mirrors Go's bot_service.go:333-344: create ChannelManager, register channels,
    // start dispatch loop, call agentLoop.SetChannelManager().

    // Create WebServer early so we can inject SessionManager into WebChannel.
    // G9：绑定地址与展示地址分离（见 web_bind_and_display_hosts）——集群
    // 场景 0.0.0.0 如实绑定所有网卡，资产 bundle 广告的 LAN IP 才为真。
    #[cfg(feature = "cluster")]
    let web_cluster_starts = cluster_should_start;
    #[cfg(not(feature = "cluster"))]
    let web_cluster_starts = false;
    let (web_bind_host, web_display_host) =
        web_bind_and_display_hosts(&cfg.channels.web.host, web_cluster_starts);
    // SEC-001：引导态凭据（空/默认令牌）只允许守护回环控制面——非回环绑定
    // + 引导值 = 拒绝启动并指引补凭据（见 common::ensure_control_plane_credential）。
    // web 通道（/ws）与 web server（/api）共用本监听与同一原始 token 值，
    // 此处一闸双护。
    common::ensure_control_plane_credential(
        &web_bind_host,
        &cfg.channels.web.auth_token,
        "channels.web.auth_token",
    )
    .map_err(anyhow::Error::msg)?;
    // SEC-001：websocket 通道是独立监听面（host 原样生效，无 0.0.0.0→回环
    // 翻译），同机制同闸。
    if cfg.channels.websocket.enabled {
        common::ensure_control_plane_credential(
            &cfg.channels.websocket.host,
            &cfg.channels.websocket.auth_token,
            "channels.websocket.auth_token",
        )
        .map_err(anyhow::Error::msg)?;
    }
    let web_port = cfg.channels.web.port;
    let cors_origins = {
        let cors_path = common::cors_config_path(&home);
        if cors_path.exists() {
            match nemesis_web::cors::CORSManager::new(&cors_path) {
                Ok(mgr) => {
                    let mgr_cfg = mgr.config();
                    if mgr_cfg.development_mode {
                        info!("[Gateway] CORS: development_mode enabled, allowing all origins");
                        vec![]
                    } else {
                        let origins = mgr.list_origins();
                        info!(
                            "[Gateway] CORS: loaded {} allowed origins from {}",
                            origins.len(),
                            cors_path.display()
                        );
                        origins
                    }
                }
                Err(e) => {
                    warn!(
                        "[Gateway] Failed to load CORS config: {}, using permissive defaults",
                        e
                    );
                    vec![]
                }
            }
        } else {
            vec![]
        }
    };
    let static_files = crate::embedded::resolve_static_files();
    let web_config = nemesis_web::server::WebServerConfig {
        listen_addr: format!("{}:{}", web_bind_host, web_port),
        // P0 vault（B3）：auth_token 支持 vault:/env:/yaml: 引用（web server
        // 侧与 web channel 侧同源解析，两处比较值一致）。鉴权验证类字段：
        // 解析失败回一次性随机 token（fail-closed，绝不静默回空串关鉴权）。
        auth_token: crate::common::resolve_auth_token_or_random(
            &cfg.channels.web.auth_token,
            "channels.web.auth_token",
        ),
        cors_origins,
        ws_path: "/ws".to_string(),
        workspace: Some(home.join("workspace").to_string_lossy().to_string()),
        home: Some(home.to_string_lossy().to_string()),
        version: crate::common::VERSION_INFO.version.to_string(),
        static_dir: None,
        static_files: Some(static_files),
        index_file: "index.html".to_string(),
    };
    let mut web_server = nemesis_web::server::WebServer::new(web_config);

    // P8（2026-09-21）：chat_event_log 装配 EventHub——record/record_tool 落
    // 环时同步广播 SSE `chat.activity {session_id, seq}`，让**其他**浏览器
    // 标签/端感知到本会话有新帧（本端走 WS 实时 push，天然领先；落后端
    // 防抖全量刷新兜底）。`--relay` 纯中继路径（run_relay）无 chat 流量，
    // 不装配（未安装时 record 静默跳过广播，单测零开销）。
    nemesis_web::chat_event_log::install_event_hub(web_server.event_hub().clone());

    // 反向桥中继服务端（goal：反向桥与多设备汇聚，一期批次一）：配置了
    // bridge.server.token 才开放接入门（fail-closed）——未配置则桥路由
    // 不存在。`--relay` 纯中继不走此路径（run_relay 独立轻量启动）。
    if let Some(bridge) = &cfg.bridge {
        if !bridge.server.token.is_empty() {
            let relay_server = std::sync::Arc::new(nemesis_web::relay::RelayServer::new(
                bridge.server.token.clone(),
                true,
            ));
            relay_server.ensure_maintenance();
            // 二期批次五（hub 侧）：正常模式全量启动 = 桥入设备注册进集群
            // registry（同权，与 UDP 发现节点一致，不降级）+ welcome 帧告知
            // hub 集群身份。identity sink 由宿主注入（relay 模块零集群依赖）；
            // cluster feature 关 = 桥退化为纯隧道语义（一期行为）。
            // `--relay` 纯中继走 run_relay 不经此路径——只转发不注册边界不动。
            #[cfg(feature = "cluster")]
            if let Some(hub_cluster) = bridge_cluster_slot.get() {
                // 身份 sink 与桥 RPC 枢纽共用同一映射表（桥链路 id ↔ 集群 id）。
                let bridge_sink = std::sync::Arc::new(
                    crate::bridge_cluster::BridgeClusterSink::new(hub_cluster.clone()),
                );
                relay_server.set_identity_sink(bridge_sink.clone());
                relay_server.set_hub_node_id(hub_cluster.node_id().to_string());
                // 二期批次六（hub 侧）：桥帧 RPC 枢纽——设备上行 cluster_rpc
                // 喂本地 RPC 链（与 TCP 同一 handler 链），RpcClient 桥出口
                // 经 relay 下行投递（网段仲裁见 rpc/client.rs）。RPC server
                // 未启动（rpc_port==0）→ 不装配，relay 维持一期忽略语义。
                if let (Some(hub_rpc_server), Some(hub_rpc_client)) =
                    (hub_cluster.rpc_server(), hub_cluster.rpc_client_arc())
                {
                    let hub_bridge = std::sync::Arc::new(crate::bridge_rpc::HubBridgeRpc::new(
                        hub_rpc_server.clone(),
                        hub_cluster.node_id().to_string(),
                        bridge_sink.clone(),
                        relay_server.clone(),
                    ));
                    relay_server.set_cluster_frame_sink(hub_bridge.clone());
                    hub_rpc_client.set_bridge_transport(hub_bridge);
                    info!("[Relay] 桥帧 RPC 枢纽已装配（上行喂本地 RPC 链 + 出口桥仲裁）");
                }
                // 三期批次八（hub 侧）：成员表快照闭包——relay 广播
                // member_sync 时拉取（registry 摘要 + hub 自身条目；桥入
                // 成员经映射反查打 via_bridge 标）。`--relay` 不经此路径，
                // 广播回落为桥设备表摘要（relay 内建）。
                let members_cluster = hub_cluster.clone();
                let members_sink = bridge_sink.clone();
                let self_node_id = hub_cluster.node_id().to_string();
                let self_node_name = hub_cluster.node_name();
                let self_rpc_port = hub_cluster.rpc_port();
                relay_server.set_member_snapshot(std::sync::Arc::new(move || {
                    let mut members = vec![serde_json::json!({
                        "node_id": self_node_id,
                        "name": self_node_name,
                        "online": true,
                        "via_bridge": false,
                        "addresses": [],
                        "rpc_port": self_rpc_port,
                        "role": "coordinator",
                        "category": "general",
                        "capabilities": [],
                        "node_type": "agent",
                    })];
                    for n in members_cluster.list_nodes() {
                        let port = n
                            .base
                            .address
                            .rsplit(':')
                            .next()
                            .and_then(|p| p.parse::<u16>().ok())
                            .unwrap_or(0);
                        let ips = if n.addresses.is_empty() {
                            // registry 无多地址记录时从 primary "ip:port" 剥出 IP。
                            n.base
                                .address
                                .rsplit_once(':')
                                .map(|(ip, _)| ip.to_string())
                                .into_iter()
                                .collect()
                        } else {
                            n.addresses.clone()
                        };
                        members.push(serde_json::json!({
                            "node_id": n.base.id,
                            "name": n.base.name,
                            "online": n.is_online(),
                            "via_bridge": members_sink.bridge_of(&n.base.id).is_some(),
                            "addresses": ips,
                            "rpc_port": port,
                            "role": n.base.role.as_role_str(),
                            "category": n.base.category,
                            "capabilities": n.capabilities,
                            "node_type": n.node_type,
                        }));
                    }
                    serde_json::json!({ "members": members })
                }));
            }
            web_server.set_relay(relay_server);
            info!(
                "[Relay] 内置中继服务端已开放（/bridge 接入、/d/<node_id>/ 转发、/relay 状态页）"
            );
        } else {
            info!("[Relay] bridge.server.token 未配置，接入门不开放");
        }
    }

    // 反向桥客户端（goal 批次二）：client.enabled 时注入本机桥身份
    // （/d/<node_id>/ 子路径命中本机面板——必须在 build_router 前注入），
    // 并在 Step 17 real_port 确定后 spawn 出站连接。桥为旁路：配置不完整
    // 只 ERROR + 不启动，绝不阻断主服务。
    let bridge_client_launch = cfg.bridge.as_ref().and_then(|bridge| {
        if !bridge.client.enabled {
            return None;
        }
        if bridge.client.relay_url.trim().is_empty() || bridge.client.token.is_empty() {
            tracing::error!(
                "[Bridge] bridge.client.enabled=true 但 relay_url/token 为空，桥客户端不启动（旁路不影响主服务）"
            );
            return None;
        }
        Some(bridge.client.clone())
    });
    if let Some(client) = &bridge_client_launch {
        let node_id = crate::bridge_client::hostname_node_id();
        info!(
            "[Bridge] 桥客户端已配置（中继 {}，身份 {}）",
            client.relay_url, node_id
        );
        web_server.set_bridge_identity(node_id);
    }

    let web_server_ops = std::sync::Arc::new(crate::adapters::WebServerOpsAdapter::new(
        web_server.session_manager().clone(),
    ));

    // Build list of enabled channels from config (needed by SharedResources + ChannelManager).
    let mut enabled_channels = Vec::new();
    if cfg.channels.web.enabled {
        enabled_channels.push("web".to_string());
    }
    if cfg.channels.websocket.enabled {
        enabled_channels.push("websocket".to_string());
    }
    if cfg.channels.telegram.enabled {
        enabled_channels.push("telegram".to_string());
    }
    if cfg.channels.discord.enabled {
        enabled_channels.push("discord".to_string());
    }
    if cfg.channels.feishu.enabled {
        enabled_channels.push("feishu".to_string());
    }
    if cfg.channels.slack.enabled {
        enabled_channels.push("slack".to_string());
    }
    if cfg.channels.whatsapp.enabled {
        enabled_channels.push("whatsapp".to_string());
    }
    if cfg.channels.dingtalk.enabled {
        enabled_channels.push("dingtalk".to_string());
    }
    if cfg.channels.qq.enabled {
        enabled_channels.push("qq".to_string());
    }
    if cfg.channels.line.enabled {
        enabled_channels.push("line".to_string());
    }
    if cfg.channels.onebot.enabled {
        enabled_channels.push("onebot".to_string());
    }
    if cfg.channels.maixcam.enabled {
        enabled_channels.push("maixcam".to_string());
    }
    if cfg.channels.external.enabled {
        enabled_channels.push("external".to_string());
    }

    {
        let channel_manager = Arc::new(
            nemesis_channels::manager::ChannelManager::with_allowed_channels(
                enabled_channels.clone(),
            ),
        );

        // Build ChannelInitConfig from gateway config (web channel is always available).
        // Feature-gated channel fields only exist under some cfg combos, so the
        // `..Default::default()` base is required (not dead code); the lint can't see that.
        #[allow(clippy::needless_update)]
        let init_config = nemesis_channels::manager::ChannelInitConfig {
            web: if cfg.channels.web.enabled {
                Some(nemesis_channels::web::WebChannelConfig {
                    host: cfg.channels.web.host.clone(),
                    port: cfg.channels.web.port as u16,
                    ws_path: cfg.channels.web.path.clone(),
                    // P0 vault（B3）：auth_token 支持 vault:/env:/yaml: 引用。
                    // 鉴权验证类：解析失败回一次性随机 token（fail-closed）。
                    auth_token: crate::common::resolve_auth_token_or_random(
                        &cfg.channels.web.auth_token,
                        "channels.web.auth_token",
                    ),
                    session_timeout_secs: cfg.channels.web.session_timeout as u64,
                    allow_from: cfg.channels.web.allow_from.clone(),
                })
            } else {
                None
            },
            web_server_ops: Some(web_server_ops),
            external: if cfg.channels.external.enabled {
                Some(nemesis_channels::external::ExternalConfig {
                    input_exe: cfg.channels.external.input_exe.clone(),
                    output_exe: cfg.channels.external.output_exe.clone(),
                    chat_id: cfg.channels.external.chat_id.clone(),
                    sync_to: cfg.channels.external.sync_to.clone(),
                    allow_from: cfg.channels.external.allow_from.clone(),
                })
            } else {
                None
            },
            maixcam: if cfg.channels.maixcam.enabled {
                Some(nemesis_channels::maixcam::MaixCamConfig {
                    host: cfg.channels.maixcam.host.clone(),
                    port: cfg.channels.maixcam.port as u16,
                    allow_from: cfg.channels.maixcam.allow_from.clone(),
                })
            } else {
                None
            },
            line: if cfg.channels.line.enabled {
                Some(nemesis_channels::line::LineConfig {
                    // P0 vault（B3）：两个字段均支持 vault:/env:/yaml: 引用。
                    // access_token 是出站调用凭据（保持空串语义）；
                    // channel_secret 用于签名验证（鉴权验证类 → fail-closed 随机）。
                    channel_access_token: crate::common::resolve_secret_or_empty(
                        &cfg.channels.line.channel_access_token,
                        "channels.line.channel_access_token",
                    ),
                    channel_secret: crate::common::resolve_auth_token_or_random(
                        &cfg.channels.line.channel_secret,
                        "channels.line.channel_secret",
                    ),
                    webhook_port: cfg.channels.line.webhook_port as u16,
                    allow_from: cfg.channels.line.allow_from.clone(),
                })
            } else {
                None
            },
            websocket: if cfg.channels.websocket.enabled {
                Some(nemesis_channels::websocket::WebSocketChannelConfig {
                    host: cfg.channels.websocket.host.clone(),
                    port: cfg.channels.websocket.port as u16,
                    path: cfg.channels.websocket.path.clone(),
                    // P0 vault（B3）：auth_token 支持 vault:/env:/yaml: 引用。
                    // 鉴权验证类：解析失败回一次性随机 token（fail-closed）。
                    auth_token: crate::common::resolve_auth_token_or_random(
                        &cfg.channels.websocket.auth_token,
                        "channels.websocket.auth_token",
                    ),
                    allow_from: cfg.channels.websocket.allow_from.clone(),
                    sync_to: cfg.channels.websocket.sync_to.clone(),
                })
            } else {
                None
            },
            // Feature-gated channels (telegram/discord/feishu/slack/etc.) are mapped
            // when the corresponding feature is enabled in nemesisbot's Cargo.toml:
            //   nemesis-channels = { workspace = true, features = ["telegram"] }
            ..Default::default()
        };

        // Initialize channels from config (registers them in the manager).
        let bus_inbound_sender = bus.inbound_sender();
        if let Err(e) = channel_manager
            .init_channels(&init_config, bus_inbound_sender)
            .await
        {
            warn!(
                "[Gateway] ChannelManager init_channels note: {} (non-fatal)",
                e
            );
        }

        // Setup sync targets — reads each channel's sync_to config and calls add_sync_target().
        // Mirrors Go's manager.go: m.setupSyncTargets() called after initChannels().
        {
            let mut sync_map = std::collections::HashMap::new();
            // Collect sync_to from all channel configs that are enabled
            macro_rules! add_sync {
                ($cfg:expr, $name:expr) => {
                    if $cfg.enabled && !$cfg.sync_to.is_empty() {
                        sync_map.insert($name.to_string(), $cfg.sync_to.clone());
                    }
                };
            }
            add_sync!(cfg.channels.websocket, "websocket");
            add_sync!(cfg.channels.external, "external");
            add_sync!(cfg.channels.web, "web");
            add_sync!(cfg.channels.telegram, "telegram");
            add_sync!(cfg.channels.discord, "discord");
            add_sync!(cfg.channels.feishu, "feishu");
            add_sync!(cfg.channels.dingtalk, "dingtalk");
            add_sync!(cfg.channels.slack, "slack");
            add_sync!(cfg.channels.whatsapp, "whatsapp");
            add_sync!(cfg.channels.qq, "qq");
            add_sync!(cfg.channels.line, "line");
            add_sync!(cfg.channels.maixcam, "maixcam");
            add_sync!(cfg.channels.onebot, "onebot");
            let sync_config = nemesis_channels::manager::ChannelSyncConfig { targets: sync_map };
            channel_manager.setup_sync_targets(&sync_config).await;
        }

        // Bridge: bus outbound broadcast → ChannelManager mpsc.
        // Mirrors Go's manager.go: dispatchOutbound reading from bus.OutboundChannel().
        // Without this, non-web channel outbound is silently dropped.
        let bus_for_cm = bus.clone();
        let cm_outbound_tx = channel_manager.outbound_sender();
        let _cm_bridge_handle = tokio::spawn(async move {
            let mut rx = bus_for_cm.subscribe_outbound();
            loop {
                match rx.recv().await {
                    Ok(msg) => {
                        if cm_outbound_tx.send(msg).await.is_err() {
                            break; // ChannelManager dispatch loop stopped
                        }
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        tracing::warn!(
                            "[Gateway] ChannelManager outbound bridge lagged {} messages",
                            n
                        );
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => {
                        break;
                    }
                }
            }
        });
        info!("[Gateway] Bus outbound → ChannelManager bridge connected");

        // Start the outbound dispatch loop (reads from internal mpsc, dispatches to channels).
        if let Err(e) = channel_manager.start_dispatch_loop() {
            warn!(
                "[Gateway] ChannelManager start_dispatch_loop note: {} (non-fatal)",
                e
            );
        }

        // Start all registered channels.
        if let Err(e) = channel_manager.start_all().await {
            warn!("[Gateway] ChannelManager start_all note: {} (non-fatal)", e);
        }

        // Keep the ChannelManager alive.
        std::mem::forget(channel_manager);
        info!(
            "[Gateway] ChannelManager created with {} enabled channel(s)",
            enabled_channels.len()
        );
        // Channel manager injection into agent_loop is now handled by the factory function.
    }

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
