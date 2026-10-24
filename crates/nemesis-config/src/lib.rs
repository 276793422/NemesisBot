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
pub mod plugin_wasm;
pub mod plugins_native;
pub mod provider_resolver;
pub mod store;
pub mod vault_ref;

pub use commands::{CommandEntry, CommandsConfig, load_commands_config, save_commands_config};
pub use hot_reload::HotReloader;

// P0 vault（B1，2026-09-22 计划）：`vault:<alias>` 引用的前缀常量、全局
// 解析器钩子与通用秘密字段解析（channel/cluster/MCP 消费点共用）。
pub use vault_ref::{
    VAULT_PREFIX, VaultResolver, clear_global_vault_resolver, global_vault_resolver,
    is_secret_reference, resolve_secret_field, resolve_vault_reference, set_global_vault_resolver,
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
    normalize_stream_value, provider_preset_ids, resolve_model_config, resolve_model_resolution,
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
    /// 托管 Agent 看板旗标（W2 P4；None = 全默认——超时兜底 3600s）。
    #[serde(default)]
    pub board: Option<BoardFlagConfig>,
    /// 使用统计保留策略（A3；None = 全默认——30 天 rollup，无条数上限）。
    #[serde(default)]
    pub usage: Option<UsageConfig>,
    /// PTY 内嵌终端（L8；None = 全默认——**enabled=false 关**，双闸的另一闸
    /// 是 `terminal` cargo feature）。
    #[serde(default)]
    pub terminal: Option<TerminalConfig>,
    /// 项目注册表参数（L6++ 对话/项目双分组；None = 全默认——上限 4）。
    #[serde(default)]
    pub projects: Option<ProjectsConfig>,
    /// 反向桥配置（None = 全默认——服务端接入门不开放、客户端不连接）。
    #[serde(default)]
    pub bridge: Option<BridgeConfig>,
    /// Dashboard 前端表现（皮肤等；None = 全默认——内置 static 皮肤）。
    #[serde(default)]
    pub ui: Option<UiConfig>,
    /// 行动代理 connector 注册表（PLAN 行动代理 P0；None = 全默认——
    /// enabled=true，预算 fail-closed：资金限额 0=拒绝所有）。
    #[serde(default)]
    pub connectors: Option<ConnectorsConfig>,
    /// 紫微系统（三方程序受控接管，【万物皆工具】；None = 全默认——
    /// **enabled=false 总闸关**，对齐 terminal.enabled 双闸模式：另一闸是
    /// `ziwei` cargo feature）。见 [`ZiweiConfig`]。
    #[serde(default)]
    pub ziwei: Option<ZiweiConfig>,
}

/// 紫微系统配置（`config.json` 的 `"ziwei"` 段；总控 §9.1）。
///
/// `enabled` 默认 false：总闸关 = WSAPI 全拒 + CLI 子命令诚实报未启用 +
/// payload 嵌入字节不落盘（feature 关则三 crate 连同 payload 整体不编译）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ZiweiConfig {
    /// 总闸（false 默认：全部命令诚实拒绝）。
    pub enabled: bool,
    /// exe 路径/sha256 白名单；空 = 仅配置期可附加（使用期附加全弹卡）。
    #[serde(default)]
    pub allowed_targets: Vec<String>,
    /// payload DLL 落位目录（None = `<workspace>/tools/ziwei/`，bot 内嵌
    /// 落盘——决议 9）。
    #[serde(default)]
    pub payload_dir: Option<String>,
    /// 同时附加的会话上限。
    pub max_sessions: u32,
    /// 配置期快照保留天数（决议 3）。
    pub snapshots_ttl_days: u32,
    /// 使用期数据回流保留天数（对齐 uploads 惯例）。
    pub runs_ttl_days: u32,
}

impl Default for ZiweiConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            allowed_targets: Vec::new(),
            payload_dir: None,
            max_sessions: 4,
            snapshots_ttl_days: 30,
            runs_ttl_days: 7,
        }
    }
}

/// connector 注册表配置（`config.json` 的 `connectors` 段；`#[serde(default)]`
/// 每字段全可省——老 config.json 零迁移）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsConfig {
    /// 总闸（false = connector_find/connector_invoke 两工具不上供给面）。
    pub enabled: bool,
    /// per-connector 每小时执行上限（0=限频关闭）。
    pub max_ops_per_hour: u32,
    /// 资金类限额（None = 全 0 = fail-closed 拒绝所有资金 op）。
    #[serde(default)]
    pub finance: Option<ConnectorsFinanceConfig>,
    /// 远程执行面总闸（集群 E1：worker 节点是否接受 master 经 RPC 派发的
    /// `connector_invoke`；false = worker 侧 handler 诚实拒绝。缺省 true）。
    /// 凭据铁律不变：派发载荷只含 op+args，凭据永不上 RPC 线——worker 用
    /// 自己的 vault 解锁。
    #[serde(default = "default_remote_invoke_enabled")]
    pub remote_invoke_enabled: bool,
    /// 信任总闸（M1）：true = unsigned connector 一律拒执行（trust_blocked
    /// 诚实凭条）；false（缺省）= 只在审批卡加警示行，不拦截——本地自签
    /// 开发场景合法，拦截是用户显式选择。
    #[serde(default)]
    pub require_signed: bool,
    /// script 绑定全局配置（M2；None = 全缺省：default_timeout_secs 60）。
    #[serde(default)]
    pub script: Option<ConnectorsScriptConfig>,
}

fn default_remote_invoke_enabled() -> bool {
    true
}

impl ConnectorsConfig {
    /// script 绑定全局缺省超时秒数（op 级 timeout_secs 可覆盖；**0 = 拒绝
    /// 所有 script op**——防误配无限挂；上限 600 与 exec 工具闸对齐）。
    pub fn script_timeout_secs(&self) -> u64 {
        self.script
            .as_ref()
            .map(|s| s.default_timeout_secs)
            .unwrap_or(60)
    }
}

impl Default for ConnectorsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_ops_per_hour: 0,
            finance: None,
            remote_invoke_enabled: true,
            require_signed: false,
            script: None,
        }
    }
}

/// script 绑定全局配置（M2，2026-10-19）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsScriptConfig {
    /// 全局缺省超时秒数（0 = 拒绝所有 script op）。
    pub default_timeout_secs: u64,
}

impl Default for ConnectorsScriptConfig {
    fn default() -> Self {
        Self {
            default_timeout_secs: 60,
        }
    }
}

/// 资金类限额（fail-closed：0 = 拒绝所有——不设限 = 不成交）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ConnectorsFinanceConfig {
    /// 单笔数量上限（0 = 拒绝所有资金 op）。
    pub max_order_qty: f64,
    /// 日累计笔数上限（0 = 拒绝所有资金 op；UTC 自然日滚转）。
    pub max_orders_per_day: u32,
}

impl Default for ConnectorsFinanceConfig {
    fn default() -> Self {
        Self {
            max_order_qty: 0.0,
            max_orders_per_day: 0,
        }
    }
}

/// 皮肤子系统策略（`config.json` 的 `ui.skins` 段；`#[serde(default)]`
/// 每字段全可省——老 config.json 零迁移）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct SkinsPolicy {
    /// true = 「设为默认观感」（WSAPI `skins.set_active`）拒绝非 verified
    /// 皮肤包。闸在**信任决策点**——数据面（active.css / app 分发）不拦，
    /// 签名仍是徽标而非加载闸（D1 定案），这里只约束「把哪个包设为默认」
    /// 这一个动作。默认 false（未签名皮肤照常可用）。
    pub require_signed: bool,
    /// true = 皮肤脚本载荷（manifest `script`）可经 `/skins/active/script`
    /// 分发。**脚本能力总闸**（默认 false）：关 = 端点 403，脚本不进前端。
    /// 开 ≠ 免许可——每个带脚本包首次激活仍需用户逐包同意（consent 卡，
    /// `skins.script_consent` 记账）。签名状态照旧只是徽标（同意卡展示
    /// ⚠/🚫 但不拦截）。
    pub allow_scripts: bool,
}

/// Dashboard 前端表现配置（`config.json` 的 `ui` 段）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct UiConfig {
    /// 激活皮肤 id。`"default"`（默认）/ 空 = 不启用任何 `.nbskin`（内置
    /// static 皮肤原样）；其余值 = 服务端从 exe 同级 `skins/<id>.nbskin`
    /// 加载并在 `/skins/active.css` 提供 CSS（前端注入后打
    /// `<html data-skin="<id>">`）。Dashboard 设置页「皮肤」tab 写回此处。
    #[serde(default)]
    pub skin: String,
    /// 皮肤子系统策略段（require_signed 后手开关等）。
    #[serde(default)]
    pub skins: SkinsPolicy,
}

impl Default for UiConfig {
    fn default() -> Self {
        Self {
            skin: "default".to_string(),
            skins: SkinsPolicy::default(),
        }
    }
}

/// 反向桥配置（`config.json` 的 `bridge` 段；`#[serde(default)]` 每字段全可省）。
/// 分服务端/客户端两侧：服务端 = 中继接入门（`--relay` 与正常启动共用）；
/// 客户端 = 本机作为桥设备连远端中继服务端（goal：节点显示名 + 反向桥）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeConfig {
    /// 服务端角色配置（`/bridge` 接入门）。
    pub server: BridgeServerConfig,
    /// 客户端角色配置（桥出站连接）。
    pub client: BridgeClientConfig,
}

/// 服务端接入门配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeServerConfig {
    /// 接入门 ws token（预共享钥匙的服务端半把；与桥客户端 `client.token`
    /// 同值——同一把钥匙的两端各自配置、永不交换、hello 帧校验、不走 URL
    /// query）。**空 = 接入门不开放**（fail-closed：不挂 /bridge 等桥路由）；
    /// `--relay` 启动时此处为空 → 拒绝启动并提示。
    pub token: String,
}

/// 客户端（桥设备）配置。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BridgeClientConfig {
    /// 客户端总开关（false = 零行为变化，不连中继）。
    pub enabled: bool,
    /// 中继服务端地址（如 `ws://vps.example.com:60600`；客户端自动拼
    /// `/bridge` 路径）。
    pub relay_url: String,
    /// 接入门 token（与远端服务端 `server.token` 同值）。
    pub token: String,
    /// 面板访问密码（**只存本机**，服务端零存储——VPS 被拿下也翻不出任何
    /// 设备密码；远程面板访问经 access_check 帧由本机比对哈希）。空 =
    /// 拒绝所有远程面板访问（fail-closed）。本地直连面板不受影响（两回事）。
    pub access_token: String,
}

/// PTY 内嵌终端配置（`config.json` 的 `terminal` 段；L8。交互 shell 无法
/// 逐命令审批——默认关闭，启用即接受「dashboard 登录用户=shell 信任级」
/// +全 I/O 审计 `<workspace>/logs/terminal/`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct TerminalConfig {
    /// 终端总开关（默认 false；启用还需 `terminal` cargo feature 在场）。
    pub enabled: bool,
    /// 并发 PTY 会话上限（满员拒绝新连接；默认 4）。
    pub max_sessions: u32,
    /// shell 覆盖（None=Windows powershell / Unix $SHELL >/bin/sh）。
    pub shell: Option<String>,
}

impl Default for TerminalConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_sessions: 4,
            shell: None,
        }
    }
}

/// 项目注册表参数（`config.json` 的 `projects` 段；L6++ Phase 1。
/// `#[serde(default)]` 每字段全可省）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct ProjectsConfig {
    /// 项目数上限（每项目 = 一个常驻 AgentLoop，上限即可预期的资源面；
    /// 默认 4。`config set_field projects.max` 经 typed round-trip 自动可用）。
    pub max: usize,
    /// N7（2026-10-13）：**未受信**项目（注册表条目 `instructions_trusted:
    /// false`，`projects.trust` 翻转）的工作区指令处理策略——`""`/`annotate`
    /// = 注入但前置不可信来源横幅（缺省）；`isolated` = 整段跳过；未知值
    /// lenient 按 annotate 处理 + warn。受信条目恒 verbatim 注入（逐项目
    /// 显式信任是用户最具体的意图，全局策略不降级它）。空串不序列化
    /// （`skip_serializing_if`）——缺省态 typed 保存字节零变化（role 先例）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub instruction_trust: String,
}

impl Default for ProjectsConfig {
    fn default() -> Self {
        Self {
            max: 4,
            instruction_trust: String::new(),
        }
    }
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

/// 停车场兜底派发档位（`board.dispatch_fallback`；用户裁决 2026-09-29：
/// 角色缺失是集群常态，默认不停车但仍保角色纪律）。三档语义：
/// - `Off` = 无人匹配即诚实停车等节点（旧 false）。
/// - `Role`（默认）= 松弛阶梯走到「丢职能」为止（D13 ②保职能丢标签 →
///   ③丢职能保角色标签）：职能与随行的 tier 门槛放开，但 worker/coordinator
///   **角色纪律保留**（架构单可降级给开发，不可降级给 coordinator）。
/// - `Full` = 阶梯全开 ②③④，角色也放开（旧 true）。
///
/// serde 兼容：旧配置布尔 false→`Off`、true→`Full`；字符串 `off`/`role`/
/// `full` 大小写不敏感；序列化恒为小写字符串。缺省键 = `Role`。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DispatchFallbackMode {
    Off,
    #[default]
    Role,
    Full,
}

impl Serialize for DispatchFallbackMode {
    fn serialize<S: serde::Serializer>(
        &self,
        serializer: S,
    ) -> std::result::Result<S::Ok, S::Error> {
        let s = match self {
            Self::Off => "off",
            Self::Role => "role",
            Self::Full => "full",
        };
        serializer.serialize_str(s)
    }
}

impl<'de> Deserialize<'de> for DispatchFallbackMode {
    fn deserialize<D: serde::Deserializer<'de>>(
        deserializer: D,
    ) -> std::result::Result<Self, D::Error> {
        struct V;
        impl serde::de::Visitor<'_> for V {
            type Value = DispatchFallbackMode;
            fn expecting(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                f.write_str("\"off\"|\"role\"|\"full\" 或旧布尔 false/true")
            }
            fn visit_bool<E: serde::de::Error>(
                self,
                v: bool,
            ) -> std::result::Result<Self::Value, E> {
                // 旧布尔兼容：false=停车、true=全松弛（旧行为字节保真）。
                Ok(if v {
                    DispatchFallbackMode::Full
                } else {
                    DispatchFallbackMode::Off
                })
            }
            fn visit_str<E: serde::de::Error>(
                self,
                v: &str,
            ) -> std::result::Result<Self::Value, E> {
                match v.trim().to_ascii_lowercase().as_str() {
                    "off" | "false" => Ok(DispatchFallbackMode::Off),
                    "role" => Ok(DispatchFallbackMode::Role),
                    "full" | "true" => Ok(DispatchFallbackMode::Full),
                    other => Err(E::invalid_value(
                        serde::de::Unexpected::Str(other),
                        &"off | role | full",
                    )),
                }
            }
        }
        deserializer.deserialize_any(V)
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
    /// 频道讨论区配置（`board.discussion`）。
    pub discussion: BoardDiscussionConfig,
    /// 每日备份配置（`board.backup`）。
    pub backup: BoardBackupConfig,
    /// 任务拆解配置（`board.plan`；Swarm M1）。
    pub plan: BoardPlanConfig,
    /// 任务档案导出配置（`board.dossier`；2026-09-29）。
    pub dossier: BoardDossierConfig,
    /// in_review 自动触发验收 agent（Swarm M3+；默认 true）。删除本键即回
    /// 「纯人工验收」。
    pub auto_review: bool,
    /// 验收 PASS 是否自动 done（Swarm M3+；默认 false = 人工签字放行）。
    pub auto_accept: bool,
    /// FAIL 重派上限，超次转 UNSURE 等人工（Swarm M4）。
    pub max_redispatch: u32,
    /// 子单全部落定后父单自动收口（全自动流转 P1；默认 false = 停 in_review
    /// 等人工）。true 时父单进 in_review 即触发一次汇总验收（输入=全部子单
    /// 结论 + 各子单最新交付汇报），PASS 自动 done；FAIL/UNSURE 转人工。
    /// 存在 cancelled 子单的父单不适用（范围缺口必须人工裁决）。
    pub auto_close_parent: bool,
    /// 无限模式（全自动流转 P1；用户裁定 2026-09-10；默认 false = 护栏生效）。
    /// true：max_redispatch 视为 ∞（FAIL 无限重派）、UNSURE 带「无法定案」
    /// 意见继续重派——但 §6.4B 语义变化（2026-10-12 批准）：UNSURE 连续达到
    /// `board.review.max_unsure_streak`（默认 2，0=不限恢复完全旧行为）转
    /// 人工——unlimited 放大的是重派次数不设限，不是无收敛重派授权。
    /// estop 急停不受影响（保险丝非护栏）。停滞仍可观测：连续重派且差距无
    /// 变化时 WARN 告警（只告警不停）。
    pub unlimited_mode: bool,
    /// 停车场兜底派发档位（集群完备性加固 2026-09-11 引入；2026-09-29 三档化，
    /// 默认 `Role` = 角色缺失是常态：职能/tier 门槛随职能放开，但角色纪律
    /// 保留，不停车）。`Off` = 无人匹配即停车等节点；`Full` = 全松弛（角色
    /// 也放开）。所有档位：派发前落 ⚠ 系统评论留痕（松弛级说明）；无任何
    /// 在线节点时仍诚实停车（兜底造不出客户端）；estop 急停冻结兜底派发；
    /// 钉住目标（`dispatch_fallback_target`）不在线 = 停车不悄悄换人。
    pub dispatch_fallback: DispatchFallbackMode,
    /// 兜底客户端钉住（`dispatch_fallback` 开时生效；按节点 name 或 id 匹配，
    /// 大小写不敏感）。Some 且该节点不在线 = 诚实停车**不悄悄换人**（钉住
    /// 即点名）；None = 在线节点里松弛排序自动选。
    pub dispatch_fallback_target: Option<String>,
    /// 派发准入控制（goal P2/D0；默认 1）：自动派发链里，目标 worker 的
    /// 在途派发数达到该上限 → 本单不派、留 backlog——新节点上线后由 sweep
    /// 自然承接（防「同波多就绪单全挤单 worker、后上线的设备接不到活」）。
    /// 0 = 不限（旧行为）。人工 `issue.dispatch` 显式派发不受此闸（人类明确
    /// 意图，后果自担）。
    #[serde(default = "default_worker_max_inflight")]
    pub worker_max_inflight: i64,
    /// 验收 agent 行为配置（全自动流转 P4；`board.review` 段）。
    pub review: BoardReviewConfig,
    /// 自动流转预算保险丝（全自动流转 P4；`board.budget` 段；0 = 该项关闭）。
    pub budget: BoardBudgetConfig,
    /// 项目档案传输护栏（看板项目档案 goal P3/D4；`board.archive` 段）。
    pub archive: BoardArchiveConfig,
    /// 合并冲突自动处置（看板项目档案 goal P5/F1；默认 false = human 档：
    /// 项目域冻结等人工解，`project.resume` 续行）。true = auto 档：AI 硬解
    /// →机械失败重派原 worker→离线三轮接触→换人——项目不停摆为第一原则，
    /// 重派循环套既有预算保险丝打满转人工。开关只控制冲突处置策略，不改
    /// 变影响域（冻结始终限本项目，不株连其他项目）。
    pub conflict_auto_resolve: bool,
    /// 重派决策量化（能力扩展 P34；默认 false = D3 换节点重派维持匹配器
    /// 决策表现状行为字节等价）。true：换节点候选在匹配器排序之上叠加
    /// worker × 任务类型历史成功率指纹三档权重（prefer 提前 / avoid 靠后，
    /// 非绝对排除；样本 <3 一律 neutral），重派换节点评论带档位留痕。
    /// 记账与开关解耦（评审定案照常入账，灰度期攒数据，开闸即有历史）。
    pub fingerprint_weighting: bool,
    /// LLM 端点不可用期封存（U-B 断网封存；`board.unavailable` 段）。
    pub unavailable: BoardUnavailableConfig,
}

impl Default for BoardFlagConfig {
    fn default() -> Self {
        Self {
            dispatch_timeout_secs: 3600,
            dispatch_sweep_interval_secs: 20,
            auto_dispatch: false,
            discussion: BoardDiscussionConfig::default(),
            backup: BoardBackupConfig::default(),
            plan: BoardPlanConfig::default(),
            dossier: BoardDossierConfig::default(),
            auto_review: true,
            auto_accept: false,
            max_redispatch: 2,
            auto_close_parent: false,
            unlimited_mode: false,
            dispatch_fallback: DispatchFallbackMode::default(),
            dispatch_fallback_target: None,
            worker_max_inflight: 1,
            review: BoardReviewConfig::default(),
            budget: BoardBudgetConfig::default(),
            archive: BoardArchiveConfig::default(),
            conflict_auto_resolve: false,
            fingerprint_weighting: false,
            unavailable: BoardUnavailableConfig::default(),
        }
    }
}

/// 验收 agent 行为配置（`board.review` 段；全自动流转 P4）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardReviewConfig {
    /// 执行型验收轮数上限（默认 1 = 只跑一轮纯文本验收，与历史行为字节
    /// 等价）。>1 且最近一次派发的 worker 是本机节点时，验收 agent 以只读
    /// 工具白名单最多执行 N 轮取证后再下结论；远端 worker 一律纯文本
    /// （B2a 安全闸）。
    pub max_turns: u32,
    /// 验收需要更多证据时允许向执行 worker 发起一轮自检（B2b；默认 false）。
    /// worker 在其安全 8 层内按证据请求取证，回报后触发二段验收。
    pub selfcheck: bool,
    /// 项目验收（F3）：项目下全部顶层父单 done 后自动汇总验收，PASS 自动
    /// completed（默认 false = completed 仅人工/归档脚本设置）。FAIL/UNSURE
    /// 只写缺口评论 + 回滚 completed→in_progress，不自动重开父单。
    pub auto_close_project: bool,
    /// 多检查员面板（全自动流转 P5/B3）：每次验收并行发出的评审路数
    /// （默认 1 = 单检查员，与历史行为字节等价）。>1 时 N 路独立评审多数
    /// 投票聚合（平票/无多数 → Unsure 转人工，fail-safe 不偏向自动收货）；
    /// 消费侧收敛上限 5，并发上限 4。
    pub checkers: u32,
    /// 评审档位（gap ②：评审分层；"thorough" | "fast"）。thorough（默认）
    /// = 全量纪律 + 取证 + 经验蒸馏（渲染字节与历史一致）；fast = 精简档
    /// （保留注入防线与三态 JSON 契约，关闭取证/经验）。无法识别的值安全
    /// 回落 thorough（宁可全量也不放水）。
    pub tier: String,
    /// exec: 锚点执行开关（M7 + §6.1 激活；代码默认 false = 安全缺省不变，
    /// 三机环境配置开启）。开启后 `[CHECK] exec:<命令>` 在锚点根目录以子进程
    /// 执行，退出码 0 = 过。planner 对可运行时验证的子单应产出此形态（§6.1
    /// 改动二），危险命令形态解析期黑名单拒绝；环境依赖缺失诚实跳过不进
    /// 失败集（探针闸）。
    pub exec_anchors_enabled: bool,
    /// exec: 锚点单条执行超时秒数（默认 120；超时 = 不过 + kill）。
    pub exec_anchor_timeout_secs: u64,
    /// 评审自身失败自动重评次数（N2；默认 1，**0 = 关**回旧行为——评审
    /// LLM 调用失败恒转人工）。评审 LLM 失败（非 verdict）时延迟自动重评，
    /// 全部重评仍失败才转人工。约束：estop / 单据状态已变则不重试（尊重
    /// 迟到评审守卫）；每次重评审计评论留痕。与 fail_class 重派方向相反
    ///——重评不重派，评审侧故障不放大成 worker 重写循环。
    pub failure_retries: u32,
    /// UNSURE 连续转人工阈值（§6.4 B；默认 **2**，**0 = 不限**=旧行为）。
    /// 同签名 UNSURE 连续达到阈值 → 复用 EscalateHuman 路径转人工（自带
    /// 清账 + 审计 + reasons 评论）。语义：给一次 worker 自愈机会，超出
    /// 即认定「只有人能解」（NB-8 第 4 轮实证）。unlimited_mode 下 UNSURE
    /// 处置从「无限重派」限缩为「同签名连续 2 轮转人工」（2026-10-12
    /// 用户批准的语义变化）。
    pub max_unsure_streak: u32,
    /// 职能→评审预设查表（职能能力演进 B1；`board.review.professions`
    /// 嵌套 map，键=职能 slug，大小写不敏感）。查表三层覆盖：职能预设 >
    /// 本结构全局字段 > 内置默认；slug 先精确匹配再裸 family 回落
    /// （`dev:cpp` 无精确键时落 `dev` 键——自定义 spec 项继承 family 预设）。
    /// 代码内置预设：`product`（max_turns=0 + checkers=2 + selfcheck=true，
    /// 需求源头单全维加强）、`test-blackbox`（checkers=2，人工级防线双
    /// 检查员防单评委漏判）——config 同名键**逐维度覆盖**内置预设。
    /// 缺键零迁移（`#[serde(default)]`，空 map = 全部走全局/内置）。
    #[serde(default)]
    pub professions: std::collections::HashMap<String, BoardProfessionReviewPreset>,
    /// 父单收口 FAIL 自动返工承接轮数上限（2026-10-16；默认 **2**，**0 = 关**
    /// = 旧行为恒转人工）。FAIL（差距具体）时自动创建「返工：」子单承接差距
    /// 并派发，返工单落定后 sync 链自动复评父单；UNSURE（证据不足）不在此列
    /// 恒转人工（返工修不了「无法判定」）。轮数按「返工：」前缀子单累计数计
    /// （含 cancelled——按尝试计不按存活计），超限保持转人工。
    pub parent_remediation_max: u32,
    /// 交付证据契约总闸（G4，2026-10-18 批六；默认 false = 语义不变）。
    /// 开启后**所有**单子验收 PASS 前置校验最新派发任务的交付记录在场
    /// （delivered/confirmed），缺失 = 降级 FAIL「缺交付证据」（不凭自然
    /// 语言进 done）。issue 级 `requires_evidence`（AC 含 file: 锚点自动
    /// 置位）不受本闸控制、恒生效——本闸是全局加码，不是全局豁免。
    pub require_delivery_evidence: bool,
    /// 交付证据 typed 块强制闸（诚实边界收口 ②，2026-10-19；默认 **false**
    /// 烧入期）。true 后交付证据闸要求 `$.delivery.files` typed 块**在场**
    /// （git tree diff 确定性派生，任何 git 交付都应可派生——缺席 = 证据面
    /// 残缺，FAIL「交付缺类型化证据块」）。`checks` 块**恒可选**（没跑检查
    /// = 诚实的事实陈述，不是证据缺失；强制会诱发造值）。typed 块**在场
    /// 时**无论本闸开关都验形态（path 形态闸 + sha256 hex 形态 + cmd 非空），
    /// 验不过 = FAIL——加法演进，旧 payload 缺席块行为不变。
    pub evidence_typed_required: bool,
    /// 必需检查契约（项 B「必需的必须跑」，2026-10-22 拍板；键 = 匹配键，
    /// 值 = 必需锚点 id 列表）。匹配键与 issue `required_tags` 做**大小写
    /// 不敏感交集**（与派发 matcher 标签硬条件同规则同词表）；命中任一
    /// 标签键 → 该键的检查列表生效（多键命中取并集）；无标签命中 → 落
    /// **空串键**的全局兜底规则（无空串键 = 无要求，行为不变）。锚点 id
    /// 匹配 `[CHECK]` 锚点的 target（file 路径 / exec 命令）或 pattern
    ///（re:/contains: 文本）。命中单子验收时，任一必需锚点「AC 未声明 /
    /// 被跳过（exec 关或环境缺）/ 未产出核验结果」→ 强制停车转人工：
    /// 不进语义评审、不允许 auto_accept，系统评论列缺口明细（哪条规则/
    /// 哪个锚点/为何未满足）。已运行（过或不过）= 覆盖满足，裁决归锚点
    /// 双检既有短路。空 map = 契约关闭（默认，零回归）。
    #[serde(default)]
    pub required_checks: std::collections::HashMap<String, Vec<String>>,
}

impl BoardReviewConfig {
    /// 职能评审参数解析（B1 单一入口）：全局字段打底 → 内置默认预设 →
    /// config 职能预设逐维度覆盖。`profession` 空/None = 无职能，返回全局值。
    pub fn resolve_for_profession(&self, profession: Option<&str>) -> BoardReviewParams {
        let mut out = BoardReviewParams {
            max_turns: self.max_turns,
            checkers: self.checkers,
            selfcheck: self.selfcheck,
        };
        let Some(slug) = profession.map(str::trim).filter(|s| !s.is_empty()) else {
            return out;
        };
        let lower = slug.to_ascii_lowercase();
        let preset = self.professions.get(&lower).or_else(|| {
            // 裸 family 回落：`dev:cpp` 无精确键 → `dev` 键（无冒号时精确
            // 查找已覆盖，不重复取）。
            let family = lower.split(':').next().unwrap_or(&lower);
            (family != lower)
                .then_some(family)
                .and_then(|f| self.professions.get(f))
        });
        let builtin = builtin_profession_presets()
            .into_iter()
            .find(|(k, _)| *k == lower)
            .or_else(|| {
                let family = lower.split(':').next().unwrap_or(&lower);
                builtin_profession_presets()
                    .into_iter()
                    .find(|(k, _)| *k == family)
            })
            .map(|(_, p)| p);
        // config 预设逐维度覆盖内置预设（None 维度沿用内置）。
        let merged = match (preset.cloned(), builtin) {
            (Some(cfg_p), Some(b)) => BoardProfessionReviewPreset {
                max_turns: cfg_p.max_turns.or(b.max_turns),
                checkers: cfg_p.checkers.or(b.checkers),
                selfcheck: cfg_p.selfcheck.or(b.selfcheck),
            },
            (Some(p), None) | (None, Some(p)) => p,
            (None, None) => return out,
        };
        if let Some(v) = merged.max_turns {
            out.max_turns = v;
        }
        if let Some(v) = merged.checkers {
            out.checkers = v;
        }
        if let Some(v) = merged.selfcheck {
            out.selfcheck = v;
        }
        out
    }
}

/// 代码内置职能预设（B1；config `board.review.professions` 同名键逐维度
/// 覆盖）。`dev:*` 无内置预设（实现单评审与通用评审无差异诉求，维持全局
/// 默认）。
fn builtin_profession_presets() -> [(&'static str, BoardProfessionReviewPreset); 2] {
    [
        (
            "product",
            BoardProfessionReviewPreset {
                max_turns: Some(0),
                checkers: Some(2),
                selfcheck: Some(true),
            },
        ),
        (
            "test-blackbox",
            BoardProfessionReviewPreset {
                max_turns: None,
                checkers: Some(2),
                selfcheck: None,
            },
        ),
    ]
}

/// 职能评审预设（`board.review.professions.<slug>`；B1）。三个键全 Option：
/// 只覆盖显式设置的维度，其余沿用内置预设/全局 `board.review.*`——局部
/// 预设不必照抄全局值。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Default)]
pub struct BoardProfessionReviewPreset {
    /// 覆盖 `board.review.max_turns`（0 = 不限取证轮数）。
    #[serde(default)]
    pub max_turns: Option<u32>,
    /// 覆盖 `board.review.checkers`（多检查员路数）。
    #[serde(default)]
    pub checkers: Option<u32>,
    /// 覆盖 `board.review.selfcheck`（评审取证挂起开关）。
    #[serde(default)]
    pub selfcheck: Option<bool>,
}

/// 解析后的评审参数三元组（[`BoardReviewConfig::resolve_for_profession`]
/// 返回值；消费点直接替换对 `board.review.*` 的直读）。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BoardReviewParams {
    pub max_turns: u32,
    pub checkers: u32,
    pub selfcheck: bool,
}

impl Default for BoardReviewConfig {
    fn default() -> Self {
        Self {
            // M6-3（2026-10-10）：默认 1 → 0（0 = 不限，取证到能下结论为
            // 止）。max_turns=1 时评审 agent 单轮取证经常不够下结论 → UNSURE
            // 转人工或假 FAIL；doom-loop / estop 守卫仍在，0 语义安全（T17
            // 已对齐 UI min=0）。
            max_turns: 0,
            selfcheck: false,
            auto_close_project: false,
            checkers: 1,
            tier: "thorough".to_string(),
            exec_anchors_enabled: false,
            exec_anchor_timeout_secs: 120,
            failure_retries: 1,
            max_unsure_streak: 2,
            professions: std::collections::HashMap::new(),
            parent_remediation_max: 2,
            require_delivery_evidence: false,
            evidence_typed_required: false,
            required_checks: std::collections::HashMap::new(),
        }
    }
}

/// 自动流转预算保险丝（`board.budget` 段；全自动流转 P4/E1；0 = 关闭）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardBudgetConfig {
    /// 单个父单最大子单数（planner 拆解超出即告警；硬性派发侧预算）。
    pub max_subissues_per_parent: u32,
    /// 单个父单全链累计重派次数上限（含全部子单的 dispatch 记录；0 = 关闭）。
    pub max_total_redispatch: u32,
    /// 单个父单从创建起的墙钟预算秒数（超时不再自动重派转人工；0 = 关闭）。
    pub wall_clock_budget_secs: u64,
    /// 单个父单全链 token 预算（E1 二期：worker 经回调回传的用量累计，
    /// input+output 口径；0 = 关闭。依赖集群回传，单机路径无 token 记账）。
    pub max_tokens_per_parent: u64,
    /// 单个子单送达失败自动重试次数上限（F2 重试池；**按本单 ⛔ 标记评论
    /// 计数**，与 max_total_redispatch 的父单全链派发行口径互不掺和）。
    /// 超限 → 🛑 系统评论转人工、出池。默认 5（退避阶梯 60+120+240+480+960
    /// = 1860s ≈ 31 分钟内重试 5 次，此后第 6 次失败熔断）；0 = 关闭保险丝
    ///（无限重试）。`unlimited_mode` = true 时视为 0（操作者显式放弃护栏）。
    pub max_send_retries: u32,
}

impl Default for BoardBudgetConfig {
    fn default() -> Self {
        Self {
            max_subissues_per_parent: 20,
            max_total_redispatch: 0,
            wall_clock_budget_secs: 0,
            max_tokens_per_parent: 0,
            max_send_retries: 5,
        }
    }
}

/// 项目档案传输护栏（`board.archive` 段；看板项目档案 goal P3/D4）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardArchiveConfig {
    /// 单次档案传输最大字节数（worker 与 master 双侧同源护栏）。超限诚实
    /// 失败：worker 侧标记 over_limit 停推 + 通知 master 出决策流/收件箱卡，
    /// 评审转人工；**绝不截断、绝不只回索引**。0 = 不限（不建议）。
    /// 默认 2 GiB。
    pub max_transfer_bytes: u64,
}

impl Default for BoardArchiveConfig {
    fn default() -> Self {
        Self {
            max_transfer_bytes: 2 * 1024 * 1024 * 1024, // 2 GiB
        }
    }
}

/// 任务档案导出配置（`board.dossier` 段；2026-09-29）。导出本体见
/// `nemesis_board::dossier`（单任务树/整项目两入口；WSAPI `dossier.export`
/// 与 CLI `nemesisbot dossier export`）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardDossierConfig {
    /// 项目收口（completed，自动 PASS / 人工 project.update 两条路径）
    /// 后自动生成项目任务档案到 `<workspace>/logs/dossiers/`（默认 true；
    /// 失败只 WARN + 父单评论留痕，不阻塞收口）。
    pub auto_on_complete: bool,
}

impl Default for BoardDossierConfig {
    fn default() -> Self {
        Self {
            auto_on_complete: true,
        }
    }
}

/// 任务拆解配置（`board.plan` 段；Swarm M1；超时重拆四键 2026-10-11 无人值守
/// 硬化 §5#1——凡可致流程停滞处必有开关）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardPlanConfig {
    /// planner 拆解用模型别名（None = 主模型）。
    pub model: Option<String>,
    /// 拆解后跳过人工确认闸直接建单派发（默认 false = 拆完等人确认）。
    /// 消费点：`issue_plan` 一段异步拆解产出 plan_ready 时现读本旗标，
    /// true 即自动 confirm 发车（全自动流转 P1 接线；planner 失败仍转
    /// 人工通知，不自动重试）。
    pub auto_confirm: bool,
    /// 单次拆解链超时阈值秒数（默认 1200 = 20 分钟，用户裁定；0 = 不限）。
    /// 超时处置链见 `timeout_reattempt`。计时起点 = 单次链的 started_at
    ///（重拆后重置）——这是「单次拆解」阈值，不是累计。
    pub timeout_secs: u64,
    /// 拆解重试上限（默认 3；0 = 不限）。超时重拆达到上限后停车：父单评论
    ///「拆解连续超时」+ `board.plan_failed` 事件 + RUNNING_PLANS 摘除，
    /// 等人工重拆——停车而非无限烧 LLM。
    pub max_attempts: u32,
    /// 拆解超时是否自动重拆（默认 true）。false = 超时不重拆，直接转人工
    /// 评论（现行为补强前的语义，保留开关）。
    pub timeout_reattempt: bool,
    /// gateway 重启后自动恢复中断拆解（N1；默认 true）。重启杀拆解任务且
    /// RUNNING_PLANS 是内存态——旧行为 = 父单停留「已开始拆解」评论态等
    /// 手动重拆（无人值守语义下 = 流程中断）。true 时启动扫描满足
    /// 「状态为 backlog/todo 且带拆解中评论形态且不在 RUNNING_PLANS」的
    /// 单子，自动重新发起拆解（走同一超时重拆链，attempt 重计）。
    /// false = 关回旧行为。手动 cancel 的单子状态非 backlog/todo 天然排除；
    /// 恢复拆解同受 `max_attempts` 闸。
    pub restart_recover: bool,
}

impl Default for BoardPlanConfig {
    fn default() -> Self {
        Self {
            model: None,
            auto_confirm: false,
            timeout_secs: 1200,
            max_attempts: 3,
            timeout_reattempt: true,
            restart_recover: true,
        }
    }
}

/// LLM 端点不可用期封存配置（`board.unavailable` 段；U-B 断网封存）。
/// 语义：master 侧 circuit breaker 只认**连接建立类**错误（connect refused /
/// DNS 失败；reqwest `is_connect`/`is_request`）——超时不计入（慢模型/长
/// 思考跑满 provider 600s 是常态，计入会误封存）。连续阈值次连接类失败 →
/// 端点标记 unavailable → 封存：只短路「下一步需要 LLM 的自动动作」（拆解
/// 发起 / 评审触发 / 自动重派），不需 LLM 的动作照常；手动操作放行 + WARN。
/// 恢复探针发最小真实 completion（TCP 通 ≠ LLM 活）。estop 优先级更高；
/// 封存状态独立于 estop（release 不解封，探针才解封）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardUnavailableConfig {
    /// 封存开关（默认 **true**——封存是无人值守语义的默认正确行为）。
    /// false = 关回各环节各自失败现状（连接失败各自 fail_class 处理）。
    pub freeze: bool,
    /// 连续连接类失败判定阈值（默认 3）。
    pub connect_fail_threshold: u32,
    /// 恢复探针间隔秒数（默认 300）。**0 = 只封存不自动解封**（人工恢复：
    /// 手动触发探针或重启）。
    pub probe_interval_secs: u64,
}

impl Default for BoardUnavailableConfig {
    fn default() -> Self {
        Self {
            freeze: true,
            connect_fail_threshold: 3,
            probe_interval_secs: 300,
        }
    }
}

/// 频道讨论区配置（`board.discussion` 段；Swarm M2/M3）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardDiscussionConfig {
    /// 频道消息保留天数（默认 90；0 = 永久保留）。
    pub retention_days: i64,
    /// 每线程 agent 发言额度（master 记账随 wake.post 下发；默认 8，0=不限）。
    pub max_agent_turns_per_thread: u32,
    /// 每节点每小时发言上限（默认 20，0=不限）。
    pub hourly_budget_per_node: u32,
    /// 每节点每分钟 board.comment.post 上行限速（默认 12，0=不限）。
    pub rate_limit_per_min: u32,
    /// D2 定稿评审有界等待（秒；默认 1800，0=不等直接裁决）。定稿单
    /// worker 据此 sleep + 轮询讨论线程（双阈值的时间维；条数维在提示词
    /// 契约里按职能自选）。消费面 = 定稿单任务提示词渲染（master config
    /// 经 planner/派发描述传给 worker，worker 本地 config 不作数）。
    pub review_wait_secs: u32,
}

impl Default for BoardDiscussionConfig {
    fn default() -> Self {
        Self {
            retention_days: 90,
            max_agent_turns_per_thread: 8,
            hourly_budget_per_node: 20,
            rate_limit_per_min: 12,
            review_wait_secs: 1800,
        }
    }
}

/// 看板数据库每日备份配置（`board.backup` 段；Swarm M2）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(default)]
pub struct BoardBackupConfig {
    /// 备份保留份数（默认 14 = 两周；0 = 关闭每日备份）。
    pub keep: u32,
}

impl Default for BoardBackupConfig {
    fn default() -> Self {
        Self { keep: 14 }
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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorSeparationConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub sandbox: bool,
    #[serde(default)]
    pub allow_network: bool,
    #[serde(default)]
    pub strict: bool,
    /// P24（2026-09-25 能力扩展 WS1）：沙盒后端选择（`executor.backend`，
    /// 缺省 `"auto"`）。值域：`auto`（默认——Windows 上 Sandboxie 就绪则
    /// sandboxie，否则退 Windows 用户态 ACL 轻量档）/ `sandboxie`（显式钉
    /// Sandboxie）/ `acl`（显式钉 ACL 档）。未知值诚实拒绝（选型返回
    /// None + warn，不静默改道）。非 Windows 平台无消费方（landlock/bwrap/
    /// Seatbelt 选型不读此键）。
    ///
    /// ⚠ 实验性标注：acl 档是**半档隔离**（强制完整性标签 No-Write-Up
    /// 围栏 + DACL 原语），禁不了网、写围栏依赖令牌降级——详见
    /// `crates/nemesis-sandbox/src/backend.rs` 的 P24 模块文档与诚实边界。
    /// 兼容性：`#[serde(default)]`——老 config.json 缺键 = `"auto"`，行为与
    /// 升级前完全一致（typed save 会把缺省值显式写回，语义不变）。
    #[serde(default = "default_executor_backend")]
    pub backend: String,
    /// D4（2026-09-27）：DACL 定向档开关组（`executor.acl` 段，缺省全关
    /// = 现状字节不变）。见 [`ExecutorAclConfig`]。
    #[serde(default)]
    pub acl: ExecutorAclConfig,
    /// P3 T4：常驻执行体开关组（`executor.persistent` 段，缺省全关 =
    /// per-call 现状）。见 [`ExecutorPersistentConfig`]。
    #[serde(default)]
    pub persistent: ExecutorPersistentConfig,
    /// P5（桌面级 VM，2026-10-17）：VM 后端开关组（`executor.vm` 段）。
    /// **铁律：`enabled` 默认 false**——关闭态 VM 路径零激活（零 QEMU 进程、
    /// 零镜像下载、工具走原路径），全链行为字节不变（验收门 G4 专项断言）。
    /// 见 [`ExecutorVmConfig`]。
    #[serde(default)]
    pub vm: ExecutorVmConfig,
}

/// `executor.persistent` 段：常驻执行体（per-agent 长驻子进程，P3 T4）的
/// 细粒度开关。缺省全关 = per-call 形态字节不变——常驻是增强档不是替换
/// 闸，不可用（满员/spawn 失败/传输不可恢复）时全量回落 per-call Layer1/2。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorPersistentConfig {
    /// 常驻执行体总开关（默认 false；true 时高危工具优先路由到常驻子进程）。
    #[serde(default)]
    pub enabled: bool,
    /// 本节点常驻执行体槽位上限（满员时新调用回落 per-call，不排队不拒绝）。
    #[serde(default = "default_persistent_max_slots")]
    pub max_slots: u32,
    /// 空闲自动 sleep 秒数（watchdog 扫描；0 = 不自动睡，由显式 sleep 托管）。
    #[serde(default = "default_persistent_idle_secs")]
    pub idle_shutdown_secs: u64,
    /// 单次工具调用的传输超时秒数（子进程活着但没回话）。
    #[serde(default = "default_persistent_call_timeout_secs")]
    pub call_timeout_secs: u64,
}

fn default_persistent_max_slots() -> u32 {
    4
}

fn default_persistent_idle_secs() -> u64 {
    300
}

fn default_persistent_call_timeout_secs() -> u64 {
    600
}

impl Default for ExecutorPersistentConfig {
    /// 手写而非 derive（ExecutorAclConfig 同款 serde 双路径陷阱——结构体级
    /// Default 不经过字段级 serde 缺省）：缺键 = enabled 关 + 满配额缺省。
    fn default() -> Self {
        Self {
            enabled: false,
            max_slots: default_persistent_max_slots(),
            idle_shutdown_secs: default_persistent_idle_secs(),
            call_timeout_secs: default_persistent_call_timeout_secs(),
        }
    }
}

/// `executor.acl` 段：DACL 定向档（write-restricted 受限令牌 + workspace
/// SID GRANT ACE 树，D1-D3 原语）的细粒度开关。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExecutorAclConfig {
    /// DACL 定向档总开关（默认 false = opt-in；开 = executor 子进程经
    /// `CreateProcessAsUserW` + 受限令牌 spawn——内核强制写围栏，与子进程
    /// 自装完整性标签两层叠加）。关/装配失败（宽松模式）→ 回退现状路径。
    #[serde(default)]
    pub dacl: bool,
    /// 本档自己的 fail-closed 闸（与全局 `executor.strict` 独立——那条只认
    /// Sandboxie）：true 时 DACL 装配失败 = **拒绝执行**（不降级）。
    #[serde(default)]
    pub strict: bool,
    /// 树遍历预算（GRANT ACE 打标的文件数上限，同 `label_tree` 既有语义）。
    #[serde(default = "default_acl_max_files")]
    pub max_files: u32,
}

impl Default for ExecutorAclConfig {
    /// 手写而非 derive（serde 双路径陷阱，hook_flips 真进程测试抓出）：
    /// executor 段**无 acl 键**时 serde 走**结构体级** `Default::default()`
    /// ——字段级 `#[serde(default = "...")]` 不经过，derive 的 `u32` 默认
    /// 是 0，会让 hook 的铺树预算直接耗尽（写操作全死）。手写让两条构造
    /// 路径收敛到同一组默认值。
    fn default() -> Self {
        Self {
            dacl: false,
            strict: false,
            max_files: default_acl_max_files(),
        }
    }
}

/// `executor.acl.max_files` 缺省值（设计稿 §3.4：20000）。
fn default_acl_max_files() -> u32 {
    20_000
}

/// `executor.backend` 的 serde 缺省值（`"auto"`）。独立函数而非 `String::default`
/// ——空字符串不是合法选择（选型按未知值诚实拒绝），缺键必须落成 `"auto"`。
fn default_executor_backend() -> String {
    "auto".to_string()
}

impl Default for ExecutorSeparationConfig {
    /// 手写而非 derive（与 [`ExecutorAclConfig`] 同款 serde 双路径陷阱）：
    /// `backend` 的 derive 默认是空串，而空串不是合法选型值（诚实拒绝）——
    /// `unwrap_or_default()`（sandbox handler 对缺失 executor 段的补齐路径）
    /// 必须落成 `"auto"` 而不是 `""`。
    fn default() -> Self {
        Self {
            enabled: false,
            sandbox: false,
            allow_network: false,
            strict: false,
            backend: default_executor_backend(),
            acl: ExecutorAclConfig::default(),
            persistent: ExecutorPersistentConfig::default(),
            vm: ExecutorVmConfig::default(),
        }
    }
}

/// `executor.vm` 段：桌面级 VM 后端（P5，第三类隔离后端——独立内核整机
/// 隔离）。缺省 = VM 路径零激活（铁律：总开关默认关，绝不替用户决定）。
///
/// 枚举型字符串（arch/hypervisor/desktop/gc.over_limit）未知值 loud 拒绝
/// （[`ExecutorVmConfig::validate`]，对齐 `executor.backend` P24 先例）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutorVmConfig {
    /// VM 总开关（默认 false——关闭态零 QEMU 进程、零镜像下载、工具走
    /// 原路径；Dashboard VM 页同款开关，用户可选开、关）。
    #[serde(default)]
    pub enabled: bool,
    /// Guest CPU 架构：`x86_64`（当前唯一产出档）| `aarch64`（接口在位、
    /// 当前不产出——选了诚实报不支持，恢复路径见实施计划 §5）。
    #[serde(default = "default_vm_arch")]
    pub arch: String,
    /// 硬件加速：`auto`（探测裁决）| `kvm`（显式钉 Linux KVM）| `whpx`
    /// （显式钉 Windows WHPX）。加速缺席 = 诚实 Unavailable（不做 TCG
    /// 假跑降级；`executor.strict` 开启时拒绝执行）。
    #[serde(default = "default_vm_hypervisor")]
    pub hypervisor: String,
    /// base 镜像桌面形态：`minimal`（轻量 WM + Chromium，默认）| `full`
    /// （完整 DE，轮次 3 拍板的可选项）。两变体 base 镜像分开构建。
    #[serde(default = "default_vm_desktop")]
    pub desktop: String,
    /// base 镜像来源（轮次 3-3：来源写死在本地配置中，页面可看可选可配
    /// 可改）。
    #[serde(default)]
    pub image: VmImageConfig,
    /// per-VM 资源配额。
    #[serde(default)]
    pub resources: VmResourceConfig,
    /// 本节点并发 VM 实例上限（满员闸，对齐 persistent.max_slots 语义——
    /// 满员诚实拒绝）。
    #[serde(default = "default_vm_max_instances")]
    pub max_instances: u32,
    /// 空闲自动 sleep 秒数（0 = 不自动睡；对齐 persistent.idle_shutdown_secs）。
    #[serde(default = "default_vm_idle_sleep_secs")]
    pub idle_sleep_secs: u64,
    /// guest agent 就绪等待预算秒数（冷启/唤醒统一）：QMP running ≠ guest
    /// 侧 agent 已监听——cloud-init/systemd 还在走（微型镜像典型 20-60s，
    /// 缺省 240 给慢盘慢机余量）。就绪前交槽 = 首次 guest 调用 connect
    /// refused → 宿主回落混两套文件视图——引导墙钟必须在交槽前吃掉。
    /// 缺省 240 = 原 vm_runtime 编译期常量行为字节不变（P5-R4 配置化）。
    /// 0 拒绝（无预算 = 启动可无限挂起，不是合法形态）。
    #[serde(default = "default_vm_ready_budget_secs")]
    pub ready_budget_secs: u64,
    /// 显示面（P5-R2 带界面 VM）：`none`（默认，`-display none` 无头形态
    /// 字节不变）| `vnc`（QEMU `-vnc 127.0.0.1:<display>` 回环 VNC——端口
    /// 走临时端口段分配、真实 TCP 端口进 registry，QEMU 参数按 display=
    /// port-5900 反向换算；经 plugin-vm 插件中继到 Dashboard）。vnc 态要求
    /// desktop variant 镜像（headless 镜像 + vnc = 诚实报错，黑屏不是合法形态）。
    #[serde(default = "default_vm_display")]
    pub display: String,
    /// vnc 态分辨率预设（guest 侧 xinit/xrandr 消费；`宽x高` 形态，缺省
    /// 1280x720）。display=none 时该键无效（不校验与显示面的耦合，键独立
    /// 合法即可——无头形态下改分辨率无消费方）。
    #[serde(default = "default_vm_display_resolution")]
    pub display_resolution: String,
    /// 磁盘 GC（总磁盘预算）。
    #[serde(default)]
    pub gc: VmGcConfig,
}

/// `executor.vm.image` 段：base 镜像来源（URL + SHA-256）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmImageConfig {
    /// 镜像下载 URL（默认 Debian generic cloud image；可自由编辑——镜像
    /// 来源可换是验收面）。
    #[serde(default = "default_vm_image_source")]
    pub source: String,
    /// 镜像 SHA-256（hex）。**空 = 构建时从 `{source}` 同目录的 SHA256SUMS
    /// sidecar 按文件名查表**（Debian 目录惯例）——模板不带易过期的硬编码
    /// 哈希；显式钉住优先于 sidecar。
    #[serde(default)]
    pub sha256: String,
}

/// `executor.vm.resources` 段：per-VM 资源配额。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmResourceConfig {
    #[serde(default = "default_vm_vcpu")]
    pub vcpu: u32,
    #[serde(default = "default_vm_ram_mb")]
    pub ram_mb: u32,
    #[serde(default = "default_vm_disk_gb")]
    pub disk_gb: u32,
}

/// `executor.vm.gc` 段：磁盘 GC（总磁盘预算）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VmGcConfig {
    /// 全部 VM 磁盘产物（base + overlay）总预算 GB；超限动作按 `over_limit`。
    /// 0 = 不设预算。
    #[serde(default = "default_vm_gc_max_disk_gb")]
    pub max_total_disk_gb: u32,
    /// 超限动作：`warn`（默认，日志横幅可见）| `stop`（拒绝新实例创建，
    /// 既有实例不受影响）。
    #[serde(default = "default_vm_gc_over_limit")]
    pub over_limit: String,
}

fn default_vm_arch() -> String {
    "x86_64".to_string()
}

fn default_vm_hypervisor() -> String {
    "auto".to_string()
}

fn default_vm_desktop() -> String {
    "minimal".to_string()
}

fn default_vm_display() -> String {
    "none".to_string()
}

fn default_vm_display_resolution() -> String {
    "1280x720".to_string()
}

fn default_vm_max_instances() -> u32 {
    2
}

fn default_vm_idle_sleep_secs() -> u64 {
    300
}

fn default_vm_ready_budget_secs() -> u64 {
    240
}

fn default_vm_vcpu() -> u32 {
    2
}

fn default_vm_ram_mb() -> u32 {
    2048
}

fn default_vm_disk_gb() -> u32 {
    20
}

fn default_vm_gc_max_disk_gb() -> u32 {
    50
}

fn default_vm_gc_over_limit() -> String {
    "warn".to_string()
}

/// Debian generic cloud image（trixie/latest 的 genericcloud 变体——无
/// 云初始化代理依赖的最小 cloud-init 面）。可自由编辑（轮次 3-3 拍板）。
/// 与 build-vm-image.sh（plugins/plugin-vm/，随 plugin-vm 插件包分发）的
/// DEFAULT_BASE_URL 必须同源：config 缺省
/// sha256 为空时，脚本只对**自己的默认 URL** 自动走上游 SHA256SUMS→
/// SHA512SUMS 校验链；两处默认一旦分叉，出盒链路会被脚本按「换源」诚实
/// 拒绝（缺显式哈希）——B5 全新环境实跑实录。
fn default_vm_image_source() -> String {
    "https://cloud.debian.org/images/cloud/trixie/latest/debian-13-genericcloud-amd64.qcow2"
        .to_string()
}

impl Default for ExecutorVmConfig {
    /// 手写而非 derive（serde 双路径陷阱，ExecutorPersistentConfig 同款）：
    /// executor 段**无 vm 键**时 serde 走结构体级 Default，字段级
    /// `#[serde(default = "...")]` 不经过——两条构造路径必须收敛到同一组
    /// 默认值（enabled=false 铁律在两条路径都成立）。
    fn default() -> Self {
        Self {
            enabled: false,
            arch: default_vm_arch(),
            hypervisor: default_vm_hypervisor(),
            desktop: default_vm_desktop(),
            display: default_vm_display(),
            display_resolution: default_vm_display_resolution(),
            image: VmImageConfig::default(),
            resources: VmResourceConfig::default(),
            max_instances: default_vm_max_instances(),
            idle_sleep_secs: default_vm_idle_sleep_secs(),
            ready_budget_secs: default_vm_ready_budget_secs(),
            gc: VmGcConfig::default(),
        }
    }
}

impl Default for VmImageConfig {
    fn default() -> Self {
        Self {
            source: default_vm_image_source(),
            sha256: String::new(),
        }
    }
}

impl Default for VmResourceConfig {
    fn default() -> Self {
        Self {
            vcpu: default_vm_vcpu(),
            ram_mb: default_vm_ram_mb(),
            disk_gb: default_vm_disk_gb(),
        }
    }
}

impl Default for VmGcConfig {
    fn default() -> Self {
        Self {
            max_total_disk_gb: default_vm_gc_max_disk_gb(),
            over_limit: default_vm_gc_over_limit(),
        }
    }
}

impl ExecutorVmConfig {
    /// 枚举型字符串字段校验（未知值 loud 拒绝——选型错误不静默改道）。
    /// 配置装载/`vm.config.set` 两面同源消费。
    pub fn validate(&self) -> Result<()> {
        // arch：与 nemesis-sandbox vm::guest_arch 同值域（crate 分层上
        // nemesis-config 不依赖 sandbox——值域在此重复一份，单测钉同步）。
        match self.arch.as_str() {
            "x86_64" | "aarch64" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.arch 未知值 {other:?}（合法值：x86_64 | aarch64）"
                )));
            }
        }
        match self.hypervisor.as_str() {
            "auto" | "kvm" | "whpx" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.hypervisor 未知值 {other:?}（合法值：auto | kvm | whpx）"
                )));
            }
        }
        match self.desktop.as_str() {
            "minimal" | "full" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.desktop 未知值 {other:?}（合法值：minimal | full）"
                )));
            }
        }
        match self.display.as_str() {
            "none" | "vnc" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.display 未知值 {other:?}（合法值：none | vnc；\
                     vnc 需 plugin-vm 插件 + desktop variant 镜像）"
                )));
            }
        }
        // display_resolution：`宽x高` 形态（正整数），guest xrandr 消费面。
        if let Err(msg) = validate_display_resolution(&self.display_resolution) {
            return Err(ConfigError::Validation(format!(
                "executor.vm.display_resolution {msg}（合法形态：宽x高，如 1280x720）"
            )));
        }
        match self.gc.over_limit.as_str() {
            "warn" | "stop" => {}
            other => {
                return Err(ConfigError::Validation(format!(
                    "executor.vm.gc.over_limit 未知值 {other:?}（合法值：warn | stop）"
                )));
            }
        }
        if self.max_instances == 0 {
            return Err(ConfigError::Validation(
                "executor.vm.max_instances 必须 ≥ 1（0 无法创建任何实例；\
                 要彻底关闭 VM 请用 executor.vm.enabled=false）"
                    .to_string(),
            ));
        }
        if self.ready_budget_secs == 0 {
            return Err(ConfigError::Validation(
                "executor.vm.ready_budget_secs 必须 ≥ 1（0 = guest 就绪等待无上限，\
                 启动可无限挂起不是合法形态；要更多余量请直接调大数值）"
                    .to_string(),
            ));
        }
        Ok(())
    }
}

/// `executor.vm.display_resolution` 形态校验：`宽x高`（正整数）。
/// 单独函数（validate 与未来消费方同源复用），错误消息只含值态不含键名
/// （键名由调用方补——文案单一出口）。
pub fn validate_display_resolution(v: &str) -> std::result::Result<(), String> {
    let (w, h) = v
        .split_once('x')
        .ok_or_else(|| format!("未知值 {v:?}，缺 'x' 分隔"))?;
    let w: u32 = w
        .parse()
        .map_err(|_| format!("未知值 {v:?}，宽度非正整数"))?;
    let h: u32 = h
        .parse()
        .map_err(|_| format!("未知值 {v:?}，高度非正整数"))?;
    if w == 0 || h == 0 {
        return Err(format!("未知值 {v:?}，宽高必须 > 0"));
    }
    Ok(())
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
    /// H7 (U13 half): claude CLI delegation tool settings. Default
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
    /// G2 (devtool-upgrade 阶段 3): sub-agent spawn settings.
    #[serde(default)]
    pub subagent: SubagentConfig,
    /// I1 (devtool-upgrade 阶段 3): workspace fs-watcher settings.
    #[serde(default)]
    pub fs_watcher: FsWatcherConfig,
    /// T2（L1 codemode）: `code_run` 脚本工具开关与资源限制（默认关）。
    #[serde(default)]
    pub codemode: CodeModeConfig,
    /// A3（委派持续操控）: delegate 四工具开关与子代理生命上限（默认关）。
    #[serde(default)]
    pub delegate: DelegateToolConfig,
    /// L8（内部专项总计划重评估）：工具调用三态 WAL——工具
    /// admit/start/finish 每态一行 JSONL 落 `<workspace>/logs/turn_wal/`，
    /// 进程被强杀（taskkill /F）后重启扫描补 interrupted 并自动续轮。
    /// 默认开；关闭后完全不落盘。诚实边界：纯 flush 无 fsync，断电不保。
    #[serde(default = "default_true")]
    pub turn_wal: bool,
    /// L10（内部专项总计划重评估）：provider 流式输出观测面——主 turn
    /// LLM 调用经流式 lane（provider 支持则真实增量，否则聚合后合成）发
    /// `provider_stream_event` 易逝帧（SSE + WS push，不入补拉环）。默认
    /// 开；关闭后零事件零开销。运行时改键下一轮生效（每轮新鲜读）。
    #[serde(default = "default_true")]
    pub provider_stream_events: bool,
    /// F8 (devtool-upgrade 阶段 3): tool names hidden from the model's
    /// tool_defs AND rejected at dispatch (double gate). Entries may carry a
    /// trailing `*` wildcard (e.g. `mcp_*` hides every MCP tool). Runtime
    /// edits take effect from the next turn (fresh-read, no restart).
    #[serde(default)]
    pub hidden_tools: Vec<String>,
    /// 角色目录分档供给配套（2026-09-28 角色目录与分档供给）：`hidden`
    /// = 从 spawn schema 与 dispatch 双闸隐藏的角色 slug（镜像
    /// hidden_tools 的 F8 双闸模型；通配语义不开放——角色目录是静态
    /// 17 项，按名精确匹配）。运行时改键下一轮生效（fresh-read）。
    #[serde(default)]
    pub roles: RolesConfig,
    /// N2 (devtool-upgrade 阶段 4): 小模型专职杂务通道 — a model reference
    /// (alias from `model_list` or `vendor/model`) used for cheap chores.
    /// Sole consumer today: manual `/compact` summarization (E6) — auto
    /// compression stays on the main model (quality-sensitive). Resolved to
    /// a provider at AgentLoop construction; absent/empty/unresolvable =
    /// honest fallback to the main model (warn in the factory, never a
    /// startup blocker). Runtime edits need an Agent restart.
    #[serde(default)]
    pub small_model: Option<String>,
    /// B7（2026-10-13）：guardian LLM 审计专用模型通道（优先于
    /// `small_model`——审计点独立于 /compact 杂务，可用不同档位；两者都
    /// 未配置 = 回落主模型）。Absent/empty/unresolvable = 沿阶梯回落
    /// （small_model → main），never a startup blocker。消费点唯一：
    /// gateway guardian judge 装配（commands/gateway/runtime.rs）。
    #[serde(default)]
    pub guardian_model: Option<String>,
    /// J5 (devtool-upgrade 阶段 6): doom-loop 审批化开关。默认关——turn_guard
    /// 的 escalation 硬停是安全底座。开且 question asker 已装配时，escalation
    /// 触发先发提问卡问用户「继续吗？」：approve = 清签名计数继续；deny /
    /// 超时 / asker 缺失 = 现行为（停轮）。loop 侧 fresh-read（每次 escalation
    /// 现读 config.json），运行时改键下一轮生效，无需重启。
    #[serde(default)]
    pub doom_loop_approval: bool,
    /// J6 (devtool-upgrade 阶段 6): 超限图片自动降采样开关。默认**开**——
    /// 超过 25MB 或最长边超过 8000px 的图片在附加前先降采样（阶梯
    /// 4096→2048→1024，JPEG q85，产物不超过 8MB 落 uploads 内容寻址复用）；
    /// 关 = 原 25MB 硬拒绝。loop 侧 fresh-read（每条消息现读 config.json），
    /// 运行时改键下一轮生效。
    #[serde(default = "default_true")]
    pub image_downscale: bool,
    /// WS9/P22：workspace 写租约开关。默认**开**——主 loop / 项目 loop /
    /// executor 子进程的写类工具经 workspace 级文件锁互斥（宽限 30s 超时
    /// 诚实拒绝；持有进程死亡 OS 自动释放）。**装配期消费**（gateway/
    /// exec_worker 启动时按本键构造租约并包装写类工具）——改键需重启
    /// 生效（与 executor 段同语义，非 loop 侧 fresh-read）。
    #[serde(default = "default_true")]
    pub lease_enabled: bool,
    /// 件4（2026-09-24 HOOK 三合一收口 §6）：纪律闭环总开关。默认
    /// **false**（D5 灰度）——关 = 闸/证伪钩子不注册、`/discipline` 提示
    /// 未启用、任务 marker 不生效。开 = 任务描述含 `[discipline:bugfix]`
    /// 或 `/discipline on` 的会话进入参与态（声明闸 + 证伪预算，见
    /// nemesis-agent::discipline）。
    #[serde(default)]
    pub discipline: DisciplineConfig,
    /// 提示词包体系选择（2026-09-27 prompt-pack pro）。`pro` = 全中文分层
    /// 提示词体系（前置段 → 人格文件 → 行为段池 → 环境段，段落池确定性
    /// 组装，工具描述按模型能力档位两档供给）；`classic` = 既有装配路径
    /// （人格文件拼接 + 固定尾巴，字节级不变）。启动装配型：system prompt
    /// 在启动时构建后随会话冻结，运行时改键需重启（或重建实例）生效。
    /// 空串按未配置 = `pro`；未知非空值按 `classic` 处理并 warn
    /// （fail-safe 到已知旧行为）。解析在 nemesis-agent::prompt。
    #[serde(default = "default_prompt_system")]
    pub prompt_system: String,
    /// P32（cache-warmer）配置节（默认关；语义见 [`CacheWarmerConfig`]）。
    /// typed 字段化以保证 save_config 往返不丢键（vision extra 键回归的
    /// 同类预防——AgentsConfig 无 flatten extra，未声明键会被 typed 保存
    /// 静默抹掉）。
    #[serde(default)]
    pub cache_warmer: CacheWarmerConfig,
    /// NB-122 批（2026-10-16）④：单次工具调用墙钟看门狗（秒）。0 = 关。
    /// 默认 1800s（合法长活如 cargo build --release 10-20 分钟要放得过；
    /// 卡死调用的兜底闸而非提速闸）。loop 侧 fresh-read（每次 dispatch 现
    /// 读 config.json），运行时改键下一轮生效。诚实边界：超时放弃的是
    /// future，已 spawn 的子进程可能成为孤儿（B4 后台注册表/停摆清扫兜底）。
    #[serde(default = "default_executor_tool_timeout_secs")]
    pub executor_tool_timeout_secs: u64,
}

/// `agents.executor_tool_timeout_secs` 缺键默认（30 分钟）。
fn default_executor_tool_timeout_secs() -> u64 {
    1800
}

/// `agents.discipline` 配置节。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DisciplineConfig {
    /// 总开关，默认 false（D5）。
    #[serde(default)]
    pub enabled: bool,
}

/// P32（cache-warmer）`agents.cache_warmer` 配置节。消费方在
/// `nemesis-agent` loop/cache_warmer.rs（fresh-read，运行中改键热关生效；
/// 启动后才开需重启——诚实不对称见该模块注释）。类型在此单一真相源，
/// agent 侧经 serde 反序列化消费同一份默认值。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheWarmerConfig {
    /// 总开关，默认 **false**（D-4 成本敏感 opt-in）——关 = gateway 启动
    /// 不 spawn 任何定时任务（零副作用）。开 = idle 会话在 prompt cache
    /// TTL×90% 时用 1-token 重放刷新缓存（TTL 需模型条目显式声明
    /// `cache_ttl_secs`，未知不 warm）。
    #[serde(default)]
    pub enabled: bool,
    /// 单次 warm 重放的成本上限（USD）。按价目表保守估算（全价 input +
    /// 1 output token，不假设 cache read 折扣）；估算超限或价目表未命中
    /// → 跳过。默认 0.05。
    #[serde(default = "default_cache_warmer_cost_limit_usd")]
    pub cost_limit_usd: f64,
}

impl Default for CacheWarmerConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cost_limit_usd: default_cache_warmer_cost_limit_usd(),
        }
    }
}

fn default_cache_warmer_cost_limit_usd() -> f64 {
    0.05
}

/// `agents.roles` 配置节（2026-09-28 角色目录与分档供给）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RolesConfig {
    /// 隐藏的角色 slug 列表（spawn schema 枚举照旧全量、dispatch 拒绝——
    /// F8 双闸；`roles_visible_to` 过滤后的子集再减本表）。
    #[serde(default)]
    pub hidden: Vec<String>,
}

/// I1 (devtool-upgrade 阶段 3): `agents.fs_watcher` config section —
/// workspace file watching (notify-based, recursive). External edits
/// surface to the agent as a one-shot `<external_changes>` annotation in
/// the next context snapshot; instruction-chain files (AGENTS.md/CLAUDE.md)
/// additionally invalidate context digests (structural no-op since round-5
/// stateless digests — kept as the anchor, same as the dispatch path's
/// touch-driven call). Watcher startup failure degrades to warn-once and
/// never blocks the loop.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FsWatcherConfig {
    /// Master switch (default true). Failure to start is warn-once +
    /// disabled either way — this only controls whether we try.
    #[serde(default = "default_fs_watcher_enabled")]
    pub enabled: bool,
    /// Extra ignore entries merged over the built-in table (built-in:
    /// `.git`/`node_modules`/`target`/`dist`/`logs`/`uploads`/`memory`/
    /// `cluster`/`board`/`data`/`config`/`tools` dir names + `*.jsonl`/
    /// `*.db*`/`*.sqlite*` extensions). A bare name matches any path
    /// component (workspace-relative); `*.ext` matches by extension.
    #[serde(default)]
    pub ignore: Vec<String>,
}

/// goal P2/D0：派发准入默认上限（单 worker 在途 1 单；0=不限）。
fn default_worker_max_inflight() -> i64 {
    1
}

fn default_fs_watcher_enabled() -> bool {
    true
}

// 手写 Default（不 derive）：`enabled` 的本意默认是 true，derive 给零值
// false（同 DiagnosticsLoopConfig 的坑）。
impl Default for FsWatcherConfig {
    fn default() -> Self {
        Self {
            enabled: default_fs_watcher_enabled(),
            ignore: Vec::new(),
        }
    }
}

/// G2 (devtool-upgrade 阶段 3): `agents.subagent` config section — spawn
/// nesting governance. The enforcement lives in `SpawnTool` (nemesis-agent):
/// a spawn request is rejected when `parent_depth + 1 > max_depth`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentConfig {
    /// Maximum sub-agent nesting depth. Depth 0 = the top-level agent;
    /// a spawn from depth N creates a child at depth N+1, allowed only when
    /// N+1 <= max_depth. Default 1 = sub-agents cannot spawn sub-agents
    /// (grandchild requests bounce back to the model with a limit message).
    #[serde(default = "default_subagent_max_depth")]
    pub max_depth: usize,
}

impl Default for SubagentConfig {
    fn default() -> Self {
        Self {
            max_depth: default_subagent_max_depth(),
        }
    }
}

fn default_subagent_max_depth() -> usize {
    1
}

/// T2（内部专项总计划 L1 codemode，2026-10-13）: `agents.codemode` config
/// section — `code_run` 脚本工具（QuickJS 沙箱）的运行开关与资源限制。
/// 编译闸是 `codemode` cargo feature；这里管运行态：`enabled` 默认关
/// （不注册即供给不可见），注册发生在 AgentLoop 装配期（改键需 loop 重建
/// ——切模型/人格重启自动带上，与插件工具同语义）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CodeModeConfig {
    /// Enable the `code_run` tool registration (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Per-call wall-clock budget in seconds; the QuickJS interrupt handler
    /// checks it (default 30). Infinite JS loops are cut at the deadline and
    /// the error is returned to the model as tool result text.
    #[serde(default = "default_codemode_timeout_secs")]
    pub timeout_secs: u64,
    /// QuickJS memory cap in MB via `set_memory_limit` (default 64). A
    /// larger allocation fails as a JS OOM error inside the sandbox.
    #[serde(default = "default_codemode_memory_limit_mb")]
    pub memory_limit_mb: u64,
}

fn default_codemode_timeout_secs() -> u64 {
    30
}

fn default_codemode_memory_limit_mb() -> u64 {
    64
}

impl Default for CodeModeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            timeout_secs: default_codemode_timeout_secs(),
            memory_limit_mb: default_codemode_memory_limit_mb(),
        }
    }
}

/// A3（委派持续操控，2026-10-18）: `agents.delegate` config section —
/// delegate 四工具（delegate_start/send/output/stop）的运行开关与子代理
/// 生命上限。注册发生在 AgentLoop 装配期且要求 CLI 在场（claude/codex 至
/// 少一个在 PATH），改键需 loop 重建生效。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DelegateToolConfig {
    /// Enable the delegate_* tool registration (default false).
    #[serde(default)]
    pub enabled: bool,
    /// Delegate sub-agent lifetime cap in seconds (default 3600). 0 = no
    /// limit; the watchdog tree-kills at the deadline and leaves an honest
    /// marker in the delegate's output stream.
    #[serde(default = "default_delegate_max_lifetime_secs")]
    pub max_lifetime_secs: u64,
    /// claude permission-mode passthrough (default "" = CLI default). Never
    /// implies bypassing the child CLI's own permission surface.
    #[serde(default)]
    pub permission_mode: String,
}

fn default_delegate_max_lifetime_secs() -> u64 {
    3600
}

impl Default for DelegateToolConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            max_lifetime_secs: default_delegate_max_lifetime_secs(),
            permission_mode: String::new(),
        }
    }
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
    /// C6 (devtool-upgrade 阶段 6): 静默自举缺失的语言服务器——true 时网关
    /// 启动期后台串行安装全部缺失的非交互语言服务器（官方安装通道白名单
    /// 目录），装完需重启 Agent 重新探测注册（default false——默认绝不自动装东西）。
    #[serde(default)]
    pub auto_install: bool,
}

/// C4 (2026-09-04 devtool-upgrade 阶段 1): `agents.diagnostics_loop` config
/// section. 编辑→诊断回灌闭环（阶段 2 C1-C3 消费）的独立开关。**与
/// `agents.lsp_tool.enabled` 解耦**：诊断闭环开而 lsp 工具关是合法组合
/// （闭环自身按需起服务器）。
///
/// P2（2026-09-25 能力扩展 WS3）：`enabled` 默认翻转为 **true**（对齐
/// 业界通行的「内置无条件开启」）——缺键/缺段按开处理；用户显式写
/// `false` 的配置不受影响（serde 显式值优先于 default fn）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DiagnosticsLoopConfig {
    /// Enable the edit→diagnostics feedback loop (default true since P2).
    #[serde(default = "default_diagnostics_enabled")]
    pub enabled: bool,
    /// Max diagnostics fed back per edit round (default 20; P3 起为跨文件
    /// 聚合后的总量上限).
    #[serde(default = "default_diagnostics_max_errors")]
    pub max_errors: usize,
    /// Max wall-clock wait for diagnostics after an edit, in ms (default 2000).
    #[serde(default = "default_diagnostics_wait_max_ms")]
    pub wait_max_ms: u64,
}

// 手写 Default（不 derive）：字段级 serde default fn 在 **整个
// `diagnostics_loop` 键缺席**时用的是类型的 Default impl（derive 会给零值
// 20/2000 的本意就落空了——首测抓出）。手写保证「全键缺席」与「部分键
// 缺席」（内部字段的 serde default fn 管）两条路都落在同一组默认值上。
impl Default for DiagnosticsLoopConfig {
    fn default() -> Self {
        Self {
            enabled: default_diagnostics_enabled(),
            max_errors: default_diagnostics_max_errors(),
            wait_max_ms: default_diagnostics_wait_max_ms(),
        }
    }
}

/// P2（能力扩展 WS3）：诊断回灌默认开（同源供 serde 缺键路径与手写
/// Default impl 消费——两路必须落同一值）。
fn default_diagnostics_enabled() -> bool {
    true
}

fn default_diagnostics_max_errors() -> usize {
    20
}

fn default_diagnostics_wait_max_ms() -> u64 {
    2000
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

/// A6 (2026-09-06 devtool-upgrade 阶段 5): `agents.defaults.format_on_save`
/// config section — write/edit 成功后自动跑外部格式化工具（format-on-save）。
/// 执行点在 PostToolUse hooks 之前（用户自定义 hook 看到格式化后文件）、
/// C3 诊断带之前（诊断看到格式化后文件）；`executor.enabled=true` 时诚实
/// 停用（写落子进程/盒内，gateway 侧后格式化会读到陈旧内容甚至绕盒写）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FormatOnSaveConfig {
    /// Enable format-on-save (default false — opt-in).
    #[serde(default)]
    pub enabled: bool,
    /// Per-extension formatter override: ext → argv template（首元素 = 工具
    /// 名，其余 = 参数；`{file}` 占位符替换为目标路径）。按条目覆盖——命中的
    /// ext 用配置 argv，未命中的 ext 回落内置表。空表 = 全内置。
    #[serde(default)]
    pub formatters: std::collections::BTreeMap<String, Vec<String>>,
    /// 单次格式化墙钟预算（秒；超时 kill 子进程，静默放行）。默认 3——冷环境
    /// （CI / 慢盘 / rustup shim 链冷启）可能超 3s，可调大。0 视作缺省。
    #[serde(default = "default_format_timeout_secs")]
    pub timeout_secs: u64,
}

/// 格式化超时缺省秒数（单一真相源：serde 缺省 / Default / 0 防呆共用）。
pub fn default_format_timeout_secs() -> u64 {
    3
}

impl Default for FormatOnSaveConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            formatters: std::collections::BTreeMap::new(),
            timeout_secs: default_format_timeout_secs(),
        }
    }
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
    /// Full-review M4: context-snapshot message role. "user" (default)
    /// places the merged snapshot as a user message; some
    /// strict chat templates reject adjacent user/user pairs — set
    /// "system" to restore the pre-I2 single system-role injection.
    #[serde(default = "default_snapshot_role")]
    pub snapshot_role: String,
    #[serde(default = "default_queue_size")]
    pub queue_size: i64,
    #[serde(default)]
    pub max_continuation_permits: i64,
    /// D-4（2026-09-23 多会话并行清账）：单 loop 并发 turn 上限（安全阀）。
    /// 泵统一 gate+spawn 后跨会话天然并发，此值防止极端场景无界并发。
    /// 0 = 不设限。默认 8（每个 AgentLoop——主 loop / 项目 loop 各自独立
    /// 计数；同会话仍串行，不受此值影响）。
    #[serde(default = "default_max_concurrent_turns")]
    pub max_concurrent_turns: i64,
    /// U4: tool-result spill retention (days). Spill files under
    /// `<home>/logs/spill/` older than this are deleted (startup scan +
    /// daily midnight task in agent_factory). 0 = never clean up. Default 7.
    #[serde(default = "default_spill_retention_days")]
    pub spill_retention_days: i64,
    /// C4: edit→diagnostics feedback loop switch (see [`DiagnosticsLoopConfig`]).
    #[serde(default)]
    pub diagnostics_loop: DiagnosticsLoopConfig,
    /// A6: format-on-save after write/edit (see [`FormatOnSaveConfig`]).
    #[serde(default)]
    pub format_on_save: FormatOnSaveConfig,
    /// 429 重试环（2026-09-17 BUG 文档裁决④）：上游限流（HTTP 429）重试次数，
    /// 默认 10。间隔走 10 档阶梯（5s→…→210s），上游带 Retry-After 时取
    /// max(上游要求, 阶梯值)。0 = 关闭重试（一次失败即终局，旧行为）。
    #[serde(default = "default_rate_limit_retries")]
    pub rate_limit_retries: i64,
    /// P5（能力扩展 WS2 compaction）：自动压缩的逐字尾巴 token 预算
    /// （默认 20000，对齐业界通行 keepRecentTokens）。压缩边界从「保留近
    /// K_TARGET=6 条」改为「保留近 N token」（MODEL-FACING 投影估算），
    /// 预算边界再经 tool_safe_boundary 回退，保证永不切在 tool_call/result
    /// 对中间。**0 = 回退旧按条数行为**。loop 侧 fresh-read（每轮现读
    /// config.json），运行中改键下一轮生效。
    #[serde(default = "default_compact_keep_recent_tokens")]
    pub compact_keep_recent_tokens: i64,
    /// P8（2026-10-10 集群无人值守硬化 A2）：会话 token 预算——单会话累计
    /// 用量（input+cache_read+output，与 aggregate_session_usage 账本同
    /// 口径）上限。**0 = 关（默认）**，注意与 `max_tool_iterations` 的
    /// 0=不限惯例相反。消费点 = loop 侧轮顶检（硬停 ≥100% / 软提示 ≥75%）、
    /// cluster_agent 重派入口拒接、board_review 转人工；读法统一
    /// `agents_defaults_u64("session_token_budget", 0)` fresh-read
    /// （config.json 单一真相源，运行中改键下一轮生效）。typed 字段的
    /// 意义 = 防 typed save_config round-trip 静默抹键（vision 键
    /// 2026-09-03 同类回归；AgentDefaults 无 flatten extra）。
    #[serde(default)]
    pub session_token_budget: u64,
    /// 输出撞顶翻倍阶梯的封顶值（§6.2 无人值守硬化；默认 262144 = 256K）。
    /// LLM 响应 `finish_reason=length` 且正文为空（思考烧穿形态）时翻倍重发
    ///（`max(当前×2, 32768)` 起步地板），直到有正文或触及本封顶。触及封顶
    /// 仍空 = 按现行失败收口（文案注明已试到封顶）。只改当次请求，不回写
    /// config（会话级 escalation，防单次事故永久抬高全局基线）。
    #[serde(default = "default_max_output_ceiling")]
    pub max_output_ceiling: u64,
}

/// 翻倍阶梯封顶缺省（serde 缺省 / Default 单一真相源）。
pub fn default_max_output_ceiling() -> u64 {
    262144
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
            max_concurrent_turns: default_max_concurrent_turns(),
            spill_retention_days: default_spill_retention_days(),
            diagnostics_loop: DiagnosticsLoopConfig::default(),
            format_on_save: FormatOnSaveConfig::default(),
            rate_limit_retries: default_rate_limit_retries(),
            compact_keep_recent_tokens: default_compact_keep_recent_tokens(),
            session_token_budget: 0,
            max_output_ceiling: default_max_output_ceiling(),
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
    // Wave 3（WS12 gateway 集成面）：存量 feature-gated 通道补缺 + 五新通道。
    // 字段对照 nemesis-channels 侧各通道 Config（真相源在各通道 mod 顶部），
    // gateway web_init 逐字段转换到 ChannelInitConfig。
    #[serde(default)]
    pub matrix: MatrixConfig,
    #[serde(default)]
    pub irc: IrcConfig,
    #[serde(default)]
    pub signal: SignalConfig,
    #[serde(default)]
    pub mastodon: MastodonConfig,
    #[serde(default)]
    pub bluesky: BlueskyConfig,
    #[serde(default)]
    pub wecom: WeComConfig,
    #[serde(default)]
    pub mattermost: MattermostConfig,
    #[serde(default)]
    pub nostr: NostrConfig,
    #[serde(default)]
    pub mqtt: MqttConfig,
    #[serde(default)]
    pub wechat: WeChatConfig,
}

// Channel configuration structs follow below.

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WhatsAppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub bridge_url: String,
    /// 桥 API key（出站鉴权凭据；WS12 补齐——通道侧 `api_key: Option<String>`
    /// 此前无 config 出口，装配点空串 → None）。支持 vault/env/yaml 引用。
    #[serde(default)]
    pub api_key: String,
    /// 出站代理 URL（桥可部署在远端，与 telegram 同构；空 = 直连）。
    /// BUG-20261024-12：模板早已声明 `channels.whatsapp.proxy`，typed 字段
    /// 缺失 → 读取端静默忽略、typed save 静默删除。
    #[serde(default)]
    pub proxy: String,
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

#[derive(Clone, Serialize, Deserialize)]
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

// SEC-002（2026-09-22 审查）：手写 Debug 替代 derive——auth_token 一律脱敏
// （Debug/panic/错误上下文会把整个 config 打进日志）。字段名保留，值只输出
// 形态；其余字段与 derive 输出一致。
impl std::fmt::Debug for WebChannelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("WebChannelConfig")
            .field("enabled", &self.enabled)
            .field("host", &self.host)
            .field("port", &self.port)
            .field("path", &self.path)
            .field(
                "auth_token",
                &if self.auth_token.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("allow_from", &self.allow_from)
            .field("heartbeat_interval", &self.heartbeat_interval)
            .field("session_timeout", &self.session_timeout)
            .field("sync_to", &self.sync_to)
            .finish()
    }
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

// ---------------------------------------------------------------------------
// feature-gated 通道 config（Wave 3 WS12 集成面补齐）
//
// 与 nemesis-channels 侧各通道 Config 字段一一对应（真相源在各通道 mod 顶部
// 的 `pub struct XConfig`）；`enabled` / `sync_to` 是宿主侧路由字段（通道侧
// 无此二字段——enabled 由 gateway init 臂消费，sync_to 由 setup_sync_targets
// 消费）。秘密字段在 config.json 里只存 `vault:` / `env:` / `yaml:` 引用，
// gateway 装配点统一经 resolve_secret_or_empty / resolve_auth_token_or_random
// 解析（见 web_init.rs 各转换臂）。
// ---------------------------------------------------------------------------

/// Matrix 通道配置（对应 `nemesis_channels::matrix::MatrixConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MatrixConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Homeserver URL（如 `https://matrix.org`）。
    #[serde(default)]
    pub homeserver: String,
    /// Bot 用户 ID（如 `@bot:matrix.org`）。
    #[serde(default)]
    pub user_id: String,
    /// Access token（出站 API 凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub access_token: String,
    /// 默认房间 ID；空 = 未设（通道侧 `room_id: Option<String>`，装配点
    /// 空串 → None）。
    #[serde(default)]
    pub room_id: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// IRC 通道配置（对应 `nemesis_channels::irc::IRCConfig`）。
/// Default 对齐通道侧（use_tls=true、退避 5/300）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IrcConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器地址（如 `irc.libera.chat:6697`）。
    #[serde(default)]
    pub server: String,
    /// 是否走 TLS（通道侧默认 true）。
    #[serde(default)]
    pub use_tls: bool,
    /// 昵称。
    #[serde(default)]
    pub nick: String,
    /// 服务器密码（出站鉴权凭据；空 = 无，通道侧 `password: Option<String>`）。
    #[serde(default)]
    pub password: String,
    /// 加入的频道（如 `#nemesisbot`）。
    #[serde(default)]
    pub channel: String,
    /// 重连退避基数秒（通道侧默认 5）。
    #[serde(default)]
    pub reconnect_backoff_secs: i64,
    /// 重连退避上限秒（通道侧默认 300）。
    #[serde(default)]
    pub max_reconnect_backoff_secs: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for IrcConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: String::new(),
            use_tls: true,
            nick: String::new(),
            password: String::new(),
            channel: String::new(),
            reconnect_backoff_secs: 5,
            max_reconnect_backoff_secs: 300,
            allow_from: vec![],
            sync_to: vec![],
        }
    }
}

/// Signal 通道配置（对应 `nemesis_channels::signal::SignalConfig`，
/// signal-cli-rest-api 桥接形态）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SignalConfig {
    #[serde(default)]
    pub enabled: bool,
    /// signal-cli-rest-api 基地址。
    #[serde(default)]
    pub api_url: String,
    /// 本机电话号码（带国家码）。
    #[serde(default)]
    pub phone_number: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Mastodon 通道配置（对应 `nemesis_channels::mastodon::MastodonConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MastodonConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 实例地址（如 `https://mastodon.social`）。
    #[serde(default)]
    pub server: String,
    /// OAuth access token（出站 API 凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub access_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Bluesky 通道配置（对应 `nemesis_channels::bluesky::BlueskyConfig`）。
/// Default 对齐通道侧（poll_interval=10）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BlueskyConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器地址（如 `https://bsky.social`）。
    #[serde(default)]
    pub server: String,
    /// 账号 handle（如 `nemesisbot.bsky.social`）。
    #[serde(default)]
    pub handle: String,
    /// App password（出站登录凭据；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub password: String,
    /// DID；空 = 未设（通道侧自动解析，装配点空串 → None）。
    #[serde(default)]
    pub did: String,
    /// 通知轮询间隔秒（通道侧默认 10）。
    #[serde(default)]
    pub poll_interval: i64,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for BlueskyConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            server: String::new(),
            handle: String::new(),
            password: String::new(),
            did: String::new(),
            poll_interval: 10,
            allow_from: vec![],
            sync_to: vec![],
        }
    }
}

/// WeCom（企业微信）通道配置（P25，对应 `nemesis_channels::wecom::WeComConfig`）。
/// 字段默认全空——通道侧 new() 对 listen_addr/callback_path 有空值归一。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct WeComConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 群机器人 webhook 地址（出站默认目标；支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub webhook_url: String,
    /// chat_id → 群机器人 webhook 路由表（多群出站；chat_id 命中优先于默认）。
    #[serde(default)]
    pub webhooks: std::collections::HashMap<String, String>,
    /// 回调验签 token（企业微信后台配置；鉴权验证类，支持引用）。
    #[serde(default)]
    pub token: String,
    /// 回调消息加密密钥 EncodingAESKey（43 字符；鉴权验证类，支持引用）。
    #[serde(default)]
    pub encoding_aes_key: String,
    /// 企业微信 corp_id（解密 receiveid 强校验用；空 = 不校验）。
    #[serde(default)]
    pub corp_id: String,
    /// 回调 HTTP 监听地址（空 = 通道侧默认 0.0.0.0:9898）。
    #[serde(default)]
    pub listen_addr: String,
    /// 回调路径（空 = 通道侧默认 /wecom/callback）。
    #[serde(default)]
    pub callback_path: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// Mattermost 通道配置（P26，对应 `nemesis_channels::mattermost::MattermostConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MattermostConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 服务器基地址（如 `https://mattermost.example.com`，http/https）。
    #[serde(default)]
    pub base_url: String,
    /// Bot token（WS 网关鉴权 + REST 出站鉴权共用；vault/env/yaml 解析走
    /// 出站凭据语义 `resolve_secret_or_empty`——缺失不阻断启动，鉴权失败在
    /// users/me / WS challenge 处暴露）。
    #[serde(default)]
    pub bot_token: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 监听的频道 ID / 名称列表（空 = bot 所在全部频道）。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub channels: Vec<String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// nostr 通道配置（P27，对应 `nemesis_channels::nostr::NostrConfig`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct NostrConfig {
    #[serde(default)]
    pub enabled: bool,
    /// relay WebSocket 地址列表（`wss://` / `ws://`），至少一条。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub relays: Vec<String>,
    /// 本机私钥（64-hex）。config.json 只存 `vault:`/`env:`/`yaml:` 引用，
    /// 装配点解析后传明文（nostr.rs 模块头"密钥管理"约定；
    /// 通道侧对未解析引用前缀响亮拒绝）。
    #[serde(default)]
    pub private_key: String,
    /// 发件人 x-only 公钥白名单（64-hex；空 = 放行所有）。
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 断线重连间隔秒（通道侧"0 视作缺省"= 默认 5）。
    #[serde(default)]
    pub reconnect_secs: i64,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

/// MQTT 订阅 topic filter → 回包 topic 映射条目（对应
/// `nemesis_channels::mqtt::MqttTopicMapping`）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MqttTopicMapping {
    /// 订阅的 topic filter（支持 `+`/`#` 通配）。
    #[serde(default)]
    pub topic: String,
    /// 出站回包 topic；空 = 此映射不承接回包。
    #[serde(default)]
    pub reply_topic: String,
}

/// MQTT 通道配置（P28，对应 `nemesis_channels::mqtt::MqttChannelConfig`）。
/// Default 对齐通道侧文档默认值（broker 127.0.0.1:1883、keep-alive 30、
/// clean_session=true、退避 5、QoS 1）——通道侧对 broker_host 空值响亮报错、
/// 端口/keep-alive 原样透传 rumqttc，缺省值必须在 config 层给足。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct MqttConfig {
    #[serde(default)]
    pub enabled: bool,
    /// broker 地址（默认 127.0.0.1）。
    #[serde(default)]
    pub broker_host: String,
    /// broker 端口（默认 1883）。
    #[serde(default)]
    pub broker_port: i64,
    /// MQTT client id；空 = 通道侧自动生成 `nemesisbot-<pid>`。
    #[serde(default)]
    pub client_id: String,
    /// 用户名；空 = 匿名连接（支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub username: String,
    /// 密码；空 = 无（支持 vault/env/yaml 引用）。
    #[serde(default)]
    pub password: String,
    /// keep-alive 秒数（默认 30；0 = 关闭保活）。
    #[serde(default)]
    pub keep_alive_secs: i64,
    /// clean session（默认 true）。
    #[serde(default)]
    pub clean_session: bool,
    /// 断线重连退避秒数（默认 5，通道侧下限 1）。
    #[serde(default)]
    pub reconnect_delay_secs: i64,
    /// 订阅与出站统一 QoS（默认 1；0/1/2，>2 通道侧构造期拒绝）。
    #[serde(default)]
    pub qos: i64,
    /// 订阅 + 回包映射表。
    #[serde(default)]
    pub topics: Vec<MqttTopicMapping>,
    /// 全局默认回包 topic；空 = 无默认（映射/学习都缺位时出站报错）。
    #[serde(default)]
    pub default_reply_topic: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 启用 TLS（默认 false = 纯 TCP）。true 时通道侧走 rustls（ring provider）。
    #[serde(default)]
    pub use_tls: bool,
    /// CA 证书 PEM 路径（use_tls=true 必填；自签/私有 PKI 场景）。
    #[serde(default)]
    pub ca_cert_path: String,
    /// 客户端证书 PEM 路径（mTLS；与 client_key_path 成对出现）。
    #[serde(default)]
    pub client_cert_path: String,
    /// 客户端私钥 PEM 路径（mTLS；与 client_cert_path 成对出现）。
    #[serde(default)]
    pub client_key_path: String,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for MqttConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            broker_host: "127.0.0.1".to_string(),
            broker_port: 1883,
            client_id: String::new(),
            username: String::new(),
            password: String::new(),
            keep_alive_secs: 30,
            clean_session: true,
            reconnect_delay_secs: 5,
            qos: 1,
            topics: vec![],
            default_reply_topic: String::new(),
            allow_from: vec![],
            use_tls: false,
            ca_cert_path: String::new(),
            client_cert_path: String::new(),
            client_key_path: String::new(),
            sync_to: vec![],
        }
    }
}

/// 微信个人微信通道配置（P29，iLink Bot API，对应
/// `nemesis_channels::wechat::WeChatConfig`）。Default 对齐通道侧
/// （base_url/send_path/回调监听/签名方案均为通道侧常量——协议形态为
/// 实现假设，真机校准只需要改 config）。
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WeChatConfig {
    #[serde(default)]
    pub enabled: bool,
    /// 出站 REST 基址（通道侧默认 `https://ilink.bot.weixin.qq.com`，假设值）。
    #[serde(default)]
    pub base_url: String,
    /// 回调鉴权 token（鉴权验证类；必填，通道侧空值拒绝启动；支持引用）。
    #[serde(default)]
    pub token: String,
    /// 出站消息接口路径（通道侧默认 /v1/message/send，假设值）。
    #[serde(default)]
    pub send_path: String,
    /// 回调监听地址（通道侧默认 0.0.0.0:9541）。
    #[serde(default)]
    pub callback_listen_addr: String,
    /// 回调路径（通道侧默认 /wechat/callback）。
    #[serde(default)]
    pub callback_path: String,
    /// 签名方案：`hmac-sha256`（默认）/ `sha1` / `none`（未知值通道侧 loud 拒绝）。
    #[serde(default)]
    pub signature_scheme: String,
    #[serde(default, deserialize_with = "deserialize_flexible_string_vec")]
    pub allow_from: Vec<String>,
    /// 联系人映射：user_id → 备注别名（命中时写入 metadata `contact_alias`）。
    #[serde(default)]
    pub contacts: std::collections::HashMap<String, String>,
    #[serde(default)]
    pub sync_to: Vec<String>,
}

impl Default for WeChatConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            base_url: "https://ilink.bot.weixin.qq.com".to_string(),
            token: String::new(),
            send_path: "/v1/message/send".to_string(),
            callback_listen_addr: "0.0.0.0:9541".to_string(),
            callback_path: "/wechat/callback".to_string(),
            signature_scheme: "hmac-sha256".to_string(),
            allow_from: vec![],
            contacts: std::collections::HashMap::new(),
            sync_to: vec![],
        }
    }
}

// ============================================================================
// Model Config
// ============================================================================

#[derive(Clone, Default, Serialize, Deserialize)]
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
    /// 显式协议类型（LLM 协议选择器，2026-09-11）：`anthropic`（别名
    /// `claude`，/v1/messages + x-api-key）/ `openai`（chat/completions +
    /// Bearer）/ `responses`（OpenAI Responses API）。空串 = 不指定，按
    /// provider/ 前缀与模型名关键词自动推断（现状行为，老条目零迁移）。
    /// 显式值优先于推断（显式 > 推断）；未知值在 factory 解析时 loud 拒绝。
    /// 只决定 wire 格式与 auth header——api_base 默认推断、价目表归属、
    /// 显示名仍由 provider 前缀管辖。
    #[serde(default)]
    pub protocol: String,
    /// 声明式模型角色（2026-10-13 N6）：`conversation`（缺省/空，agent loop
    /// 主通道合法形态）/ `utility`（杂务通道，agents.small_model 消费：/compact
    /// 摘要 + 会话标题生成）/ `classifier`（分类/路由/打标）。后两者是对话
    /// 形态条目（probe/messages 可收发），但**不该坐默认对话模型槽**——三个
    /// default 槽写入点（CLI `--default`、唯一模型自动坐槽、WSAPI
    /// `models.set_default`）与 factory 装配点按 `is_conversation_role` 拒绝/
    /// 降级。值集单一真相源 `nemesis_types::capability::normalize_model_role`。
    /// 空串不序列化（`skip_serializing_if`）——老条目 typed 保存字节零变化
    /// （零迁移完整语义；protocol 先例是无条件序列化，role 以空为常态故收紧）。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub role: String,
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

// SEC-002（2026-09-22 审查）：手写 Debug 替代 derive——api_key 一律脱敏。
// 模型列表常被 Debug/日志整段打印（agent_factory、dashboard handler 等），
// derive 会把 key 带进日志与故障报告。extra flatten map 不含结构化 secret
// 字段（per-model 键为 tier/vision/timeout 等开关），照常输出。
impl std::fmt::Debug for ModelConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ModelConfig")
            .field("model_name", &self.model_name)
            .field("model", &self.model)
            .field("api_base", &self.api_base)
            .field(
                "api_key",
                &if self.api_key.is_empty() {
                    "<empty>"
                } else {
                    "<redacted>"
                },
            )
            .field("proxy", &self.proxy)
            .field("auth_method", &self.auth_method)
            .field("connect_mode", &self.connect_mode)
            .field("protocol", &self.protocol)
            .field("workspace", &self.workspace)
            .field("reasoning_effort", &self.reasoning_effort)
            .field("extra", &self.extra)
            .finish()
    }
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
    #[serde(default)]
    pub hardware: HardwareToolsConfig,
    /// 集群专业职能框架（M4）：图像生成（`generate_image` 工具）。
    #[serde(default)]
    pub image_gen: ImageGenConfig,
}

/// 图像生成配置（`tools.image_gen` 段，M4）。模型解析：`model` 显式别名
/// 优先；缺省取 model_list 中**唯一** protocol=images-openai 条目（缺失/
/// 歧义在注册期诚实报错，工具不注册）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct ImageGenConfig {
    /// 图像模型别名（model_list 条目的 model_name）。None = 唯一 images-openai
    /// 条目自动解析。
    #[serde(default)]
    pub model: Option<String>,
    /// 单次图像生成墙钟超时（秒）。0/缺省 = 120。
    #[serde(default)]
    pub timeout_secs: u64,
}

impl ImageGenConfig {
    /// 生效超时（0/缺省 → 120s）。
    pub fn effective_timeout_secs(&self) -> u64 {
        if self.timeout_secs == 0 {
            120
        } else {
            self.timeout_secs
        }
    }
}

/// P8 GPIO 白名单：I2C 地址访问策略（`tools.hardware` 段）。
/// 空段列表 = 内置默认（可自由寻址空间 0x08-0x77）；deny 优先于 allow。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct HardwareToolsConfig {
    /// 允许的 7 位地址段 `[[lo, hi], ...]`（闭区间）；空 = 内置默认 0x08-0x77。
    #[serde(default)]
    pub i2c_allow_ranges: Vec<(u8, u8)>,
    /// 拒绝的地址段（在 allow 之上再排除，用于屏蔽已知有风险的设备地址）。
    #[serde(default)]
    pub i2c_deny_ranges: Vec<(u8, u8)>,
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
/// 用户习惯画像配置（B 线）。
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct UserProfileConfig {
    /// 默认关：开启后画像自动学习并注入（仅向前，不回溯）。
    #[serde(default)]
    pub enabled: bool,
    /// 置信度阈值（缺省 0.7）。
    #[serde(default)]
    pub confidence_threshold: Option<f64>,
    /// 注入条目上限（缺省 10）。
    #[serde(default)]
    pub max_entries: Option<usize>,
    /// 衰减天数（缺省 30：active→decaying；再 30 天→expired，两段同值）。
    #[serde(default)]
    pub decay_days: Option<i64>,
    /// 统计扫描间隔秒数（缺省 3600 = 每小时）。
    #[serde(default)]
    pub stats_scan_interval_secs: Option<u64>,
    /// 反思通道触发间隔（每 N 次扫描一次，缺省 20）。
    #[serde(default)]
    pub reflect_every_sweeps: Option<u32>,
}

impl UserProfileConfig {
    /// 置信度阈值（缺省回落 user_profile 模块常量）。
    pub fn confidence_threshold_or(&self, default: f64) -> f64 {
        self.confidence_threshold.unwrap_or(default)
    }
    /// 注入条目上限（缺省 10；0 = 回落缺省，上限不允许关闭）。
    pub fn max_entries_or(&self, default: usize) -> usize {
        self.max_entries.filter(|m| *m > 0).unwrap_or(default)
    }
    /// 衰减天数（缺省 30）。
    pub fn decay_days_or(&self, default: i64) -> i64 {
        self.decay_days.filter(|d| *d > 0).unwrap_or(default)
    }
    /// 扫描间隔秒数（缺省 3600；非法值 0/负 = 回落缺省）。
    pub fn stats_scan_interval_secs_or(&self, default: u64) -> u64 {
        self.stats_scan_interval_secs
            .filter(|s| *s > 0)
            .unwrap_or(default)
    }
    /// 反思间隔 sweep 数（缺省 20；0 = 回落缺省，不允许关掉反思闸）。
    pub fn reflect_every_sweeps_or(&self, default: u32) -> u32 {
        self.reflect_every_sweeps
            .filter(|n| *n > 0)
            .unwrap_or(default)
    }
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
    /// P31 记忆 dreaming（三段式记忆巩固；None = 全默认——enabled=false 关）。
    #[serde(default)]
    pub dreaming: Option<DreamingConfig>,
}

/// `memory.dreaming` 配置节（P31 记忆巩固）。
///
/// sweep 挂载于 gateway 启动同步（job 名 `memory-dreaming:sweep`）；LLM 决策
/// 走 `agents.small_model` 通道（未配则回落主模型）。默认全关——dreaming 是
/// 自主改写记忆的行为，opt-in。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DreamingConfig {
    /// 总开关，默认 false（opt-in）。
    #[serde(default)]
    pub enabled: bool,
    /// sweep cron 表达式（5/6 字段，本地时间），默认每日 03:30。
    #[serde(default = "default_dreaming_cron")]
    pub cron: String,
    /// 每次 sweep 送 LLM 决策的候选条目上限（按 6 信号加权分取 topK）。
    #[serde(default = "default_dreaming_top_k")]
    pub top_k: usize,
    /// 6 信号权重逐项覆盖（None = 内置默认；未给的项用默认值）。
    #[serde(default)]
    pub weights: Option<DreamingWeightsConfig>,
}

impl Default for DreamingConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            cron: default_dreaming_cron(),
            top_k: default_dreaming_top_k(),
            weights: None,
        }
    }
}

/// 6 信号权重覆盖节（P31）。全部 Option——只覆盖用户显式给出的项。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DreamingWeightsConfig {
    /// 召回次数权重。
    #[serde(default)]
    pub recall: Option<f64>,
    /// 时间衰减（久未召回）权重。
    #[serde(default)]
    pub decay: Option<f64>,
    /// 条目冲突权重。
    #[serde(default)]
    pub conflict: Option<f64>,
    /// 冗余度权重。
    #[serde(default)]
    pub redundancy: Option<f64>,
    /// 年龄权重。
    #[serde(default)]
    pub age: Option<f64>,
    /// 来源可靠性（不可靠→更需审查）权重。
    #[serde(default)]
    pub source: Option<f64>,
}

fn default_dreaming_cron() -> String {
    "30 3 * * *".to_string()
}

fn default_dreaming_top_k() -> usize {
    8
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillsConfig {
    #[serde(default)]
    pub enabled: bool,
    /// Whether `skill_manage` writes require interactive approval (default false).
    #[serde(default)]
    pub manage_approval: bool,
    /// P11 验签：无签名技能是否放行（默认 true 兼容存量；false = strict，拒绝无签名）。
    #[serde(default = "default_skills_allow_unsigned")]
    pub allow_unsigned: bool,
    /// P13 装前审批：安装走审批卡（默认 true；CLI 可 --yes 跳过，WSAPI 走审批基建）。
    #[serde(default = "default_skills_install_approval")]
    pub install_approval: bool,
    /// P16 版本龄下限（天；默认 0 = 关）。
    #[serde(default)]
    pub min_age_days: i64,
    /// P16 版本龄策略："warn"（默认）| "block"。
    #[serde(default = "default_skills_min_age_policy")]
    pub min_age_policy: String,
}

// Default 手工实现与 serde 缺省函数同源（derive 会把 allow_unsigned 落成
// false，与「默认 true 兼容存量」语义相反）。
impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            manage_approval: false,
            allow_unsigned: default_skills_allow_unsigned(),
            install_approval: default_skills_install_approval(),
            min_age_days: 0,
            min_age_policy: default_skills_min_age_policy(),
        }
    }
}

fn default_skills_allow_unsigned() -> bool {
    true
}

fn default_skills_install_approval() -> bool {
    true
}

fn default_skills_min_age_policy() -> String {
    "warn".to_string()
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
    /// G2（2026-10-18 三方复评整改批四）：浏览器 profile 注入 opt-in 闸。
    /// true = 本 server 允许宿主按上下文注入 per-agent Chromium
    /// `--user-data-dir`（gateway agent loop 路径 = agent home
    /// browser-profile/；执行体子进程路径 = `NEMESISBOT_BROWSER_PROFILE`
    /// env 回落）。默认 false = 普通 MCP 零变化（共享 server 不被塞身份）；
    /// server args 已显式带 `--user-data-dir`（空格或 `=` 形态）= 用户配置
    /// 优先，不重复注入。false 序列化跳过（存量 config.mcp.json 字节稳定）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub browser_profile: bool,
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
            browser_profile: false,
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

    /// Builder: 打开浏览器 profile 注入 opt-in（G2）。
    pub fn browser_profile(mut self, yes: bool) -> Self {
        self.browser_profile = yes;
        self
    }
}

/// MCP HTTP 头行语法校验（单一真相源，2026-10-15 第十二轮复查）：CLI
/// `mcp add`/`mcp discover`、Dashboard `server.add`/`server.update`、运行期
/// `resolved_header_pairs` 三方同源。只验语法，绝不解引用、绝不回显值段
/// （值段可能夹带真凭据；错误文案只接头名/引用名——两者非秘密）。
///
/// 拒绝集：
/// - 缺冒号（应为 `"KEY: VALUE"`）；
/// - 头名为空 / 含空白或控制字符；
/// - `Mcp-Session-Id` 协议保留头（server 指派、transport 自管，配置行
///   只有伪造会话身份一种效果）；
/// - 整值 `env:`/`yaml:`/`vault:` 引用名为空或带首尾空格（运行期按原样
///   解析不 trim，必失败 → WARN 跳过头 → 401——写入面挡住省一轮学费；
///   语义与 [`crate::resolve_secret_field`] 的整值前缀识别一致，内嵌引用
///   如 `Bearer vault:x` 按字面量发送、放行）。
pub fn validate_mcp_header_line(line: &str) -> std::result::Result<(), String> {
    let Some((raw_name, raw_value)) = line.split_once(':') else {
        return Err("header 行缺冒号（应为 \"KEY: VALUE\"）".to_string());
    };
    let name = raw_name.trim();
    if name.is_empty() || name.chars().any(|c| c.is_whitespace() || c.is_control()) {
        return Err(format!("header 名非法（空 / 含空白或控制字符）：{name}"));
    }
    if name.eq_ignore_ascii_case("mcp-session-id") {
        return Err("Mcp-Session-Id 是协议保留头（server 指派），不能配置".to_string());
    }
    let value = raw_value.trim();
    for (prefix, what) in [
        ("env:", "环境变量名"),
        ("yaml:", "凭据别名"),
        ("vault:", "vault 别名"),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() {
                return Err(format!(
                    "header 值 {what} 引用名为空——写作 {prefix}<name>（如 vault:pet-token）"
                ));
            }
            if rest.trim() != rest {
                return Err(format!(
                    "header 值 {what} 引用名带首尾空格（运行期按原样解析，必失败）——\
                     写作 {prefix}{rest}（冒号后不带空格）"
                ));
            }
        }
    }
    Ok(())
}

/// MCP stdio env 行语法校验（"KEY=VALUE"；2026-10-15 第十三轮单一真相源——
/// CLI `mcp add` 写入面 / Dashboard server.add·update / 运行期
/// `resolved_env` WARN 跳过同源）：缺 `=` / 键名空 / 键名含空白 / 整值
/// `vault:`/`env:`/`yaml:` 引用名为空或带首尾空格。与
/// [`validate_mcp_header_line`] 同款 trim-后检查（值部首尾空格在校验与
/// 运行期解析两侧都剥掉——字面量值带首尾空格的场景让位于两侧一致性）。
/// 只验语法，值原样落盘，绝不解引用。
pub fn validate_mcp_env_line(line: &str) -> std::result::Result<(), String> {
    let Some((raw_name, raw_value)) = line.split_once('=') else {
        return Err("env 行缺 '='（应为 \"KEY=VALUE\"）".to_string());
    };
    let name = raw_name.trim();
    if name.is_empty() {
        return Err("env 键名为空".to_string());
    }
    if name.chars().any(|c| c.is_whitespace()) {
        return Err(format!("env 键名含空白：{name}"));
    }
    let value = raw_value.trim();
    for (prefix, what) in [
        ("env:", "环境变量名"),
        ("yaml:", "凭据别名"),
        ("vault:", "vault 别名"),
    ] {
        if let Some(rest) = value.strip_prefix(prefix) {
            if rest.is_empty() {
                return Err(format!(
                    "env 值 {what} 引用名为空——写作 {prefix}<name>（如 vault:pet-token）"
                ));
            }
            if rest.trim() != rest {
                return Err(format!(
                    "env 值 {what} 引用名带首尾空格（运行期按原样解析，必失败）——\
                     写作 {prefix}{rest}（'=' 后不带空格）"
                ));
            }
        }
    }
    Ok(())
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
    /// Passthrough for untyped user-defined keys（CFG-08：没有这个 flatten
    /// 时，任何 typed save 都会静默丢掉用户手加的自定义键——与 ModelConfig
    /// `extra` 同款先例）。BTreeMap 保 round-trip 字节稳定；空表序列化时
    /// 跳过（输出与既有形态字节一致）。
    #[serde(
        flatten,
        default,
        skip_serializing_if = "std::collections::BTreeMap::is_empty"
    )]
    pub extra: std::collections::BTreeMap<String, serde_json::Value>,
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
            extra: std::collections::BTreeMap::new(),
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
    /// CFG-05（2026-09-16 死键接线）：审计 JSONL 是否记录放行事件
    /// （false = 只记拦截/告警）。runtime 侧 security_setup.rs 构造期读取。
    #[serde(default = "default_true")]
    pub log_all_operations: bool,
    // ── 死键恢复区（2026-09-16 用户指令：代码不得随便删，先恢复保留待
    // 讨论）────────────────────────────────────────────────────
    // 以下 5 键曾在 CFG-05 被裁「确认无用直接删」（D3），现按用户指令
    // 恢复原字段原属性。**未接线**（零 runtime 消费方，runtime 走裸 JSON
    // 或 NemesisPath 硬拼）：log_denials_only（与 log_all_operations=false
    // 同义）、max_pending_requests（无满额语义实现）、audit_log_retention_days
    // （无保留清扫器，盲清扫会误伤审计链文件）、audit_log_path（gateway 经
    // nemesis-path 硬拼安全日志目录）、synchronous_mode（无语义）。
    // 保留原因：字段必须与出厂模板键共存（typed round-trip 不丢键），
    // 删除与否待用户后续裁决。
    #[serde(default)]
    pub log_denials_only: bool,
    /// CFG-05：审批卡超时秒数。runtime 侧 security_setup.rs 构造期读取，
    /// 接入 AuditorConfig.approval_timeout_secs。
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
    // ── 死键恢复区结束 ────────────────────────────────────────────
    /// 审计链（Merkle append-only，第 8 层）开关。runtime 侧
    /// security_setup.rs 按裸 JSON 顶层键读取；此前 typed 无此字段，
    /// Dashboard 安全设置页保存一次即把用户手加的该键整行抹掉
    /// （与 CFG-01 directory_rules 同族 round-trip 删键 bug）。
    #[serde(default)]
    pub audit_chain_enabled: bool,
    // T4（扩展计划 D2a，2026-09-24）：子进程环境凭据清洗开关（默认开）。
    // runtime 侧 security_setup.rs 构造期同步到
    // nemesis_utils::env_sanitize 进程级静态；三处 spawn 点
    // （agent loop exec/run_checks 闸 + nemesis-tools shell/async_shell）
    // 统一查询。
    #[serde(default = "default_true")]
    pub sanitize_child_env: bool,
    // L6（2026-10-12 T1）：pre-exec 脚本内容扫描开关（默认开）。runtime 侧
    // security_setup.rs 构造期按裸 JSON 键读取注入 SecurityPluginConfig；
    // typed 字段的存在只为 Dashboard typed round-trip 不丢键（CFG-01 同族
    // 删键 bug 防线）。
    #[serde(default = "default_true")]
    pub preexec_scan_enabled: bool,
    #[serde(default)]
    pub file_rules: Option<FileSecurityRules>,
    // CFG-01（2026-09-16 横扫存量加固）：磁盘/出厂模板真相源键名是
    // `dir_rules`（runtime 侧 security_setup.rs 按裸 JSON 键 `dir_rules`
    // 读取）。此前 typed 字段无 rename → 反序列化拿不到、保存时以
    // `directory_rules` 键（或 None）整文件覆盖，磁盘 `dir_rules` 被整段
    // 抹掉（Dashboard 安全设置页保存一次即触发）。rename 对齐后 round-trip
    // 字节稳定；历史键名 `directory_rules` 的别名读取仍在 runtime 侧保留
    // （security_setup.rs，WARN 提示改名）。
    // alias（复核 2026-09-16）：存量部署文件里写的是历史键名
    // `directory_rules`——仅 rename 时该文件经 typed 读-改-写会把规则段
    // 变成 `"dir_rules": null`（内容丢失），且 runtime 读 `dir_rules` 得
    // Some(Null) 连别名分支也不再触发。alias 让 legacy 键内容被读进
    // typed、保存统一迁到 `dir_rules`，规则内容零丢失。
    #[serde(default, rename = "dir_rules", alias = "directory_rules")]
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
    // D1（2026-09-16 用户裁决）：exec/spawn 未知命令（无规则命中）姿态。
    // allow（默认=旧行为）/ ask（送审批卡）/ deny（硬拦）。runtime 侧
    // security_setup.rs 按裸 JSON 键读取注入 auditor。
    // A-F4（复核 2026-09-16）：typed 缺省必须是**空串**（= runtime 缺键
    // 语义，落 auditor 旧行为），不得物化裁决值——Dashboard 保存走 typed
    // round-trip，若 serde 缺省给 "allow"/"ask"，用户从未显式配置的部署
    // 保存一次即被静默写入（缺键语义：D1 落 default_action、D2 旧
    // fail-open——被替换成裁决值 = 语义漂移）。前端已按「缺键显示裁决
    // 默认、保存不写键」实现（SecurityView POLICY_KEYS + spec 钉死），
    // typed 侧对齐；`skip_serializing_if` 让未配置（空串）不落盘。用户
    // 显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub exec_unknown_policy: String,
    // D2（2026-09-16 用户裁决）：Guardian LLM judge 失败（异常/不可用）
    // 时的姿态。ask（默认，送审批卡）/ allow（按策略放行+WARN）/ deny。
    // 缺省语义同上（A-F4）：空串 = runtime 缺键 = 旧行为，不物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guardian_failure_policy: String,
    // 无上下文 LLM 命令审计（2026-09-16 用户拍板默认 off）：guardian 覆盖
    // 面开关。off（默认）= 不装配 judge（零 LLM 成本）；critical = 旧
    // CRITICAL 全审；high = HIGH+CRITICAL 破坏形态预筛 + LLM 审。A-F4
    // 同款：typed 缺省空串（= runtime 缺键 = off），`skip_serializing_if`
    // 未配置不落盘，用户显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub guardian_mode: String,
    // ── P0 vault（D1，2026-09-22 计划 §4）：声明式量化风险限制 ──────
    // 类别 → 规则。**缺省空表 = 全关**（A-F4 同款缺省语义：空表不落盘，
    // 未配置的部署保存一次字节不变）。类别键由工具经
    // `Tool::limit_categories()` 声明（机制不认识业务名词，工具 declares，
    // 配置 binds）。窗口 = 滑动窗口（进程内存计数，重启清零——诚实边界，
    // 见计划 §4）。
    #[serde(default, skip_serializing_if = "std::collections::BTreeMap::is_empty")]
    pub limits: std::collections::BTreeMap<String, RateLimitRule>,
    // ── 签名验证三态开关（接入计划 §1，2026-09-23）──────────────────
    // ""（缺省）/ "off" / "warn" / "enforce"；未知值启动时 loud 拒绝。
    // **只读于启动，改动必须重启**（与 tier 等热载键刻意不同——避免
    // 「运行中验证策略被改」这个额外可篡改面）。A-F4 同款缺省语义：
    // 空串 = 用户从未配置 = 落默认 warn，`skip_serializing_if` 不落盘，
    // Dashboard 保存一次字节不变；用户显式选择后才物化。
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub signature_verify: String,
    // ── §2 免打扰（2026-10-11 无人值守计划 Part 2）──────────────────
    // 审批自动批准段。缺省关（弹卡照旧）；`auto_approve=true` 时
    // RequireApproval 一律自动放行（审计标 auto_by_config，安全 8 层其余
    // 各层照常全跑）。整段缺省不落盘（skip_serializing_if）：未配置的
    // 部署 typed round-trip 字节不变（A-F4 缺省语义同款）。
    #[serde(default, skip_serializing_if = "approvals_is_default")]
    pub approvals: ApprovalsConfig,
}

/// §2 免打扰：审批自动批准配置段（2026-10-11 无人值守计划 Part 2）。
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct ApprovalsConfig {
    /// 无人值守免打扰：RequireApproval 审批卡一律自动批准（pattern 记忆
    /// 优先；guardian 语义二审不是弹窗不纳入；技能安装闸按发起流程分叉
    /// ——见计划 §2.4）。默认关 = 现行为（弹窗）。
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub auto_approve: bool,
}

/// serde skip_serializing_if：整段处于缺省（关）时不落盘。
fn approvals_is_default(v: &ApprovalsConfig) -> bool {
    !v.auto_approve
}

/// 单类别的滑动窗口限制规则（P0 D1）。
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct RateLimitRule {
    /// 窗口内允许的最大次数。
    pub max: u32,
    /// 窗口长度（秒）。
    pub window_secs: u64,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            default_action: "deny".to_string(),
            log_all_operations: true,
            // 死键恢复区（2026-09-16 用户指令恢复保留，未接线，见字段区注释）
            log_denials_only: false,
            approval_timeout_seconds: 300,
            max_pending_requests: 100,
            audit_log_retention_days: 90,
            audit_log_path: String::new(),
            audit_log_file_enabled: true,
            synchronous_mode: false,
            audit_chain_enabled: false,
            sanitize_child_env: true,
            preexec_scan_enabled: true,
            file_rules: None,
            directory_rules: None,
            process_rules: None,
            network_rules: None,
            hardware_rules: None,
            registry_rules: None,
            layers: None,
            exec_unknown_policy: String::new(),
            guardian_failure_policy: String::new(),
            guardian_mode: String::new(),
            // P0 D1：缺省空表 = 限制全关。
            limits: std::collections::BTreeMap::new(),
            // 签名验证三态开关：空串 = 未配置 = 落默认 warn。
            signature_verify: String::new(),
            // §2 免打扰：缺省关。
            approvals: ApprovalsConfig::default(),
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
    // ── 死键恢复区（2026-09-16 用户指令恢复保留，未接线）──────
    // `signature`：签名验证层尚未接入管线（v4 verify 是独立子系统）；
    // `audit_chain`：runtime 读取的是顶层 `audit_chain_enabled` 而非本键，
    // 曾被定性误导性假开关。恢复保留待用户裁决。
    #[serde(default)]
    pub signature: Option<SignatureLayerConfig>,
    #[serde(default)]
    pub audit_chain: Option<SecurityLayerConfig>,
    // ── 死键恢复区结束 ────────────────────────────────────────
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SignatureLayerConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub strict: bool,
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

// CFG-05：`SignatureLayerConfig` 已随 `SecurityLayersConfig.signature` 死键
// 一并删除（签名验证层尚未接入安全管线；v4 verify 是独立子系统，未来接入
// 时再补 schema）。

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecurityRule {
    #[serde(default)]
    pub pattern: String,
    #[serde(default)]
    pub action: String,
    /// 规则注释（复核 2026-09-16）：CLI `security rules add` 会写入
    /// `comment` 字段、runtime `parse_rules` 读取展示；typed 缺此字段时
    /// Dashboard 保存一次即把手写 comment 抹掉（CFG-01 同族）。
    #[serde(default)]
    pub comment: String,
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
    // REL-002（2026-09-23）：统一原子写入（tmp+sync+rename，失败清理）。
    // 0600（unix）保护 API keys/tokens；Windows 权限为 no-op。
    nemesis_utils::write_file_atomic(&config_path.to_string_lossy(), content.as_bytes(), 0o600)
        .map_err(|e| {
            error!("[Config] Failed to write config file: {}", e);
            // helper 的 String 错误带步骤上下文（create temp/write/rename），
            // 包回 Io 变体保持错误分类语义。
            ConfigError::Io(std::io::Error::other(e))
        })?;
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
    if let Ok(v) = std::env::var("NEMESISBOT_AGENTS_DEFAULTS_MAX_CONCURRENT_TURNS")
        && let Ok(n) = v.parse()
    {
        config.agents.defaults.max_concurrent_turns = n;
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
            discipline: DisciplineConfig::default(),
            prompt_system: default_prompt_system(),
            cache_warmer: CacheWarmerConfig::default(),
            claude_code_tool: ClaudeCodeToolConfig::default(),
            codex_tool: CodexToolConfig::default(),
            lsp_tool: LspToolConfig::default(),
            tool_doc_folding: ToolDocFoldingConfig::default(),
            subagent: SubagentConfig::default(),
            fs_watcher: FsWatcherConfig::default(),
            hidden_tools: Vec::new(),
            roles: RolesConfig::default(),
            small_model: None,
            guardian_model: None,
            doom_loop_approval: false,
            image_downscale: true,
            lease_enabled: true,
            codemode: CodeModeConfig::default(),
            delegate: DelegateToolConfig::default(),
            turn_wal: true,
            provider_stream_events: true,
            executor_tool_timeout_secs: default_executor_tool_timeout_secs(),
            defaults: AgentDefaults {
                workspace: ws,
                restrict_to_workspace: true,
                llm: "zhipu/glm-4.7-flash".to_string(),
                max_tokens: nemesis_types::capability::DEFAULT_MAX_TOKENS,
                temperature: 0.7,
                max_tool_iterations: 100,
                // concurrent_request_mode intentionally omitted — filled by
                // `..Default::default()` → `default_concurrent_request_mode()`
                // (E1: single truth source; a stale "reject" literal lived here
                // until 2026-09-05 and silently outvoted the default fn).
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
            hardware: HardwareToolsConfig::default(),
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
            image_gen: ImageGenConfig::default(),
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
        user_profile: None,
        cluster: None,
        memory: None,
        skills: None,
        mcp: None,
        executor: None,
        debug: None,
        board: None,
        usage: None,
        terminal: None,
        projects: None,
        bridge: None,
        ui: None,
        connectors: None,
        ziwei: None,
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
    // REL-002：统一原子写入；0600（unix）——子系统配置同样可能含凭据。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write MCP config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
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
    // REL-002：统一原子写入；0600（unix）——含 limits 规则与策略。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write security config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
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
    // REL-002：统一原子写入。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write scanner config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
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
    // REL-002：统一原子写入。
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600).map_err(
        |e| {
            error!("[Config] Failed to write skills config: {}", e);
            ConfigError::Io(std::io::Error::other(e))
        },
    )?;
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
// 死键恢复区（2026-09-16 用户指令恢复保留，未接线，见 SecurityConfig 字段区注释）
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
/// `agents.prompt_system` 缺省值 = pro（段落池组装的新体系）。
fn default_prompt_system() -> String {
    "pro".to_string()
}
fn default_max_tokens() -> i64 {
    // 单一真相源：nemesis-types capability（2026-10-23 P0，8192→64K，
    // docs/BUG/2026-10-23_max-tokens-8192截断撞顶复发.md）
    nemesis_types::capability::DEFAULT_MAX_TOKENS
}
fn default_temperature() -> f64 {
    0.7
}
fn default_max_tool_iterations() -> i64 {
    100
}
/// E1 (2026-09-05 拍板): default concurrent mode is **queue** — busy sessions
/// park messages in the per-session inbox instead of bouncing them (devtool
/// upgrade: multi-entry coding agent needs turn-end continuation, not
/// rejection). Applies ONLY when config.json omits the key (serde default);
/// explicit `"reject"` configs keep legacy behavior. Unknown strings still
/// fail safe to reject at `parse_concurrent_mode` (nemesis-agent).
fn default_concurrent_request_mode() -> String {
    "queue".to_string()
}

fn default_snapshot_role() -> String {
    "user".to_string()
}
fn default_queue_size() -> i64 {
    8
}

/// D-4（2026-09-23 多会话并行清账）：单 loop 并发 turn 上限缺省 8。泵统一
/// 后跨会话并发是结构不变量，此缺省给极端场景一个有界并发兜底；0 显式
/// 关闭限制。
fn default_max_concurrent_turns() -> i64 {
    8
}
fn default_spill_retention_days() -> i64 {
    7
}
/// 429 重试环默认次数（2026-09-17 裁决④：对齐业界口径，默认 10）。
fn default_rate_limit_retries() -> i64 {
    10
}

/// P5（能力扩展 WS2 compaction）：逐字尾巴 token 预算缺省值（单一真相源：
/// serde 缺省 / raw-JSON 解析 / Default impl 三方共用）。对齐业界通行的
/// keepRecentTokens=20000 口径。
pub const DEFAULT_COMPACT_KEEP_RECENT_TOKENS: i64 = 20_000;

/// [`DEFAULT_COMPACT_KEEP_RECENT_TOKENS`] 的 serde 缺省函数形态。
fn default_compact_keep_recent_tokens() -> i64 {
    DEFAULT_COMPACT_KEEP_RECENT_TOKENS
}

/// P5：raw config JSON 读取 `agents.defaults.compact_keep_recent_tokens`。
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
        // REL-002：统一原子写入（与 save_config 同 0600）。
        nemesis_utils::write_file_atomic(&path.to_string_lossy(), content.as_bytes(), 0o600)
            .map_err(|e| ConfigError::Io(std::io::Error::other(e)))?;
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
mod bridge_serde_tests;

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod mcp_serde_tests;

// P5（能力扩展 WS2）：compact_keep_recent_tokens 配置面（serde 缺省 /
// roundtrip / raw-JSON 解析）测试。
#[cfg(test)]
mod ws2_compact_config_tests;

// dispatch_fallback 三档化（2026-09-29）：旧布尔保真 + 字符串档位 serde
// 兼容测试。
#[cfg(test)]
mod dispatch_fallback_mode_tests;

// P5（桌面级 VM）：executor.vm 段配置面（serde 缺省铁律 / roundtrip /
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

// HotReloader force_reload 语义（写方自刷新，绕过 mtime 检测）测试。
#[cfg(test)]
mod hot_reload_tests;

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
