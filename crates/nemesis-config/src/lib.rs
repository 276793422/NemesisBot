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
pub mod hot_reload;
pub mod provider_resolver;
pub mod store;

pub use commands::{CommandEntry, CommandsConfig, load_commands_config, save_commands_config};
pub use hot_reload::HotReloader;

// Runtime config cache (single source of truth for the live config).
pub use store::{ConfigHandle, ConfigStore, global, load_live, save_live, set_global};

// Re-export provider_resolver types and functions for backward compatibility
pub use provider_resolver::{
    ModelResolution, ProviderResolution, ProviderResolver, find_model_by_name,
    get_default_api_base, get_effective_llm, get_model_by_name, infer_default_model,
    infer_provider_from_model, resolve_model_config, resolve_model_resolution,
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
    /// 托管 Agent 看板旗标（W2 P4；None = 全默认——超时兜底 3600s）。
    #[serde(default)]
    pub board: Option<BoardFlagConfig>,
    /// 使用统计保留策略（A3；None = 全默认——30 天 rollup，无条数上限）。
    #[serde(default)]
    pub usage: Option<UsageConfig>,
}

/// 使用统计保留策略（`config.json` 的 `usage` 段；`#[serde(default)]`
/// 每字段全可省）。gateway 周期任务（启动 + 每 6h）对 request_logs 执行
/// retention sweep：到期行 rollup 进 daily_rollups 后删除；条数超限按
/// 最旧优先直接裁。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct UsageConfig {
    /// 明细行保留天数（0 = 不按天清理，只 rollup？——0 视为「永不按天
    /// 清理」；默认 30）。rollup 聚合不受此值影响：到期行先聚合再删。
    pub retention_days: i64,
    /// 明细行条数上限（0 = 无上限；默认 500_000）。超出按 created_at
    /// 最旧优先直接删除。
    pub max_rows: i64,
}

impl Default for UsageConfig {
    fn default() -> Self {
        Self {
            retention_days: 30,
            max_rows: 500_000,
        }
    }
}

/// 看板系统旗标（`config.json` 的 `board` 段；`#[serde(default)]` 每字段全可省）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardFlagConfig {
    /// 派发超时兜底秒数：dispatched 超过该时长未回报 → sweep 标 failed +
    /// 评论 + dispatch_failed 通知（0 = 关闭 sweep）。
    pub dispatch_timeout_secs: u64,
    /// sweep 扫描间隔秒数（默认 20s；不暴露到配置模板，供测试调快）。
    pub dispatch_sweep_interval_secs: u64,
    /// 指派后自动派发（W2.5 接口预留；默认 false = 不自动派发）。用户拍板
    /// （2026-08-31）：现阶段不做自动派发，但留出接口——置 true 后，
    /// `issue.assign` 指派给 worker 成功即自动走派发单一入口
    /// （dispatch_issue_core；失败不回滚指派，⛔ 系统评论留痕）。
    pub auto_dispatch: bool,
}

impl Default for BoardFlagConfig {
    fn default() -> Self {
        Self {
            dispatch_timeout_secs: 3600,
            dispatch_sweep_interval_secs: 20,
            auto_dispatch: false,
        }
    }
}

// ============================================================================
// Executor Separation Config (执行体剥离 + 沙盒开关)
// ============================================================================

/// Executor separation + sandbox switch (Layer 1 / Layer 2). See
/// `docs/PLAN/2026-07-08_executor-separation.md`.
///
/// - `enabled`: Layer 1 — run execution-class tools (exec/file/grep/git/...) in
///   a separate child process (per-call spawn). `false` = today's in-process
///   behavior (safe fallback).
/// - `sandbox`: Layer 2 — wrap the child spawn with Sandboxie `Start.exe /box:`.
///   Requires `enabled = true` AND Sandboxie installed (next phase). The gateway
///   probes for Start.exe and falls back if absent.
/// - `allow_network`: Sandboxie box-level network switch — writes
///   `AllowNetworkAccess=y/n` into Sandboxie.ini for the NemesisBox box. Applies
///   to every process in the box (the executor child AND any program the user
///   launches from the in-box explorer). Independent of `enabled`/`sandbox`.
///   A flip takes effect for NEWLY started box processes after `Start.exe
///   /reload`; already-running box processes keep their old network state until
///   restarted (Sandboxie's WFP callout reads a per-process `BlockInternet`
///   cache that reload does not refresh).
/// - `strict`（P5-2 严格模式）: fail-closed 闸门 — `sandbox=true` 被要求但本机
///   沙盒后端不可用（Windows=Sandboxie 未就绪；Linux=landlock/bwrap 均不可用；
///   裁剪构建=`sandbox` feature 被裁掉）时**拒绝执行高危工具**并明确报错，
///   而非默认的 warn + 无盒降级（fail-open）。注意 Partial 强制（后端可用但
///   有能力缺口，如 landlock 不覆盖网络）**不算不可用**——严格模式放行并
///   在日志/状态里如实标注缺口。默认 false = 现状字节不变。开关经
///   ConfigStore 翻转后对后续工具调用实时生效（与 `sandbox` 同链路）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecutorSeparationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub strict: bool,
}

// ============================================================================
// Debug Config (diagnostic capture)
// ============================================================================

/// Debug/diagnostic switches. Currently only `capture` — the failure-triggered
/// diagnostic capture sink for the intermittent "context error → session
/// corruption → LLM no-response" bug. Capture writes only on failure signals,
/// so the happy path has zero disk overhead; it defaults to enabled.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DebugConfig {
    #[serde(default)]
    pub capture: CaptureConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureConfig {
    /// Enable failure-triggered diagnostic capture (writes to
    /// `logs/capture/{session_key}/{ts}_{signal}/` only when a failure signal
    /// fires — LLM retry exhausted / context overflow / session overwrite /
    /// agent error funnel).
    #[serde(default = "default_true")]
    pub enabled: bool,
}

impl Default for CaptureConfig {
    fn default() -> Self {
        Self { enabled: true }
    }
}

// ============================================================================
// Agents Config
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AgentsConfig {
    #[serde(default)]
    pub defaults: AgentDefaults,
    #[serde(default)]
    pub list: Vec<AgentConfigEntry>,
    /// H7 (U13 half): Claude Code delegation tool settings. Default
    /// disabled (opt-in) — absent section = tool never registered.
    #[serde(default)]
    pub claude_code_tool: ClaudeCodeToolConfig,
    /// I4 (U13 other half): Codex delegation tool settings (same shape).
    #[serde(default)]
    pub codex_tool: CodexToolConfig,
    /// L1 (U19): read-only LSP tool settings. Default disabled (opt-in) —
    /// and even when enabled, the tool registers only if at least one
    /// language server (rust-analyzer/gopls/…) is found on PATH.
    #[serde(default)]
    pub lsp_tool: LspToolConfig,
    /// Y1 (Phase4-a): semantic tool-documentation folding. Default off —
    /// absent section = tool defs render byte-identical to the pre-Y1 build.
    #[serde(default)]
    pub tool_doc_folding: ToolDocFoldingConfig,
}

/// Y1 (Phase4-a): `agents.tool_doc_folding` config section. When enabled,
/// the `expand_top_n` tools most similar to the current conversation (cosine
/// between the latest user message and each tool description, via the P3.1
/// embed backend) keep their full description; the rest collapse to a
/// one-line summary. Orthogonal to tier supply — folding rewrites
/// description TEXT only, every tool stays callable with its full parameter
/// schema. The Mini tier never participates (13-tool core set, nothing to
/// save). Requires a wired memory manager (embed backend); without one the
/// loop degrades to unfolded defs.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDocFoldingConfig {
    /// Master switch (default false).
    #[serde(default)]
    pub enabled: bool,
    /// How many tools stay fully expanded, ranked by similarity
    /// (default 8; ties break alphabetically for determinism).
    #[serde(default = "default_tool_doc_folding_top_n")]
    pub expand_top_n: usize,
}

fn default_tool_doc_folding_top_n() -> usize {
    8
}

impl Default for ToolDocFoldingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            expand_top_n: default_tool_doc_folding_top_n(),
        }
    }
}

/// L1 (U19): `agents.lsp_tool` config section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LspToolConfig {
    /// Enable the `lsp` semantic-code tool (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Per-request LSP timeout in seconds (default 120 — first queries wait
    /// behind server indexing on big repos).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// Idle session reap threshold in seconds (default 600). Sessions idle
    /// longer than this get shut down lazily on the next query.
    #[serde(default)]
    pub idle_secs: Option<u64>,
}

/// I4: `agents.codex_tool` config section (mirrors claude_code_tool).
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CodexToolConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// T5 (U13): fixed sandbox tier — `read_only` | `workspace_write` |
    /// `danger_full_access` (default `read_only`). Config-side snake_case,
    /// mapped to codex's kebab-case CLI value (`--sandbox read-only` etc.) at
    /// spawn. Empty/unknown → default (fail-safe). NOT model-selectable.
    #[serde(default)]
    pub sandbox: String,
}

/// H7 (U13 half): `agents.claude_code_tool` config section.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ClaudeCodeToolConfig {
    /// Enable the claude_code delegation tool (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Per-delegation wall-clock timeout in seconds (default 300).
    #[serde(default)]
    pub timeout_secs: Option<u64>,
    /// T5 (U13): fixed permission tier — `default` | `accept_edits` | `plan`
    /// | `bypass_permissions` (default `accept_edits`, the
    /// non-interactive-safe tier) → `--permission-mode <tier>` at spawn.
    /// Empty/unknown → default (fail-safe). NOT model-selectable.
    #[serde(default)]
    pub permission_mode: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentDefaults {
    #[serde(default)]
    pub workspace: String,
    #[serde(default = "default_true")]
    pub restrict_to_workspace: bool,
    #[serde(default)]
    pub llm: String,
    #[serde(default)]
    pub image_model: String,
    #[serde(default)]
    pub image_model_fallbacks: Vec<String>,
    #[serde(default = "default_max_tokens")]
    pub max_tokens: i64,
    #[serde(default = "default_temperature")]
    pub temperature: f64,
    #[serde(default = "default_max_tool_iterations")]
    pub max_tool_iterations: i64,
    #[serde(default = "default_concurrent_request_mode")]
    pub concurrent_request_mode: String,
    /// Full-review M4: context-snapshot message role. "user" (default,
    /// dsh-aligned) places the merged snapshot as a user message; some
    /// strict chat templates reject adjacent user/user pairs — set
    /// "system" to restore the pre-I2 single system-role injection.
    #[serde(default = "default_snapshot_role")]
    pub snapshot_role: String,
    #[serde(default = "default_queue_size")]
    pub queue_size: i64,
    #[serde(default)]
    pub max_continuation_permits: i64,
    /// U4: tool-result spill retention (days). Spill files under
    /// `<home>/logs/spill/` older than this are deleted (startup scan +
    /// daily midnight task in agent_factory). 0 = never clean up. Default 7.
    #[serde(default = "default_spill_retention_days")]
    pub spill_retention_days: i64,
}

impl Default for AgentDefaults {
    fn default() -> Self {
        Self {
            workspace: String::new(),
            restrict_to_workspace: true,
            llm: String::new(),
            image_model: String::new(),
            image_model_fallbacks: vec![],
            max_tokens: default_max_tokens(),
            temperature: default_temperature(),
            max_tool_iterations: default_max_tool_iterations(),
            concurrent_request_mode: default_concurrent_request_mode(),
            snapshot_role: default_snapshot_role(),
            queue_size: default_queue_size(),
            max_continuation_permits: 0,
            spill_retention_days: default_spill_retention_days(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct AgentConfigEntry {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub default: bool,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub workspace: String,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
    #[serde(default)]
    pub skills: Vec<String>,
    #[serde(default)]
    pub subagents: Option<SubagentsConfig>,
}

#[derive(Debug, Clone, Serialize)]
pub struct AgentModelConfig {
    #[serde(default)]
    pub primary: String,
    #[serde(default)]
    pub fallbacks: Vec<String>,
}

/// Support both string and structured model config during deserialization.
impl<'de> serde::Deserialize<'de> for AgentModelConfig {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        use serde::de::{self, Visitor};
        use std::fmt;

        pub struct AgentModelConfigVisitor;

        impl<'de> Visitor<'de> for AgentModelConfigVisitor {
            type Value = AgentModelConfig;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                write!(f, "a string or an object with primary and fallbacks")
            }

            fn visit_str<E: de::Error>(self, v: &str) -> std::result::Result<Self::Value, E> {
                Ok(AgentModelConfig {
                    primary: v.to_string(),
                    fallbacks: vec![],
                })
            }

            fn visit_map<A: serde::de::MapAccess<'de>>(
                self,
                map: A,
            ) -> std::result::Result<Self::Value, A::Error> {
                #[derive(Deserialize)]
                struct Raw {
                    #[serde(default)]
                    primary: String,
                    #[serde(default)]
                    fallbacks: Vec<String>,
                }
                let raw = Raw::deserialize(serde::de::value::MapAccessDeserializer::new(map))?;
                Ok(AgentModelConfig {
                    primary: raw.primary,
                    fallbacks: raw.fallbacks,
                })
            }
        }

        deserializer.deserialize_any(AgentModelConfigVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentsConfig {
    #[serde(default)]
    pub allow_agents: Vec<String>,
    #[serde(default)]
    pub model: Option<AgentModelConfig>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AgentBinding {
    #[serde(default)]
    pub agent_id: String,
    pub r#match: BindingMatch,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BindingMatch {
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub account_id: String,
    #[serde(default)]
    pub peer: Option<PeerMatch>,
    #[serde(default)]
    pub guild_id: String,
    #[serde(default)]
    pub team_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerMatch {
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub id: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SessionConfig {
    #[serde(default)]
    pub dm_scope: String,
    #[serde(default)]
    pub identity_links: std::collections::HashMap<String, Vec<String>>,
}

// ============================================================================
// Channels Config
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ChannelsConfig {
    #[serde(default)]
    pub whatsapp: WhatsAppConfig,
    #[serde(default)]
    pub telegram: TelegramConfig,
    #[serde(default)]
    pub feishu: FeishuConfig,
    #[serde(default)]
    pub discord: DiscordConfig,
    #[serde(default)]
    pub maixcam: MaixCamConfig,
    #[serde(default)]
    pub qq: QqConfig,
    #[serde(default)]
    pub dingtalk: DingTalkConfig,
    #[serde(default)]
    pub slack: SlackConfig,
    #[serde(default)]
    pub line: LineConfig,
    #[serde(default)]
    pub onebot: OneBotConfig,
    #[serde(default)]
    pub web: WebChannelConfig,
    #[serde(default)]
    pub websocket: WebSocketChannelConfig,
    #[serde(default)]
    pub external: ExternalConfig,
}

// Channel configuration structs follow below.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bridge_url: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TelegramConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct FeishuConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default)]
    pub encrypt_key: String,
    #[serde(default)]
    pub verification_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DiscordConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MaixCamConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    #[serde(default)]
    pub port: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct QqConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub app_id: String,
    #[serde(default)]
    pub app_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DingTalkConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub client_id: String,
    #[serde(default)]
    pub client_secret: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SlackConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bot_token: String,
    #[serde(default)]
    pub app_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct LineConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub channel_secret: String,
    #[serde(default)]
    pub channel_access_token: String,
    #[serde(default)]
    pub webhook_host: String,
    #[serde(default)]
    pub webhook_port: i64,
    #[serde(default)]
    pub webhook_path: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct OneBotConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub ws_url: String,
    #[serde(default)]
    pub access_token: String,
    #[serde(default)]
    pub reconnect_interval: i64,
    #[serde(default)]
    pub group_trigger_prefix: Vec<String>,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WebChannelConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_web_host")]
    pub host: String,
    // flexible_i64: 历史 CLI 把端口写成字符串（BUG #41），容忍两种磁盘形态
    #[serde(
        default = "default_web_port",
        deserialize_with = "deserialize_flexible_i64"
    )]
    pub port: i64,
    #[serde(default = "default_web_path")]
    pub path: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default = "default_heartbeat_interval")]
    pub heartbeat_interval: i64,
    #[serde(default = "default_session_timeout")]
    pub session_timeout: i64,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for WebChannelConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            host: default_web_host(),
            port: default_web_port(),
            path: default_web_path(),
            auth_token: String::new(),
            allow_from: vec![],
            heartbeat_interval: default_heartbeat_interval(),
            session_timeout: default_session_timeout(),
            sync_to: vec![],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WebSocketChannelConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub host: String,
    // flexible_i64: 历史 CLI 把端口写成字符串（BUG #41），容忍两种磁盘形态
    #[serde(default, deserialize_with = "deserialize_flexible_i64")]
    pub port: i64,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub auth_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
    #[serde(default)]
    pub sync_to_web: bool,
    #[serde(default)]
    pub web_session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ExternalConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub input_exe: String,
    #[serde(default)]
    pub output_exe: String,
    #[serde(default)]
    pub chat_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
    #[serde(default)]
    pub sync_to_web: bool,
    #[serde(default)]
    pub web_session_id: String,
}

// ============================================================================
// Model Config
// ============================================================================

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ModelConfig {
    #[serde(default)]
    pub model_name: String,
    #[serde(default)]
    pub model: String,
    #[serde(default)]
    pub api_base: String,
    #[serde(default)]
    pub api_key: String,
    #[serde(default)]
    pub proxy: String,
    #[serde(default)]
    pub auth_method: String,
    #[serde(default)]
    pub connect_mode: String,
    #[serde(default)]
    pub workspace: String,
    /// H4 (U16 half): reasoning-effort tier for this model ("off" | "low" |
    /// "medium" | "high"). Empty (the default) = send nothing and let the
    /// provider default apply. Written by `nemesisbot model set-effort`.
    #[serde(default)]
    pub reasoning_effort: String,
    /// Passthrough for untyped per-model keys（raw-Value 层约定：`model_tier` /
    /// `vision` / `vision_probe` / `max_output_tokens` 及未来键）。**没有这个
    /// flatten 时，任何 typed `save_config` 都会静默丢掉这些键**（dashboard
    /// models/config/channels/forge handler、save_live、credentials 迁移全走
    /// typed 保存）——2026-09-03 二次回归根修。空表序列化时跳过（输出与
    /// 既有形态字节一致）。
    /// L7（2026-09-04 四轮盲审）：BTreeMap——flatten map 的键序决定
    /// `save_config` 落盘字节；HashMap 每进程随机种子 → 同一份 config
    /// 两次保存字节不同（无谓 diff / mtime reload 抖动），BTreeMap 键序
    /// 确定，round-trip 字节稳定。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl ModelConfig {
    pub fn validate(&self) -> Result<()> {
        if self.model_name.is_empty() {
            return Err(ConfigError::Validation("model_name is required".into()));
        }
        if self.model.is_empty() {
            return Err(ConfigError::Validation("model is required".into()));
        }
        Ok(())
    }

    /// Parse the model string to extract protocol and model identifier.
    /// Format: [protocol/]model-identifier (e.g., "openai/gpt-4o")
    pub fn parse_model(&self) -> (&str, &str) {
        if let Some(slash_pos) = self.model.find('/') {
            (&self.model[..slash_pos], &self.model[slash_pos + 1..])
        } else {
            ("openai", &self.model)
        }
    }
}

// ============================================================================
// Gateway, Tools, etc.
// ============================================================================

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct GatewayConfig {
    #[serde(default = "default_gateway_host")]
    pub host: String,
    #[serde(default = "default_gateway_port")]
    pub port: i64,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        Self {
            host: default_gateway_host(),
            port: default_gateway_port(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ToolsConfig {
    #[serde(default)]
    pub web: WebToolsConfig,
    #[serde(default)]
    pub cron: CronToolsConfig,
    #[serde(default)]
    pub exec: ExecConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct WebToolsConfig {
    #[serde(default)]
    pub brave: BraveConfig,
    #[serde(default)]
    pub duckduckgo: DuckDuckGoConfig,
    #[serde(default)]
    pub perplexity: PerplexityConfig,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct BraveConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DuckDuckGoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct PerplexityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CronToolsConfig {
    #[serde(default)]
    pub exec_timeout_minutes: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ExecConfig {
    #[serde(default)]
    pub enable_deny_patterns: bool,
    #[serde(default)]
    pub custom_deny_patterns: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HeartbeatConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_heartbeat_interval_minutes")]
    pub interval: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DevicesConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub monitor_usb: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LoggingConfig {
    pub llm: Option<LlmLogConfig>,
    pub general: Option<GeneralLogConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct LlmLogConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub log_dir: String,
    #[serde(default = "default_detail_level")]
    pub detail_level: String,
    #[serde(default)]
    pub save_raw: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GeneralLogConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_true")]
    pub enable_console: bool,
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default)]
    pub file: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ForgeFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClusterFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct MemoryFlagConfig {
    #[serde(default)]
    pub enabled: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Whether `skill_manage` writes require interactive approval (default false).
    #[serde(default)]
    pub manage_approval: bool,
}

/// MCP 主配置（config.mcp.json 顶层）。**单一真相源**（2026-08-31 收敛）：
/// 此前 nemesis-config 与 nemesis-mcp 各持一份服务器配置结构（`env:
/// Vec<String>` vs `Option<Vec<String>>`、`timeout: i64` vs
/// `timeout_secs: u64`），同一份 config.mcp.json 出现两种解析结果——用户
/// 实盘文件里的 `"env": null` 让 Dashboard 侧直接解析崩溃（"invalid type:
/// null, expected a sequence"），MCP runtime 却一切正常。现在本结构是唯一
/// 定义，nemesis-mcp 直接复用（`pub type ServerConfig =
/// McpServerConfig`）；反序列化全量宽容（见 [`mcp_config_from_value`]）。
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct McpConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub servers: Vec<McpServerConfig>,
    #[serde(default = "default_mcp_timeout")]
    pub timeout: i64,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
            timeout: default_mcp_timeout(),
        }
    }
}

impl<'de> serde::Deserialize<'de> for McpConfig {
    fn deserialize<D>(deserializer: D) -> std::result::Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        let v = serde_json::Value::deserialize(deserializer)?;
        mcp_config_from_value(v).map_err(serde::de::Error::custom)
    }
}

/// 宽容解析 config.mcp.json（2026-08-31 根因修复："invalid type: null,
/// expected a sequence" 一类解析崩溃的统一出口）。接受三种顶层形状：
/// 1. 本项目标准形状 `{enabled, servers: [...], timeout}`；
/// 2. Claude Desktop 生态形状 `{"mcpServers": {name: {...}}}`（map，name
///    缺省时从 key 取；数组亦可）——LLM 抄外部配置进盘也能吃；
/// 3. 仅顶层字段（无 servers/mcpServers）——视为空配置，不再报错。
///    env/args/headers/tags 逐字段宽容（`null`/map/数组皆收，见
///    [`flexible_string_list`]）；全局与 per-server timeout 皆宽容
///    （`null`/字符串数字 → 默认值）。
fn mcp_config_from_value(v: serde_json::Value) -> std::result::Result<McpConfig, String> {
    let obj = match v {
        serde_json::Value::Object(o) => o,
        _ => return Err("MCP config must be a JSON object".to_string()),
    };

    if obj.contains_key("servers") {
        #[derive(serde::Deserialize)]
        struct StandardShape {
            #[serde(default)]
            enabled: bool,
            #[serde(default)]
            servers: Vec<McpServerConfig>,
            #[serde(default = "default_mcp_timeout", deserialize_with = "flexible_i64")]
            timeout: i64,
        }
        let shape: StandardShape =
            serde_json::from_value(serde_json::Value::Object(obj)).map_err(|e| e.to_string())?;
        return Ok(McpConfig {
            enabled: shape.enabled,
            servers: shape.servers,
            timeout: shape.timeout,
        });
    }

    if let Some(ms) = obj.get("mcpServers") {
        let mut servers = Vec::new();
        match ms {
            serde_json::Value::Object(map) => {
                for (key, sv) in map {
                    let mut entry = sv.clone();
                    if entry.get("name").is_none() {
                        entry["name"] = serde_json::Value::String(key.clone());
                    }
                    servers.push(
                        serde_json::from_value::<McpServerConfig>(entry)
                            .map_err(|e| format!("mcpServers[{key}]: {e}"))?,
                    );
                }
            }
            serde_json::Value::Array(arr) => {
                for (i, sv) in arr.iter().enumerate() {
                    servers.push(
                        serde_json::from_value::<McpServerConfig>(sv.clone())
                            .map_err(|e| format!("mcpServers[{i}]: {e}"))?,
                    );
                }
            }
            _ => return Err("`mcpServers` must be an object or array".to_string()),
        }
        // 外部生态配置文件的语义是「配置存在即启用」；顶层 enabled 显式声明则优先。
        let enabled = obj.get("enabled").and_then(|b| b.as_bool()).unwrap_or(true);
        return Ok(McpConfig {
            enabled,
            servers,
            timeout: default_mcp_timeout(),
        });
    }

    // 无 servers/mcpServers：仅开关等顶层字段（容忍空/残缺配置）。
    let enabled = obj
        .get("enabled")
        .and_then(|b| b.as_bool())
        .unwrap_or(false);
    Ok(McpConfig {
        enabled,
        servers: Vec::new(),
        timeout: default_mcp_timeout(),
    })
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct McpServerConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub transport_type: String,
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub description: String,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub headers: Vec<String>,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub args: Vec<String>,
    /// `"KEY=VALUE"` 列表。宽容反序列化：`null` → 空表；`{"K":"V"}` →
    /// `["K=V"]`——用户实盘 config.mcp.json 里 `"env": null` 触发
    /// Dashboard "invalid type: null" 崩溃的根因出口。
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub env: Vec<String>,
    /// 请求超时（秒）。线上键名保持 `timeout`（Dashboard/CLI/存盘早已用
    /// 该键），`timeout_secs` 作为别名同时接受；`null`/字符串数字宽容。
    #[serde(
        rename = "timeout",
        alias = "timeout_secs",
        default = "default_server_timeout",
        deserialize_with = "flexible_timeout_secs"
    )]
    pub timeout_secs: u64,
    #[serde(default)]
    pub provider_name: String,
    #[serde(default)]
    pub provider_url: String,
    #[serde(default, deserialize_with = "flexible_string_list")]
    pub tags: Vec<String>,
    #[serde(default)]
    pub command: String,
    /// 未类型化键兜底（2026-09-03 二次回归 F2 同族根修，对齐 `ModelConfig.extra`
    /// 惯例）：typed `save_config` round-trip 时编译器保证保留未知键，不再静默
    /// 抹掉（如未来新增的 per-server 键）。空 map 序列化跳过 → 无 unknown 键的
    /// 条目输出形态与既有字节一致。注意：PartialEq 参与相等比较（round-trip
    /// 两侧携带同一 extra 时语义不变）。
    /// L7：BTreeMap 键序确定（同 `ModelConfig.extra`，字节稳定）。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            transport_type: String::new(),
            url: String::new(),
            description: String::new(),
            headers: Vec::new(),
            args: Vec::new(),
            env: Vec::new(),
            timeout_secs: default_server_timeout(),
            provider_name: String::new(),
            provider_url: String::new(),
            tags: Vec::new(),
            command: String::new(),
            extra: std::collections::BTreeMap::new(),
        }
    }
}

impl McpServerConfig {
    /// Normalize legacy fields: map old `command` to `url` and set default `transport_type`.
    pub fn normalize(&mut self) {
        if self.url.is_empty() && !self.command.is_empty() {
            self.url = self.command.clone();
        }
        if self.transport_type.is_empty() {
            self.transport_type = "stdio".to_string();
        }
    }

    /// Builder: stdio 便捷构造（nemesis-mcp 侧测试与 ad-hoc 连接用）。
    pub fn new(name: impl Into<String>, command: impl Into<String>) -> Self {
        Self {
            name: name.into(),
            command: command.into(),
            ..Default::default()
        }
    }

    /// Builder: 追加一个命令行参数。
    pub fn arg(mut self, arg: impl Into<String>) -> Self {
        self.args.push(arg.into());
        self
    }

    /// Builder: 追加一个环境变量（`"KEY=VALUE"`）。
    pub fn env(mut self, kv: impl Into<String>) -> Self {
        self.env.push(kv.into());
        self
    }

    /// Builder: 设置请求超时（秒）。
    pub fn timeout(mut self, secs: u64) -> Self {
        self.timeout_secs = secs;
        self
    }
}

/// 宽容字符串列表反序列化（单一真相源收敛配套）：`null` → 空表；
/// `{"K":"V"}` → `["K=V"]`（LLM/外部生态最常见的 env 写法）；`["a","b"]`
/// → 原样；标量 → 单元素表。元素非字符串时尽力转字符串。
fn flexible_string_list<'de, D>(d: D) -> std::result::Result<Vec<String>, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(string_list_from_value(v))
}

fn string_list_from_value(v: serde_json::Value) -> Vec<String> {
    match v {
        serde_json::Value::Null => Vec::new(),
        serde_json::Value::Array(items) => items
            .into_iter()
            .filter_map(|item| match item {
                serde_json::Value::String(s) => Some(s),
                serde_json::Value::Null => None,
                other => Some(other.to_string()),
            })
            .collect(),
        serde_json::Value::Object(map) => map
            .into_iter()
            .map(|(k, v)| match v {
                serde_json::Value::String(s) => format!("{k}={s}"),
                serde_json::Value::Null => k,
                other => format!("{k}={other}"),
            })
            .collect(),
        // 标量字符串取原值（Value::to_string 是 JSON 表示，会带引号）
        serde_json::Value::String(s) => vec![s],
        other => vec![other.to_string()],
    }
}

/// 宽容 u64（per-server timeout）：`null`/字符串数字/解析失败 → fallback。
fn flexible_timeout_secs<'de, D>(d: D) -> std::result::Result<u64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(u64_from_value(v, default_server_timeout()))
}

fn u64_from_value(v: serde_json::Value, fallback: u64) -> u64 {
    match v {
        serde_json::Value::Null => fallback,
        serde_json::Value::Number(n) => n.as_u64().unwrap_or(fallback),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(fallback),
        _ => fallback,
    }
}

/// 宽容 i64（全局 timeout）：`null`/字符串数字/解析失败 → 默认值。
fn flexible_i64<'de, D>(d: D) -> std::result::Result<i64, D::Error>
where
    D: serde::Deserializer<'de>,
{
    let v = serde_json::Value::deserialize(d)?;
    Ok(match v {
        serde_json::Value::Null => default_mcp_timeout(),
        serde_json::Value::Number(n) => n.as_i64().unwrap_or_else(default_mcp_timeout),
        serde_json::Value::String(s) => s.trim().parse().unwrap_or(default_mcp_timeout()),
        _ => default_mcp_timeout(),
    })
}

fn default_server_timeout() -> u64 {
    30
}

// ============================================================================
// Full sub-configurations (loaded from separate files)
// ============================================================================

/// Full skills configuration loaded from config.skills.json.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsFullConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub search_cache: SkillsSearchCacheConfig,
    #[serde(default = "default_max_concurrent_searches")]
    pub max_concurrent_searches: i64,
    #[serde(default = "default_search_limit")]
    pub search_limit: i64,
    #[serde(default)]
    pub github_sources: Vec<GitHubSourceConfig>,
    #[serde(default)]
    pub clawhub: SkillsClawHubConfig,
    #[serde(default)]
    pub modelscope: SkillsModelScopeConfig,
}

impl Default for SkillsFullConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            search_cache: SkillsSearchCacheConfig::default(),
            max_concurrent_searches: 2,
            search_limit: 50,
            github_sources: vec![],
            clawhub: SkillsClawHubConfig::default(),
            modelscope: SkillsModelScopeConfig::default(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsSearchCacheConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_cache_max_size")]
    pub max_size: i64,
    #[serde(default = "default_cache_ttl_seconds")]
    pub ttl_seconds: i64,
}

impl Default for SkillsSearchCacheConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_size: 50,
            ttl_seconds: 300,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct GitHubSourceConfig {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub repo: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub branch: String,
    #[serde(default)]
    pub index_type: String,
    #[serde(default)]
    pub index_path: String,
    #[serde(default)]
    pub skill_path_pattern: String,
    #[serde(default)]
    pub timeout: i64,
    #[serde(default)]
    pub max_size: i64,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SkillsClawHubConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub convex_url: String,
    #[serde(default)]
    pub timeout: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsModelScopeConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default)]
    pub timeout: i64,
}

impl Default for SkillsModelScopeConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout: 30,
        }
    }
}
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SecurityConfig {
    #[serde(default = "default_security_action")]
    pub default_action: String,
    #[serde(default = "default_true")]
    pub log_all_operations: bool,
    #[serde(default)]
    pub log_denials_only: bool,
    #[serde(default = "default_approval_timeout")]
    pub approval_timeout_seconds: i64,
    #[serde(default = "default_max_pending")]
    pub max_pending_requests: i64,
    #[serde(default = "default_audit_retention")]
    pub audit_log_retention_days: i64,
    #[serde(default)]
    pub audit_log_path: String,
    #[serde(default = "default_true")]
    pub audit_log_file_enabled: bool,
    #[serde(default)]
    pub synchronous_mode: bool,
    #[serde(default)]
    pub file_rules: Option<FileSecurityRules>,
    #[serde(default)]
    pub directory_rules: Option<DirectorySecurityRules>,
    #[serde(default)]
    pub process_rules: Option<ProcessSecurityRules>,
    #[serde(default)]
    pub network_rules: Option<NetworkSecurityRules>,
    #[serde(default)]
    pub hardware_rules: Option<HardwareSecurityRules>,
    #[serde(default)]
    pub registry_rules: Option<RegistrySecurityRules>,
    #[serde(default)]
    pub layers: Option<SecurityLayersConfig>,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_action: "deny".to_string(),
            log_all_operations: true,
            log_denials_only: false,
            approval_timeout_seconds: 300,
            max_pending_requests: 100,
            audit_log_retention_days: 90,
            audit_log_path: String::new(),
            audit_log_file_enabled: true,
            synchronous_mode: false,
            file_rules: None,
            directory_rules: None,
            process_rules: None,
            network_rules: None,
            hardware_rules: None,
            registry_rules: None,
            layers: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayersConfig {
    #[serde(default)]
    pub injection: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub command_guard: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub dlp: Option<DLPLayerConfig>,
    #[serde(default)]
    pub ssrf: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub credential: Option<SecurityLayerConfig>,
    #[serde(default)]
    pub signature: Option<SignatureLayerConfig>,
    #[serde(default)]
    pub audit_chain: Option<SecurityLayerConfig>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    /// L7：BTreeMap 键序确定（同 `ModelConfig.extra`，security 段字节稳定）。
    #[serde(default)]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DLPLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub rules: Vec<String>,
    #[serde(default)]
    pub action: String,
    /// Action for low-confidence matches. Empty → default "log" (detect, don't block).
    #[serde(default)]
    pub low_confidence_action: String,
    /// Action for DLP hits on inbound/local-storage ops. Empty → default "log".
    #[serde(default)]
    pub inbound_action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub strict: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub action: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct FileSecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DirectorySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub create: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ProcessSecurityRules {
    #[serde(default)]
    pub exec: Vec<SecurityRule>,
    #[serde(default)]
    pub spawn: Vec<SecurityRule>,
    #[serde(default)]
    pub kill: Vec<SecurityRule>,
    #[serde(default)]
    pub suspend: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct NetworkSecurityRules {
    #[serde(default)]
    pub request: Vec<SecurityRule>,
    #[serde(default)]
    pub download: Vec<SecurityRule>,
    #[serde(default)]
    pub upload: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareSecurityRules {
    #[serde(default)]
    pub i2c: Vec<SecurityRule>,
    #[serde(default)]
    pub spi: Vec<SecurityRule>,
    #[serde(default)]
    pub gpio: Vec<SecurityRule>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct RegistrySecurityRules {
    #[serde(default)]
    pub read: Vec<SecurityRule>,
    #[serde(default)]
    pub write: Vec<SecurityRule>,
    #[serde(default)]
    pub delete: Vec<SecurityRule>,
}

/// Scanner configuration loaded from config.scanner.json.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ScannerFullConfig {
    #[serde(default)]
    pub enabled: Vec<String>,
    #[serde(default)]
    pub engines: std::collections::HashMap<String, serde_json::Value>,
}

/// Engine state tracking for scanner engines.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct EngineState {
    #[serde(default)]
    pub install_status: String,
    #[serde(default)]
    pub install_error: String,
    #[serde(default)]
    pub last_install_attempt: String,
    #[serde(default)]
    pub db_status: String,
    #[serde(default)]
    pub last_db_update: String,
}

/// ClamAV-specific scanner configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ClamAVEngineConfig {
    #[serde(default)]
    pub url: String,
    #[serde(default)]
    pub clamav_path: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub scan_on_write: bool,
    #[serde(default)]
    pub scan_on_download: bool,
    #[serde(default)]
    pub scan_on_exec: bool,
    #[serde(default)]
    pub scan_extensions: Vec<String>,
    #[serde(default)]
    pub skip_extensions: Vec<String>,
    #[serde(default)]
    pub max_file_size: i64,
    #[serde(default)]
    pub update_interval: String,
    #[serde(default)]
    pub data_dir: String,
    #[serde(default)]
    pub state: EngineState,
}

// ============================================================================
// Embedded defaults management
// ============================================================================

/// Holds embedded default configuration data (set at compile time).
#[derive(Debug, Clone, Default)]
pub struct EmbeddedDefaults {
    pub config: Vec<u8>,
    pub mcp: Vec<u8>,
    pub security: Vec<u8>,
    pub cluster: Vec<u8>,
    pub skills: Vec<u8>,
    pub scanner: Vec<u8>,
}

/// Global embedded defaults storage.
static EMBEDDED_DEFAULTS: std::sync::OnceLock<std::sync::RwLock<EmbeddedDefaults>> =
    std::sync::OnceLock::new();

fn embedded_defaults() -> &'static std::sync::RwLock<EmbeddedDefaults> {
    EMBEDDED_DEFAULTS.get_or_init(|| std::sync::RwLock::new(EmbeddedDefaults::default()))
}

/// Get the current embedded defaults.
pub fn get_embedded_defaults() -> EmbeddedDefaults {
    embedded_defaults().read().unwrap().clone()
}

/// Set embedded defaults from byte arrays.
pub fn set_embedded_defaults(
    config: Vec<u8>,
    mcp: Vec<u8>,
    security: Vec<u8>,
    cluster: Vec<u8>,
    skills: Vec<u8>,
    scanner: Vec<u8>,
) {
    let mut guard = embedded_defaults().write().unwrap();
    guard.config = config;
    guard.mcp = mcp;
    guard.security = security;
    guard.cluster = cluster;
    guard.skills = skills;
    guard.scanner = scanner;
}

/// Set embedded defaults from a filesystem path (directory of config files).
///
/// This mirrors the Go `SetEmbeddedDefaultsFromFS` function. Reads the
/// standard set of configuration files from the given directory:
/// - `config.default.json`
/// - `config.mcp.default.json`
/// - platform-specific security config
/// - `config.cluster.default.json`
/// - `config.skills.default.json`
/// - `config.scanner.default.json` (optional)
pub fn set_embedded_defaults_from_fs(config_dir: &Path) -> Result<()> {
    // Read config.default.json
    let config_path = config_dir.join("config.default.json");
    let config_data = std::fs::read(&config_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.default.json: {}", e),
        ))
    })?;

    // Read config.mcp.default.json
    let mcp_path = config_dir.join("config.mcp.default.json");
    let mcp_data = std::fs::read(&mcp_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.mcp.default.json: {}", e),
        ))
    })?;

    // Read platform-specific security config
    let security_filename = get_platform_security_config_filename();
    let security_path = config_dir.join(&security_filename);
    let security_data = std::fs::read(&security_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read {}: {}", security_filename, e),
        ))
    })?;

    // Read config.cluster.default.json
    let cluster_path = config_dir.join("config.cluster.default.json");
    let cluster_data = std::fs::read(&cluster_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.cluster.default.json: {}", e),
        ))
    })?;

    // Read config.skills.default.json
    let skills_path = config_dir.join("config.skills.default.json");
    let skills_data = std::fs::read(&skills_path).map_err(|e| {
        ConfigError::Io(std::io::Error::new(
            e.kind(),
            format!("failed to read config.skills.default.json: {}", e),
        ))
    })?;

    // Read config.scanner.default.json (optional -- don't fail if missing)
    let scanner_path = config_dir.join("config.scanner.default.json");
    let scanner_data = std::fs::read(&scanner_path).unwrap_or_default();

    set_embedded_defaults(
        config_data,
        mcp_data,
        security_data,
        cluster_data,
        skills_data,
        scanner_data,
    );

    Ok(())
}

// ============================================================================
// Provider resolution functions are now in the provider_resolver module.
// They are re-exported above for backward compatibility.
// ============================================================================

/// Load configuration from a file path.
/// Mirrors Go LoadConfig: tries file → embedded default → hardcoded default.
/// After loading, applies environment variable overrides (NEMESISBOT_*).
pub fn load_config(config_path: &Path) -> Result<Config> {
    info!("[Config] Loading config from: {:?}", config_path);
    if config_path.exists() {
        let content = std::fs::read_to_string(config_path).map_err(|e| {
            error!(
                "[Config] Failed to read config file {:?}: {}",
                config_path, e
            );
            e
        })?;
        let mut config: Config = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse config JSON: {}", e);
            e
        })?;
        apply_env_overrides(&mut config);
        config.post_process_for_compatibility();
        config.adjust_paths_for_environment();
        info!("[Config] Config loaded successfully from file");
        return Ok(config);
    }

    info!(
        "[Config] Config file not found at {:?}, trying embedded default",
        config_path
    );
    let defaults = get_embedded_defaults();
    if !defaults.config.is_empty() {
        if let Ok(mut config) = serde_json::from_slice::<Config>(&defaults.config) {
            apply_env_overrides(&mut config);
            config.post_process_for_compatibility();
            config.adjust_paths_for_environment();
            info!("[Config] Config loaded successfully from embedded default");
            return Ok(config);
        } else {
            warn!(
                "[Config] Failed to parse embedded default config, falling back to hardcoded default"
            );
        }
    }

    info!("[Config] Using hardcoded default config");
    let mut config = default_config();
    apply_env_overrides(&mut config);
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    info!("[Config] Config loaded successfully (hardcoded default)");
    Ok(config)
}

/// Save configuration to a file path.
/// Mirrors Go SaveConfig: auto-adjusts paths for local mode before saving.
pub fn save_config(config_path: &Path, config: &mut Config) -> Result<()> {
    info!("[Config] Saving config to: {:?}", config_path);
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create config directory: {}", e);
            e
        })?;
    }

    // Auto-adjust paths for local mode before saving (mirrors Go behavior)
    // When --local is used or .nemesisbot is detected in current directory,
    // workspace and other paths should be relative to the config directory
    if is_local_mode(config_path) {
        let config_dir = config_path.parent().unwrap_or(Path::new("."));
        if config_dir.file_name().is_some_and(|n| n == ".nemesisbot") {
            // Get the expected default workspace path
            let user_home = dirs::home_dir().unwrap_or_else(|| PathBuf::from("~"));
            let default_workspace_path = user_home.join(".nemesisbot").join("workspace");
            let default_workspace_str = default_workspace_path.to_string_lossy().to_string();

            let ws = &config.agents.defaults.workspace;
            // Check if workspace is using default path pattern
            if ws.starts_with("~/")
                || ws.starts_with("~\\")
                || ws.starts_with(&user_home.to_string_lossy().to_string())
                || ws == &default_workspace_str
            {
                config.agents.defaults.workspace = PathBuf::from(".nemesisbot")
                    .join("workspace")
                    .to_string_lossy()
                    .to_string();
            }

            // Normalize logging directory
            if let Some(ref mut logging) = config.logging
                && let Some(ref mut llm) = logging.llm
                && llm.log_dir.is_empty()
            {
                llm.log_dir = "logs/request_logs".to_string();
            }
        }
    }

    let content = serde_json::to_string_pretty(&*config).map_err(|e| {
        error!("[Config] Failed to serialize config: {}", e);
        e
    })?;
    // Write with restricted permissions (0600 on Unix) to protect API keys/tokens
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        let mut f = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .mode(0o600)
            .open(config_path)
            .map_err(|e| {
                error!("[Config] Failed to open config file for writing: {}", e);
                e
            })?;
        std::io::Write::write_all(&mut f, content.as_bytes()).map_err(|e| {
            error!("[Config] Failed to write config file: {}", e);
            e
        })?;
    }
    #[cfg(not(unix))]
    {
        std::fs::write(config_path, content).map_err(|e| {
            error!("[Config] Failed to write config file: {}", e);
            e
        })?;
    }
    info!("[Config] Config saved successfully");
    Ok(())
}

/// Check if we're in local mode (config is in current directory's .nemesisbot).
pub fn is_local_mode(config_path: &Path) -> bool {
    if let Ok(cwd) = std::env::current_dir() {
        let local_dir = cwd.join(".nemesisbot");
        if local_dir.exists()
            && let Ok(canonical_config) = config_path.canonicalize()
            && let Ok(canonical_local) = local_dir.canonicalize()
            && let Some(parent) = canonical_config.parent()
        {
            return parent == canonical_local;
        }
    }
    false
}

/// Apply environment variable overrides to the config.
/// Mirrors Go's `env.Parse(cfg)` which reads NEMESISBOT_* env vars.
///
/// Supported environment variables (matching Go's env tags):
/// - NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE
/// - NEMESISBOT_AGENTS_DEFAULTS_LLM
/// - NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS
/// - NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE
/// - NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS
/// - NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE
/// - NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE
/// - NEMESISBOT_CHANNELS_WEB_ENABLED
/// - NEMESISBOT_CHANNELS_WEB_HOST
/// - NEMESISBOT_CHANNELS_WEB_PORT
/// - NEMESISBOT_GATEWAY_HOST
/// - NEMESISBOT_GATEWAY_PORT
/// - NEMESISBOT_HEARTBEAT_ENABLED
/// - NEMESISBOT_HEARTBEAT_INTERVAL
/// - NEMESISBOT_SECURITY_ENABLED
/// - NEMESISBOT_FORGE_ENABLED
pub fn apply_env_overrides(config: &mut Config) {
    // Agent defaults
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_WORKSPACE") {
        config.agents.defaults.workspace = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_RESTRICT_TO_WORKSPACE") {
        config.agents.defaults.restrict_to_workspace = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_LLM") {
        config.agents.defaults.llm = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_IMAGE_MODEL") {
        config.agents.defaults.image_model = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOKENS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_tokens = n;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_TEMPERATURE")
        && let Ok(f) = v.parse()
    {
        config.agents.defaults.temperature = f;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_TOOL_ITERATIONS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_tool_iterations = n;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_CONCURRENT_REQUEST_MODE") {
        config.agents.defaults.concurrent_request_mode = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_QUEUE_SIZE")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.queue_size = n;
    }

    // Web channel
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_ENABLED") {
        config.channels.web.enabled = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_HOST") {
        config.channels.web.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_CHANNELS_WEB_PORT")
        && let Ok(n) = v.parse()
    {
        config.channels.web.port = n;
    }

    // Gateway
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_HOST") {
        config.gateway.host = v;
    }
    if let Ok(v) = std::env::var("NEMESISBOT_GATEWAY_PORT")
        && let Ok(n) = v.parse()
    {
        config.gateway.port = n;
    }

    // Heartbeat
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_ENABLED") {
        config.heartbeat.enabled = v.parse().unwrap_or(true);
    }
    if let Ok(v) = std::env::var("NEMESISBOT_HEARTBEAT_INTERVAL")
        && let Ok(n) = v.parse()
    {
        config.heartbeat.interval = n;
    }

    // Security
    if let Ok(v) = std::env::var("NEMESISBOT_SECURITY_ENABLED") {
        let enabled = v.parse().unwrap_or(false);
        config.security = Some(SecurityFlagConfig { enabled });
    }

    // Forge
    if let Ok(v) = std::env::var("NEMESISBOT_FORGE_ENABLED") {
        let enabled = v.parse().unwrap_or(false);
        config.forge = Some(ForgeFlagConfig { enabled });
    }

    // Session
    if let Ok(v) = std::env::var("NEMESISBOT_SESSION_DM_SCOPE") {
        config.session.dm_scope = v;
    }
}

/// Load embedded default config.
pub fn load_embedded_config() -> Result<Config> {
    info!("[Config] Loading embedded config");
    let defaults = get_embedded_defaults();
    if defaults.config.is_empty() {
        error!("[Config] Embedded default config not available");
        return Err(ConfigError::Validation(
            "embedded default config not available".into(),
        ));
    }
    let mut config: Config = serde_json::from_slice(&defaults.config).map_err(|e| {
        error!("[Config] Failed to parse embedded config: {}", e);
        e
    })?;
    config.post_process_for_compatibility();
    config.adjust_paths_for_environment();
    info!("[Config] Embedded config loaded successfully");
    Ok(config)
}

/// Create a fully populated Config with all sensible defaults.
pub fn default_config() -> Config {
    let ws = WorkspaceResolver::resolve(false)
        .join("workspace")
        .to_string_lossy()
        .to_string();
    Config {
        agents: AgentsConfig {
            claude_code_tool: ClaudeCodeToolConfig::default(),
            codex_tool: CodexToolConfig::default(),
            lsp_tool: LspToolConfig::default(),
            tool_doc_folding: ToolDocFoldingConfig::default(),
            defaults: AgentDefaults {
                workspace: ws,
                restrict_to_workspace: true,
                llm: "zhipu/glm-4.7-flash".to_string(),
                max_tokens: 8192,
                temperature: 0.7,
                max_tool_iterations: 100,
                concurrent_request_mode: "reject".to_string(),
                queue_size: 8,
                ..Default::default()
            },
            list: vec![],
        },
        bindings: vec![],
        session: SessionConfig::default(),
        channels: ChannelsConfig {
            whatsapp: WhatsAppConfig {
                bridge_url: "ws://localhost:3001".to_string(),
                ..Default::default()
            },
            maixcam: MaixCamConfig {
                host: "0.0.0.0".to_string(),
                port: 18790,
                ..Default::default()
            },
            line: LineConfig {
                webhook_host: "0.0.0.0".to_string(),
                webhook_port: 18791,
                webhook_path: "/webhook/line".to_string(),
                ..Default::default()
            },
            onebot: OneBotConfig {
                ws_url: "ws://127.0.0.1:3001".to_string(),
                reconnect_interval: 5,
                ..Default::default()
            },
            web: WebChannelConfig {
                enabled: true,
                host: "0.0.0.0".to_string(),
                port: 8080,
                path: "/ws".to_string(),
                heartbeat_interval: 30,
                session_timeout: 3600,
                ..Default::default()
            },
            external: ExternalConfig {
                chat_id: "external:main".to_string(),
                sync_to: vec!["web".to_string()],
                ..Default::default()
            },
            ..Default::default()
        },
        model_list: vec![],
        gateway: GatewayConfig {
            host: "0.0.0.0".to_string(),
            port: 18790,
        },
        tools: ToolsConfig {
            web: WebToolsConfig {
                duckduckgo: DuckDuckGoConfig {
                    enabled: true,
                    max_results: 5,
                },
                brave: BraveConfig {
                    max_results: 5,
                    ..Default::default()
                },
                perplexity: PerplexityConfig {
                    max_results: 5,
                    ..Default::default()
                },
            },
            cron: CronToolsConfig {
                exec_timeout_minutes: 5,
            },
            exec: ExecConfig {
                enable_deny_patterns: true,
                ..Default::default()
            },
        },
        heartbeat: HeartbeatConfig {
            enabled: true,
            interval: 30,
        },
        devices: DevicesConfig {
            monitor_usb: true,
            ..Default::default()
        },
        logging: Some(LoggingConfig {
            llm: Some(LlmLogConfig {
                enabled: false,
                log_dir: "logs/request_logs".to_string(),
                detail_level: "full".to_string(),
                save_raw: false,
            }),
            general: None,
        }),
        security: Some(SecurityFlagConfig { enabled: false }),
        forge: Some(ForgeFlagConfig { enabled: false }),
        cluster: None,
        memory: None,
        skills: None,
        mcp: None,
        executor: None,
        debug: None,
        board: None,
        usage: None,
    }
}

impl Config {
    /// Get the workspace path from config.
    pub fn workspace_path(&self) -> String {
        let ws = &self.agents.defaults.workspace;
        if ws.starts_with("~/")
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
        // First try exact match with model_name
        for mc in &self.model_list {
            if mc.model_name == model_ref {
                return Ok(mc);
            }
        }

        // Then try prefix match with model field (vendor/model)
        for mc in &self.model_list {
            if mc.model == model_ref {
                return Ok(mc);
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
    /// This mirrors the Go `postProcessForCompatibility` function:
    /// - Sync `sync_to_web` from `sync_to` for External and WebSocket channels.
    pub fn post_process_for_compatibility(&mut self) {
        // External channel: populate SyncToWeb from SyncTo
        self.channels.external.sync_to_web = !self.channels.external.sync_to.is_empty();

        // WebSocket channel: populate SyncToWeb from SyncTo
        self.channels.websocket.sync_to_web = !self.channels.websocket.sync_to.is_empty();
    }

    /// Adjust hardcoded default paths to respect NEMESISBOT_HOME.
    ///
    /// This mirrors the Go `adjustPathsForEnvironment` function:
    /// - If the workspace uses a default path, replace it with the actual resolved path.
    /// - Set a default log directory if none is specified.
    pub fn adjust_paths_for_environment(&mut self) {
        let expected_workspace = WorkspaceResolver::resolve(false).join("workspace");

        // Check if workspace is using a hardcoded default path
        let ws = &self.agents.defaults.workspace;
        let is_default = ws.is_empty()
            || ws == "~/.nemesisbot/workspace"
            || ws.ends_with(".nemesisbot/workspace");

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

/// Load MCP configuration from a separate config.mcp.json file.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_mcp_config(path: &Path) -> Result<McpConfig> {
    info!("[Config] Loading MCP config from: {:?}", path);
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            error!("[Config] Failed to read MCP config: {}", e);
            e
        })?;
        let cfg: McpConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse MCP config JSON: {}", e);
            e
        })?;
        info!("[Config] MCP config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] MCP config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.mcp.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<McpConfig>(&defaults.mcp)
    {
        info!("[Config] MCP config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default MCP config");
    Ok(McpConfig {
        enabled: false,
        servers: vec![],
        timeout: 30,
    })
}

/// Save MCP configuration to a separate config.mcp.json file.
pub fn save_mcp_config(path: &Path, cfg: &McpConfig) -> Result<()> {
    info!("[Config] Saving MCP config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create MCP config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize MCP config: {}", e);
        e
    })?;
    std::fs::write(path, content).map_err(|e| {
        error!("[Config] Failed to write MCP config: {}", e);
        e
    })?;
    info!("[Config] MCP config saved successfully");
    Ok(())
}

/// Load security configuration from config.security.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_security_config(path: &Path) -> Result<SecurityConfig> {
    info!("[Config] Loading security config from: {:?}", path);
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            error!("[Config] Failed to read security config: {}", e);
            e
        })?;
        let cfg: SecurityConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse security config JSON: {}", e);
            e
        })?;
        info!("[Config] Security config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Security config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.security.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<SecurityConfig>(&defaults.security)
    {
        info!("[Config] Security config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default security config");
    Ok(SecurityConfig::default())
}

/// Save security configuration to config.security.json.
pub fn save_security_config(path: &Path, cfg: &SecurityConfig) -> Result<()> {
    info!("[Config] Saving security config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create security config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize security config: {}", e);
        e
    })?;
    std::fs::write(path, content).map_err(|e| {
        error!("[Config] Failed to write security config: {}", e);
        e
    })?;
    info!("[Config] Security config saved successfully");
    Ok(())
}

/// Load scanner configuration from config.scanner.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_scanner_config(path: &Path) -> Result<ScannerFullConfig> {
    info!("[Config] Loading scanner config from: {:?}", path);
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            error!("[Config] Failed to read scanner config: {}", e);
            e
        })?;
        let cfg: ScannerFullConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse scanner config JSON: {}", e);
            e
        })?;
        info!("[Config] Scanner config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Scanner config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.scanner.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<ScannerFullConfig>(&defaults.scanner)
    {
        info!("[Config] Scanner config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default scanner config");
    Ok(ScannerFullConfig::default())
}

/// Save scanner configuration to config.scanner.json.
pub fn save_scanner_config(path: &Path, cfg: &ScannerFullConfig) -> Result<()> {
    info!("[Config] Saving scanner config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create scanner config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize scanner config: {}", e);
        e
    })?;
    std::fs::write(path, content).map_err(|e| {
        error!("[Config] Failed to write scanner config: {}", e);
        e
    })?;
    info!("[Config] Scanner config saved successfully");
    Ok(())
}

/// Load skills configuration from config.skills.json.
/// Three-tier fallback: file -> embedded default -> hardcoded default.
pub fn load_skills_config(path: &Path) -> Result<SkillsFullConfig> {
    info!("[Config] Loading skills config from: {:?}", path);
    if path.exists() {
        let content = std::fs::read_to_string(path).map_err(|e| {
            error!("[Config] Failed to read skills config: {}", e);
            e
        })?;
        let cfg: SkillsFullConfig = serde_json::from_str(&content).map_err(|e| {
            error!("[Config] Failed to parse skills config JSON: {}", e);
            e
        })?;
        info!("[Config] Skills config loaded from file");
        return Ok(cfg);
    }

    info!("[Config] Skills config file not found, trying embedded default");
    let defaults = get_embedded_defaults();
    if !defaults.skills.is_empty()
        && let Ok(cfg) = serde_json::from_slice::<SkillsFullConfig>(&defaults.skills)
    {
        info!("[Config] Skills config loaded from embedded default");
        return Ok(cfg);
    }

    info!("[Config] Using hardcoded default skills config");
    Ok(SkillsFullConfig::default())
}

/// Save skills configuration to config.skills.json.
pub fn save_skills_config(path: &Path, cfg: &SkillsFullConfig) -> Result<()> {
    info!("[Config] Saving skills config to: {:?}", path);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|e| {
            error!("[Config] Failed to create skills config directory: {}", e);
            e
        })?;
    }
    let content = serde_json::to_string_pretty(cfg).map_err(|e| {
        error!("[Config] Failed to serialize skills config: {}", e);
        e
    })?;
    std::fs::write(path, content).map_err(|e| {
        error!("[Config] Failed to write skills config: {}", e);
        e
    })?;
    info!("[Config] Skills config saved successfully");
    Ok(())
}

// ============================================================================
// Additional default value functions
// ============================================================================

fn default_security_action() -> String {
    "deny".to_string()
}
fn default_approval_timeout() -> i64 {
    300
}
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
fn default_max_tokens() -> i64 {
    8192
}
fn default_temperature() -> f64 {
    0.7
}
fn default_max_tool_iterations() -> i64 {
    100
}
fn default_concurrent_request_mode() -> String {
    "reject".to_string()
}

fn default_snapshot_role() -> String {
    "user".to_string()
}
fn default_queue_size() -> i64 {
    8
}
fn default_spill_retention_days() -> i64 {
    7
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

/// Expand ~ in a path to the user's home directory.
/// Mirrors Go's `ExpandHome` function.
pub fn expand_tilde(path: &str) -> PathBuf {
    if (path.starts_with("~/") || path == "~")
        && let Some(home) = dirs::home_dir()
    {
        if path == "~" {
            return home;
        }
        return home.join(&path[2..]);
    }
    PathBuf::from(path)
}

/// Workspace path resolver.
pub struct WorkspaceResolver;

impl WorkspaceResolver {
    /// Resolve workspace directory path.
    /// Priority: local_flag > env_var > auto_detect > default
    pub fn resolve(local: bool) -> PathBuf {
        if local {
            return PathBuf::from("./.nemesisbot");
        }

        if let Ok(home) = std::env::var("NEMESISBOT_HOME") {
            // Expand ~ to user home, then append .nemesisbot (matching Go behavior)
            let expanded = expand_tilde(&home);
            return expanded.join(".nemesisbot");
        }

        // Auto-detect: if .nemesisbot exists in current directory
        let local_path = PathBuf::from("./.nemesisbot");
        if local_path.exists() {
            return local_path;
        }

        // Default: ~/.nemesisbot
        dirs::home_dir()
            .map(|h| h.join(".nemesisbot"))
            .unwrap_or(local_path)
    }

    /// Get the config file path within the workspace.
    pub fn config_path(workspace: &Path) -> PathBuf {
        workspace.join("config.json")
    }

    /// Ensure workspace directory exists.
    pub fn ensure_workspace(workspace: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(workspace)?;
        Ok(())
    }
}

/// Configuration loader.
pub struct ConfigLoader;

impl ConfigLoader {
    /// Load config from a file path.
    pub fn load_from_file(path: &Path) -> Result<Config> {
        let content = std::fs::read_to_string(path)?;
        let config: Config = serde_json::from_str(&content)?;
        Ok(config)
    }

    /// Load config from workspace directory.
    /// Three-tier fallback: file -> embedded default -> hardcoded default.
    /// Mirrors Go LoadConfig behavior.
    pub fn load_from_workspace(workspace: &Path) -> Result<Config> {
        let config_path = WorkspaceResolver::config_path(workspace);
        load_config(&config_path)
    }

    /// Save config to a file.
    pub fn save_to_file(config: &Config, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let content = serde_json::to_string_pretty(config)?;
        std::fs::write(path, content)?;
        Ok(())
    }

    /// Save config to workspace.
    pub fn save_to_workspace(config: &Config, workspace: &Path) -> Result<()> {
        let config_path = WorkspaceResolver::config_path(workspace);
        Self::save_to_file(config, &config_path)
    }

    /// Load embedded default config (compile-time embedded).
    pub fn load_embedded_default() -> Result<Config> {
        let default_json = include_str!("../../../nemesisbot/config/config.default.json");
        let config: Config =
            serde_json::from_str(default_json).unwrap_or_else(|_| Config::default());
        Ok(config)
    }
}

/// Get platform-specific security config filename.
/// On Linux: config.security.linux.json
/// On Windows: config.security.windows.json
/// On macOS: config.security.darwin.json
pub fn get_platform_security_config_filename() -> String {
    if cfg!(target_os = "linux") {
        "config.security.linux.json".to_string()
    } else if cfg!(target_os = "windows") {
        "config.security.windows.json".to_string()
    } else if cfg!(target_os = "macos") {
        "config.security.darwin.json".to_string()
    } else {
        "config.security.json".to_string()
    }
}

/// Get platform display name.
pub fn get_platform_display_name() -> String {
    if cfg!(target_os = "linux") {
        "Linux".to_string()
    } else if cfg!(target_os = "windows") {
        "Windows".to_string()
    } else if cfg!(target_os = "macos") {
        "macOS".to_string()
    } else {
        "Unknown".to_string()
    }
}

/// Get platform info as a JSON value.
pub fn get_platform_info() -> serde_json::Value {
    serde_json::json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "family": std::env::consts::FAMILY,
        "display_name": get_platform_display_name(),
        "security_config": get_platform_security_config_filename(),
    })
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod mcp_serde_tests;

// Single shared process-global-state lock for ALL tests in this crate that touch
// `std::env::set_var` / `set_current_dir` / load config (which reads env). These
// mutate/read PROCESS-GLOBAL state, so under parallel test execution they race:
// a writer in one test module pollutes the env a reader in another module sees.
// Both `tests` and `extra_tests` `use super::GLOBAL_STATE_LOCK` so they share
// ONE mutex (previously each module had its own → cross-module races still flaked
// under parallel). With this single lock, `cargo test -p nemesis-config` (default
// parallel) is as reliable as `--test-threads=1` for these tests.
#[cfg(test)]
static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
