//! Provider resolution: resolves model references to provider configurations.
//!
//! Translated from Go `module/config/provider_resolver.go`.
//!
//! This module provides:
//! - [`resolve_model_config`] - resolves a model reference to a full provider config
//! - [`get_model_by_name`] - finds a model by name with round-robin load balancing
//! - [`get_effective_llm`] - gets the effective LLM for the default agent
//! - [`infer_provider_from_model`] - infers provider from model name
//! - [`infer_default_model`] - gets default model for a provider
//! - [`get_default_api_base`] - gets default API base URL for a provider

use serde::{Deserialize, Serialize};

use crate::{Config, ConfigError, ModelConfig, Result};

// ============================================================================
// Resolution types
// ============================================================================

/// Model resolution result with primary and fallback models.
/// Mirrors Go `ModelResolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelResolution {
    pub primary: String,
    pub fallbacks: Vec<String>,
}

/// Resolved provider and model configuration.
/// Mirrors Go `ProviderResolution`.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResolution {
    pub provider_name: String,
    pub model_name: String,
    pub api_key: String,
    pub api_base: String,
    pub proxy: String,
    pub auth_method: String,
    pub connect_mode: String,
    /// 显式协议类型（模型条目 protocol 字段，trim+lowercase 归一）。
    /// 空 = 未指定，factory 按前缀推断；非空时 factory 钉死 wire 协议
    /// （anthropic/openai/responses，显式 > 推断）。
    pub protocol: String,
    pub workspace: String,
    pub enabled: bool,
    /// H4 (U16 half): reasoning-effort tier from the model entry ("" = unset).
    #[serde(default)]
    pub reasoning_effort: String,
    /// Per-model 单请求超时秒数（P3A 超时对齐，2026-09-12）：模型条目 extra 的
    /// `timeout_secs`（别名 `timeout`）。0 = 未设置 → factory 落 lane 默认
    /// 600s（修复 anthropic/codex/http-compat lane 曾各写死 120s、与
    /// 超时阶梯「provider 单请求 600s」口径不一致：评审 LLM 连续精确 120s
    /// 超时的根因）。
    #[serde(default)]
    pub timeout_secs: u64,
}

impl Default for ProviderResolution {
    fn default() -> Self {
        Self {
            provider_name: String::new(),
            model_name: String::new(),
            reasoning_effort: String::new(),
            timeout_secs: 0,
            api_key: String::new(),
            api_base: String::new(),
            proxy: String::new(),
            auth_method: String::new(),
            connect_mode: String::new(),
            protocol: String::new(),
            workspace: String::new(),
            enabled: true,
        }
    }
}

// ============================================================================
// Core resolution functions
// ============================================================================

/// Resolve a model reference from the config's model list.
///
/// `model_ref` can be either a `model_name` or a `vendor/model` string.
/// Returns `ProviderResolution` with all configuration needed for API calls.
///
/// Mirrors Go `ResolveModelConfig`.
pub fn resolve_model_config(cfg: &Config, model_ref: &str) -> Result<ProviderResolution> {
    let model_ref = model_ref.trim();
    if model_ref.is_empty() {
        return Err(ConfigError::Validation("model reference is empty".into()));
    }

    // First, try to find by model_name (exact match)
    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            return resolve_from_model_config(mc);
        }
    }

    // Then, try to find by model field (vendor/model format)
    if model_ref.contains('/') {
        for mc in &cfg.model_list {
            if mc.model == model_ref {
                return resolve_from_model_config(mc);
            }
        }
    }

    // Not found, try to infer provider from model name
    let inferred = infer_provider_from_model(model_ref);
    if !inferred.is_empty() {
        return Ok(ProviderResolution {
            provider_name: inferred.clone(),
            model_name: model_ref.to_string(),
            api_base: get_default_api_base(&inferred),
            enabled: true,
            ..Default::default()
        });
    }

    Err(ConfigError::Validation(format!(
        "model {:?} not found in model_list",
        model_ref
    )))
}

/// Convert a ModelConfig to ProviderResolution.
/// Mirrors Go `resolveFromModelConfig`.
/// H2 (U15): resolve an api_key VALUE that may be a credential REFERENCE.
/// Three layers, checked in order (structural — the prefixes are disjoint):
/// 1. `env:VAR_NAME` — resolved from the process environment at read time.
/// 2. `yaml:<alias>` — resolved from `workspace/config/credentials.yaml`
///    (U15 completion; see `credentials` module).
/// 3. Literal value — passes through unchanged (full backward compatibility).
///    References are resolved per-operation with NO caching: changing the env var
///    or credentials.yaml takes effect on the next resolve. A reference that
///    does not resolve
///    fails LOUD with the variable/alias name and remedy — never silently
///    degrades to an empty key (which would surface later as a confusing 401).
pub(crate) fn resolve_api_key_value(raw: &str, model_for_error: &str) -> Result<String> {
    if let Some(var) = raw.strip_prefix("env:") {
        if var.is_empty() {
            return Err(ConfigError::Validation(format!(
                "model '{}': api_key uses an empty env: reference — name a variable (e.g. \"env:MY_PROVIDER_KEY\") or use a literal key in config.json",
                model_for_error
            )));
        }
        return match std::env::var(var) {
            Ok(v) if !v.is_empty() => Ok(v),
            Ok(_) => Err(ConfigError::Validation(format!(
                "model '{}': environment variable '{}' for api_key is set but empty — set it to the key, or use a literal key in config.json",
                model_for_error, var
            ))),
            Err(_) => Err(ConfigError::Validation(format!(
                "model '{}': environment variable '{}' for api_key is not set — set the env var, or use a literal key in config.json",
                model_for_error, var
            ))),
        };
    }
    if let Some(alias) = raw.strip_prefix("yaml:") {
        return crate::credentials::resolve_yaml_reference(alias, model_for_error);
    }
    // P0 vault（B1，2026-09-22 计划）：`vault:<alias>` 是链上第四层显式
    // 前缀，位于 yaml 之后；解析器由 nemesisbot 启动时注入（依赖环约束，
    // 见 vault_ref 模块文档）。非 vault 引用返回 None，直落字面量分支。
    if let Some(resolved) = crate::vault_ref::resolve_vault_reference(raw) {
        return resolved.map_err(|msg| {
            ConfigError::Validation(format!("model '{}': {}", model_for_error, msg))
        });
    }
    // B4（同计划）：明文明文 key 仍兼容，但每进程 loud warn 一次，提示
    // 迁移到加密 vault（迁移期不强制——明文继续工作，只是不再无声）。
    static PLAINTEXT_KEY_WARN: std::sync::Once = std::sync::Once::new();
    PLAINTEXT_KEY_WARN.call_once(|| {
        tracing::warn!(
            "model '{}': api_key 为明文存储在 config.json——建议运行 `nemesisbot vault migrate` \
             迁移到加密 vault（AES-256-GCM，DPAPI/Argon2id）；明文在迁移期继续工作",
            model_for_error
        );
    });
    Ok(raw.to_string())
}

/// Per-model 请求超时秒数提取（P3A 超时对齐）：读模型条目 extra（flatten
/// 未类型化键）的 `timeout_secs`，兼容别名 `timeout`。0/缺省/非正整数 =
/// 未设置（factory 落 lane 默认 600s）。非 u64 值（字符串/负数/浮点）诚实
/// 忽略按未设置处理——配置层不做静默钳位。
fn extract_timeout_secs(extra: &std::collections::BTreeMap<String, serde_json::Value>) -> u64 {
    for key in ["timeout_secs", "timeout"] {
        if let Some(n) = extra.get(key).and_then(|v| v.as_u64()) {
            return n;
        }
    }
    0
}

fn resolve_from_model_config(mc: &ModelConfig) -> Result<ProviderResolution> {
    let (provider_name, model_name) = if mc.model.contains('/') {
        let mut parts = mc.model.splitn(2, '/');
        let provider = parts.next().unwrap_or("").to_lowercase();
        let model = parts.next().unwrap_or("").to_string();
        (provider, model)
    } else {
        let provider = infer_provider_from_model(&mc.model);
        (provider, mc.model.clone())
    };

    let api_base = if mc.api_base.is_empty() {
        get_default_api_base(&provider_name)
    } else {
        mc.api_base.clone()
    };

    Ok(ProviderResolution {
        provider_name,
        model_name,
        api_key: resolve_api_key_value(&mc.api_key, &mc.model)?,
        api_base,
        proxy: mc.proxy.clone(),
        auth_method: mc.auth_method.clone(),
        connect_mode: mc.connect_mode.clone(),
        // 显式协议归一（trim + lowercase），空串保持空串 = 自动推断。
        protocol: mc.protocol.trim().to_lowercase(),
        workspace: mc.workspace.clone(),
        enabled: true,
        reasoning_effort: mc.reasoning_effort.clone(),
        timeout_secs: extract_timeout_secs(&mc.extra),
    })
}

/// Find a model configuration by name or model field (returns reference).
/// For round-robin load balancing, use `get_model_by_name` instead.
pub fn find_model_by_name<'a>(cfg: &'a Config, model_ref: &str) -> Result<&'a ModelConfig> {
    // Search by model_name
    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            return Ok(mc);
        }
    }

    // Search by model field
    for mc in &cfg.model_list {
        if mc.model == model_ref {
            return Ok(mc);
        }
    }

    Err(ConfigError::Validation(format!(
        "model {:?} not found in model_list",
        model_ref
    )))
}

/// Get the effective LLM reference for the default agent.
///
/// After migration, only the LLM field is used (old Provider/Model fields are removed).
/// Mirrors Go `GetEffectiveLLM`.
pub fn get_effective_llm(cfg: Option<&Config>) -> String {
    match cfg {
        Some(c) if !c.agents.defaults.llm.is_empty() => c.agents.defaults.llm.clone(),
        _ => "zhipu/glm-4.7-flash".to_string(),
    }
}

/// Infer the provider name from a model string.
///
/// Examines the model name for known keywords (e.g., "claude" -> "anthropic").
/// Mirrors Go `inferProviderFromModel`.
pub fn infer_provider_from_model(model: &str) -> String {
    let m = model.to_lowercase();
    if m.contains("claude") {
        return "anthropic".to_string();
    }
    if m.contains("gpt") {
        return "openai".to_string();
    }
    if m.contains("gemini") {
        return "gemini".to_string();
    }
    if m.contains("glm") || m.contains("zhipu") {
        return "zhipu".to_string();
    }
    if m.contains("groq") {
        return "groq".to_string();
    }
    if m.contains("llama") {
        return "ollama".to_string();
    }
    if m.contains("moonshot") || m.contains("kimi") {
        return "moonshot".to_string();
    }
    if m.contains("nvidia") {
        return "nvidia".to_string();
    }
    if m.contains("deepseek") {
        return "deepseek".to_string();
    }
    if m.contains("mistral") || m.contains("mixtral") || m.contains("codestral") {
        return "mistral".to_string();
    }
    if m.contains("command") || m.contains("cohere") {
        return "cohere".to_string();
    }
    if m.contains("sonar") || m.contains("perplexity") {
        return "perplexity".to_string();
    }
    String::new()
}

// ============================================================================
// Provider preset table（P9，能力扩展 WS5，2026-09-25）
// ============================================================================

/// 一个内置 provider 家族预设。
///
/// 单一真相源：`get_default_api_base` / `infer_default_model` / CLI
/// `model add --provider` / Dashboard 家族分组全部查这张表（前端镜像
/// `web/src/utils/providerFamilies.ts`，只镜像 id/aliases/display_name 三字段）。
pub struct ProviderPreset {
    /// 规范家族 id（`vendor/` 前缀与 `--provider` 取值）。
    pub id: &'static str,
    /// 等价拼写（查表兼容旧入口名；全表内不得与任何 id/alias 冲突）。
    pub aliases: &'static [&'static str],
    /// 默认 API base（各家公开文档的 OpenAI 兼容端点；本地推理服务器为
    /// 本机默认端口）。
    pub api_base: &'static str,
    /// wire 协议：`anthropic` | `openai` | `responses`（与
    /// `nemesis_types::capability::normalize_model_protocol` 值域一致）。
    pub protocol: &'static str,
    /// 只给 `--provider` 不给 `--model` 时自动采用的默认型号。
    /// 空串 = 该家族无公开固定默认型号（须显式 `--model`）。
    pub default_model: &'static str,
    /// 展示名（Dashboard 分组头）。
    pub display_name: &'static str,
}

/// 内置 provider 家族预设表（70 家）。
///
/// 数据来源：各家公开文档的 OpenAI 兼容端点 + 内置 LiteLLM 价目表的
/// provider 维度交叉核对。表数据完整性（URL/协议枚举/别名唯一/默认型号）
/// 由 `provider_resolver/tests.rs` 的预设完整性测试钉住；错一家修一家，
/// 不影响查表结构。
///
/// 分区顺序：前沿实验室 → 中国厂商 → 国际厂商 → 聚合/GPU 云 → 本地推理
/// 服务器 → 特例（历史遗留入口）。
pub static PROVIDER_PRESETS: &[ProviderPreset] = &[
    // ---- 前沿实验室 ----
    ProviderPreset {
        id: "openai",
        aliases: &["gpt"],
        api_base: "https://api.openai.com/v1",
        protocol: "openai",
        default_model: "gpt-4o",
        display_name: "OpenAI",
    },
    ProviderPreset {
        id: "anthropic",
        aliases: &["claude"],
        api_base: "https://api.anthropic.com/v1",
        protocol: "anthropic",
        default_model: "claude-sonnet-4-20250514",
        display_name: "Anthropic",
    },
    ProviderPreset {
        id: "gemini",
        aliases: &["google"],
        api_base: "https://generativelanguage.googleapis.com/v1beta",
        protocol: "openai",
        default_model: "gemini-2.0-flash-exp",
        display_name: "Google Gemini",
    },
    ProviderPreset {
        id: "xai",
        aliases: &["grok"],
        api_base: "https://api.x.ai/v1",
        protocol: "openai",
        default_model: "grok-3",
        display_name: "xAI",
    },
    // ---- 中国厂商 ----
    ProviderPreset {
        id: "zhipu",
        aliases: &["glm", "bigmodel"],
        api_base: "https://open.bigmodel.cn/api/paas/v4",
        protocol: "openai",
        default_model: "glm-4.7-flash",
        display_name: "智谱 AI",
    },
    ProviderPreset {
        id: "zai",
        aliases: &[],
        api_base: "https://api.z.ai/api/paas/v4",
        protocol: "openai",
        default_model: "glm-4.6",
        display_name: "Z.ai（智谱国际）",
    },
    ProviderPreset {
        id: "deepseek",
        aliases: &[],
        api_base: "https://api.deepseek.com/v1",
        protocol: "openai",
        default_model: "deepseek-chat",
        display_name: "DeepSeek",
    },
    ProviderPreset {
        id: "moonshot",
        aliases: &["kimi"],
        api_base: "https://api.moonshot.cn/v1",
        protocol: "openai",
        default_model: "moonshot-v1-8k",
        display_name: "Moonshot AI（月之暗面）",
    },
    ProviderPreset {
        id: "dashscope",
        aliases: &["qwen", "bailian"],
        api_base: "https://dashscope.aliyuncs.com/compatible-mode/v1",
        protocol: "openai",
        default_model: "qwen-max",
        display_name: "阿里云百炼（通义千问）",
    },
    ProviderPreset {
        id: "doubao",
        aliases: &["ark", "volcengine"],
        api_base: "https://ark.cn-beijing.volces.com/api/v3",
        protocol: "openai",
        default_model: "doubao-seed-1-6-flash-250615",
        display_name: "火山方舟（豆包）",
    },
    ProviderPreset {
        id: "hunyuan",
        aliases: &["tencent"],
        api_base: "https://api.hunyuan.cloud.tencent.com/v1",
        protocol: "openai",
        default_model: "hunyuan-turbos-latest",
        display_name: "腾讯混元",
    },
    ProviderPreset {
        id: "minimax",
        aliases: &["minimaxi"],
        api_base: "https://api.minimaxi.com/v1",
        protocol: "openai",
        default_model: "MiniMax-Text-01",
        display_name: "MiniMax",
    },
    ProviderPreset {
        id: "baichuan",
        aliases: &[],
        api_base: "https://api.baichuan-ai.com/v1",
        protocol: "openai",
        default_model: "Baichuan4-Air",
        display_name: "百川智能",
    },
    ProviderPreset {
        id: "stepfun",
        aliases: &[],
        api_base: "https://api.stepfun.com/v1",
        protocol: "openai",
        default_model: "step-2-16k",
        display_name: "阶跃星辰",
    },
    ProviderPreset {
        id: "yi",
        aliases: &["01ai", "lingyi"],
        api_base: "https://api.lingyiwanwu.com/v1",
        protocol: "openai",
        default_model: "yi-large",
        display_name: "零一万物（01.AI）",
    },
    ProviderPreset {
        id: "siliconflow",
        aliases: &["silicon"],
        api_base: "https://api.siliconflow.cn/v1",
        protocol: "openai",
        default_model: "deepseek-ai/DeepSeek-V3",
        display_name: "硅基流动",
    },
    ProviderPreset {
        id: "modelscope",
        aliases: &[],
        api_base: "https://api-inference.modelscope.cn/v1",
        protocol: "openai",
        default_model: "Qwen/Qwen2.5-72B-Instruct",
        display_name: "魔搭社区",
    },
    ProviderPreset {
        id: "sensenova",
        aliases: &["sensetime"],
        api_base: "https://api.sensenova.cn/compatible-mode/v1",
        protocol: "openai",
        default_model: "SenseChat-5",
        display_name: "商汤日日新",
    },
    ProviderPreset {
        id: "ai360",
        aliases: &["360", "qihoo"],
        api_base: "https://api.360.cn/v1",
        protocol: "openai",
        default_model: "360gpt2-pro",
        display_name: "360 智脑",
    },
    ProviderPreset {
        id: "spark",
        aliases: &["xfyun", "iflytek"],
        api_base: "https://spark-api-open.xf-yun.com/v1",
        protocol: "openai",
        default_model: "generalv3.5",
        display_name: "讯飞星火",
    },
    ProviderPreset {
        id: "baidu",
        aliases: &["qianfan", "ernie"],
        api_base: "https://qianfan.baidubce.com/v2",
        protocol: "openai",
        default_model: "ernie-4.0-8k-latest",
        display_name: "百度千帆",
    },
    ProviderPreset {
        id: "gitee_ai",
        aliases: &["gitee"],
        api_base: "https://ai.gitee.com/v1",
        protocol: "openai",
        default_model: "DeepSeek-V3",
        display_name: "Gitee AI",
    },
    // ---- 国际厂商 ----
    ProviderPreset {
        id: "mistral",
        aliases: &[],
        api_base: "https://api.mistral.ai/v1",
        protocol: "openai",
        default_model: "mistral-large-latest",
        display_name: "Mistral AI",
    },
    ProviderPreset {
        id: "codestral",
        aliases: &[],
        api_base: "https://codestral.mistral.ai/v1",
        protocol: "openai",
        default_model: "codestral-latest",
        display_name: "Codestral（Mistral 代码端点）",
    },
    ProviderPreset {
        id: "cohere",
        aliases: &[],
        // Cohere 官方 OpenAI 兼容端点（旧值 api.cohere.ai/v2 是原生 v2 API）。
        api_base: "https://api.cohere.com/compatibility/v1",
        protocol: "openai",
        default_model: "command-r-plus",
        display_name: "Cohere",
    },
    ProviderPreset {
        id: "perplexity",
        aliases: &["pplx"],
        // 官方文档 base_url 无 /v1 后缀（SDK 自动拼 /chat/completions）。
        api_base: "https://api.perplexity.ai",
        protocol: "openai",
        default_model: "sonar",
        display_name: "Perplexity",
    },
    ProviderPreset {
        id: "ai21",
        aliases: &["jamba"],
        api_base: "https://api.ai21.com/studio/v1",
        protocol: "openai",
        default_model: "jamba-large-1.6",
        display_name: "AI21 Labs",
    },
    ProviderPreset {
        id: "writer",
        aliases: &[],
        api_base: "https://api.writer.com/v1",
        protocol: "openai",
        default_model: "palmyra-x5",
        display_name: "Writer",
    },
    ProviderPreset {
        id: "reka",
        aliases: &[],
        api_base: "https://api.reka.ai/v1",
        protocol: "openai",
        default_model: "reka-core",
        display_name: "Reka AI",
    },
    ProviderPreset {
        id: "upstage",
        aliases: &["solar"],
        api_base: "https://api.upstage.ai/v1/solar",
        protocol: "openai",
        default_model: "solar-pro",
        display_name: "Upstage",
    },
    ProviderPreset {
        id: "gigachat",
        aliases: &[],
        api_base: "https://gigachat.devices.sberbank.ru/api/v1",
        protocol: "openai",
        default_model: "GigaChat",
        display_name: "GigaChat（Sber）",
    },
    ProviderPreset {
        id: "yandex",
        aliases: &[],
        api_base: "https://llm.api.cloud.yandex.net/v1",
        protocol: "openai",
        default_model: "yandexgpt",
        display_name: "Yandex Cloud",
    },
    ProviderPreset {
        id: "sarvam",
        aliases: &[],
        api_base: "https://api.sarvam.ai/v1",
        protocol: "openai",
        default_model: "sarvam-m",
        display_name: "Sarvam AI",
    },
    ProviderPreset {
        id: "llama_api",
        aliases: &["meta"],
        api_base: "https://api.llama.com/compat/v1",
        protocol: "openai",
        default_model: "Llama-4-Maverick-17B-128E-Instruct-FP8",
        display_name: "Meta Llama API",
    },
    // ---- 聚合 / GPU 云 ----
    ProviderPreset {
        id: "openrouter",
        aliases: &[],
        api_base: "https://openrouter.ai/api/v1",
        protocol: "openai",
        default_model: "openai/gpt-4o",
        display_name: "OpenRouter",
    },
    ProviderPreset {
        id: "groq",
        aliases: &[],
        api_base: "https://api.groq.com/openai/v1",
        protocol: "openai",
        default_model: "llama-3.3-70b-versatile",
        display_name: "Groq",
    },
    ProviderPreset {
        id: "together",
        aliases: &["together_ai"],
        api_base: "https://api.together.xyz/v1",
        protocol: "openai",
        default_model: "meta-llama/Llama-3.3-70B-Instruct-Turbo",
        display_name: "Together AI",
    },
    ProviderPreset {
        id: "fireworks",
        aliases: &["fireworks_ai"],
        api_base: "https://api.fireworks.ai/inference/v1",
        protocol: "openai",
        default_model: "accounts/fireworks/models/llama-v3p3-70b-instruct",
        display_name: "Fireworks AI",
    },
    ProviderPreset {
        id: "cerebras",
        aliases: &[],
        api_base: "https://api.cerebras.ai/v1",
        protocol: "openai",
        default_model: "llama-3.3-70b",
        display_name: "Cerebras",
    },
    ProviderPreset {
        id: "sambanova",
        aliases: &[],
        api_base: "https://api.sambanova.ai/v1",
        protocol: "openai",
        default_model: "Meta-Llama-3.3-70B-Instruct",
        display_name: "SambaNova",
    },
    ProviderPreset {
        id: "nvidia",
        aliases: &["nim"],
        api_base: "https://integrate.api.nvidia.com/v1",
        protocol: "openai",
        default_model: "nvidia/llama-3.1-nemotron-70b-instruct",
        display_name: "NVIDIA NIM",
    },
    ProviderPreset {
        id: "deepinfra",
        aliases: &[],
        api_base: "https://api.deepinfra.com/v1/openai",
        protocol: "openai",
        default_model: "meta-llama/Meta-Llama-3.3-70B-Instruct",
        display_name: "DeepInfra",
    },
    ProviderPreset {
        id: "novita",
        aliases: &["novita_ai"],
        api_base: "https://api.novita.ai/v3/openai",
        protocol: "openai",
        default_model: "deepseek/deepseek-v3",
        display_name: "Novita AI",
    },
    ProviderPreset {
        id: "hyperbolic",
        aliases: &[],
        api_base: "https://api.hyperbolic.xyz/v1",
        protocol: "openai",
        default_model: "meta-llama/Meta-Llama-3.3-70B-Instruct",
        display_name: "Hyperbolic",
    },
    ProviderPreset {
        id: "nebius",
        aliases: &[],
        api_base: "https://api.studio.nebius.ai/v1",
        protocol: "openai",
        default_model: "deepseek-ai/DeepSeek-V3",
        display_name: "Nebius AI Studio",
    },
    ProviderPreset {
        id: "lambda",
        aliases: &["lambdalabs"],
        api_base: "https://api.lambda.ai/v1",
        protocol: "openai",
        default_model: "llama3.3-70b-instruct-fp8",
        display_name: "Lambda",
    },
    ProviderPreset {
        id: "friendliai",
        aliases: &[],
        api_base: "https://api.friendliai.com/v1",
        protocol: "openai",
        default_model: "meta-llama/Meta-Llama-3.1-70B-Instruct",
        display_name: "FriendliAI",
    },
    ProviderPreset {
        id: "baseten",
        aliases: &[],
        api_base: "https://inference.baseten.co/v1",
        protocol: "openai",
        default_model: "meta-llama/Meta-Llama-3.1-8B-Instruct",
        display_name: "Baseten",
    },
    ProviderPreset {
        id: "kluster",
        aliases: &[],
        api_base: "https://api.kluster.ai/v1",
        protocol: "openai",
        default_model: "klusterai/Meta-Llama-3.3-70B-Instruct-Turbo",
        display_name: "Kluster AI",
    },
    ProviderPreset {
        id: "ovhcloud",
        aliases: &["ovh"],
        api_base: "https://oai.endpoints.kepler.ai.cloud.ovh.net/v1",
        protocol: "openai",
        default_model: "Meta-Llama-3.3-70B-Instruct",
        display_name: "OVHcloud AI Endpoints",
    },
    ProviderPreset {
        id: "scaleway",
        aliases: &[],
        api_base: "https://api.scaleway.ai/v1",
        protocol: "openai",
        default_model: "llama-3.3-70b-instruct",
        display_name: "Scaleway",
    },
    ProviderPreset {
        id: "gmi",
        aliases: &[],
        api_base: "https://api.gmi.ai/v1",
        protocol: "openai",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        display_name: "GMI Cloud",
    },
    ProviderPreset {
        id: "nscale",
        aliases: &[],
        api_base: "https://inference.api.nscale.com/v1",
        protocol: "openai",
        default_model: "deepseek-ai/DeepSeek-V3",
        display_name: "Nscale",
    },
    ProviderPreset {
        id: "replicate",
        aliases: &[],
        api_base: "https://api.replicate.com/v1",
        protocol: "openai",
        default_model: "meta/meta-llama-3.3-70b-instruct",
        display_name: "Replicate",
    },
    ProviderPreset {
        id: "huggingface",
        aliases: &["hf"],
        api_base: "https://router.huggingface.co/v1",
        protocol: "openai",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        display_name: "Hugging Face Router",
    },
    ProviderPreset {
        id: "github_models",
        aliases: &["github", "ghmodels"],
        api_base: "https://models.github.ai/inference",
        protocol: "openai",
        default_model: "openai/gpt-4o-mini",
        display_name: "GitHub Models",
    },
    ProviderPreset {
        id: "vercel",
        aliases: &["vercel_gateway"],
        api_base: "https://ai-gateway.vercel.sh/v1",
        protocol: "openai",
        default_model: "openai/gpt-4o",
        display_name: "Vercel AI Gateway",
    },
    ProviderPreset {
        id: "featherless",
        aliases: &[],
        api_base: "https://api.featherless.ai/v1",
        protocol: "openai",
        default_model: "meta-llama/Meta-Llama-3.1-8B-Instruct",
        display_name: "Featherless AI",
    },
    ProviderPreset {
        id: "ionet",
        aliases: &[],
        api_base: "https://api.intelligence.io.solutions/api/v1",
        protocol: "openai",
        default_model: "meta-llama/Llama-3.3-70B-Instruct",
        display_name: "IO.NET Intelligence",
    },
    ProviderPreset {
        id: "ppinfra",
        aliases: &["ppio"],
        api_base: "https://api.ppinfra.com/v3/openai",
        protocol: "openai",
        default_model: "deepseek/deepseek-r1",
        display_name: "PPIO 派欧云",
    },
    ProviderPreset {
        id: "byteplus",
        aliases: &[],
        api_base: "https://ark.ap-southeast.bytepluses.com/api/v3",
        protocol: "openai",
        default_model: "doubao-seed-1-6-flash-250615",
        display_name: "BytePlus Model Ark",
    },
    ProviderPreset {
        id: "aihubmix",
        aliases: &["hubmix"],
        api_base: "https://aihubmix.com/v1",
        protocol: "openai",
        default_model: "gpt-4o",
        display_name: "AiHubMix",
    },
    // ---- 本地推理服务器 ----
    ProviderPreset {
        id: "ollama",
        aliases: &[],
        api_base: "http://localhost:11434/v1",
        protocol: "openai",
        default_model: "llama3.3",
        display_name: "Ollama（本机）",
    },
    ProviderPreset {
        id: "lmstudio",
        aliases: &["lm_studio"],
        api_base: "http://localhost:1234/v1",
        protocol: "openai",
        default_model: "qwen2.5-7b-instruct",
        display_name: "LM Studio（本机）",
    },
    ProviderPreset {
        id: "vllm",
        aliases: &[],
        api_base: "http://localhost:8000/v1",
        protocol: "openai",
        default_model: "Qwen/Qwen2.5-7B-Instruct",
        display_name: "vLLM（本机）",
    },
    ProviderPreset {
        id: "sglang",
        aliases: &[],
        api_base: "http://localhost:30000/v1",
        protocol: "openai",
        default_model: "meta-llama/Llama-3.1-8B-Instruct",
        display_name: "SGLang（本机）",
    },
    // llama-server 忽略 model 字段（服务已加载的 GGUF），"local-model" 是
    // 官方文档示例的占位约定。
    ProviderPreset {
        id: "llama_cpp",
        aliases: &["llamacpp"],
        api_base: "http://localhost:8080/v1",
        protocol: "openai",
        default_model: "local-model",
        display_name: "llama.cpp（本机）",
    },
    // Jan 本地服务器同 llama.cpp：model 字段取已下载模型的 id，官方示例
    // 允许任意值，"local-model" 为占位约定。
    ProviderPreset {
        id: "jan",
        aliases: &[],
        api_base: "http://127.0.0.1:1337/v1",
        protocol: "openai",
        default_model: "local-model",
        display_name: "Jan（本机）",
    },
    // ---- 特例（历史遗留入口）----
    // GitHub Copilot 本地代理（LiteLLM 同款默认端口）。
    ProviderPreset {
        id: "github_copilot",
        aliases: &["copilot"],
        api_base: "http://localhost:4321",
        protocol: "openai",
        default_model: "gpt-4o",
        display_name: "GitHub Copilot（本地代理）",
    },
    // 声通云路由：平台按租户下发型号，无公开固定默认型号——default_model
    // 留空（`--provider` 须显式 `--model`；完整性测试按此豁免）。
    ProviderPreset {
        id: "shengsuanyun",
        aliases: &[],
        api_base: "https://router.shengsuanyun.com/api/v1",
        protocol: "openai",
        default_model: "",
        display_name: "声通云路由",
    },
];

/// Look up a provider preset by id or alias (trimmed, ASCII case-insensitive).
pub fn find_provider_preset(name: &str) -> Option<&'static ProviderPreset> {
    let needle = name.trim().to_ascii_lowercase();
    if needle.is_empty() {
        return None;
    }
    PROVIDER_PRESETS
        .iter()
        .find(|p| p.id == needle || p.aliases.contains(&needle.as_str()))
}

/// Sorted list of all preset family ids (error-message / completion helper).
pub fn provider_preset_ids() -> Vec<&'static str> {
    let mut ids: Vec<&'static str> = PROVIDER_PRESETS.iter().map(|p| p.id).collect();
    ids.sort_unstable();
    ids
}

/// Get the default API base URL for a provider.
///
/// 查 [`PROVIDER_PRESETS`] 预设表（id 或别名命中）。未知家族返回空串。
/// Mirrors Go `getDefaultAPIBase`（表化后语义不变）。
pub fn get_default_api_base(provider: &str) -> String {
    find_provider_preset(provider)
        .map(|p| p.api_base.to_string())
        .unwrap_or_default()
}

/// Return the default model for a given provider name.
///
/// When a provider is known but no specific model is configured, this provides
/// a reasonable default（查 [`PROVIDER_PRESETS`]；家族 default_model 为空或
/// 家族未知返回空串）。Mirrors Go `inferDefaultModel`（表化后语义不变）。
pub fn infer_default_model(provider: &str) -> String {
    find_provider_preset(provider)
        .map(|p| p.default_model.to_string())
        .unwrap_or_default()
}

/// Find a model configuration by name with round-robin load balancing.
///
/// When multiple models match, uses an atomic counter to distribute load.
/// Mirrors Go `GetModelByName`.
pub fn get_model_by_name(cfg: &Config, model_ref: &str) -> Result<ModelConfig> {
    let mut matches: Vec<&ModelConfig> = Vec::new();

    for mc in &cfg.model_list {
        if mc.model_name == model_ref {
            matches.push(mc);
        }
    }

    if matches.is_empty() {
        for mc in &cfg.model_list {
            if mc.model == model_ref {
                matches.push(mc);
            }
        }
    }

    if matches.is_empty() {
        return Err(ConfigError::Validation(format!(
            "model {:?} not found in model_list",
            model_ref
        )));
    }

    if matches.len() == 1 {
        return Ok(matches[0].clone());
    }

    static RR_COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let idx =
        RR_COUNTER.fetch_add(1, std::sync::atomic::Ordering::Relaxed) % (matches.len() as u64);
    Ok(matches[idx as usize].clone())
}

/// Resolve the model resolution (primary + fallbacks) for the default agent.
///
/// Mirrors Go `ModelResolution` construction.
pub fn resolve_model_resolution(cfg: &Config) -> ModelResolution {
    let llm = get_effective_llm(Some(cfg));
    ModelResolution {
        primary: llm,
        fallbacks: vec![],
    }
}

// ============================================================================
// ProviderResolver struct (convenience wrapper)
// ============================================================================

/// Provider resolver: finds model config by model name/alias.
pub struct ProviderResolver;

impl ProviderResolver {
    /// Find a model config by name from the model list.
    pub fn find_by_name<'a>(models: &'a [ModelConfig], name: &str) -> Option<&'a ModelConfig> {
        models.iter().find(|m| m.model_name == name)
    }

    /// Find default model (first model in the list, or one marked as default).
    pub fn find_default(models: &[ModelConfig]) -> Option<&ModelConfig> {
        models.first()
    }

    /// Resolve model string to a provider and model identifier.
    pub fn resolve_model_string(model_str: &str) -> (&str, &str) {
        if let Some(slash_pos) = model_str.find('/') {
            (&model_str[..slash_pos], &model_str[slash_pos + 1..])
        } else {
            ("openai", model_str)
        }
    }
}

// ============================================================================
// Tests
// ============================================================================

#[cfg(test)]
mod tests;
