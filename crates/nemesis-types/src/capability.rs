//! Model capability tier (small-model-tool-robustness plan, Phase 4a).
//!
//! A per-model size/capability classification that drives tool-set size,
//! validation-retry budget, and format-repair gating. Stored on each model
//! entry in `config.json` as `model_tier` (default `"auto"`), resolved to a
//! concrete tier at agent-construction time via name/size heuristics.
//!
//! Design note: when detection is unsure, we default to [`ModelTier::Big`] (full
//! toolset). Wrongly withholding tools from a strong model is unrecoverable;
//! wrongly over-provisioning a weak model is caught by Phase 2 schema validation
//! and degrades gracefully.

use serde::{Deserialize, Serialize};

/// User-facing tier. `Auto` means "detect via heuristic"; the others are
/// explicit user overrides that short-circuit detection ("user knows best").
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum ModelTier {
    #[default]
    Auto,
    /// Small model (~9B–35B). Restricted core toolset, generous retry budget,
    /// format-repair layer enabled.
    Mini,
    /// Medium model (~70B–120B+). Mid toolset.
    Normal,
    /// Large model (200B+ or cloud flagship). Full toolset, minimal retry.
    Big,
}

impl ModelTier {
    pub fn is_auto(self) -> bool {
        matches!(self, ModelTier::Auto)
    }

    /// Resolve a (possibly `Auto`) tier against gathered hints. Explicit
    /// (non-Auto) tiers always pass through unchanged.
    pub fn resolve(self, hint: &TierHint) -> ModelTier {
        match self {
            ModelTier::Auto => detect_tier(hint),
            other => other,
        }
    }

    /// Phase 2 validation-retry budget for this tier — how many consecutive
    /// schema-violating tool calls to tolerate before stopping the loop.
    /// Smaller models get more rope, since they stumble more often.
    pub fn validation_retry_budget(self) -> u32 {
        match self {
            ModelTier::Mini => 3,
            ModelTier::Normal => 2,
            ModelTier::Big => 1,
            ModelTier::Auto => 2, // pre-resolution fallback; resolve() first
        }
    }
}

impl std::fmt::Display for ModelTier {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let s = match self {
            ModelTier::Auto => "auto",
            ModelTier::Mini => "mini",
            ModelTier::Normal => "normal",
            ModelTier::Big => "big",
        };
        write!(f, "{}", s)
    }
}

/// Hints gathered by the auto-detection chain. All optional; detection is
/// best-effort. (Phase 4b will extend this with backend-metadata and probe
/// results.)
#[derive(Debug, Clone, Default)]
pub struct TierHint {
    /// Full model identifier, e.g. `"vendor/qwen3-30b-a3b"`.
    pub full_model: Option<String>,
    /// User-provided real name, e.g. `"Qwen3-30B-A3B"` (when the alias is
    /// opaque, e.g. `astron-code-latest`).
    pub real_name: Option<String>,
    /// User-provided explicit parameter size in billions.
    pub size_b: Option<u32>,
}

/// Best-effort auto detection. Priority: explicit size → size marker in
/// real_name/full_model → provider/family keywords → default Big.
pub fn detect_tier(hint: &TierHint) -> ModelTier {
    if let Some(b) = hint.size_b {
        return tier_from_size_b(b);
    }
    for name in [hint.real_name.as_deref(), hint.full_model.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Some(b) = parse_size_marker(name) {
            return tier_from_size_b(b);
        }
    }
    for name in [hint.real_name.as_deref(), hint.full_model.as_deref()]
        .into_iter()
        .flatten()
    {
        if let Some(t) = detect_tier_from_keywords(name) {
            return t;
        }
    }
    ModelTier::Big
}

/// Size buckets per the user's spec: mini 9–35B, normal 70–120B+, big 200B+.
/// (Values ≤ 8B also count as Mini — the very-small edge of the range.)
pub fn tier_from_size_b(b: u32) -> ModelTier {
    match b {
        0..=39 => ModelTier::Mini,
        40..=199 => ModelTier::Normal,
        _ => ModelTier::Big,
    }
}

/// Parse a parameter-size marker like `"30b"`, `"9b"`, `"120b"` from anywhere in
/// a model name (case-insensitive). Returns size in whole billions (rounded
/// down). Handles fractional values like `"1.5b"`.
pub fn parse_size_marker(s: &str) -> Option<u32> {
    let re = regex::Regex::new(r"(\d+(?:\.\d+)?)\s*b\b").ok()?;
    let lower = s.to_lowercase();
    let c = re.captures(&lower)?;
    let n: f64 = c.get(1)?.as_str().parse().ok()?;
    Some(n as u32)
}

/// Resolve the capability tier for the active model in a `config.json` Value.
///
/// Looks up `model_list[]` for the entry matching `active_alias` (by
/// `model_name` or `model`), reads its `model_tier` (default `Auto`), and
/// resolves via the heuristic. Returns `Big` if the model can't be found.
pub fn resolve_active_tier(cfg: &serde_json::Value, active_alias: &str) -> ModelTier {
    let entry = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        });

    let Some(entry) = entry else {
        return ModelTier::Big;
    };

    let tier: ModelTier = entry
        .get("model_tier")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let hint = TierHint {
        full_model: entry
            .get("model")
            .and_then(|v| v.as_str())
            .map(String::from),
        real_name: entry
            .get("real_name")
            .and_then(|v| v.as_str())
            .map(String::from),
        size_b: entry
            .get("model_size_b")
            .and_then(|v| v.as_u64())
            .map(|n| n as u32),
    };

    tier.resolve(&hint)
}

/// Resolve the per-model output token cap (`max_output_tokens`) for the active
/// model alias, by looking up `model_list[]` for the matching entry (by
/// `model_name` or `model`) and returning its `max_output_tokens` field. `None`
/// when the entry isn't found or the field is absent — the caller falls back to
/// a default.
///
/// Lets each model declare its real output ceiling so the agent requests
/// `max_tokens = cap` instead of a one-size-fits-all default: large files that
/// fit within the model's real cap write in one shot instead of truncating.
/// Pure (no IO); `AgentLoop::current_max_tokens` reads config.json fresh and
/// hands the parsed value here. Mirrors [`resolve_active_tier`].
pub fn resolve_max_output_tokens(cfg: &serde_json::Value, active_alias: &str) -> Option<i64> {
    cfg.get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        })
        .and_then(|m| m.get("max_output_tokens"))
        .and_then(|v| v.as_i64())
}

/// H4 (U16 half): resolve the per-model `reasoning_effort` tier for the
/// active model alias. Mirrors [`resolve_max_output_tokens`]: finds the
/// matching `model_list[]` entry and returns its `reasoning_effort` string
/// ("off"|"low"|"medium"|"high"); None/empty → None (send nothing).
pub fn resolve_reasoning_effort(cfg: &serde_json::Value, active_alias: &str) -> Option<String> {
    let e = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        })
        .and_then(|m| m.get("reasoning_effort"))
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_default();
    match e.as_str() {
        "" | "off" => None,
        tier @ ("low" | "medium" | "high") => Some(tier.to_string()),
        _ => None,
    }
}

/// U16 (sixth batch): resolve the per-model `context_window` (input token
/// capacity) for the active model alias. Mirrors
/// [`resolve_max_output_tokens`]. `None` → caller keeps its default (the
/// historical 32000 hardcoded in `AgentInstance`). Written by `model add`
/// when the models.dev catalog hit fills it, or manually via config.json.
pub fn resolve_context_window(cfg: &serde_json::Value, active_alias: &str) -> Option<i64> {
    cfg.get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        })
        .and_then(|m| m.get("context_window"))
        .and_then(|v| v.as_i64())
        .filter(|w| *w > 0)
}

/// T4 (U1): resolve the per-model `summarizer_prefix_reuse` flag for the
/// active model alias. Mirrors [`resolve_max_output_tokens`]. `None` when
/// unset/entry missing (caller's default applies = keep the prefix-reuse
/// shape); `Some(false)` opts this model's summarizer out of prefix reuse —
/// cheap summarizer models can break the assumed warm KV prefix (different
/// tokenizer, no prompt caching), for which the fallback is the old
/// shape-neutral single-message summary request. Explicit `true` is
/// equivalent to the default but records intent in config.
pub fn resolve_summarizer_prefix_reuse(
    cfg: &serde_json::Value,
    active_alias: &str,
) -> Option<bool> {
    cfg.get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        })
        .and_then(|m| m.get("summarizer_prefix_reuse"))
        .and_then(|v| v.as_bool())
}

// ============================================================================
// Vision capability（多模态 goal T10，2026-09-03）
// ============================================================================

/// T10（多模态 goal）：vision 能力解析结果——`supported` 决定图片是否进
/// 请求，`source` 记录判定来源（诊断/展示用）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VisionResolution {
    /// 该模型当前是否按"支持图像输入"处理。
    pub supported: bool,
    /// 判定来源。
    pub source: VisionSource,
}

/// vision 判定来源。解析序（钉死）：`User > Probe > Name > DefaultAllow`。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VisionSource {
    /// config.json 条目 `vision`——用户钉死，最大权威（L8：宽容解析，
    /// 接受 "yes"/"no"/"true"/"false" 任意大小写及 JSON 布尔）。
    User,
    /// `model probe` 第 8 题实测（条目 `vision_probe` bool）。
    Probe,
    /// 名字关键词命中（关键词只判"支持"，永不判"不支持"）。
    Name,
    /// 认不出的名字默认放行——避免把用户的 VL 模型误判成纯文本；真不支持
    /// 由 provider 4xx 兜底 + 错误文案提示可跑 `model probe` 实测或钉
    /// `vision: "no"`。
    DefaultAllow,
}

impl VisionSource {
    pub fn as_str(self) -> &'static str {
        match self {
            VisionSource::User => "user",
            VisionSource::Probe => "probe",
            VisionSource::Name => "name",
            VisionSource::DefaultAllow => "default_allow",
        }
    }
}

/// 默认放行（standalone / 条目缺失 / 名字认不出共用的同一语义）。
pub fn vision_default_allow() -> VisionResolution {
    VisionResolution {
        supported: true,
        source: VisionSource::DefaultAllow,
    }
}

/// L8（2026-09-04 四轮盲审）：`vision` 钉死值的宽容解析（`None` = 非钉死值，
/// 走后续解析层）。接受：`"yes"/"no"`（任意大小写/首尾空白）、JSON 布尔
/// `true/false`、`"true"/"false"`；其余（空串/垃圾值）返回 None。
fn parse_vision_pin(v: &serde_json::Value) -> Option<bool> {
    match v {
        serde_json::Value::Bool(b) => Some(*b),
        serde_json::Value::String(s) => match s.trim().to_lowercase().as_str() {
            "yes" | "true" => Some(true),
            "no" | "false" => Some(false),
            _ => None,
        },
        _ => None,
    }
}

/// T10：名字关键词视觉识别。**只判"支持"**（命中返回 true），认不出一律
/// 返回 false（由调用方落到默认放行）——检测链永不产生"不支持"结论，避免
/// 把用户的 VL 模型误判成纯文本（provider 4xx 才是兜底真相）。
///
/// 匹配在归一化形态（仅保留 `[a-z0-9]`）上做 contains，兼容连字符/点号/
/// 大小写变体：`Qwen2.5-VL` → `qwen25vl`、`gpt-4o` → `gpt4o`、
/// `glm-4.1v` → `glm41v`。误报（把纯文本模型标成"支持"）与默认放行同向，
/// 零额外行为代价（都进 provider 4xx 兜底）。
pub fn detect_vision_from_name(name: &str) -> bool {
    let norm: String = name
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .map(|c| c.to_ascii_lowercase())
        .collect();
    // 单标记家族（云端旗舰 + 常见开源 VL 家族）。
    const MARKERS: &[&str] = &[
        "gpt4o", "gpt41", "gpt42", "gpt45", "gpt5", "chatgpt", //
        "o1", "o3", "o4", //
        "gemini", "claude", //
        "llava", "internvl", "minicpmv", "minicpmo", "pixtral", "qvq",    //
        "vision", //
        "glm4v", "glm5v", "glm41v", "glm45v",
    ];
    if MARKERS.iter().any(|m| norm.contains(m)) {
        return true;
    }
    // 组合规则：qwen*/kimi* 系的 -VL 变体（qwen2.5-vl / kimi-vl 等）。
    if norm.contains("vl") && (norm.contains("qwen") || norm.contains("kimi")) {
        return true;
    }
    false
}

/// T10（多模态 goal）：解析 active 模型的 vision 能力。镜像
/// [`resolve_active_tier`] 的条目查找（`model_name` 或 `model` 匹配），条目
/// 额外读 `real_name` 参与名字识别。
///
/// 解析序（钉死）：
/// 1. 条目 `vision`——用户钉死，直接采信（L8：大小写不敏感 + 布尔宽容）；
/// 2. 条目 `vision_probe`（`model probe` 第 8 题实测 bool）——true/false 均
///    采信（实测 > 猜测）；
/// 3. 名字关键词（[`detect_vision_from_name`]，model_name / model /
///    real_name 任一命中即"支持"）——只判支持；
/// 4. 默认放行——认不出的名字按支持处理，provider 4xx 兜底。
///
/// 条目缺失 / 无 model_list（standalone）→ 默认放行。
pub fn resolve_active_vision(cfg: &serde_json::Value, active_alias: &str) -> VisionResolution {
    let entry = cfg
        .get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        });
    let Some(entry) = entry else {
        return vision_default_allow();
    };

    // 1) 用户钉死。
    // L8（2026-09-04 四轮盲审）：钉死值宽容解析——手改 config 用大写
    // （"Yes"/"NO"）或 JSON 布尔（`"vision": true`）不该被静默忽略落回
    // 名字识别/默认放行（用户明确表达了意图）。
    match entry.get("vision").map(parse_vision_pin) {
        Some(Some(true)) => {
            return VisionResolution {
                supported: true,
                source: VisionSource::User,
            };
        }
        Some(Some(false)) => {
            return VisionResolution {
                supported: false,
                source: VisionSource::User,
            };
        }
        _ => {}
    }
    // 2) 探针实测（D9）。
    if let Some(measured) = entry.get("vision_probe").and_then(|v| v.as_bool()) {
        return VisionResolution {
            supported: measured,
            source: VisionSource::Probe,
        };
    }
    // 3) 名字关键词（只判支持）。
    let name_fields = [
        entry.get("model_name").and_then(|v| v.as_str()),
        entry.get("model").and_then(|v| v.as_str()),
        entry.get("real_name").and_then(|v| v.as_str()),
    ];
    if name_fields
        .into_iter()
        .flatten()
        .any(detect_vision_from_name)
    {
        return VisionResolution {
            supported: true,
            source: VisionSource::Name,
        };
    }
    // 4) 默认放行。
    vision_default_allow()
}

/// Resolve the display model id (`provider/name`, e.g. `deepseek/deepseek-v4-flash`)
/// for the active model alias, by looking up `model_list[]` for the matching
/// entry (by `model_name` or `model`) and returning its `model` field. Falls
/// back to `active_alias` itself when config is unavailable or no entry matches.
///
/// Used by the web channel to render a per-message "供应商·模型名" badge. Pure
/// (no IO) so it's unit-testable; `AgentLoop::current_display_model` reads
/// config.json fresh each call and hands the parsed value here.
pub fn resolve_display_model(cfg: &serde_json::Value, active_alias: &str) -> String {
    cfg.get("model_list")
        .and_then(|v| v.as_array())
        .and_then(|arr| {
            arr.iter().find(|m| {
                let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
                let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
                name == active_alias || full == active_alias
            })
        })
        .and_then(|m| m.get("model").and_then(|v| v.as_str()).map(String::from))
        .unwrap_or_else(|| active_alias.to_string())
}

fn detect_tier_from_keywords(name: &str) -> Option<ModelTier> {
    let l = name.to_lowercase();
    // Cloud flagships / known-strong (no size marker needed).
    let big_markers = [
        "gpt-4",
        "gpt-5",
        "gpt-4o",
        "o1-",
        "o3-",
        "o4-",
        "claude-opus",
        "claude-sonnet",
        "claude-3",
        "claude-4",
        "gemini-1.5-pro",
        "gemini-2",
        "gemini-3",
        "deepseek-v3",
        "deepseek-r1",
        "deepseek-chat",
        "deepseek3",
        "grok-2",
        "grok-3",
        "grok-4",
        "llama-3.1-405",
        "llama-3.3-70",
        "llama-4",
        "qwen3-235",
        "qwen2.5-72",
        "qwen3-72",
        "mistral-large",
        "command-r-plus",
    ];
    if big_markers.iter().any(|m| l.contains(m)) {
        return Some(ModelTier::Big);
    }
    // Known-small local families — absent a size marker, assume small.
    let small_markers = [
        "llama-3-8",
        "llama-3.1-8",
        "llama-3.2-",
        "qwen2.5-",
        "qwen3-",
        "qwen-",
        "mistral-7",
        "mistral-nemo",
        "gemma-",
        "gemma2-",
        "phi-",
        "phi3",
        "tinyllama",
        "yi-6",
        "yi-9",
        "internlm",
        "chatglm3",
        "glm-edge",
    ];
    if small_markers.iter().any(|m| l.contains(m)) {
        return Some(ModelTier::Mini);
    }
    None
}

/// Tool names exposed to the model at each tier (small-model-tool-robustness
/// plan, Phase 3). An empty slice means "no filtering" — Tier A (Big) and
/// unresolved Auto see the full toolset. Tier C (Mini) sees a core 13; Tier B
/// (Normal) a mid ~23 set. Tools not present at runtime are simply skipped.
///
/// Sixth-batch sweep: the CLI-delegation tools (`claude_code`, `codex_delegate`)
/// are Normal-tier and above. Delegation means composing a self-contained task
/// for another agent — a Mini-class model gets better results doing the work
/// with its core tools than mis-scoping a delegation prompt. Big/Auto are
/// unaffected (empty slice = full toolset).
/// L1 (U19): `lsp` likewise Normal+ — it needs 4 exact parameters (enum op +
/// 0-based UTF-16 line/character), which Mini-class models fumble.
pub fn tier_allowed_tools(tier: ModelTier) -> &'static [&'static str] {
    match tier {
        ModelTier::Mini => &[
            "message",
            "read_file",
            "write_file",
            "edit_file",
            "list_dir",
            "exec",
            "exec_async",
            "grep",
            "git",
            "web_fetch",
            "memory_search",
            "cli_reference",
            "cron",
        ],
        ModelTier::Normal => &[
            "message",
            "read_file",
            "write_file",
            "edit_file",
            "append_file",
            "delete_file",
            "list_dir",
            "create_dir",
            "delete_dir",
            "exec",
            "exec_async",
            "grep",
            "git",
            "web_fetch",
            "memory_search",
            "memory_list",
            "cli_reference",
            "cron",
            "sleep",
            "skills_list",
            "skills_info",
            "mcp_list",
            "workflow_run",
            "claude_code",
            "codex_delegate",
            "lsp",
        ],
        ModelTier::Big | ModelTier::Auto => &[],
    }
}

#[cfg(test)]
mod tests;
