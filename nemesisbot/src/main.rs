//! NemesisBot CLI entry point.
//!
//! Routes all commands to their respective handler modules.

// 双击直启 = 正式桌面应用语义（2026-09-21 BUG：双击 bot 后弹出的
// Dashboard 附带控制台黑窗）。release 构建用 windows 子系统——系统不再
// 为进程分配控制台，双击直启只有托盘 + Dashboard。debug 构建保留 console：
// 真机测试范式（`target/debug/nemesisbot.exe gateway > console.log` 重定向、
// 交互调试）全依赖它，不得动。仅 Windows 生效，其它平台不触碰该属性。
// 连带纪律：windows 子系统进程的 console 子进程会各自弹新控制台——生产
// 路径 spawn 一律 CREATE_NO_WINDOW（remote_executor_tool.rs / background_registry.rs
// 等既有先例，手动 CLI 命令交互场景除外）。
#![cfg_attr(
    all(target_os = "windows", not(debug_assertions)),
    windows_subsystem = "windows"
)]

/// L7（devtool-upgrade 阶段 7）：ACP agent 侧 server——编辑器等 ACP 客户端经
/// stdio JSON-RPC 接入；安全 8 层与 gateway 同源生效（详见模块头）。
mod acp;
mod adapters;
mod agent_factory;
/// 看板项目档案 goal P3（D3/D4 master 侧）：执行档案落地回灌 + 超限出卡
/// （TransferSink on_landed/on_overlimit 回调入口）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_archive_ingest;
/// Swarm M3 资产 RPC 兜底通路（提供方侧，2026-09-20）：asset.meta /
/// asset.chunk 两命令——跨网段 HTTP 不可达时消费方经集群 RPC 分块拉取。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_asset_rpc;
/// Swarm M3（G9 资产拉取/发布执行者）：board_asset 工具（仅注册进 cluster
/// agent；自包含——只需 workspace 路径，fetch 纯 HTTP+sha256，publish 自开
/// store + 幂等密钥 + 读 gateway 落盘的 node url）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_asset_tool;
/// Swarm M3：master 侧 nb_bus 装配（board 信封协议 gateway glue；
/// 幂等/额度/裁决器/wake 下行/主持人裁决）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_bus;
/// Swarm M3（G4 主动发言通道）：board_discuss 工具（仅注册进 cluster
/// agent；信封/RPC 复用 cluster_agent 共用件）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_discuss_tool;
/// 全自动流转 P3/D1：board_issue 工具（仅注册进主 agent——「对 master 说
/// 一句话建单」入口；create 复用 WSAPI issue.create 解析，plan 复用
/// execute_plan_chain 四入口单一真相源）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_issue_tool;
/// Swarm M4：验收 agent 编排（in_review 批作业三态处置；触发挂
/// write_back in_review 路径，评审 LLM 走主 loop 后置装配桥）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_review;
/// 反向桥客户端（goal：节点显示名 + 反向桥与多设备汇聚，一期批次二）：
/// 出站连远端中继、hello 握手、退避重连、conn 泵（本机 web server 字节流
/// 搬运）、access_check 比对。旁路——任何失败不影响本机 dashboard。
mod bridge_client;
/// 桥集群身份注册（goal 二期批次五，hub 侧）：桥 hello 集群身份事件 →
/// registry 同权注册 / Offline（依赖 nemesis_cluster，随 feature 门控）。
/// `--relay` 纯中继不注入（只转发不注册边界不动）。
#[cfg(feature = "cluster")]
mod bridge_cluster;
#[cfg(feature = "cluster")]
mod bridge_rpc;
#[cfg(feature = "cluster")]
mod cluster_agent;
#[cfg(feature = "cluster")]
mod cluster_request_logger_observer;
#[cfg(feature = "cluster")]
mod cluster_service;
mod commands;
mod common;
/// 看板项目档案 goal P5/F5：冲突硬解执行体（auto 档——AI 硬解→机械失败
/// 重派原 worker→离线三轮接触→换人；budget 保险丝打满回落 human 档）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod conflict_resolver;
mod embedded;
/// eval 结果评估器（规则驱动三分类；纯函数读报告，无 Windows API——
/// rules 管理命令在所有平台可用）。
#[cfg(feature = "eval")]
mod eval_assessor;
#[cfg(feature = "eval")]
mod eval_worker;
mod exec_worker;
/// U10 统一执行世界：executor 通道装配单一真相源 + workflow 引擎的
/// ExecutionWorld 桥（world 部分 `sandbox` feature 门控）。
mod exec_world;
/// L6++（2026-09-08）：项目注册表（config/projects.json）+ 项目常驻
/// AgentLoop 管理（对话/项目双分组；注册不拥有——删项目只解除分组）。
/// M1 中间态：registry API 尚无二进制消费方（M2 manager / G4 WSAPI 接线），
/// allow 随接线移除。
#[allow(dead_code)]
mod projects;
/// F7（devtool-upgrade 阶段 5）：Dashboard 结构化提问 broker——question
/// 工具的阻塞端 + WSAPI question.respond/pending 端（同审批 broker 形态）。
mod question_broker;
/// K1（devtool-upgrade 阶段 4）：SecurityPlugin 构造单一真相源（gateway /
/// headless `run` 共用，安全 9 层在无端口形态不降级）。
mod security_setup;
/// M7（devtool-upgrade 阶段 5）：Dashboard 审批管理器——auditor 的
/// require_approval 走 dashboard 审批卡（AgentEvent 广播 + WSAPI respond）。
/// 门控随消费方：唯一生产调用点在 gateway 的 security cfg 装配块内，
/// feature off 时整块消失，本模块必须同门（--no-default-features 编译）。
#[cfg(feature = "security")]
mod web_approval;

/// K4 (b)（devtool-upgrade 阶段 7）：IM 通道审批卡管理器 + 组合分流。
/// web_approval 同门（依赖 nemesis_security::auditor trait）。
#[cfg(feature = "security")]
mod channel_approval;

use anyhow::Result;
use clap::{Parser, Subcommand};

// Embed all config templates at compile time (mirrors Go's //go:embed config)
// pub(crate)：onboard 提取模块（commands/onboard.rs）共用同一份嵌入常量，
// 不二次 include_str（单一真相源）。
pub(crate) const CONFIG_DEFAULT: &str = include_str!("../config/config.default.json");
pub(crate) const CONFIG_MCP_DEFAULT: &str = include_str!("../config/config.mcp.default.json");
pub(crate) const CONFIG_CLUSTER_DEFAULT: &str =
    include_str!("../config/config.cluster.default.json");
pub(crate) const CONFIG_SKILLS_DEFAULT: &str = include_str!("../config/config.skills.default.json");
pub(crate) const CONFIG_SCANNER_DEFAULT: &str =
    include_str!("../config/config.scanner.default.json");
pub(crate) const CONFIG_ENHANCED_MEMORY_DEFAULT: &str =
    include_str!("../config/config.enhanced_memory.default.json");
pub(crate) const CONFIG_CHAT_DEFAULT: &str = include_str!("../config/config.chat.default.json");
pub(crate) const CONFIG_FORGE_DEFAULT: &str = include_str!("../config/config.forge.default.json");
pub(crate) const CONFIG_SECURITY_WINDOWS: &str =
    include_str!("../config/config.security.windows.json");
pub(crate) const CONFIG_SECURITY_LINUX: &str = include_str!("../config/config.security.linux.json");
pub(crate) const CONFIG_SECURITY_DARWIN: &str =
    include_str!("../config/config.security.darwin.json");
pub(crate) const CONFIG_SECURITY_OTHER: &str = include_str!("../config/config.security.other.json");

// Embed personality files at compile time
pub(crate) const DEFAULT_IDENTITY: &str = include_str!("../default/IDENTITY.md");
pub(crate) const DEFAULT_SOUL: &str = include_str!("../default/SOUL.md");
pub(crate) const DEFAULT_USER: &str = include_str!("../default/USER.md");
pub(crate) const DEFAULT_IDENTITY_CLUSTER: &str = include_str!("../default/IDENTITY_Cluster.md");
#[cfg(feature = "cluster")]
const CLUSTER_IDENTITY_TEMPLATE: &str = include_str!("../config/IDENTITY.cluster.template.md");

#[derive(Parser)]
#[command(
    name = "nemesisbot",
    version,
    about = "NemesisBot - Personal AI Agent System"
)]
struct Cli {
    /// Use local directory for config (.nemesisbot in current dir)
    #[arg(long)]
    local: bool,

    /// 无参 = 直启 gateway（双击直启语义，2026-09-17）：home 缺失自动
    /// auto-init（种子语义，不 clobber 用户文件）、无 LLM 降级启动
    /// （NullProvider）、双击场景无窗重生 + 托盘 + 自动开 Dashboard。
    #[command(subcommand)]
    command: Option<Commands>,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration and workspace
    Onboard {
        /// Use default configuration (also accepts `onboard default` as positional argument)
        #[arg(long, short)]
        default: bool,

        /// Optional subcommand: "default" for default configuration
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Start the gateway server
    Gateway {
        /// Enable debug logging
        #[arg(short, long)]
        debug: bool,
        /// Disable all logging
        #[arg(short, long)]
        quiet: bool,
        /// Disable console output (file only)
        #[arg(long)]
        no_console: bool,
        /// 反向桥纯中继模式（goal：反向桥与多设备汇聚）：只起 web server
        /// （状态页 + /bridge + /d/ 转发），不起本地 agent/board/集群——
        /// 状态页即全部 UI。需要 bridge.server.token 非空（空则拒绝启动）。
        #[arg(long)]
        relay: bool,
    },
    /// Run a single headless agent task and exit (no ports, no gateway; K1)
    Run {
        /// Task prompt. Omit (or pass `-`) to read the task from stdin.
        task: Option<String>,

        /// Workspace root for this run (default: <home>/workspace)
        #[arg(long)]
        workspace: Option<std::path::PathBuf>,

        /// Working mode: plan (read-only, writes confined to plans/) or build
        #[arg(long)]
        mode: Option<String>,

        /// Output format: text (default) or json (K2, not yet implemented)
        #[arg(long)]
        format: Option<String>,

        /// Tool-turn budget for this task (0 = use config default)
        #[arg(long)]
        max_turns: Option<u32>,

        /// Model override for this run (must exist in model_list)
        #[arg(long)]
        model: Option<String>,
    },
    /// Run the ACP agent server over stdio (ACP editors/clients; L7)
    Acp,
    /// Interact with the agent directly
    Agent {
        #[command(subcommand)]
        subcommand: Option<commands::agent::AgentSetCommand>,
        /// Send a single message and exit
        #[arg(short, long)]
        message: Option<String>,
        /// Session key
        #[arg(short, long, default_value = "cli:default")]
        session: String,
        /// Enable debug logging
        #[arg(short, long)]
        debug: bool,
        /// Disable all logging
        #[arg(short, long)]
        quiet: bool,
        /// Disable console output (file only)
        #[arg(long)]
        no_console: bool,
    },
    /// Show system status
    Status,
    /// Search conversation history across sessions (U20 full-text search)
    History {
        #[command(subcommand)]
        action: commands::history::HistoryAction,
    },
    /// Manage conversation sessions (Z1: fork a session at a turn boundary)
    Session {
        #[command(subcommand)]
        action: commands::session::SessionAction,
    },
    /// Manage communication channels
    Channel {
        #[command(subcommand)]
        action: commands::channel::ChannelAction,
    },
    /// Manage bot cluster
    #[cfg(feature = "cluster")]
    Cluster {
        #[command(subcommand)]
        action: commands::cluster::ClusterAction,
    },
    /// Manage CORS configuration
    Cors {
        #[command(subcommand)]
        action: commands::cors::CorsAction,
    },
    /// Manage LLM models
    Model {
        #[command(subcommand)]
        action: commands::model::ModelAction,
    },
    /// Manage scheduled tasks
    Cron {
        #[command(subcommand)]
        action: commands::cron::CronAction,
    },
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: commands::mcp::McpAction,
    },
    /// Manage security settings
    #[cfg(feature = "security")]
    Security {
        #[command(subcommand)]
        action: commands::security::SecurityAction,
    },
    /// Manage logging configuration
    Log {
        #[command(subcommand)]
        action: commands::log::LogAction,
    },
    /// Manage authentication
    #[cfg(feature = "auth")]
    Auth {
        #[command(subcommand)]
        action: commands::auth::AuthAction,
    },
    /// Migrate model API keys into credentials.yaml references (U15).
    /// NOTE: model API keys — distinct from `auth`'s OAuth credential store.
    Credentials {
        #[command(subcommand)]
        action: commands::credentials::CredentialsAction,
    },
    /// Manage skills
    Skills {
        #[command(subcommand)]
        action: commands::skills::SkillsAction,
    },
    /// Manage self-learning module
    #[cfg(feature = "forge")]
    Forge {
        #[command(subcommand)]
        action: commands::forge::ForgeAction,
    },
    /// Manage the managed-agent board (issues)
    #[cfg(feature = "board")]
    Issue {
        #[command(subcommand)]
        action: commands::issue::IssueAction,
    },
    /// Manage board autopilot rules (cron-triggered issue creation)
    #[cfg(feature = "board")]
    Autopilot {
        #[command(subcommand)]
        action: commands::autopilot::AutopilotAction,
    },
    /// Manage DAG workflows
    #[cfg(feature = "workflow")]
    Workflow {
        #[command(subcommand)]
        action: commands::workflow::WorkflowAction,
    },
    /// Manage virus scanner
    #[cfg(feature = "security")]
    Scanner {
        #[command(subcommand)]
        action: commands::scanner::ScannerAction,
    },
    /// Sandboxie sandbox management (install / uninstall / status)
    #[cfg(feature = "sandbox")]
    Sandbox {
        #[command(subcommand)]
        action: commands::sandbox::SandboxCommand,
    },
    /// Evaluate a prompt / skill's runtime behaviour in a sandbox and assess
    /// the report (rules-driven risk / safe / unknown verdict)
    #[cfg(feature = "eval")]
    Eval {
        #[command(subcommand)]
        action: commands::eval::EvalAction,
    },
    /// Manage local voice pipeline
    #[cfg(feature = "voice")]
    Voice {
        #[command(subcommand)]
        action: commands::voice::VoiceAction,
    },
    /// Manage enhanced memory
    #[cfg(feature = "memory")]
    Memory {
        #[command(subcommand)]
        action: commands::memory::MemoryAction,
    },
    /// Manage AI personas
    Persona {
        #[command(subcommand)]
        action: commands::persona::PersonaAction,
    },
    /// Graceful shutdown
    Shutdown,
    /// Migrate from OpenClaw
    #[cfg(feature = "migrate")]
    Migrate {
        #[command(flatten)]
        options: commands::migrate::MigrateOptions,
    },
    /// Show version information
    Version,
    /// Open the dashboard UI
    Dashboard,
    /// Emergency stop — freeze all agent activity (kill switch).
    /// Bare `estop` engages; `--release` resumes; `--status` queries.
    Estop {
        /// Release the e-stop (resume agent activity). Default action is to engage.
        #[arg(long)]
        release: bool,
        /// Query e-stop status without changing it.
        #[arg(long)]
        status: bool,
    },
    /// Internal test commands (hidden)
    #[cfg(feature = "desktop")]
    #[command(hide = true)]
    Test {
        #[command(subcommand)]
        action: commands::test_cmd::TestAction,
    },
}

#[cfg(not(target_os = "macos"))]
#[tokio::main]
async fn main() -> Result<()> {
    // Executor role short-circuit: if spawned as a tool-executor child (env
    // NEMESISBOT_ROLE=executor, set by the gateway's ExecutorChannel when it
    // spawns a child per tool call), run the executor entrypoint instead of CLI
    // dispatch. Must precede Cli::parse_from — the child is spawned with no
    // subcommand. The early return also prevents any fork loop: the child never
    // reaches the gateway code that spawns executors.
    if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("executor") {
        return exec_worker::run().await;
    }

    // Eval-agent role short-circuit: spawned inside the NemesisEvalBox sandbox
    // by `nemesisbot eval` (via Start.exe). Same pattern as the executor role —
    // no CLI parsing, workspace comes from env, never runs path resolution.
    #[cfg(feature = "eval")]
    {
        if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("eval-agent") {
            common::ensure_default_logger();
            return eval_worker::run().await;
        }
    }

    // Early check for child mode (--multiple flag) before any CLI parsing.
    // This allows the parent process to self-spawn a child that loads plugin-ui.dll.
    #[cfg(feature = "desktop")]
    {
        if nemesis_desktop::child_mode::has_child_mode_flag() {
            match nemesis_desktop::child_mode::run_child_mode().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("[Child] Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    // Pre-parse --local from all args (Go-compatible: --local can appear anywhere).
    // Go strips --local from os.Args before command dispatch, so we do the same.
    let mut local_mode = false;
    let filtered_args: Vec<String> = std::env::args()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();

    let mut cli = Cli::parse_from(filtered_args);
    if local_mode {
        cli.local = true;
        println!("Local mode enabled: using ./.nemesisbot");
    }

    // 双击直启（2026-09-17）：无参/`gateway` 语义且独占新控制台（双击 exe
    // 形态）→ 无窗重生 DETACHED_PROCESS 子进程后本进程退出；子进程带
    // NEMESISBOT_BARE_CHILD 跳过检测继续跑。其他子命令保持控制台输出
    // （短命 CLI 操作，双击场景用户需要看到输出）。
    if is_gateway_launch(&cli)
        && std::env::var(common::BARE_CHILD_ENV).is_err()
        && common::console_is_solo_fresh()
        && common::try_respawn_detached()
    {
        std::process::exit(0);
    }

    // R1 真机验收修复（2026-08-28）：把 CLI 解析出的 home 同步进 nemesis-path
    // 的进程单例（default_path_manager）。该单例独立解析 home，而
    // `set_local_mode` 从未被本二进制调用 —— exe 旁存在 `.nemesisbot` 的部署
    // （bin_windows / --local 多实例）里，单例按 exe-dir 优先级抢在 cwd 之前，
    // 导致 boundary 事件 / replay ledger / session_logs / history_search 写进
    // exe 同级的另一个 home（跨实例数据串写；config/session 等走 DI 的路径
    // 不受影响，所以才隐蔽）。`set_home_dir` 是 OnceLock 初始化后唯一的运行时
    // 重定向缝；非 --local 时与单例自身解析结果一致，是无害的 no-op。
    nemesis_path::default_path_manager().set_home_dir(common::resolve_home(local_mode));

    // Lazy logging initialization:
    // Commands like gateway/agent call init_logger_from_config() internally
    // which reads config.json and configures tracing properly. We must NOT init
    // a global subscriber here because tracing only allows ONE global init — if we
    // called try_init() here, the config-based init in those commands would silently
    // fail and all logging configuration (level, console, file) would be ignored.
    //
    // Instead, we use a helper function that inits a default subscriber only once
    // and is called by commands that don't have their own config-based init.

    run_command(cli).await
}

/// 双击直启判定：无参（None）或显式 gateway——都按 gateway 语义长驻，
/// 双击独占新控制台时值得无窗重生（见 non-mac 入口的 respawn 块）。
fn is_gateway_launch(cli: &Cli) -> bool {
    matches!(cli.command, None | Some(Commands::Gateway { .. }))
}

/// macOS entry point.
///
/// winit's `EventLoop` must be created and run on the process main thread on
/// macOS (no `with_any_thread` escape hatch). We therefore cannot use
/// `#[tokio::main]` (which owns the main thread for its runtime): instead we
/// build a multi-thread runtime manually, run the gateway on a worker thread,
/// and hand the system tray to the main thread so its event loop runs there.
#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    // Executor role short-circuit (see the non-mac entry for rationale). macOS
    // main is sync, so drive the async executor entrypoint on a current-thread
    // runtime. The executor never needs the main-thread tray handoff.
    if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("executor") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(exec_worker::run());
    }

    // Eval-agent role short-circuit (see the non-mac entry for rationale).
    #[cfg(feature = "eval")]
    {
        if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("eval-agent") {
            common::ensure_default_logger();
            let rt = tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()?;
            return rt.block_on(eval_worker::run());
        }
    }

    // macOS: winit's EventLoop must run on the process main thread. We run the
    // gateway on a dedicated OS thread with its OWN multi-thread runtime and
    // drive it via `block_on` (NOT `tokio::spawn`), so the gateway's future
    // does NOT have to be `Send` — preserving the same property the old
    // `#[tokio::main]`'s `block_on` had (gateway::run holds std MutexGuards
    // across awaits, e.g. CronService). The main thread stays free for the tray.

    // Child mode must run on the main thread (wry/tao also require it on macOS).
    #[cfg(feature = "desktop")]
    if nemesis_desktop::child_mode::has_child_mode_flag() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return match rt.block_on(nemesis_desktop::child_mode::run_child_mode()) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("[Child] Error: {}", e);
                std::process::exit(1);
            }
        };
    }

    // Same `--local` pre-parse as the non-mac entry (Go-compatible: `--local`
    // may appear anywhere; we strip it before clap parsing).
    let mut local_mode = false;
    let filtered_args: Vec<String> = std::env::args()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    let mut cli = Cli::parse_from(filtered_args);
    if local_mode {
        cli.local = true;
        println!("Local mode enabled: using ./.nemesisbot");
    }

    // R1 真机验收修复（2026-08-28）：与非 mac 入口同构 —— 把解析出的 home
    // 同步进 nemesis-path 进程单例（详见非 mac 入口同位置的注释）。
    nemesis_path::default_path_manager().set_home_dir(common::resolve_home(local_mode));

    // Only the Gateway command needs the main-thread tray handoff.
    // 双击直启（2026-09-17）：无参（None）= gateway 语义，同样需要托盘
    // 主线程 handoff。
    if matches!(&cli.command, None | Some(Commands::Gateway { .. })) {
        let tray_rx = nemesis_desktop::main_thread_handoff::init();

        // Run the gateway on a dedicated thread. It builds its own multi-thread
        // runtime and drives run_command via `block_on`, so run_command's future
        // need not be Send (it never had to be under the old #[tokio::main]).
        let gateway_handle = std::thread::Builder::new()
            .name("nemesisbot-gateway".into())
            .spawn(move || {
                let gw_rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                gw_rt.block_on(run_command(cli))
            })
            .expect("failed to spawn gateway thread");

        // Block the main thread (no runtime needed — std channel) until the
        // gateway hands off the tray, or the gateway thread finishes first. In
        // the early-finish case the gateway's TrayChannelGuard has closed the
        // channel, so recv() returns Err and we skip the tray loop entirely.
        let tray_opt = tray_rx.recv().ok();
        if let Some(tray) = tray_opt {
            // Runs the winit EventLoop on the main thread until el.exit()
            // (quit menu item, or request_exit() from the gateway after cleanup).
            tray.run_on_current_thread();
        }

        // Ensure gateway cleanup completes before the process exits.
        return match gateway_handle.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(panic_err) => {
                eprintln!("[main:macos] Gateway thread panicked: {:?}", panic_err);
                std::process::exit(1);
            }
        };
    }

    // Non-Gateway commands: run normally on the main thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_command(cli))
}

/// Shared command dispatch, used by both the `#[tokio::main]` entry
/// (Windows / Linux) and the macOS manual-runtime entry.
async fn run_command(cli: Cli) -> Result<()> {
    // U15: point `yaml:<alias>` api_key references at this home's
    // workspace/config/credentials.yaml. Resolution reads the file
    // per-operation (mirroring `env:VAR`), so one assignment at process
    // start covers every later resolve in this process (gateway runtime,
    // web handlers, cluster agents, CLI commands).
    {
        let cred_home = common::resolve_home(cli.local);
        nemesis_config::credentials::set_global_credentials_path(
            nemesis_config::credentials::credentials_path_for_home(&cred_home),
        );
    }

    // 双击直启（2026-09-17）：无参 = gateway 语义 + auto-init + 自动开
    // Dashboard（BARE_LAUNCH env 标记，gateway 启动完成后消费）。显式
    // `nemesisbot gateway` 不带标记——那是 server 语义，不弹窗口。
    // 在 dispatch 前归一化，全部 match 臂保持原样。
    let command = match cli.command {
        Some(cmd) => cmd,
        None => {
            // SAFETY: 进程启动单线程阶段（dispatch 前），无并发读者。
            unsafe { std::env::set_var(common::BARE_LAUNCH_ENV, "1") };
            Commands::Gateway {
                debug: false,
                quiet: false,
                no_console: false,
                relay: false,
            }
        }
    };

    match command {
        Commands::Onboard { default, args } => {
            // Support both `onboard default` (Go-compatible) and `onboard --default`
            let use_default = default || args.iter().any(|a| a == "default");
            let home = common::resolve_home(cli.local);

            if use_default {
                println!("Initializing NemesisBot with default settings...");
            } else {
                println!("Interactive configuration setup...");
            }

            // 双击直启 goal（2026-09-17）：12 步初始化序列提取到
            // commands/onboard.rs（CLI 覆盖语义与 gateway auto-init 种子
            // 语义共用同一实现；此处 CLI 路径行为不变）。
            commands::onboard::onboard_default(
                &home,
                cli.local,
                commands::onboard::OnboardMode::Cli,
            )?;
        }

        Commands::Gateway {
            debug,
            quiet,
            no_console,
            relay,
        } => {
            // Build extra args for logger from flags
            let mut gateway_args: Vec<String> = Vec::new();
            if debug {
                gateway_args.push("--debug".to_string());
            }
            if quiet {
                gateway_args.push("--quiet".to_string());
            }
            if no_console {
                gateway_args.push("--no-console".to_string());
            }
            commands::gateway::run(cli.local, relay, &gateway_args).await?;
        }
        Commands::Agent {
            subcommand,
            message,
            session,
            debug,
            quiet,
            no_console,
        } => {
            commands::agent::run(
                subcommand, message, session, debug, quiet, no_console, cli.local,
            )
            .await?;
        }
        Commands::Status => {
            common::ensure_default_logger();
            commands::status::run(cli.local)?;
        }
        Commands::History { action } => {
            common::ensure_default_logger();
            commands::history::run(action, cli.local).await?;
        }
        Commands::Session { action } => {
            common::ensure_default_logger();
            commands::session::run(action, cli.local)?;
        }
        Commands::Channel { action } => {
            common::ensure_default_logger();
            commands::channel::run(action, cli.local)?;
        }
        #[cfg(feature = "cluster")]
        Commands::Cluster { action } => {
            common::ensure_default_logger();
            commands::cluster::run(action, cli.local).await?;
        }
        Commands::Cors { action } => {
            common::ensure_default_logger();
            commands::cors::run(action, cli.local)?;
        }
        Commands::Model { action } => {
            common::ensure_default_logger();
            commands::model::run(action, cli.local).await?;
        }
        Commands::Cron { action } => {
            common::ensure_default_logger();
            commands::cron::run(action, cli.local)?;
        }
        Commands::Mcp { action } => {
            common::ensure_default_logger();
            commands::mcp::run(action, cli.local)?;
        }
        #[cfg(feature = "security")]
        Commands::Security { action } => {
            common::ensure_default_logger();
            commands::security::run(action, cli.local).await?;
        }
        Commands::Log { action } => {
            common::ensure_default_logger();
            commands::log::run(action, cli.local)?;
        }
        #[cfg(feature = "auth")]
        Commands::Auth { action } => {
            common::ensure_default_logger();
            commands::auth::run(action, cli.local).await?;
        }
        Commands::Credentials { action } => {
            common::ensure_default_logger();
            commands::credentials::run(action, cli.local).await?;
        }
        Commands::Skills { action } => {
            common::ensure_default_logger();
            commands::skills::run(action, cli.local)?;
        }
        #[cfg(feature = "forge")]
        Commands::Forge { action } => {
            common::ensure_default_logger();
            commands::forge::run(action, cli.local)?;
        }
        #[cfg(feature = "board")]
        Commands::Issue { action } => {
            common::ensure_default_logger();
            commands::issue::run(action, cli.local)?;
        }
        #[cfg(feature = "board")]
        Commands::Autopilot { action } => {
            common::ensure_default_logger();
            commands::autopilot::run(action, cli.local)?;
        }
        #[cfg(feature = "workflow")]
        Commands::Workflow { action } => {
            common::ensure_default_logger();
            commands::workflow::run(action, cli.local)?;
        }
        #[cfg(feature = "security")]
        Commands::Scanner { action } => {
            common::ensure_default_logger();
            commands::scanner::run(action, cli.local).await?;
        }
        #[cfg(feature = "sandbox")]
        Commands::Sandbox { action } => {
            common::ensure_default_logger();
            commands::sandbox::run(action, cli.local).await?;
        }
        #[cfg(feature = "eval")]
        Commands::Eval { action } => {
            common::ensure_default_logger();
            commands::eval::run(action, cli.local).await?;
        }
        #[cfg(feature = "voice")]
        Commands::Voice { action } => {
            common::ensure_default_logger();
            commands::voice::run(action, cli.local)?;
        }
        #[cfg(feature = "memory")]
        Commands::Memory { action } => {
            common::ensure_default_logger();
            commands::memory::run(action, cli.local).await?;
        }
        Commands::Persona { action } => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            let workspace = common::workspace_path(&home);
            commands::persona::run(
                action,
                &home.to_string_lossy(),
                &workspace.to_string_lossy(),
            )
            .await?;
        }
        Commands::Shutdown => {
            common::ensure_default_logger();
            commands::shutdown::run(cli.local)?;
        }
        #[cfg(feature = "migrate")]
        Commands::Migrate { options } => {
            common::ensure_default_logger();
            commands::migrate::run(options, cli.local)?;
        }
        Commands::Version => {
            common::ensure_default_logger();
            common::print_version_info();
        }
        Commands::Dashboard => {
            common::ensure_default_logger();
            if let Err(e) = commands::dashboard::run(cli.local).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Estop { release, status } => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            if let Err(e) = commands::estop::run(&home, release, status).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Run {
            task,
            workspace,
            mode,
            format,
            max_turns,
            model,
        } => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            if let Err(e) =
                commands::run::run(&home, task, workspace, mode, format, max_turns, model).await
            {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        Commands::Acp => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            if let Err(e) = commands::acp::run(&home).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        #[cfg(feature = "desktop")]
        Commands::Test { action } => {
            common::ensure_default_logger();
            commands::test_cmd::run(action).await?;
        }
    }

    Ok(())
}

/// Write fallback minimal config when no embedded config is available.
///
/// [2026-08-27 R9 死码处置注记] 生产侧唯一调用方（CONFIG_DEFAULT 解析的
/// Err 兜底臂）恒不触发——编译期常量 from_str 不可能失败——该臂已删除。
/// 函数保留：tests.rs 的 5 个既有测试以它为对象钉住最小配置 schema，
/// 属"测试保活"状态。恢复生产接线：在 onboard 主配置写入处补 match/Err。
#[allow(dead_code)]
fn write_fallback_config(cfg_path: &std::path::Path) -> anyhow::Result<()> {
    let default_cfg = serde_json::json!({
        "version": "1.0",
        "default_model": "",
        "model_list": [],
        "channels": {
            "web": {"enabled": true, "host": "127.0.0.1", "port": 49000, "auth_token": "276793422"},
            "websocket": {"enabled": true, "host": "127.0.0.1", "port": 49001},
        },
        "agents": {"defaults": {"restrict_to_workspace": false}},
        "security": {"enabled": true},
        "forge": {"enabled": false},
        "logging": {"llm": {"enabled": true, "log_dir": "logs/request_logs", "detail_level": "full"}},
    });
    std::fs::write(
        cfg_path,
        serde_json::to_string_pretty(&default_cfg).unwrap_or_default(),
    )?;
    println!("  Main config saved to {}", cfg_path.display());
    Ok(())
}

// Shared lock for env-mutating tests across nemesisbot's test modules
// (common::tests, commands::migrate::tests). Env is process-global → parallel
// tests race on set_var/set_current_dir; every env-mutating test acquires this
// lock so the binary is reliable under default parallel `cargo test`.
#[cfg(test)]
static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod bridge_client_tests;
#[cfg(test)]
mod tests;
