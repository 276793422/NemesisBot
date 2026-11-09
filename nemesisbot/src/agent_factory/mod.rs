//! Agent factory — builds AgentLoop instances from shared configuration.
//!
//! Two factory functions:
//! - `build_agent_loop()` — main agent (bus mode, session store, continuation manager, etc.)
//! - `build_cluster_agent_loop()` — cluster agent (standalone mode, tier-resolved tool set, no bus)
//!
//! Both share the same tool registration and MCP logic via `register_tools_and_mcp()`.
//! The difference is in mode (bus vs standalone) and which optional subsystems are attached.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::{info, warn};

#[cfg(feature = "forge")]
use nemesis_web::ForgeProviderBridge;
use nemesis_web::ProviderAdapter;

use crate::common;

#[cfg(test)]
mod tests;

// （扩展计划）fallback 链装配测试（无 cluster 门——纯装配逻辑）。
#[cfg(test)]
mod fallback_chain_tests;

// （凭据 vault）ModelProviderReloader 测试（无 feature
// 门——字段决策与 patch 写盘都是纯逻辑；provider 重建用离线安全条目）。
#[cfg(test)]
mod reloader_tests;

// 项目 loop 工厂测试（非 cluster 门控——与 tests.rs 的
// #![cfg(feature = "cluster")] 顶部门不同，本模块默认构建必跑；模块名含
// "projects" 对齐 goal 门命令 `cargo test -p nemesisbot projects` 的过滤词）。
#[cfg(test)]
mod projects_factory_tests;

// 午夜清扫调度纯函数时区韧性测试（非 feature 门——时间逻辑与 cluster 无关）。
#[cfg(test)]
mod midnight_tests;

// 路由 config 接线测试（无 feature 门——纯映射逻辑默认
// 构建必跑；模块名含 route_resolver 对齐过滤词）。
#[cfg(test)]
mod route_resolver_tests;

// ---------------------------------------------------------------------------
// SharedResources — infrastructure that survives Agent restart
// ---------------------------------------------------------------------------

/// Resources shared across AgentLoop stop/start cycles.
///
/// Created once in `gateway::run()`, passed to `build_agent_loop()` on each
/// start. All fields are either `Arc` references to long-lived infrastructure
/// or values that don't change between restarts.
pub struct SharedResources {
    pub home: PathBuf,
    /// （devtool-upgrade）：agent 工作区根（skills / uploads /
    /// logs / plans 等的挂载点）。空 = 未覆盖，[`SharedResources::workspace_dir`]
    /// 回退 `<home>/workspace` canonical 布局；headless `run --workspace DIR`
    /// 显式设置（config.json 仍从 home 解析，工作区可独立指定）。
    pub workspace: PathBuf,
    #[allow(dead_code)] // Reserved for future use (e.g., bus subscription in factory)
    pub bus: Arc<nemesis_bus::MessageBus>,

    // Outbound channel — SharedResources holds the original Sender.
    // Factory clones it for each new AgentLoop.
    // When old AgentLoop is dropped, only the clone is dropped;
    // the original Sender stays alive → outbound bridge keeps running.
    pub agent_outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,

    // Shared infrastructure Arc references (lifecycle independent of AgentLoop)
    #[cfg(feature = "forge")]
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    pub forge: Option<()>,
    #[cfg(feature = "forge")]
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    #[cfg(not(feature = "forge"))]
    pub forge_executor: Option<()>,
    pub cron_service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    #[cfg(feature = "security")]
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub security_plugin: Option<()>,
    pub observer_manager: Option<Arc<nemesis_observer::Manager>>,
    pub data_store: Option<Arc<nemesis_data::DataStore>>,
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    #[cfg(feature = "memory")]
    pub memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    pub memory_manager: Option<()>,
    pub enabled_channels: Vec<String>,
    /// Workflow engine reference — when set, registers the `workflow`
    /// agent tool. None keeps the tool absent (e.g., during tests).
    #[cfg(feature = "workflow")]
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub workflow_engine: Option<()>,
    /// Approval manager slot, filled by the gateway after the agent loop is
    /// built. Lets `skill_manage` request interactive approval when enabled.
    pub approval_slot: nemesis_agent::loop_tools::ApprovalManagerSlot,
    /// question 工具的 broker 槽，gateway 在 agent loop 建
    /// 好后填 `WebQuestionBroker`（与 approval_slot 同一个「先建槽后填」的
    /// 模式——重启重建的 AgentLoop 拿到的都是同一槽 Arc，晚填也可）。
    /// 不依赖 security feature（提问是交互动作，不是安全动作）。
    pub question_slot: nemesis_agent::loop_tools::QuestionBrokerSlot,
    /// `install_skill` 工具的装前审批门槽，
    /// gateway 建 `LateWebSkillsGate` 后填（先建槽后填，同 approval_slot）。
    /// 槽内 None = 无审批面直装（headless / `install_approval=false`）；
    /// 漏斗其余闸（staging/验签/扫描/lockfile）与门无关恒生效。
    pub skills_install_gate_slot: nemesis_agent::loop_tools::SkillsInstallGateSlot,

    // Cluster RPC closure (Cluster itself is mem::forget'd, but rpc_call_fn must survive)
    pub cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                                Output = std::result::Result<serde_json::Value, String>,
                            > + Send,
                    >,
                > + Send
                + Sync,
        >,
    >,
    pub cluster_rpc_config: Option<nemesis_agent::loop_tools::ClusterRpcConfig>,
    /// Returns online peers for dynamic cluster_rpc tool description.
    /// Each tuple: (node_id, node_name, capabilities).
    pub cluster_peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>>,
    /// 负载感知路由（最小版）：cluster_rpc `target="auto"` 的在线节点选择器
    /// （成功率先行 → 待办压力 → 平均响应）。None = auto 未装配（工具侧诚实报错）。
    pub cluster_auto_target_fn: Option<Arc<dyn Fn() -> Option<String> + Send + Sync>>,
    /// Shared enabled flag for ClusterRpcTool.
    /// Set by factory when tool is registered, toggled by ClusterServiceAdapter
    /// to enable/disable the tool without removing it from the prompt.
    pub cluster_rpc_enabled: parking_lot::RwLock<Option<Arc<std::sync::atomic::AtomicBool>>>,

    // MCP config
    pub mcp_config_path: PathBuf,
    pub mcp_enabled: bool,

    /// 全局急停状态（kill switch）。gateway::run 里建一次，跨 agent 重启存活。
    /// 两个工厂都把每个 AgentLoop 绑到这同一个 Arc，所以急停状态在 stop/start
    /// 后自动保持——这才是真急停（不会一重启自己解除）。
    pub estop: Arc<nemesis_agent::estop::EstopState>,

    /// 常驻执行体监督器槽。gateway（agent_build）恒 Some；headless
    /// run / acp / 测试等无常驻形态 None（工厂跳过常驻路由挂接，per-call
    /// 行为零变化——RemoteExecutorTool 不挂 persistent 即旧形态）。
    pub agent_runtime: Option<Arc<crate::agents::runtime::AgentRuntimeSupervisor>>,

    /// Runtime config cache (single source of truth). Consumers read the live
    /// config via `config_store.handle()`; dashboard handlers mutate via
    /// `config_store.update(...)` (in-memory + persist). Lets executor.sandbox
    /// / tier / DLP toggles flip live without a gateway restart.
    pub config_store: Arc<nemesis_config::ConfigStore>,

    /// LSP manager 单例。LspTool 注册用同一个 Arc；
    /// gateway 优雅停机 `shutdown_all()` 收尸语言服务器子进程；web server
    /// `set_lsp_manager` 持同一引用（诊断闭环 - 消费）。
    /// LspManager 构造零进程（会话按查询惰性起），未启用 LSP 时持有无代价。
    #[cfg(feature = "lsp")]
    pub lsp_manager: Arc<nemesis_lsp::LspManager>,

    /// （devtool-upgrade）：工具事件广播通道。gateway::run 建一次
    /// broadcast channel，Some 时每个 AgentLoop 挂 ToolEventHook（纯观察者）；
    /// 接收端交给 nemesis-web pump（Dashboard WS push + EventHub）。
    /// None = 不挂 hook（CLI/exec_worker 等无 web 的形态零开销）。
    pub agent_event_tx: Option<tokio::sync::broadcast::Sender<nemesis_types::agent::AgentEvent>>,

    /// 会话执行统计聚合器单例（gateway 建一次，跨 loop
    /// 重建存活——主/集群/项目 loop 共享同一账本）。Some 时 build 出的每个
    /// loop 挂 SessionStatsSink（loop 内部计数上抛）；None = 不统计零开销。
    pub session_stats: Option<Arc<crate::session_stats::SessionStatsAggregator>>,

    /// （devtool-upgrade）：后台进程注册表单例。gateway::run 建一次，
    /// 跨 agent 重启存活；gateway 进程退出（Drop）时给残余任务发 kill 旗标。
    /// 经 SharedToolConfig 注入三件套工具（background_start/output/kill）。
    pub background_registry: Arc<nemesis_agent::BackgroundProcessRegistry>,

    /// （WASM 插件框架）：插件注册表单例。gateway init_agent 建一次
    /// （config `plugins.wasm.enabled=false` = None 整面子系统缺席）；跨
    /// agent 重启存活——每次 build_agent_loop 重建时桥工具重新注册 + invoker
    /// 重新挂接（Weak 指旧 loop，重启后 upgrade 失败）。estop watcher 是
    /// manager 级任务，init_agent spawn 一次不随重启泄。
    #[cfg(feature = "plugins-wasm")]
    pub plugin_manager: Option<Arc<nemesis_plugins_wasm::PluginManager>>,
    #[cfg(not(feature = "plugins-wasm"))]
    #[allow(dead_code)]
    pub plugin_manager: Option<()>,

    /// 入口形态变体（gap ⑤）：gateway/eval 等既有装配点用缺省 Interactive
    /// （pro system prompt 渲染字节不变）；headless `run` 显式 Headless、
    /// ACP 显式 Acp——各自入口的语境说明段随身份基座注入。
    pub entrance: nemesis_agent::prompt::Entrance,

    // ---------------- 全自动流转 /：board_issue 工具依赖 ----------------
    // gateway 装配期注入；store=None（run/acp/测试等 headless 路径）= 工具
    // 不注册，零影响。注册在主 agent——「对 master 说一句话建单」的入口；
    // cluster agent（worker）不装：worker 的 board.db 是本地 dashboard 视图，
    // 建单落不到 master 权威库（与 board_discuss 相反——发言经 RPC 上行，
    // 建单必须直写权威 store）。
    /// 看板权威 store（master 本地）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_store: Option<Arc<nemesis_board::BoardStore>>,
    /// 集群引用（单节点 cluster.enabled=false 时 None——工具仍注册，拆解
    /// 可用，派发诚实失败落系统评论，与 WSAPI issue.plan 同语义）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_cluster: Option<Arc<nemesis_cluster::cluster::Cluster>>,
    /// planner moderator 槽——gateway 级动态解析槽的共享克隆（写者唯一 =
    /// AgentLoopServiceAdapter，start 写 Some / stop 写 None；与
    /// approval_slot/question_slot 同款共享槽 Arc，重启重建的 loop 经槽
    /// 自然跟随——BUG-20261108-012）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_moderator_slot:
        Arc<parking_lot::RwLock<Option<Arc<nemesis_agent::r#loop::AgentLoop>>>>,
    /// home 目录（auto_confirm 旗标现读）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_home: std::path::PathBuf,
    /// SSE 事件 hub（plan_ready/plan_failed 推送；None = 不推）。
    #[cfg(all(feature = "board", feature = "cluster"))]
    pub board_event_hub: Option<Arc<nemesis_web::events::EventHub>>,
}

/// Default `SharedResources` for tests: empty/dummy infrastructure. Real
/// instances are always built fully by `build_agent_loop`; this exists so test
/// literals can use `..Default::default()` and stay immune to new fields.
impl Default for SharedResources {
    fn default() -> Self {
        // Sender has no Default — create a throwaway channel.
        let (agent_outbound_tx, _dropped_rx) = tokio::sync::mpsc::channel(16);
        Self {
            home: PathBuf::default(),
            workspace: PathBuf::default(),
            bus: Arc::new(nemesis_bus::MessageBus::default()),
            agent_outbound_tx,
            forge: None,
            forge_executor: None,
            cron_service: Arc::new(std::sync::Mutex::new(
                nemesis_cron::service::CronService::new(""),
            )),
            security_plugin: None,
            observer_manager: None,
            data_store: None,
            skills_loader: None,
            skills_registry: None,
            memory_manager: None,
            enabled_channels: Vec::new(),
            workflow_engine: None,
            approval_slot: Default::default(),
            question_slot: Default::default(),
            skills_install_gate_slot: Default::default(),
            cluster_rpc_call_fn: None,
            cluster_rpc_config: None,
            cluster_peers_fn: None,
            cluster_auto_target_fn: None,
            cluster_rpc_enabled: parking_lot::RwLock::new(None),
            mcp_config_path: PathBuf::default(),
            mcp_enabled: false,
            estop: Arc::new(nemesis_agent::estop::EstopState::new()),
            agent_runtime: None,
            config_store: Arc::new(nemesis_config::ConfigStore::from_config(
                nemesis_config::Config::default(),
                PathBuf::default(),
            )),
            #[cfg(feature = "lsp")]
            lsp_manager: Arc::new(nemesis_lsp::LspManager::new(None, None)),
            agent_event_tx: None,
            session_stats: None,
            background_registry: Arc::new(nemesis_agent::BackgroundProcessRegistry::new()),
            plugin_manager: None,
            entrance: nemesis_agent::prompt::Entrance::Interactive,
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_store: None,
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_cluster: None,
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_moderator_slot: Arc::new(parking_lot::RwLock::new(None)),
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_home: PathBuf::default(),
            #[cfg(all(feature = "board", feature = "cluster"))]
            board_event_hub: None,
        }
    }
}

// ---------------------------------------------------------------------------
// build_agent_loop — factory function
// ---------------------------------------------------------------------------

impl SharedResources {
    /// agent 工作区根。显式 `workspace` 覆盖优先；空（未设置）回退
    /// `<home>/workspace` canonical 布局——所有既有构造点（gateway /
    /// eval_worker / 测试 Default 字面量）行为逐字节不变。
    pub fn workspace_dir(&self) -> PathBuf {
        if self.workspace.as_os_str().is_empty() {
            self.home.join("workspace")
        } else {
            self.workspace.clone()
        }
    }
}

/// ASM-08（横扫存量加固，用户裁决 =fail）：gateway 级装配完成点
/// 的机械自检——关键件未接线即装配失败。
///
/// 治 ①（用量账本）/F-（workspace_root）/ASM-01（security_plugin）
/// 一族「漏接静默」：功能在组件层正确且有单测，但生产装配点漏接不报错，
/// 直到真机才炸。单测自己动手装配所以永远绿——接线缺口只能在装配点掀。
/// 主/集群/项目三个 builder 的收尾各调一次（headless run/acp 走主 builder，
/// 同罩）。
///
/// 语义细节（有意为之，勿「修直」）：
/// - `security_plugin` 不做恒 Some 要求——`security.enabled=false`（用户
///   显式关安全）或 feature 裁剪时 shared 为 None 是文档化合法态；断言的
///   是一致性：**shared 有而 loop 没接 = 装配漏项 → fail**。
/// - `pricing_store` 只 WARN 不 fail：open 失败是磁盘环境问题（的设计
///   选择=优雅降级不阻断启动），不是接线遗漏；「是否调用过 set」由装配
///   矩阵测试兜住（tests.rs wiring 系）。
// 非 security 组合下 `shared` 的唯一消费点（security_plugin 一致性检查）
// 整体被 cfg 掉，参数有意闲置——函数签名在两种 feature 组合下保持同一
// 形状（装配自检的调用方无需按 feature 分叉）。
#[cfg_attr(not(feature = "security"), allow(unused_variables))]
fn assert_gateway_critical_wiring(
    agent_loop: &nemesis_agent::r#loop::AgentLoop,
    shared: &SharedResources,
    context: &str,
) -> Result<()> {
    let status: std::collections::HashMap<&str, bool> =
        agent_loop.wiring_status().into_iter().collect();
    let mut missing: Vec<&str> = Vec::new();
    for key in ["estop", "workspace_root", "config_path"] {
        if !status.get(key).copied().unwrap_or(false) {
            missing.push(key);
        }
    }
    #[cfg(feature = "security")]
    if shared.security_plugin.is_some() && !status.get("security_plugin").copied().unwrap_or(false)
    {
        missing.push("security_plugin");
    }
    if !missing.is_empty() {
        anyhow::bail!(
            "[AgentFactory] {} 装配自检失败（ASM-08）：关键件未接线 {:?}——\
             漏接从静默失效升级为启动即炸；若为有意排除，请改走显式注释豁免，\
             不要绕过本断言",
            context,
            missing
        );
    }
    if !status.get("pricing_store").copied().unwrap_or(false) {
        tracing::warn!(
            "[AgentFactory] {} pricing_store 未接线——context_window L2 失效\
             （磁盘 open 失败属预期降级；否则为装配漏项）",
            context
        );
    }
    Ok(())
}

/// ASM-05（横扫存量加固）：`logging.llm.enabled` →
/// RequestLoggerObserver 注册的统一定义。gateway Step 9d 与 CLI
/// `nemesisbot agent` 共用（此前 agent.rs 手写装配里还有一份逐字拷贝，
/// 随 ASM-05 迁移收敛到这里）。
///
/// 返回是否注册（false = 未开启或配置缺失，调用方不必装配 observer
/// manager）。`register` 是 async 而本函数 sync——block_in_place 桥接
/// （gateway 同款；**须在 multi_thread runtime 上调用**，current-thread
/// 会 panic——与既有两处调用点一致的行为契约）。
pub(crate) fn register_request_logger_observer(
    mgr: &Arc<nemesis_observer::Manager>,
    cfg: &nemesis_config::Config,
    home: &std::path::Path,
) -> bool {
    let Some(ref logging_cfg) = cfg.logging else {
        return false;
    };
    let Some(llm_cfg) = &logging_cfg.llm else {
        return false;
    };
    if !llm_cfg.enabled {
        return false;
    }
    let rl_logging_config = nemesis_agent::request_logger::LoggingConfig {
        enabled: true,
        detail_level: match llm_cfg.detail_level.as_str() {
            "truncated" => nemesis_agent::request_logger::DetailLevel::Truncated,
            _ => nemesis_agent::request_logger::DetailLevel::Full,
        },
        log_dir: if llm_cfg.log_dir.is_empty() {
            "logs/request_logs".to_string()
        } else {
            llm_cfg.log_dir.clone()
        },
        save_raw: llm_cfg.save_raw,
    };
    let workspace_path = home.join("workspace");
    let rl_observer = Arc::new(
        nemesis_agent::request_logger_observer::RequestLoggerObserver::new(
            rl_logging_config,
            &workspace_path,
        ),
    );
    let mgr = mgr.clone();
    tokio::task::block_in_place(|| {
        tokio::runtime::Handle::current().block_on(async {
            mgr.register(rl_observer).await;
        })
    });
    true
}

// ---------------------------------------------------------------------------
// 机械拆分：按装配入口分文件。纯代码搬家，行为零变更；
// 各子模块 pub 项经 pub use 保持原 agent_factory:路径不变。
// ---------------------------------------------------------------------------
#[cfg(feature = "cluster")]
mod cluster;
mod main;
mod project;
mod shared;
mod spec;

#[cfg(feature = "cluster")]
pub use cluster::*;
pub use main::*;
pub use project::*;
// shared 全是内部 helper（拆前即 crate 内私有），pub(crate) 覆盖 crate 内
// 直引（如 image_gen_tool/tests.rs）与子模块 use super:* 两种解析
pub(crate) use shared::*;
pub use spec::*;
