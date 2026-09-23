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

use anyhow::Result;

// 仅测试消费的再导出（Phase A 先例）：姊妹 tests.rs 经 `use super::*` 取用，
// B7 后 run() 骨架不再直接用 Arc。
#[cfg(test)]
use std::sync::Arc;

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

mod agent_build;
mod autopilot;
mod board_dispatch;
mod bridges;
mod cluster_init;
mod cluster_support;
mod ctx;
mod display;
mod migrate;
mod post_agent;
mod relay;
mod runtime;
mod services;
mod shutdown;
mod web_init;

pub(crate) use self::agent_build::{AgentWiring, init_agent};
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
pub(crate) use self::cluster_init::{ClusterWiring, init_cluster};
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
pub(crate) use self::post_agent::init_post_agent;
pub(crate) use self::relay::run_relay;
pub(crate) use self::runtime::{RuntimeHandoff, run_runtime};
pub(crate) use self::services::{ClusterHandoff, init_services};
#[cfg(test)]
pub(crate) use self::shutdown::SHUTDOWN_REQUESTED;
#[cfg(test)]
pub(crate) use self::shutdown::is_shutdown_requested;
#[cfg(not(target_os = "android"))]
pub(crate) use self::shutdown::trigger_global_shutdown;
pub(crate) use self::web_init::{WebWiring, init_web};

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
    // ctx 影子说明：B2–B7 各相位 fn 均自行从 ctx 克隆/借取所需字段（B1 建
    // ctx 时的 16 无门影子已随相位迁移逐 PR 删除，B7 收尾后根部不再持有
    // 任何 ctx 影子）；root 仅保留 wiring 产物局部名供下游调用传参。

    // Phase B2（计划 §4.2）：PB-2 集群初始化（Step 9a）提取为 init_cluster
    //（gateway/cluster_init.rs）——参数表自 12 个上游变量缩到 ctx 单参；
    // 产物 ClusterWiring 只携带实测逃逸项（计划表列 cluster/task_list/
    // work_queue/persister 随 cluster_adapter_refs 四元组传递、node_id/
    // node_name 消费不出块，见该模块头注记）。C4 discovery Handle 捕获、
    // C2 E3 sweep 顺序、C9 占位→真身两段注册随块原样内聚。
    let cluster_wiring = init_cluster(&ctx).await?;
    // Phase B3（计划 §4.2）：PB-3 Web/Channel 装配（C1）提取为 init_web
    //（gateway/web_init.rs）——依赖 ctx 三件（home/cfg/bus）+ cluster wiring
    // 二件（均在 cfg(cluster) 门内消费）。产物 WebWiring 六项供 PB-4–PB-7
    // 消费；C7 mem::forget 挂账随块原样保留（§8）。
    let web_wiring = init_web(&ctx, &cluster_wiring).await?;
    // wiring 影子重绑：web_server 移动推迟到 init_agent 之后（B4——其以
    // &WebWiring 借读整体），其余 clone 保结构完整（B5 起不再借 web）。
    // enabled_channels 不留影子：唯一消费者 PB-4 已迁入 init_agent（其内
    // 自行 clone），Step 21 另建同名局部（count_enabled_channels）。
    let web_bind_host = web_wiring.web_bind_host.clone();
    let web_display_host = web_wiring.web_display_host.clone();
    let web_port = web_wiring.web_port;
    let bridge_client_launch = web_wiring.bridge_client_launch.clone();

    // wiring 影子重绑：bridge_cluster_slot 原局部名沿用（PB-7 桥身份快照
    // 消费）；其余五件（should_start/worker inbox/estop 停车场/selfcheck
    // 表/refs 四元组）B5 起消费全在 init_post_agent 内，影子已 delete，
    // 结构体按值传入。
    #[cfg(feature = "cluster")]
    let bridge_cluster_slot = cluster_wiring.bridge_cluster_slot.clone();

    // Phase B4（计划 §4.2）：PB-4 Agent 构建（Step 9b/9d + LSP/事件通道/
    // 后台注册表 + SharedResources + AgentLoop + workflow 桥）提取为
    // init_agent（gateway/agent_build.rs）——全引用签名（&ctx/&web/&cluster：
    // wiring 结构体须完整存活到 B5 按值消费），产物 AgentWiring 五项。
    let agent_wiring = init_agent(&ctx, &web_wiring, &cluster_wiring).await?;
    // AgentWiring 影子重绑：Arc 家族 clone 保结构体完整（B5 按值传入，
    // 部分移动后不可整体消费），克隆共享同一对象语义不变；security_plugin
    // 下游仅两处 cfg(security) 消费（审批装配 + scanner 停机），随门收放；
    // agent_event_rx 影子 B5 删除（唯一消费 set_agent_event_rx 在
    // init_post_agent 内）；initial_tool_count Copy 直读。
    let shared_resources = agent_wiring.shared_resources.clone();
    let agent_loop = agent_wiring.agent_loop.clone();
    let initial_tool_count = agent_wiring.initial_tool_count;
    #[cfg(feature = "security")]
    let security_plugin = agent_wiring.security_plugin.clone();
    // web_server &mut 绑定恢复——推迟到本点：init_agent 以 &WebWiring 借读
    // 整体（board_event_hub），部分移动后不可再整体借用。
    let mut web_server = web_wiring.web_server;

    // Phase B5（计划 §4.2）：PB-5 Agent→Web 注入族（Swarm M3 主持人桥/能力
    // 注入/ClusterServiceAdapter 构建/AgentLoopServiceAdapter/项目常驻 loop/
    // set_* 注入链/board 服务/cluster 服务）提取为 init_post_agent
    //（gateway/post_agent.rs）——ctx 引用 + &mut web_server + web_bind_host/
    // web_port 按值 + AgentWiring/ClusterWiring 按值消费（agent_event_rx
    // move、refs 四元组/worker inbox take 均原地耗尽）；产物 PostAgentWiring
    // 四项（agent_adapter/projects_manager 无门 + cluster_adapter/
    // cluster_arc_ref @cluster）。B2 的 cluster_adapter None 前向声明随其
    // 构建段迁入。
    let post_wiring = init_post_agent(
        &ctx,
        &mut web_server,
        web_bind_host.clone(),
        web_port,
        agent_wiring,
        cluster_wiring,
    )
    .await?;
    // PostAgentWiring 影子重绑：门随下游消费面（cluster 二件下游均在
    // cfg(cluster) 门内消费；cluster_adapter 根部无 take——Step 24 take 已
    // 随 B7 迁入 run_runtime，按值移交 RuntimeHandoff）。
    let agent_adapter = post_wiring.agent_adapter;
    let projects_manager = post_wiring.projects_manager;
    #[cfg(feature = "cluster")]
    let cluster_adapter = post_wiring.cluster_adapter;
    #[cfg(feature = "cluster")]
    let cluster_arc_ref = post_wiring.cluster_arc_ref;

    // Phase B6（计划 §4.2）：PB-6 服务族（Step 11–17：HealthServer 构建/
    // HeartbeatService 装配/设备监控/ServiceManager+基础服务/board autopilot
    // 启动同步/cron 装配与调度器/Agent 启动信息/WebServer 启动+real_port
    // 等待+资产基址落槽与 30s 自愈/网关状态落盘/反向桥客户端 spawn）提取为
    // init_services（gateway/services.rs）——web_server 按可变值（Step 16
    // set_internal_cmd_tx 需 &mut，Step 17 spawn 移入 web 任务，run() 此后
    // 仅持 web_handle 句柄）+ web_bind_host/web_port/bridge_client_launch/
    // initial_tool_count 按值（下游零消费）+ agent_adapter/web_display_host
    // 克隆（Step 18/19/24 仍消费原对象；agent_loop 非本块消费不入参）
    // + cluster 三件经 ClusterHandoff（无门结构体+@cluster
    // 门控字段——调用点不支持逐参 cfg 属性，ClusterWiring 同款先例）；
    // 产物 ServicesWiring 五项（health_server @health，其余无门）——
    // internal_cmd_rx（web 内部命令接收端，Step 19 泵消费）实测逃逸本块
    // 随产物携出；dispatch_handle 注释死码不进产物。
    #[cfg(feature = "cluster")]
    let cluster_handoff = ClusterHandoff {
        bridge_cluster_slot: bridge_cluster_slot.clone(),
        cluster_adapter: cluster_adapter.clone(),
        cluster_arc_ref: cluster_arc_ref.clone(),
    };
    #[cfg(not(feature = "cluster"))]
    let cluster_handoff = ClusterHandoff {};
    let services_wiring = init_services(
        &ctx,
        web_server,
        web_bind_host,
        web_display_host.clone(),
        web_port,
        bridge_client_launch,
        agent_adapter.clone(),
        initial_tool_count,
        cluster_handoff,
    )
    .await?;
    // ServicesWiring 影子重绑：门随下游消费面（health_server 唯一消费
    // set_ready 在 @health 门内；internal_cmd_rx 被 Step 19 泵 move）。
    #[cfg(feature = "health")]
    let health_server = services_wiring.health_server;
    let svc_mgr = services_wiring.svc_mgr;
    let web_handle = services_wiring.web_handle;
    let real_port = services_wiring.real_port;
    let internal_cmd_rx = services_wiring.internal_cmd_rx;

    // Phase B7（计划 §4.2）：PB-8/PB-9 运行期与关停（Step 18–24：cron arm/
    // bot 服务/展示 URL/横幅/审批接线/question broker/内部命令泵/sweeper/
    // Ctrl+C/托盘/wait_for_shutdown/统一善后 teardown）提取为 run_runtime
    //（gateway/runtime.rs）——终相无产物，运行期对象全部按值移入
    //（agent_loop/agent_adapter/projects_manager/shared_resources/
    // web_display_host/real_port/svc_mgr/web_handle/internal_cmd_rx 无门）；
    // 门控三件经 RuntimeHandoff（字段双臂声明——AgentWiring.security_plugin
    // 同款，@not 臂空桩保全组合可构造）；ctx 侧五件克隆 + bridge_outbound_
    // handle 引用（仅 Step 24 abort）。run() 自此为编排骨架（B1–B7 相位
    // 调用 + 影子重绑，Step 编号注释随各相位体保留）。
    #[cfg(feature = "security")]
    let security_slot = security_plugin;
    #[cfg(not(feature = "security"))]
    let security_slot = None;
    #[cfg(feature = "health")]
    let health_slot = health_server;
    #[cfg(not(feature = "health"))]
    let health_slot = ();
    #[cfg(feature = "cluster")]
    let cluster_slot = cluster_adapter;
    #[cfg(not(feature = "cluster"))]
    let cluster_slot = ();
    let runtime_handoff = RuntimeHandoff {
        security_plugin: security_slot,
        health_server: health_slot,
        cluster_adapter: cluster_slot,
    };
    run_runtime(
        &ctx,
        agent_loop,
        agent_adapter,
        projects_manager,
        shared_resources,
        web_display_host,
        real_port,
        svc_mgr,
        web_handle,
        internal_cmd_rx,
        runtime_handoff,
    )
    .await?;
    Ok(())
}
