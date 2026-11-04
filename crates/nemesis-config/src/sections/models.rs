// ---------------------------------------------------------------------------
// 2026-11-05 机械拆分：本文件内容由 lib.rs 原样搬入（纯代码搬家，行为零变更）。
// ---------------------------------------------------------------------------

use super::*;

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

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BraveConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

impl Default for BraveConfig {
    /// 手写收敛（BUG-20261031-226 同批）：`max_results` 字段级缺省 5，
    /// derive 给 0——enabled 缺省关使分歧平时不可达，用户开启后保存的
    /// 0 会被 agent_factory `.max(1)` 钳成 1 条结果（模板语义 5）。
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            max_results: default_max_results(),
        }
    }
}

// 注意：DuckDuckGoConfig 刻意**不**收敛双路径（BUG-20261031-226 排除项）——
// 键缺席（derive false）与出厂模板的显式 `{enabled: false}` 一致；键在场但
// enabled 缺席（字段级 true）表示用户主动配置该段=想启用。两路各服务各自
// 人群，任何收敛方向都会翻转其中一路的产品默认行为。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct DuckDuckGoConfig {
    #[serde(default = "default_true")]
    pub enabled: bool,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PerplexityConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub api_key: String,
    #[serde(default = "default_max_results")]
    pub max_results: i64,
}

impl Default for PerplexityConfig {
    /// 手写收敛（BUG-20261031-226 同批，BraveConfig 同理）。
    fn default() -> Self {
        Self {
            enabled: false,
            api_key: String::new(),
            max_results: default_max_results(),
        }
    }
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
