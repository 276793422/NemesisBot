//! NemesisBot - Configuration Management
//!
//! Handles loading, saving, and workspace detection for all configuration.
//! Translated from Go module/config/.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use thiserror::Error;
use tracing::{error, info, warn};

pub mod commands;
pub mod credentials;
pub mod eval_rules;
pub mod hot_reload;
pub mod plugin_wasm;
pub mod plugins_native;
pub mod provider_resolver;
pub mod proxy_file;
pub mod proxy_keys_file;
pub mod store;
pub mod vault_domain;
pub mod vault_ref;

pub use commands::{CommandEntry, CommandsConfig, load_commands_config, save_commands_config};
pub use hot_reload::HotReloader;

// vault（计划）：`vault:<alias>` 引用的前缀常量、全局
// 解析器钩子与通用秘密字段解析（channel/cluster/MCP 消费点共用）。
pub use vault_ref::{
    TEMPLATE_HEAD, TEMPLATE_TAIL, VAULT_PREFIX, VaultResolver, VaultWriter,
    clear_global_vault_resolver, clear_global_vault_writer, global_vault_resolver,
    global_vault_writer, is_secret_reference, resolve_secret_field, resolve_vault_reference,
    set_global_vault_resolver, set_global_vault_writer, strip_reference_forms, strip_templates,
    write_vault_secret,
};

// ②（三期）：vault 别名域绑定（alias→(工具,槽位) 白名单）
// 的配置 loader 与判定——dispatch 凭据闸 v2 臂的统一定义。
pub use vault_domain::{
    DomainBinding, DomainVerdict, DomainViolation, VAULT_DOMAIN_CONFIG_FILE, VaultDomainConfig,
    vault_domain_config_path_in,
};

// Runtime config cache (single source of truth for the live config).
pub use store::{
    ConfigHandle, ConfigStore, config_write_lock, global, load_live, save_live, set_global,
};

// Re-export provider_resolver types and functions for backward compatibility
pub use provider_resolver::{
    ModelResolution, PROVIDER_PRESETS, ProviderPreset, ProviderResolution, ProviderResolver,
    ensure_conversation_role, find_model_by_name, find_provider_preset, get_default_api_base,
    get_effective_llm, get_model_by_name, infer_default_model, infer_provider_from_model,
    normalize_stream_value, provider_preset_ids, resolve_active_timeout_secs, resolve_model_config,
    resolve_model_resolution,
};

#[derive(Error, Debug)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("Config validation error: {0}")]
    Validation(String),
    #[error("Workspace not found")]
    WorkspaceNotFound,
}

pub type Result<T> = std::result::Result<T, ConfigError>;

// ============================================================================
// Flexible string slice deserializer - accepts both strings and numbers
// (mirrors Go's FlexibleStringSlice for config compatibility)
// ============================================================================

/// Custom deserializer for `allow_from` fields that accepts JSON arrays
/// containing mixed types (strings, numbers, etc.), converting all to strings.
/// This matches Go's `FlexibleStringSlice` behavior.
pub fn deserialize_flexible_string_vec<'de, D>(
    deserializer: D,
) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{SeqAccess, Visitor};
    use std::fmt;

    pub struct FlexibleVecVisitor;

    impl<'de> Visitor<'de> for FlexibleVecVisitor {
        type Value = Vec<String>;

        fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
            formatter.write_str("an array of strings or numbers")
        }

        fn visit_seq<A>(self, mut seq: A) -> std::result::Result<Vec<String>, A::Error>
        where
            A: SeqAccess<'de>,
        {
            let mut result = Vec::new();
            while let Some(value) = seq.next_element::<serde_json::Value>()? {
                match value {
                    serde_json::Value::String(s) => result.push(s),
                    serde_json::Value::Number(n) => result.push(format!("{}", n)),
                    other => result.push(other.to_string()),
                }
            }
            Ok(result)
        }
    }

    deserializer.deserialize_seq(FlexibleVecVisitor)
}

/// Custom deserializer for numeric fields that accepts both JSON numbers and
/// decimal strings. Exists because historical CLI writers (`nemesisbot channel
/// web port N`) serialized ports as strings while typed readers expected
/// numbers — a present-but-wrong-typed field otherwise fails the whole config
/// parse at startup (see BUG ledger #41).
pub fn deserialize_flexible_i64<'de, D>(deserializer: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    use serde::de::{Error, Unexpected};

    match serde_json::Value::deserialize(deserializer)? {
        serde_json::Value::Number(n) => n.as_i64().ok_or_else(|| {
            D::Error::invalid_type(Unexpected::Other("non-integer number"), &"an integer")
        }),
        serde_json::Value::String(s) => s.trim().parse::<i64>().map_err(|_| {
            D::Error::invalid_type(Unexpected::Str(&s), &"an integer or decimal string")
        }),
        other => Err(D::Error::invalid_type(
            match &other {
                serde_json::Value::Bool(b) => Unexpected::Bool(*b),
                serde_json::Value::Array(_) => Unexpected::Seq,
                serde_json::Value::Object(_) => Unexpected::Map,
                serde_json::Value::Null => Unexpected::Unit,
                _ => unreachable!(),
            },
            &"an integer or decimal string",
        )),
    }
}

// ============================================================================
// Port normalization (config i64 fields -> assembly-domain u16)
// ============================================================================

/// config 端口字段的越域错误（`config_port_to_u16` 返回）。
#[derive(Debug, thiserror::Error)]
#[error("config {field} 端口越域: {value}（合法 0..=65535）")]
pub struct PortRangeError {
    pub field: String,
    pub value: i64,
}

/// config 端口字段（`i64`——[`deserialize_flexible_i64`] 兼容历史 CLI 写入
/// 的字符串形态）→ 装配域 `u16` 归一。越域（<0 / >65535）= 约定外输入，
/// **loud 拒绝**：裸 `as u16` 会把 70000 静默回绕成 4464、-1 回绕成 65535，
/// bind 到用户从未配置过的端口。Dashboard/CLI 保存面已有 u64 域验域习语
/// （web handlers/cluster 同款），此防线管手编 config.json 直改路径。
pub fn config_port_to_u16(value: i64, field: &str) -> std::result::Result<u16, PortRangeError> {
    u16::try_from(value).map_err(|_| PortRangeError {
        field: field.to_string(),
        value,
    })
}

// ============================================================================
// Main Config struct - mirrors Go Config exactly
// ============================================================================

/// Top-level application configuration.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Config {
    #[serde(default)]
    pub agents: AgentsConfig,
    #[serde(default)]
    pub bindings: Vec<AgentBinding>,
    #[serde(default)]
    pub session: SessionConfig,
    #[serde(default)]
    pub channels: ChannelsConfig,
    #[serde(default)]
    pub model_list: Vec<ModelConfig>,
    #[serde(default)]
    pub gateway: GatewayConfig,
    #[serde(default)]
    pub tools: ToolsConfig,
    #[serde(default)]
    pub heartbeat: HeartbeatConfig,
    #[serde(default)]
    pub devices: DevicesConfig,
    #[serde(default)]
    pub logging: Option<LoggingConfig>,
    #[serde(default)]
    pub security: Option<SecurityFlagConfig>,
    #[serde(default)]
    pub skills: Option<SkillsConfig>,
    #[serde(default)]
    pub forge: Option<ForgeFlagConfig>,
    /// 用户习惯画像（自进化升级 B 线）：enabled 默认关；阈值/上限/衰减
    /// 可调（缺省见 user_profile 模块常量）。全本地采集，仅向前不回溯。
    #[serde(default)]
    pub user_profile: Option<UserProfileConfig>,
    #[serde(default)]
    pub cluster: Option<ClusterFlagConfig>,
    #[serde(default)]
    pub memory: Option<MemoryFlagConfig>,
    #[serde(default)]
    pub mcp: Option<McpConfig>,
    #[serde(default)]
    pub executor: Option<ExecutorSeparationConfig>,
    #[serde(default)]
    pub debug: Option<DebugConfig>,
    /// 托管 Agent 看板旗标（None = 全默认——超时兜底 3600s）。
    #[serde(default)]
    pub board: Option<BoardFlagConfig>,
    /// 使用统计保留策略（None = 全默认——30 天 rollup，无条数上限）。
    #[serde(default)]
    pub usage: Option<UsageConfig>,
    /// PTY 内嵌终端（None = 全默认——**enabled=false 关**，双闸的另一闸
    /// 是 `terminal` cargo feature）。
    #[serde(default)]
    pub terminal: Option<TerminalConfig>,
    /// workflow 引擎运行期参数（None = 全默认——墙钟保险丝 3600s，0=关；
    /// A 批批 4）。
    #[serde(default)]
    pub workflow: Option<WorkflowRuntimeConfig>,
    /// 项目注册表参数（对话/项目双分组；None = 全默认——上限 4）。
    #[serde(default)]
    pub projects: Option<ProjectsConfig>,
    /// 反向桥配置（None = 全默认——服务端接入门不开放、客户端不连接）。
    #[serde(default)]
    pub bridge: Option<BridgeConfig>,
    /// Dashboard 前端表现（皮肤等；None = 全默认——内置 static 皮肤）。
    #[serde(default)]
    pub ui: Option<UiConfig>,
    /// 行动代理 connector 注册表（PLAN 行动代理；None = 全默认——
    /// enabled=true，预算 fail-closed：资金限额 0=拒绝所有）。
    #[serde(default)]
    pub connectors: Option<ConnectorsConfig>,
    /// 紫微系统（三方程序受控接管，【万物皆工具】；None = 全默认——
    /// **enabled=false 总闸关**，对齐 terminal.enabled 双闸模式：另一闸是
    /// `ziwei` cargo feature）。见 [`ZiweiConfig`]。
    #[serde(default)]
    pub ziwei: Option<ZiweiConfig>,
    /// 健康监测旗标（None = 全默认——磁盘水位开，warn 5GB / critical
    /// 1GB / 60s；critical 只升级告警面不做自动 estop）。见
    /// [`HealthFlagConfig`]。
    #[serde(default)]
    pub health: Option<HealthFlagConfig>,
    /// 日志六族 TTL 清扫（/ + 回收站；None = 全默认——
    /// session_logs 30d / boundary 30d / request_logs 14d / terminal 7d /
    /// event_ledger 30d / trash 90d；0 = 该族关闭清扫）。见
    /// [`LogsTtlConfig`]。
    #[serde(default)]
    pub logs: Option<LogsTtlConfig>,
    /// checkpoint gc（None = 全默认——**enabled=false 总闸关**，删错 =
    /// 回退网失效宁默认关；keep_turns 200 / keep_days 7）。见
    /// [`CheckpointConfig`]。
    #[serde(default)]
    pub checkpoint: Option<CheckpointConfig>,
    /// WASM 插件宿主开关段（`plugins.wasm.enabled`，缺省 true）。typed
    /// 字段化保证 save_config 往返不丢键（cache_warmer 同款预防，
    /// ）：本键由 `crate::plugin_wasm:runtime_enabled`
    /// 经原始 JSON pointer 消费、全仓无写入点（手编 config.json 是唯一
    /// 设置途径）——不建模则任何 typed 保存（dashboard config/models/
    /// skills/profile 事务、save_live、ConfigStore::update 等）静默抹掉
    /// 用户显式禁用，子系统静默重启用。
    #[serde(default)]
    pub plugins: Option<PluginsHostConfig>,
    /// 浏览器接管段（spawn 浏览器选择 kind/executable；None = 全默认
    /// auto）。typed 字段化防 set_field round-trip 丢键（plugins 段同款）。
    /// 见 [`BrowserConfig`]。
    #[serde(default)]
    pub browser: Option<BrowserConfig>,
    /// 目标系统段（评估闸/权限全局开关；None = 全默认——
    /// allow_agent_disable=true）。typed 字段化防 set_field round-trip
    /// 丢键（plugins/browser 段同款）。见 [`GoalsConfig`]。
    #[serde(default)]
    pub goals: Option<GoalsConfig>,
    /// 资产盘点段（W1 #11+#6–10 AI-BOM 快照周期刷新；None = 全默认——
    /// 300s）。typed 字段化防 set_field round-trip 丢键（plugins/browser
    /// 段同款）。见 [`AssetsConfig`]。
    #[serde(default)]
    pub assets: Option<AssetsConfig>,
}

/// `config.json` 的 `"plugins"` 段（WASM 插件宿主开关面；
/// typed 字段化——先前该段不被顶层 [`Config`] 建模，typed 保存静默丢弃
/// 整段。plugin_wasm.rs 模块注释早已点名该机制（panel_llm 因此落 side
/// file），主闸 `plugins.wasm.enabled` 却仍留在裸读面）。段内未建模键经
/// flatten extra 原样回吐（ModelConfig.extra 同款保真）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginsHostConfig {
    /// WASM 插件子系统运行期总闸（缺省 true = 未表态放行；显式 false =
    /// 禁用。消费方 `crate::plugin_wasm:runtime_enabled` 原始 pointer
    /// 现读，本字段只保证保存面不丢键）。
    #[serde(default = "default_plugins_wasm_enabled")]
    pub wasm: PluginsWasmHostConfig,
    /// 段内未建模键保真（round-trip 原样回吐，不收敛丢键）。
    #[serde(flatten)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// `plugins.wasm` 子段（仅运行期消费的 `enabled` 键）。
#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct PluginsWasmHostConfig {
    /// 缺省 true（未表态 = 开；显式 false = 禁用——typed 保存必须保留）。
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for PluginsWasmHostConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

/// `plugins.wasm` 缺键默认（enabled=true，与 runtime_enabled 的
/// `unwrap_or(true)` 同源口径）。
fn default_plugins_wasm_enabled() -> PluginsWasmHostConfig {
    PluginsWasmHostConfig { enabled: true }
}

// 机械拆分：按配置域分文件。纯代码搬家，行为零变更；
// 各子模块 pub 项经 pub use 保持原 nemesis_config:路径不变。
#[path = "sections/agents.rs"]
mod agents;
#[path = "sections/board.rs"]
mod board;
#[path = "sections/channels.rs"]
mod channels;
#[path = "sections/executor.rs"]
mod executor;
#[path = "sections/io.rs"]
mod io;
#[path = "sections/logging.rs"]
mod logging;
#[path = "sections/misc.rs"]
mod misc;
#[path = "sections/models.rs"]
mod models;
#[path = "sections/scanner.rs"]
mod scanner;
#[path = "sections/security.rs"]
mod security;
#[path = "sections/skills_mcp.rs"]
mod skills_mcp;

pub use agents::*;
pub use board::*;
pub use channels::*;
pub use executor::*;
pub use io::*;
pub use logging::*;
pub use misc::*;
pub use models::*;
pub use scanner::*;
pub use security::*;
pub use skills_mcp::*;

impl Config {
    /// Get the workspace path from config.
    pub fn workspace_path(&self) -> String {
        let ws = &self.agents.defaults.workspace;
        // `~\` 反斜杠形态与 `~/` 同判（展开）——此前只认
        // `~/`，nemesis-path 四个 resolve_*_config_path 消费方对 `~\mybots`
        // 拿到字面量落 cwd 下 `~` 目录。默认拼法 `~\.nemesisbot\workspace`
        // 在加载期已被 adjust_paths_for_environment 换成绝对路径，这里兜的
        // 是非默认自定义形态。
        if (ws.starts_with("~/") || ws.starts_with("~\\"))
            && let Some(home) = dirs::home_dir()
        {
            return home.join(&ws[2..]).to_string_lossy().to_string();
        }
        if ws.is_empty() {
            return WorkspaceResolver::resolve(false)
                .join("workspace")
                .to_string_lossy()
                .to_string();
        }
        ws.clone()
    }

    /// Find a model configuration by model name or vendor/model prefix.
    pub fn get_model_by_model_name(&self, model_ref: &str) -> Result<&ModelConfig> {
        // 层序与 provider_resolver::model_ref_eq doc 恒等：精确 model_name →
        // 精确 model 字段 → CI model_name → CI model 字段（CI 兜底层 =
        // BUG-20261117-077，与查找族其余站点同口径）。
        for ci in [false, true] {
            // First try exact match with model_name
            for mc in &self.model_list {
                if provider_resolver::model_ref_eq(&mc.model_name, model_ref, ci) {
                    return Ok(mc);
                }
            }

            // Then try prefix match with model field (vendor/model)
            for mc in &self.model_list {
                if provider_resolver::model_ref_eq(&mc.model, model_ref, ci) {
                    return Ok(mc);
                }
            }
        }

        Err(ConfigError::Validation(format!(
            "model {:?} not found in model_list",
            model_ref
        )))
    }

    /// Get model configuration for a given model name.
    /// Alias for get_model_by_model_name.
    pub fn get_model_config(&self, model_name: &str) -> Result<&ModelConfig> {
        self.get_model_by_model_name(model_name)
    }

    /// Populate deprecated fields from new fields for backward compatibility.
    ///
    /// This mirrors the Go `postProcessForCompatibility` function.
    /// （BUG-20261117-066）曾有的 `sync_to_web` 从 `sync_to` 派生臂已随
    /// 该死旋钮一并删除：bool 全仓零读者，Go 蓝本兼容残留；sync 路由
    /// 单一真相源是 `sync_to`（web_init setup_sync_targets 消费）。
    pub fn post_process_for_compatibility(&mut self) {
        // autonomy=autonomous 前置要求 unlimited_mode=true——
        // 防「监察环自动重派开着、评审 FAIL 却转人工」的矛盾态。加载期矛盾
        // 诚实降级为 assisted + WARN（安全方向回退，网关不被一个旗标卡死——
        // 沙盒未就绪回退 Layer 1 同款先例）；面板保存路径硬拒（nemesis-web
        // board_config_set 两个方向都拦）。
        if let Some(board) = self.board.as_mut()
            && board.autonomy == BoardAutonomy::Autonomous
            && !board.unlimited_mode
        {
            warn!(
                "[Config] board.autonomy=autonomous 但 unlimited_mode=false（矛盾态）：降级为 assisted 生效（请在面板开启 unlimited_mode 后再切 autonomous）"
            );
            board.autonomy = BoardAutonomy::Assisted;
        }
        // 批一①（无人值守加固）：unlimited + 预算保险丝三维全关的组合
        // WARN（纯可观测不拦截——用户显式选择不是错误）。谓词单一真相源
        // BoardFlagConfig::unlimited_without_budget_fuse（sections/board.rs）。
        if let Some(board) = self.board.as_ref()
            && board.unlimited_without_budget_fuse()
        {
            warn!(
                "[Config] board.unlimited_mode=true 且预算保险丝三维全关（max_total_redispatch=0 / wall_clock_budget_secs=0 / max_tokens_per_parent=0）：FAIL 重派无资源上限，签名逐轮翻转的失败不在任何停车闸内。无人值守请配置 board.budget.*（0=关该维是刻意的，后果自担）；estop 可随时止血"
            );
        }
    }

    /// Adjust hardcoded default paths to respect NEMESISBOT_HOME.
    ///
    /// This mirrors the Go `adjustPathsForEnvironment` function:
    /// - If the workspace uses a default path, replace it with the actual resolved path.
    /// - Set a default log directory if none is specified.
    pub fn adjust_paths_for_environment(&mut self) {
        let expected_workspace = WorkspaceResolver::resolve(false).join("workspace");

        // Check if workspace is using a default path.：旧
        // `ends_with(".nemesisbot/workspace")` 把任何以该后缀结尾的绝对路径
        // （如项目内自定义 D:/bots/proj/.nemesisbot/workspace）误判成默认
        // 形态、每次加载强改回默认；且 Windows 下 save_config 落盘的反斜杠
        // 相对形态（.nemesisbot\workspace）永不命中正斜杠后缀，保存→加载
        // 回路在 Windows 上断裂。改为相对形态的 Path 组件相等（分隔符/
        // 大小写免疫、拒绝绝对路径堂亲），外加波浪号默认拼法的两种分隔符。
        let ws = &self.agents.defaults.workspace;
        let is_default = ws.is_empty()
            || ws == "~/.nemesisbot/workspace"
            || ws == "~\\.nemesisbot\\workspace"
            || Path::new(ws) == Path::new(".nemesisbot").join("workspace");

        if is_default {
            self.agents.defaults.workspace = expected_workspace.to_string_lossy().to_string();
        }

        // Default log directory
        if let Some(ref mut logging) = self.logging
            && let Some(ref mut llm) = logging.llm
            && llm.log_dir.is_empty()
        {
            llm.log_dir = "logs/request_logs".to_string();
        }
    }
}

// ============================================================================
// Sub-config load/save functions
// ============================================================================

fn default_security_action() -> String {
    "deny".to_string()
}
fn default_approval_timeout() -> i64 {
    300
}
// 死键恢复区（用户指令恢复保留，未接线，见 SecurityConfig 字段区注释）
fn default_max_pending() -> i64 {
    100
}
fn default_audit_retention() -> i64 {
    90
}
fn default_cache_max_size() -> i64 {
    50
}
fn default_cache_ttl_seconds() -> i64 {
    300
}
fn default_max_concurrent_searches() -> i64 {
    2
}
fn default_search_limit() -> i64 {
    50
}

// ============================================================================
// Default value functions
// ============================================================================

fn default_true() -> bool {
    true
}

/// SEC-002 Debug 脱敏 helper：凭据字段的 Debug 形态（与 WebChannelConfig /
/// ModelConfig 手写 Debug 的既有形态逐字一致）。空值 `<empty>`、非空
/// `<redacted>`——诊断信息（配置了什么）以 config.json / Dashboard 回显
/// 为准，日志不落明文凭据。
pub(crate) fn debug_secret_value(v: &str) -> &'static str {
    if v.is_empty() {
        "<empty>"
    } else {
        "<redacted>"
    }
}
/// `agents.prompt_system` 缺省值 = pro（段落池组装的新体系）。
fn default_prompt_system() -> String {
    "pro".to_string()
}
fn default_max_tokens() -> i64 {
    // 统一定义：nemesis-types capability（8192→64K，
    // max-tokens-8192截断撞顶复发）
    nemesis_types::capability::DEFAULT_MAX_TOKENS
}
fn default_temperature() -> f64 {
    0.7
}
fn default_max_tool_iterations() -> i64 {
    100
}
/// default concurrent mode is **steer** — busy sessions accept new input
/// inline (the model sees the latest user intent mid-turn). 2026-11-03
/// product decision (BUG doc「busy期间输入全面锁死」): the previous default
/// "queue" let a web-UI inbox-status refresh failure dead-lock the frontend
/// into permanent reject (busy send gate + textarea disable + send button
/// hide all keyed off a stale mode snapshot with no self-heal), so the
/// default flips to the open mode and the backend adjudicates per turn.
/// Applies ONLY when config.json omits the key (serde default); explicit
/// `"reject"` / `"queue"` configs keep their behavior. Unknown strings still
/// fail safe to reject at `parse_concurrent_mode` (nemesis-agent).
fn default_concurrent_request_mode() -> String {
    "steer".to_string()
}

fn default_snapshot_role() -> String {
    "user".to_string()
}
fn default_queue_size() -> i64 {
    8
}

/// 优雅停机 drain 预算缺省 30s（0=跳过；见 [`AgentDefaults::shutdown_drain_secs`]）。
fn default_shutdown_drain_secs() -> u64 {
    30
}

/// D-4（多会话并行清账）：单 loop 并发 turn 上限缺省 8。泵统一
/// 后跨会话并发是结构不变量，此缺省给极端场景一个有界并发兜底；0 显式
/// 关闭限制。
fn default_max_concurrent_turns() -> i64 {
    8
}
fn default_spill_retention_days() -> i64 {
    7
}
/// 429 重试环默认次数（对齐业界口径，默认 10）。
fn default_rate_limit_retries() -> i64 {
    10
}

/// （compaction）：逐字尾巴 token 预算缺省值（统一定义：
/// serde 缺省 / raw-JSON 解析 / Default impl 三方共用）。对齐业界通行的
/// keepRecentTokens=20000 口径。
pub const DEFAULT_COMPACT_KEEP_RECENT_TOKENS: i64 = 20_000;

/// [`DEFAULT_COMPACT_KEEP_RECENT_TOKENS`] 的 serde 缺省函数形态。
fn default_compact_keep_recent_tokens() -> i64 {
    DEFAULT_COMPACT_KEEP_RECENT_TOKENS
}

/// raw config JSON 读取 `agents.defaults.compact_keep_recent_tokens`。
///
/// AgentLoop 的 compact 域走 fresh-read raw JSON（同 `current_max_tokens`
/// 模式，运行中改键下一轮生效），不经过 typed `Config`——键路径与缺省值
/// 必须和 typed 字段单源，这里就是那个单源。缺键/非数 → 缺省 20000；
/// 负数视为 0（回退旧按条数路径，语义见 compact 域）。
pub fn resolve_compact_keep_recent_tokens(cfg: Option<&serde_json::Value>) -> usize {
    cfg.and_then(|v| {
        v.get("agents")?
            .get("defaults")?
            .get("compact_keep_recent_tokens")?
            .as_i64()
    })
    .map(|n| n.max(0) as usize)
    .unwrap_or(DEFAULT_COMPACT_KEEP_RECENT_TOKENS as usize)
}

/// B17（2026-11-08 七轮调研落地批次③）：raw config JSON 读取
/// `agents.defaults.compact_boundary`（任务边界压缩开关）。
///
/// 与 [`resolve_compact_keep_recent_tokens`] 同模式——AgentLoop compact
/// 域 fresh-read raw JSON，键路径与缺省值必须和 typed 字段单源，这里就是
/// 那个单源。缺键/缺段/非布尔 → 缺省 **false**（off；压缩行为变更先试点，
/// 计划文档钦定）。
pub fn resolve_compact_boundary(cfg: Option<&serde_json::Value>) -> bool {
    cfg.and_then(|v| {
        v.get("agents")?
            .get("defaults")?
            .get("compact_boundary")?
            .as_bool()
    })
    .unwrap_or(false)
}

/// B30（2026-11-06 三方对比整改）：工具结果统一 cap 缺省值（单一真相源：
/// serde 缺省 / raw-JSON 解析两方共用）。对齐业界通行 8K 字符封顶口径。
pub const DEFAULT_TOOL_RESULT_CAP: usize = 8_000;

/// [`DEFAULT_TOOL_RESULT_CAP`] 的 serde 缺省函数形态（typed 字段 u64）。
fn default_tool_result_cap() -> u64 {
    DEFAULT_TOOL_RESULT_CAP as u64
}

/// B30：raw config JSON 读取 `agents.tool_result_cap`。
///
/// AgentLoop 的 dispatch 后整形点走 fresh-read raw JSON（同
/// `resolve_compact_keep_recent_tokens` 模式，运行中改键下一轮生效），不
/// 经过 typed `Config`——键路径与缺省值必须和 typed 字段单源，这里就是
/// 那个单源。缺键/缺段/非数 → 缺省 8000；**0 = 关**（跨工具兜底封顶关
/// 闭，per-tool 语义窗口照常）；负数无语义 → 钳 0（关闭）。
pub fn resolve_tool_result_cap(cfg: Option<&serde_json::Value>) -> usize {
    cfg.and_then(|v| v.get("agents")?.get("tool_result_cap")?.as_i64())
        .map(|n| n.max(0) as usize)
        .unwrap_or(DEFAULT_TOOL_RESULT_CAP)
}

// ---------------------------------------------------------------------------
// W1 安全图谱地基批（2026-11-19）：#26/#45/#46/#25/#30 缺省常量 + 解析器。
// 单一真相源纪律：typed serde 缺省（sections/misc.rs）与 raw-JSON 解析
// （下方 resolve_*）共用同一张常量表——两路解析方向相反但缺省同值。
// ---------------------------------------------------------------------------

/// #26 聊天面扫描入站缺省模式（只观测不阻断——用户是受信主体，防误报）。
pub const DEFAULT_CHAT_SCAN_INBOUND: &str = "log";
/// #26 聊天面扫描出站缺省模式（红线脱敏：High/Medium 置信凭据命中即遮蔽）。
pub const DEFAULT_CHAT_SCAN_OUTBOUND: &str = "redact";
/// #45 工具结果注入扫描缺省模式（high/critical 命中追加诚实尾注）。
pub const DEFAULT_RESULT_SCAN_MODE: &str = "note";
/// #25 网关流控缺省 QPS。
pub const DEFAULT_RATE_LIMIT_QPS: u32 = 20;
/// #25 网关流控缺省突发桶容量。
pub const DEFAULT_RATE_LIMIT_BURST: u32 = 40;
/// #25 网关流控缺省每凭据并发流上限。
pub const DEFAULT_RATE_LIMIT_MAX_CONCURRENT_STREAMS: u32 = 4;
/// #30 用量配额缺省达限行为。
pub const DEFAULT_BUDGET_MODE: &str = "reject";

/// [`DEFAULT_RESULT_SCAN_MODE`] 的 serde 缺省函数形态。
fn default_result_scan_mode() -> String {
    DEFAULT_RESULT_SCAN_MODE.to_string()
}

/// #2 安全分级缺省（basic = 现形态）。
pub const DEFAULT_SECURITY_LEVEL: &str = "basic";

/// [`DEFAULT_SECURITY_LEVEL`] 的 serde 缺省函数形态。
fn default_security_level() -> String {
    DEFAULT_SECURITY_LEVEL.to_string()
}

/// #26 聊天面扫描解析后的方向语义。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChatScanMode {
    /// 命中只告警（tracing WARN + 审计），不改写内容。
    Log,
    /// 命中即遮蔽（redact_content）+ 尾注告知。
    Redact,
    /// 关闭该方向扫描。
    Off,
}

/// #26：raw config JSON 读取 `security.chat_scan` 两方向模式。
///
/// typed 字段（`SecurityFlagConfig.chat_scan`）与本解析共用常量表。未知
/// 字符串值按该方向缺省处理（不做静默纠偏到 off——缺省是安全侧语义）。
/// `security.enabled=false` 不影响本键（聊天面扫描独立于 8 层管线总闸，
/// 有独立模式开关本身就是它的闸）。
pub fn resolve_chat_scan(cfg: Option<&serde_json::Value>) -> (ChatScanMode, ChatScanMode) {
    let parse = |v: Option<&serde_json::Value>, default: &str| match v.and_then(|x| x.as_str()) {
        Some("log") => ChatScanMode::Log,
        Some("redact") => ChatScanMode::Redact,
        Some("off") => ChatScanMode::Off,
        _ if default == "redact" => ChatScanMode::Redact,
        _ => ChatScanMode::Log,
    };
    let sec = cfg
        .and_then(|v| v.get("security"))
        .and_then(|v| v.get("chat_scan"));
    (
        parse(
            sec.and_then(|v| v.get("inbound")),
            DEFAULT_CHAT_SCAN_INBOUND,
        ),
        parse(
            sec.and_then(|v| v.get("outbound")),
            DEFAULT_CHAT_SCAN_OUTBOUND,
        ),
    )
}

/// #46：raw config JSON 读取 `security.untrusted_wrap`（默认 **true**——
/// 读不到/键缺失按开处理，与 image_downscale 同极性：包裹能力保持可用）。
pub fn resolve_untrusted_wrap(cfg: Option<&serde_json::Value>) -> bool {
    cfg.and_then(|v| v.get("security"))
        .and_then(|s| s.get("untrusted_wrap"))
        .and_then(|b| b.as_bool())
        .unwrap_or(true)
}

/// #45 工具结果注入扫描模式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultScanMode {
    /// high/critical 命中追加诚实尾注（不阻断——数据已执行）。
    Note,
    /// 关闭。
    Off,
}

/// #45：raw config JSON 读取 `security.result_scan`（缺省 `"note"`；未知值
/// 回落缺省）。
pub fn resolve_result_scan(cfg: Option<&serde_json::Value>) -> ResultScanMode {
    match cfg
        .and_then(|v| v.get("security"))
        .and_then(|s| s.get("result_scan"))
        .and_then(|x| x.as_str())
    {
        Some("off") => ResultScanMode::Off,
        Some("note") => ResultScanMode::Note,
        _ => ResultScanMode::Note,
    }
}

// ---------------------------------------------------------------------------
// #2 安全分级（W2 批，2026-11-20 拍板：三档基线按草案）
// ---------------------------------------------------------------------------

/// 安全分级档位。`security.level`，缺省 [`SecurityLevel::Basic`]（= 现形态，
/// 升级零行为突变）。档位是**起点不是锁死**：各开关显式键优先于档位表
/// （resolver 族的缺席缺省值按档位查表，显式写的键照常生效）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum SecurityLevel {
    /// 出厂默认：8 层管线开，W1 开关维持缺省，流控/配额/隔离关。
    #[default]
    Basic,
    /// Basic + 网关流控开 + 日/月 token 配额开 + charter 未登记 WARN。
    Standard,
    /// Standard + 执行体隔离开 + 默认 plan 模式 + MCP/技能装前扫描强制。
    Strict,
}

impl SecurityLevel {
    /// 按字符串解析（`"basic"`/`"standard"`/`"strict"`，大小写不敏感）。
    /// 未知值 → None（调用方决定回落或报警告——安全档位配错静默降级
    /// basic 有「以为 strict 实际 basic」的风险，装配面必须响）。
    pub fn parse(s: &str) -> Option<Self> {
        match s.trim().to_ascii_lowercase().as_str() {
            "basic" => Some(SecurityLevel::Basic),
            "standard" => Some(SecurityLevel::Standard),
            "strict" => Some(SecurityLevel::Strict),
            _ => None,
        }
    }

    /// 档位规范小写形态（wire 宣告 / 审计盖章 / 显示投影用——单一真相源，
    /// 防各处手写小写字面量漂移）。
    pub fn as_str(self) -> &'static str {
        match self {
            SecurityLevel::Basic => "basic",
            SecurityLevel::Standard => "standard",
            SecurityLevel::Strict => "strict",
        }
    }

    /// #25 流控 enabled 缺席缺省（basic 关 = 现形态；standard+ 开）。
    pub fn rate_limit_enabled_default(self) -> bool {
        matches!(self, SecurityLevel::Standard | SecurityLevel::Strict)
    }

    /// #30 日 token 配额缺席缺省（0 = 关；standard+ 2M ≈ 100 轮深度会话
    /// ×20 的余量，烧新钱前停车的护栏而非精确配额）。费用维不设档位缺省
    /// （模型价差大，定死数字反而误伤——显式配置才生效）。
    pub fn daily_token_limit_default(self) -> u64 {
        match self {
            SecurityLevel::Basic => 0,
            SecurityLevel::Standard | SecurityLevel::Strict => 2_000_000,
        }
    }

    /// #30 月 token 配额缺席缺省（日限 ×10 的量级：日限是护栏，月限是
    /// 撑爆预算的最终闸，给 10 倍余量覆盖突发 legitimate 用量）。
    pub fn monthly_token_limit_default(self) -> u64 {
        match self {
            SecurityLevel::Basic => 0,
            SecurityLevel::Standard | SecurityLevel::Strict => 20_000_000,
        }
    }

    /// 执行体隔离开关缺席缺省（`executor.enabled`；strict 开）。
    pub fn executor_enabled_default(self) -> bool {
        self == SecurityLevel::Strict
    }

    /// 默认对话模式缺席缺省（strict 默认 plan——写类工具闸先收紧，用户
    /// 显式 `/build` 或 `chat.set_mode` 放开）。
    pub fn default_chat_mode(self) -> &'static str {
        match self {
            SecurityLevel::Strict => "plan",
            _ => "build",
        }
    }

    /// MCP/技能装前扫描是否强制（#14 闸消费；strict 强制不可关）。
    pub fn preinstall_scan_required(self) -> bool {
        self == SecurityLevel::Strict
    }

    /// charter（用途登记）是否要求（standard+：缺席只 WARN 不拦截——
    /// 登记是建议面不是闸门）。
    pub fn charter_required(self) -> bool {
        matches!(self, SecurityLevel::Standard | SecurityLevel::Strict)
    }
}

/// #2：raw config JSON 解析档位 + 非法值注记。合法值（含大小写变体）→
/// 对应档位；键缺席 → (Basic, None)；**键在场但值非法 → (Basic, Some(原文))**
/// ——静默降级的风险必须在装配面响出来。
pub fn resolve_security_level_checked(
    cfg: Option<&serde_json::Value>,
) -> (SecurityLevel, Option<String>) {
    match cfg
        .and_then(|v| v.get("security"))
        .and_then(|s| s.get("level"))
        .and_then(|x| x.as_str())
    {
        None => (SecurityLevel::Basic, None),
        Some(raw) => match SecurityLevel::parse(raw) {
            Some(l) => (l, None),
            None => (SecurityLevel::Basic, Some(raw.to_string())),
        },
    }
}

/// #2：档位解析便捷形态（丢弃非法值注记——测试/展示面用；生产装配走
/// `resolve_security_level_checked`）。
pub fn resolve_security_level(cfg: Option<&serde_json::Value>) -> SecurityLevel {
    resolve_security_level_checked(cfg).0
}

/// #2：typed 形态档位解析（ConfigHandle 已解析的 `security` 段）。
/// `SecurityFlagConfig.level` 的 serde 缺省 = `"basic"`，与「键缺席 →
/// Basic」同语义；非法值 → Basic（响铃职责在装配面
/// `resolve_security_level_checked` 的 WARN——typed 读丢失了「非法」
/// 区分度，只适合装配后的旁路消费，如默认对话模式）。
pub fn security_level_from_flag(flag: Option<&SecurityFlagConfig>) -> SecurityLevel {
    flag.and_then(|f| SecurityLevel::parse(&f.level))
        .unwrap_or_default()
}

/// #2：执行体隔离总开关（`executor.enabled`）。**键在场粒度**：显式键
/// 照读（显式 false 永远生效——档位是起点不是锁死）；键缺席（含段缺席
/// 与段在场无键两态）→ 按档位查表（strict 开）。
///
/// ⚠ 消费面纪律：此键的档位联动依赖「键缺席可观测」，typed
/// `ExecutorSeparationConfig.enabled`（`#[serde(default)]` bool）无法区分
/// 「缺席」与「显式 false」，所以生产消费点一律走本 resolver 读 raw
/// config JSON（exec_world / formatter / gateway 装配 WARN 三处同源）；
/// 出厂模板（config.default.json）也已摘除 `executor.enabled` 键——模板
/// 显式写 false 会让 strict 档缺省永不生效。
pub fn resolve_executor_enabled(cfg: Option<&serde_json::Value>) -> bool {
    cfg.and_then(|v| v.get("executor"))
        .and_then(|e| e.get("enabled"))
        .and_then(|b| b.as_bool())
        .unwrap_or_else(|| resolve_security_level(cfg).executor_enabled_default())
}

/// #25 网关流控解析后的运行参数。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RateLimitSettings {
    pub enabled: bool,
    pub qps: u32,
    pub burst: u32,
    pub max_concurrent_streams: u32,
}

/// #25：raw config JSON 读取 `security.rate_limit`。先按 typed
/// `RateLimitConfig` 逐键解析（缺键 = 字段缺省），再经
/// `RateLimitConfig::settings()` 投影（钳制单一真相源：qps/burst 钳下界
/// 1——0 无语义 = 回落缺省，防误配把网关整体关死；`max_concurrent_streams`
/// 0 = 不限并发只限 QPS，有语义，透传）。
pub fn resolve_rate_limit(cfg: Option<&serde_json::Value>) -> RateLimitSettings {
    let rl = cfg
        .and_then(|v| v.get("security"))
        .and_then(|v| v.get("rate_limit"));
    let num = |key: &str, default: u32| -> u32 {
        rl.and_then(|v| v.get(key))
            .and_then(|x| x.as_u64())
            .map(|n| n as u32)
            .unwrap_or(default)
    };
    // #2：enabled 缺席按档位（standard+ 开）；显式 false 照常生效（档位是
    // 起点不是锁死）。
    let level = resolve_security_level(cfg);
    RateLimitConfig {
        enabled: rl
            .and_then(|v| v.get("enabled"))
            .and_then(|b| b.as_bool())
            .unwrap_or_else(|| level.rate_limit_enabled_default()),
        qps: num("qps", DEFAULT_RATE_LIMIT_QPS),
        burst: num("burst", DEFAULT_RATE_LIMIT_BURST),
        max_concurrent_streams: num(
            "max_concurrent_streams",
            DEFAULT_RATE_LIMIT_MAX_CONCURRENT_STREAMS,
        ),
    }
    .settings()
}

/// #30 用量配额解析后的运行参数。
#[derive(Debug, Clone, PartialEq)]
pub struct UsageBudgetSettings {
    pub daily_token_limit: u64,
    pub monthly_token_limit: u64,
    pub daily_cost_limit_usd: f64,
    pub monthly_cost_limit_usd: f64,
    /// true = reject（达限终局停车）；false = warn（只告警）。
    pub reject: bool,
}

/// #30：raw config JSON 读取 `usage.budget`。token/费用四维：显式键照读
/// （显式 0 = 关是合法意志），**键缺席按安全档位查表**（#2：basic 全 0 =
/// 现形态；standard+ 日 2M/月 20M）。费用维无档位缺省（模型价差大）。
pub fn resolve_usage_budget(cfg: Option<&serde_json::Value>) -> UsageBudgetSettings {
    let b = cfg
        .and_then(|v| v.get("usage"))
        .and_then(|v| v.get("budget"));
    let level = resolve_security_level(cfg);
    // 显式在场才读值（区分「显式 0 = 关」与「缺席 = 按档位」）。
    let u64_of = |key: &str, level_default: u64| -> u64 {
        b.and_then(|v| v.get(key))
            .and_then(|x| x.as_u64())
            .unwrap_or(level_default)
    };
    let f64_of = |key: &str| -> f64 {
        b.and_then(|v| v.get(key))
            .and_then(|x| x.as_f64())
            .unwrap_or(0.0)
            .max(0.0)
    };
    UsageBudgetSettings {
        daily_token_limit: u64_of("daily_token_limit", level.daily_token_limit_default()),
        monthly_token_limit: u64_of("monthly_token_limit", level.monthly_token_limit_default()),
        daily_cost_limit_usd: f64_of("daily_cost_limit_usd"),
        monthly_cost_limit_usd: f64_of("monthly_cost_limit_usd"),
        reject: b
            .and_then(|v| v.get("mode"))
            .and_then(|x| x.as_str())
            .map(|m| m != "warn")
            .unwrap_or(true),
    }
}

fn default_gateway_host() -> String {
    "0.0.0.0".to_string()
}
fn default_gateway_port() -> i64 {
    18790
}
fn default_web_host() -> String {
    "0.0.0.0".to_string()
}
fn default_web_port() -> i64 {
    8080
}
fn default_web_path() -> String {
    "/ws".to_string()
}
fn default_heartbeat_interval() -> i64 {
    30
}
fn default_session_timeout() -> i64 {
    3600
}
fn default_heartbeat_interval_minutes() -> i64 {
    30
}
fn default_max_results() -> i64 {
    5
}
fn default_detail_level() -> String {
    "full".to_string()
}
fn default_log_level() -> String {
    "INFO".to_string()
}
fn default_mcp_timeout() -> i64 {
    30
}

// ============================================================================
// Workspace detection and config loading
// ============================================================================

// Single shared process-global-state lock for ALL tests in this crate that touch
// `std::env:set_var` / `set_current_dir` / load config (which reads env). These
// mutate/read PROCESS-GLOBAL state, so under parallel test execution they race:
// a writer in one test module pollutes the env a reader in another module sees.
// Both `tests` and `extra_tests` `use super::GLOBAL_STATE_LOCK` so they share
// ONE mutex (previously each module had its own → cross-module races still flaked
// under parallel). With this single lock, `cargo test -p nemesis-config` (default
// parallel) is as reliable as `--test-threads=1` for these tests.
#[cfg(test)]
static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;

#[cfg(test)]
mod bridge_serde_tests;

#[cfg(test)]
mod eval_rules_tests;

#[cfg(test)]
mod tool_result_cap_tests;

// W1 安全图谱地基批（2026-11-19）：chat_scan / untrusted_wrap /
// result_scan / rate_limit / usage.budget 配置面测试。
#[cfg(test)]
mod security_scan_tests;

#[cfg(test)]
mod port_normalization_tests;

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod mcp_serde_tests;

// compact_keep_recent_tokens 配置面（serde 缺省 /
// roundtrip / raw-JSON 解析）测试。
#[cfg(test)]
mod ws2_compact_config_tests;

// dispatch_fallback 三档化：旧布尔保真 + 字符串档位 serde
// 兼容测试。
#[cfg(test)]
mod dispatch_fallback_mode_tests;

// memory.recall_gate 配置节（项2 P2 召回质量闸）：缺席 = None /
// 部分键落默认 / roundtrip 零漂移。
#[cfg(test)]
mod recall_gate_config_tests;

// 人工触点考勤（board.attendance）：serde 缺省 / mode 归一 / kind 等待窗
// 测试。
#[cfg(test)]
mod attendance_tests;

// 自治模式总开关：serde 缺省 / 矛盾态加载期降级测试。
#[cfg(test)]
mod autonomy_tests;

// （桌面级 VM）：executor.vm 段配置面（serde 缺省铁律 / roundtrip /
// validate loud 拒绝）测试。
#[cfg(test)]
mod vm_config_tests;

// 原生插件开关（plugins_native）：目录身份 / 开关文件缺省与 RMW 语义 /
// 损坏保守放行 / 即时生效标记。
#[cfg(test)]
mod plugins_native_tests;

// wasm 插件开关（plugin_wasm）：缺省不加载 / fail-closed / 旧实例文件
// 迁移 / RMW 语义。
#[cfg(test)]
mod plugin_wasm_tests;

// 独立代理配置（proxy_file）：纯读不落盘 / RMW 段互不覆盖 / 损坏重建 /
// 前缀校验 / env 注入清单配对。
#[cfg(test)]
mod proxy_file_tests;

// LLM 代理调用方 key（proxy_keys_file）：缺文件空表不落盘 / RMW 查重 /
// 启停幂等 / 损坏重建 / 宽和读回落。
#[cfg(test)]
mod proxy_keys_file_tests;

// HotReloader force_reload 语义（写方自刷新，绕过 mtime 检测）测试。
#[cfg(test)]
mod hot_reload_tests;

// SEC-002 Debug 脱敏回归（BUG-20261111-068）：通道/模型配置结构
// Debug 输出不落明文凭据。
#[cfg(test)]
mod channels_debug_redaction_tests;

// 无人值守加固配置面：unlimited+预算全关组合谓词（批一①）/
// transient_retry 两键（批二）/ board.notify 段（批三）默认与解析。
#[cfg(test)]
mod unattended_hardening_tests;
