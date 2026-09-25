// ---------------------------------------------------------------------------
// PB-3 Web/Channel 装配（计划 §4.2 B3）：C1 段（计划口径原 3969–4417）整体
// 迁入 `init_web`——WebServer 构建、CORS、relay 服务端装配（桥 hub 侧身份
// sink / 帧枢纽 / 成员快照）、桥客户端身份注入、ChannelManager 全 Channel
// 初始化与 dispatch loop。原文自 run() 逐字迁入，仅四处机械改写：fn 体包
// 裹、ctx 三件影子重绑（home/cfg/bus）、cluster wiring 二件影子重绑、末尾
// WebWiring 构造。
//
// WebWiring 字段 = 实测逃逸本块的完整集合（六项全无门）：web_server
// （PB-4 event_hub + PB-6 set_* 链 + real_port）、enabled_channels（PB-4
// SharedResources；PB-7 另建同名计数局部属独立语义非本值）、web_bind_host
// / web_display_host / web_port（PB-5 广播 + PB-6 listen/URL + PB-7 展示）、
// bridge_client_launch（PB-7 Step 17 出站连接 spawn）。
//
// 跨 cfg 参数：body 内 cluster 二件消费均在 cfg(cluster) 门内
// （web_cluster_starts 双臂对 + bridge hub 装配），not(cluster) 组合下参数
// 无消费者——fn 级 cfg_attr allow(unused_variables)（Phase A
// advertise_host_for 同款形态），参数本体保持无门使调用点单行。
//
// 块内不变量随块内聚：C7 mem::forget(channel_manager) 挂账原样随块（§8）；
// relay 模式不经此路径（run_relay 独立轻量启动）的语义注释随块保留。
// ---------------------------------------------------------------------------

use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

use super::web_bind_and_display_hosts;
use super::{ClusterWiring, GatewayCtx};
use crate::common;

/// PB-3 产物 struct（§4.2 B3）。全字段无门：web/channel 装配在所有 feature
/// 组合下恒走（仅内部 cluster 挂件随门摘除）。
pub(crate) struct WebWiring {
    pub web_server: nemesis_web::server::WebServer,
    pub enabled_channels: Vec<String>,
    pub web_bind_host: String,
    pub web_display_host: String,
    pub web_port: i64,
    pub bridge_client_launch: Option<nemesis_config::BridgeClientConfig>,
}

/// C1：WebServer + ChannelManager 装配（计划 §4.2 B3）。WebServer 早建以向
/// WebChannel 注入 SessionManager；ChannelManager init/start/mem::forget 均
/// 在本函数内完成。
#[cfg_attr(not(feature = "cluster"), allow(unused_variables))]
pub(crate) async fn init_web(ctx: &GatewayCtx, cluster: &ClusterWiring) -> Result<WebWiring> {
    // ctx 影子重绑（B1 同款）：块内原文引用局部名不变，所有权拓扑不变。
    let home = ctx.home.clone();
    let cfg = ctx.cfg.clone();
    let bus = ctx.bus.clone();
    // cluster wiring 影子：消费点均在 cfg(cluster) 门内，影子随门。
    #[cfg(feature = "cluster")]
    let cluster_should_start = cluster.cluster_should_start;
    #[cfg(feature = "cluster")]
    let bridge_cluster_slot = cluster.bridge_cluster_slot.clone();

    //
    // 变基注记（2026-09-24）：origin/main 并入 SEC-001 控制面凭据启动闸
    // （web + websocket 双监听面，common::ensure_control_plane_credential）——
    // 本体随之携带（run() PB-3 区原文含闸逐字迁入）。
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
    // 皮肤包（.nbskin）装配：目录 = exe 同级 `skins/`（与 static/ 同策略，
    // 不落 home）；激活 id = config `ui.skin`（"default"/空 = 内置皮肤）。
    // setter 注入而非 WebServerConfig 字段——后者有大量测试字面量装配，
    // 加字段即 E0063 面扩大（79b49a25 教训）。
    let skins_dir = std::env::current_exe().ok().and_then(|exe| {
        exe.parent()
            .map(|dir| dir.join("skins").to_string_lossy().to_string())
    });
    let skin_id = cfg
        .ui
        .as_ref()
        .map(|u| u.skin.trim().to_string())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "default".to_string());
    web_server.set_skins(skins_dir, skin_id);

    // 签名验证启动自验状态注入（接入计划 §4）：verify_policy 快照 → 只读
    // AppState 字段 → security.signature_verify_status / 前端徽标。
    if let Some(status) = signature_status_from_start_check() {
        web_server.set_signature_verify(status);
    }

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

    Ok(WebWiring {
        web_server,
        enabled_channels,
        web_bind_host,
        web_display_host,
        web_port,
        bridge_client_launch,
    })
}

#[cfg(test)]
mod cov_tests;

/// 签名验证启动自验快照 → nemesis-web 只读状态结构（接入计划 §4）。
/// None = 本进程没跑过自验（理论上不可能——main 阶段必调；测试装配兜底）。
fn signature_status_from_start_check()
-> Option<std::sync::Arc<nemesis_web::handlers::signature_status::SignatureVerifyStatus>> {
    let sc = crate::verify_policy::start_check()?;
    let (last_result, key_fp, detail) = match &sc.outcome {
        Some(o) => (Some(o.state.clone()), o.key_fp.clone(), o.detail.clone()),
        None if sc.degraded => (
            None,
            None,
            "无信任锚——签名验证降级 off（锁定版部署需先注入编译期锚）".to_string(),
        ),
        None => (
            None,
            None,
            "security.signature_verify=off——启动自验已跳过".to_string(),
        ),
    };
    Some(std::sync::Arc::new(
        nemesis_web::handlers::signature_status::SignatureVerifyStatus {
            mode: sc.mode.as_str().to_string(),
            locked: sc.locked,
            anchor_fp: sc.anchor_fp.map(|s| s.to_string()),
            last_result,
            key_fp,
            detail,
        },
    ))
}
