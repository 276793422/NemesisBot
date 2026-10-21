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
/// P2（2026-10-16）：Agent 持久实体管理器——两阶段创建 / main 幂等 seed /
/// 启动 sweep 收敛 / 项目镜像。记录真相源 = tasks.db agents 表；home/
/// 记忆布局单一真相源 = nemesis-path。无 feature 门（对话组身份是核心
/// 基础设施）；gateway boot 装配（post_agent seed 块）+ 进程槽供 WSAPI /
/// 集群宣告消费。
mod agents;
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
// M3 同因失败签名熔断账本已移 nemesis-board 公开（§6.4D：nemesis-web 手动
// 重派清账与评审链共用同一 JSON，nemesis_board::fail_streak 消费）。
/// 评审取证自取（evidence-on-demand）：board_evidence_read 工具（仅注册进
/// 主 agent——评审链 EvidenceOnly/ReadOnly 白名单消费；档案根围栏只读）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_evidence_tool;
/// 全自动流转 P3/D1：board_issue 工具（仅注册进主 agent——「对 master 说
/// 一句话建单」入口；create 复用 WSAPI issue.create 解析，plan 复用
/// execute_plan_chain 四入口单一真相源）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_issue_tool;
/// Swarm M4：验收 agent 编排（in_review 批作业三态处置；触发挂
/// write_back in_review 路径，评审 LLM 走主 loop 后置装配桥）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_review;
/// 集群专业职能框架 M5：验收链图像工件取回 + vision 评审（从交付/线程
/// 评论提取图像引用束 → 程序化取回 → base64 image part 进评审裸调用；
/// provider 走 default_slot 与主 loop 同源，降级按次回落文本评审）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_review_assets;
/// U-B（2026-10-12 无人值守 §9.2）：LLM 端点不可用恢复探针——封存期最小
/// completion 探测（TCP 通 ≠ LLM 活），解封边沿读中断档案逐单续做
/// （review→复评 / plan→续拆 / project→收口复评）；estop 优先级更高；
/// 数据面/闸在 nemesis-web board_unavailable（同源）。
#[cfg(all(feature = "board", feature = "cluster"))]
mod board_unavailable;
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
/// P5-R1：browser 工具（VM 档 guest 内 headless chromium 的宿主侧入口；
/// 仅 VM 档诚实执行，注册闸 = persistent 路由在场）。
mod browser_tool;
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
/// 行动代理 P0（2026-10-14）：connector_find / connector_invoke 动态工具
/// 对——注册表发现 + 按 connector.toml 声明执行（op_override 投影风险、
/// 统一回执、预算护栏、三绑定 http/tool/action）。模块本体零 security
/// 依赖（SecurityPlugin 槽位 cfg 内联，security 关 = None = 无闸直连），
/// 可独立编译——`connector demo` 在无 security 构建下照常四幕（幕 1 诚实
/// 跳过语义）；**agent 工具供给**的 security 闸在 agent_factory 注册点
/// （无 security 组合无 connector 工具供给面）。`connectors` feature 门控
/// ——nemesis-connectors 整包随它进出（连带 nemesis-security）。
#[cfg(feature = "connectors")]
mod connector_tools;
/// B4（2026-10-18 凭据 vault §7）：凭据索要卡 broker——值不 transit 通道。
/// 门控随 vault_runtime（security feature off = 无 vault 可落，卡片无意义；
/// 触发点槽空走原错误路径零回归）。
#[cfg(feature = "security")]
mod credential_broker;
/// P4 交付门（持久 Agent 实体，2026-10-17）：worker 侧交付分支捕获——
/// 仅 cluster 形态消费（execute_new_task/resume_task 成功收口）。
#[cfg(feature = "cluster")]
mod delivery_gate;
/// P4-c（2026-10-17）：A 端交付账本 web 面（deliveries bridge 实现）——
/// 真相源 tasks.db deliveries 表无 feature 依赖，裁剪形态下查空账本诚实
/// 可用（写入面只在 cluster 回调/评审里产生）。
mod delivery_ledger;
mod embedded;
/// 跨进程 estop 跟随器：独立装配入口（mcp-serve / acp）镜像 gateway 的
/// 急停态（/api/internal estop_status 轮询，gateway 唯一权威源）——堵住
/// 「gateway 急停对独立进程不可达」的安全旁路缺口。
mod estop_follower;
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
/// Forge A 线行为回归执行器（SkillTestRunner 实现 + C 线重放执行面，feature=forge）。
#[cfg(feature = "forge")]
mod forge_test_runner;
/// 集群专业职能框架 M4：generate_image 工具（images-openai lane 唯一消费
/// 点；prompt→b64→落盘 workspace/images/，路径 sanitization 钉死输出根）。
mod image_gen_tool;
/// P23（能力扩展 WS10）：stdio MCP server——NemesisBot 能力经 MCP 协议
/// 暴露给 Claude Code / Cursor 等客户端；K1 式装配与 gateway 同源
/// （安全 8 层全量生效，MCP 出口不是安全旁路）。
mod mcp_serve;
/// 原生插件装载闸 + 签名策略（四个本地 C-ABI 插件 DLL 的统一裁决点）：
/// 开关文件 `<workspace>/config/plugins_native.json`（nemesis-config
/// plugins_native）+ 与 exe 自验同源的三态签名矩阵（enforce=拒载 /
/// warn=放行示警 / dormant=放行不验）+ Dashboard 本地插件库清单
///（status_list / scan_unrecognized）。
mod native_plugins;
/// WASM 插件框架 ↔ gateway 桥（W5-3）：agent 工具桥 / 宿主调用面 / vault
/// 凭据解析 / 安装审批晚绑 / estop 联动。随 `plugins-wasm` feature 门控
/// （该 feature implies security——信任/审计/扫描机制依赖 nemesis-security）。
#[cfg(feature = "plugins-wasm")]
mod plugin_bridge;
/// 用户习惯画像周期任务（自进化升级 B 线）。唯一消费者 = agent_factory
/// 的 forge 闸装配块（反思通道依赖 nemesis-forge）——随 forge feature
/// 同闸，无 forge 构建整模块缺席。
#[cfg(feature = "forge")]
mod profile_service;
/// L6++（2026-09-08）：项目注册表（config/projects.json）+ 项目常驻
/// AgentLoop 管理（对话/项目双分组；注册不拥有——删项目只解除分组）。
/// M1 中间态：registry API 尚无二进制消费方（M2 manager / G4 WSAPI 接线），
/// allow 随接线移除。
#[allow(dead_code)]
mod projects;
/// F7（devtool-upgrade 阶段 5）：Dashboard 结构化提问 broker——question
/// 工具的阻塞端 + WSAPI question.respond/pending 端（同审批 broker 形态）。
mod question_broker;
/// P4-d（2026-10-17）：评审结论 → 交付账本闭环——record_auto_decide 漏斗
/// 尾同步落 deliveries `review` 行 + auto_accept/redispatch 分支裁决 +
/// 项目档案自动导出 `dossier` 行。
#[cfg(all(feature = "board", feature = "cluster"))]
mod review_ledger;
/// A4（2026-10-21 引入）：安全校准——规则层 × guardian LLM
/// 二审一致性标定（`security calibrate`；样本永不执行、报告不改配置）。
/// 消费 nemesis-security 的 pipeline/guardian（security feature 门控面），
/// 与 commands::security 同闸。
#[cfg(feature = "security")]
mod security_calibrate;
/// K1（devtool-upgrade 阶段 4）：SecurityPlugin 构造单一真相源（gateway /
/// headless `run` 共用，安全 9 层在无端口形态不降级）。
mod security_setup;
/// L9 selfupdate 工具面：selfupdate_check（只读）/ selfupdate_apply（提案，
/// 人类闸=编排内 CRITICAL 审批卡）。主 loop 专属注册，编译闸=selfupdate。
#[cfg(feature = "selfupdate")]
mod selfupdate_tool;
/// P0 vault（B1，2026-09-22 计划）：进程级 `vault:<alias>` 解析器注入。
/// 门控随 nemesis-security（security feature off 时 `vault:` 引用诚实报
/// "解析器未安装"，明文/env/yaml 链路不受影响）。
#[cfg(feature = "security")]
mod vault_runtime;
/// 签名验证三态裁决（接入计划 §1/§3，2026-09-23）：resolve_mode 单一真相源
/// + 启动自验单一入口（Cli::parse 后调，enforce 失败 exit 86）+ StartCheck
/// 快照（gateway 审计链 / WSAPI status / 前端徽标数据源）。
mod verify_policy;
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

/// M5（2026-10-10 集群无人值守硬化）：跨节点批复桥 worker 侧（ask 上行
/// 登记/摘除上行 + Dashboard responder 装饰器）。双门：消费方全在
/// gateway 的 security cfg 装配块（web_mgr 在手处）+ 依赖 cluster。
#[cfg(all(feature = "cluster", feature = "security"))]
mod remote_approval;

/// B9（VM 项目工作副本）：gateway 同步引擎——宿主项目目录 ↔ VM
/// /workspace 的工作树对齐（开工全量 + 基线账本 + 排除规则/路径翻译
/// 单点）。消费方 = 项目路由开工闸（projects manager）+ WSAPI/CLI
/// 强制重扫（B9-3）。
#[cfg(feature = "vm")]
mod vm_sync;

/// P5-R2 ④：plugin-vm 宿主装载器（vm-display feature 门控，implies vm
/// ——display_port_query 数据源是 VmSupervisor）。
#[cfg(feature = "vm-display")]
mod vm_display_loader;

/// A1 运行时完整性清单：打包部署的
/// 运行时文件 hash 清单（runtime-manifest.json，构建期由
/// scripts/gen_runtime_manifest.py 生成）+ 启动三态校验。无 feature 门
/// （cargo 开发态 = manifest 缺席态，warn 不阻塞，照常工作）。
mod runtime_manifest;

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
        /// QUIC 纯中继模式（统一入口；与 `quic relay` 分类管理入口同源同一
        /// 实现）：只跑自建 iroh 中继，不起本地 agent/board/集群/discovery。
        /// 读 config.cluster.json roaming.relay 段；token 空 = 拒绝启动
        /// （fail-closed）。需要 roaming-relay feature 编译。
        #[arg(long)]
        quic_relay: bool,
        /// A1 运行时硬化批：显式跳过 runtime-manifest 完整性校验（运维
        /// 逃生门；打包部署残缺默认响亮拒启，跳过必须显式输入）。
        #[arg(long)]
        skip_runtime_check: bool,
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
    /// Expose NemesisBot capabilities as a stdio MCP server (P23; Claude
    /// Code / Cursor etc. connect with zero changes; K1 same-source security)
    McpServe,
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
    #[cfg(feature = "history-search")]
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
    /// Encrypted credential vault（P0）：通用秘密加密存储，list 永不见值。
    #[cfg(feature = "security")]
    Vault {
        #[command(subcommand)]
        action: commands::vault::VaultAction,
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
    /// Manage user habit profile (自进化 B 线)
    Profile {
        #[command(subcommand)]
        action: commands::profile::ProfileAction,
    },
    /// Query the connector registry / receipts / paper ledger (read-only)
    #[cfg(feature = "connectors")]
    Connector {
        #[command(subcommand)]
        action: commands::connector::ConnectorAction,
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
    /// Export a self-contained task dossier (issue tree / project records)
    #[cfg(feature = "board")]
    Dossier {
        #[command(subcommand)]
        action: commands::dossier::DossierAction,
    },
    /// Manage DAG workflows
    #[cfg(feature = "workflow")]
    Workflow {
        #[command(subcommand)]
        action: commands::workflow::WorkflowAction,
    },
    /// Manage virus scanner
    #[cfg(feature = "scanner")]
    Scanner {
        #[command(subcommand)]
        action: commands::scanner::ScannerAction,
    },
    /// Manage WASM plugins (install / enable / sign / trust)
    #[cfg(feature = "plugins-wasm")]
    Plugin {
        #[command(subcommand)]
        action: commands::plugin::PluginAction,
    },
    /// Sandboxie sandbox management (install / uninstall / status)
    #[cfg(feature = "sandbox")]
    Sandbox {
        #[command(subcommand)]
        action: commands::sandbox::SandboxCommand,
    },
    /// QUIC relay server (standalone process / status)
    #[cfg(feature = "roaming-relay")]
    Quic {
        #[command(subcommand)]
        action: commands::quic::QuicAction,
    },
    /// VM environment bootstrap (install QEMU / enable accel / status / build image)
    #[cfg(feature = "vm")]
    Vm {
        #[command(subcommand)]
        action: commands::vm::VmCommand,
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
    /// Version self-update channel (check latest Release / apply via running
    /// gateway / show journal + fuse state)
    #[cfg(feature = "selfupdate")]
    Selfupdate {
        #[command(subcommand)]
        action: commands::selfupdate::SelfupdateCommand,
    },
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

    // 签名验证启动自验（接入计划 §3）：Cli::parse 之后、任何命令装配之前——
    // gateway / run / acp 及其余子命令同源覆盖。必须在双击直启 respawn 之前：
    // enforce 拒启要对启动者可见，不能藏进 detached 子进程。
    // executor / eval-agent 子进程已在上方短路，不重复自验（由已验父进程派生）。
    verify_policy::self_check_and_enforce(local_mode);

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

    // 签名验证启动自验（接入计划 §3）：Cli::parse 之后、任何命令装配之前——
    // gateway / run / acp 及其余子命令同源覆盖。必须在双击直启 respawn 之前：
    // enforce 拒启要对启动者可见，不能藏进 detached 子进程。
    // executor / eval-agent 子进程已在上方短路，不重复自验（由已验父进程派生）。
    verify_policy::self_check_and_enforce(local_mode);

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
        // P0 vault（B1）：`vault:<alias>` 解析器同点注入（见 vault_runtime）。
        #[cfg(feature = "security")]
        crate::vault_runtime::install(&cred_home);
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
                quic_relay: false,
                skip_runtime_check: false,
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
            quic_relay,
            skip_runtime_check,
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
            commands::gateway::run(
                cli.local,
                relay,
                quic_relay,
                &gateway_args,
                skip_runtime_check,
            )
            .await?;
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
        #[cfg(feature = "history-search")]
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
        #[cfg(feature = "security")]
        Commands::Vault { action } => {
            common::ensure_default_logger();
            commands::vault::run(action, cli.local).await?;
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
        Commands::Profile { action } => {
            common::ensure_default_logger();
            commands::profile::run(action, cli.local)?;
        }
        #[cfg(feature = "connectors")]
        Commands::Connector { action } => {
            common::ensure_default_logger();
            commands::connector::run(action, cli.local)?;
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
        #[cfg(feature = "board")]
        Commands::Dossier { action } => {
            commands::dossier::run(action, cli.local)?;
        }
        #[cfg(feature = "workflow")]
        Commands::Workflow { action } => {
            common::ensure_default_logger();
            commands::workflow::run(action, cli.local)?;
        }
        #[cfg(feature = "scanner")]
        Commands::Scanner { action } => {
            common::ensure_default_logger();
            commands::scanner::run(action, cli.local).await?;
        }
        #[cfg(feature = "plugins-wasm")]
        Commands::Plugin { action } => {
            common::ensure_default_logger();
            commands::plugin::run(cli.local, action).await?;
        }
        #[cfg(feature = "sandbox")]
        Commands::Sandbox { action } => {
            common::ensure_default_logger();
            commands::sandbox::run(action, cli.local).await?;
        }
        #[cfg(feature = "roaming-relay")]
        Commands::Quic { action } => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            if let Err(e) = commands::quic::run(&home, action).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        #[cfg(feature = "vm")]
        Commands::Vm { action } => {
            common::ensure_default_logger();
            commands::vm::run(action, cli.local).await?;
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
        #[cfg(feature = "selfupdate")]
        Commands::Selfupdate { action } => {
            common::ensure_default_logger();
            commands::selfupdate::run(action, cli.local).await?;
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
        Commands::McpServe => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            if let Err(e) = commands::mcp_serve::run(&home).await {
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
//
// 获取侧必须 poison-aware（`.lock().unwrap_or_else(|e| e.into_inner())`）：
// 本锁只做串行化，不承载任何与锁体相关的不变量——某个持锁测试 panic 后，
// 后续测试拿到的数据与从未 panic 过时完全一致，毒化标记对它们没有语义，
// `.unwrap()` 只会把 1 个真实失败放大成几十个 PoisonError 假失败（2026-09-24
// nightly run 35995518326 实证：cors 1 个根因 + 21 个级联）。同约定见
// nemesis-agent background_registry 的模块级 TEST_LOCK。
#[cfg(test)]
static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// §2 免打扰（无人值守计划 Part 2）：APPROVAL_AUTO 进程级槽（nemesis-web
// 全局态）触碰类测试的共享串行锁——question_broker / board_bus 等模块的
// 测试会装 enabled 态置换它，并行测试互相交叠会 flake。获取侧同上：
// poison-aware（`.lock().unwrap_or_else(|e| e.into_inner())`）。
#[cfg(test)]
pub(crate) static APPROVAL_AUTO_SLOT_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod bridge_client_tests;
#[cfg(test)]
mod native_plugins_tests;
#[cfg(test)]
mod tests;
