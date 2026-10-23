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
    ///
    /// Big 1→2（P2C，2026-09-12 双端真机 NB-15 根修）：并行工具调用批
    /// （单次 LLM 响应多个 tool_calls）已成常态，批内每个畸形调用各 +1——
    /// budget=1 时首个坏调用即停轮，模型从未见到结构化错误回灌就终结
    /// （NB-15：glm 单响应 2 个 exec 调用全错，第一发就烧光预算）。2 给
    /// 强模型一次「看到错误 → 下轮自纠」的机会；真正的死循环仍由
    /// turn_guard ⑥ 兜底，不会因放宽预算而空转。
    pub fn validation_retry_budget(self) -> u32 {
        match self {
            ModelTier::Mini => 3,
            ModelTier::Normal => 2,
            ModelTier::Big => 2,
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

/// `model_list` 条目定位（单一真相源，2026-10-20）：active 别名对 `model_name`
/// / `model` 字段的**精确**匹配优先；未命中回落 **bare-suffix**——`model`
/// 字段末段与 active 别名末段相等（`"glm-4.7"` ↔ `"zhipu/glm-4.7"`）。
///
/// 为什么需要 bare-suffix：活 gateway 的 active_model 是剥前缀裸名
/// （provider_resolver `resolve_from_model_config` 对含 '/' 的 model 切尾段；
/// CLI `model add --default` 写的 `agents.defaults.llm` 也是裸名，两条路在此
/// 汇合）。自定义别名条目（`model_name` ≠ 裸名，Dashboard 加模型表单允许）
/// 在纯精确匹配下全族失联——per-model 键（tier / vision / context_window /
/// compact_context_tokens / …）静默回落默认。2026-10-20 真浏览器 E2E 实证：
/// `context_window: 131072` 被无视、压缩观测落价目表值。bare-suffix 语义与
/// `nemesis_data` 价目表 lookup 的第 3 级对齐；多条目同裸名按数组序先到先得
/// （与精确匹配、价目表一致）。
pub(crate) fn find_model_entry<'a>(
    cfg: &'a serde_json::Value,
    active_alias: &str,
) -> Option<&'a serde_json::Value> {
    let arr = cfg.get("model_list")?.as_array()?;
    let bare = active_alias.rsplit('/').next().unwrap_or(active_alias);
    let matches_exact = |m: &serde_json::Value| {
        let name = m.get("model_name").and_then(|v| v.as_str()).unwrap_or("");
        let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        name == active_alias || full == active_alias
    };
    let matches_bare = |m: &serde_json::Value| {
        let full = m.get("model").and_then(|v| v.as_str()).unwrap_or("");
        full.rsplit('/').next().unwrap_or(full) == bare
    };
    arr.iter()
        .find(|m| matches_exact(m))
        .or_else(|| arr.iter().find(|m| matches_bare(m)))
}

/// Resolve the capability tier for the active model in a `config.json` Value.
///
/// Looks up `model_list[]` for the entry matching `active_alias` (by
/// `model_name` or `model`, bare-suffix 回落见 [`find_model_entry`]), reads its
/// `model_tier` (default `Auto`), and resolves via the heuristic. Returns `Big`
/// if the model can't be found.
pub fn resolve_active_tier(cfg: &serde_json::Value, active_alias: &str) -> ModelTier {
    let Some(entry) = find_model_entry(cfg, active_alias) else {
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
    find_model_entry(cfg, active_alias)?
        .get("max_output_tokens")
        .and_then(|v| v.as_i64())
}

/// M6-1（2026-10-10 机制加固）：resolve `agents.defaults.max_tool_iterations`
/// from config.json. Mirrors the fresh-read resolver family
/// ([`resolve_max_output_tokens`] et al.): None = key absent / config not a
/// JSON object — caller falls back to the startup value. Negative values are
/// passed through unmolested; the consumer (run_loop organ 1) already maps
/// <=0 to "unlimited".
pub fn resolve_max_tool_iterations(cfg: &serde_json::Value) -> Option<i64> {
    cfg.get("agents")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("max_tool_iterations"))
        .and_then(|v| v.as_i64())
}

/// §6.2（2026-10-12 无人值守）：`agents.defaults.max_output_ceiling` 缺席
/// 时的默认封顶（256K——覆盖主流思考型模型单轮烧穿量级的稳妥上限）。
pub const MAX_OUTPUT_CEILING_DEFAULT: u64 = 262_144;

/// §6.2：翻倍阶梯封顶解析（nemesis-agent loop 翻倍阶梯的单一真相源）。
/// `agents.defaults.max_output_ceiling`，0 / 缺席 / 非法（负数、非数字）→
/// 默认 [`MAX_OUTPUT_CEILING_DEFAULT`]（0 不表示「不限」——翻倍阶梯没有
/// 不限形态，封顶失守 = 成本无界，惯例与 `max_tool_iterations` 0=不限 相反）。
pub fn resolve_max_output_ceiling(cfg: &serde_json::Value) -> u64 {
    cfg.get("agents")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("max_output_ceiling"))
        .and_then(|v| v.as_u64())
        .filter(|n| *n > 0)
        .unwrap_or(MAX_OUTPUT_CEILING_DEFAULT)
}

/// 主轮 max_tokens 最终回退值（条目 `max_output_tokens` 与
/// `agents.defaults.max_tokens` 均未声明时）。2026-10-23 P0：旧值 8192 是
/// 小模型时代遗产——思考型模型思考 token 计入 max_tokens + agent 大参数
/// 工具调用必然撞顶（docs/BUG/2026-10-23_max-tokens-8192截断撞顶复发.md，
/// 同根因 2026-08-10 已爆发过一次并预言复发）。64K 起步，条目仍可按
/// per-model `max_output_tokens` 覆盖。
pub const DEFAULT_MAX_TOKENS: i64 = 65_536;

/// `agents.defaults.max_tokens` 解析（主轮回退链第二级：条目
/// `max_output_tokens` > 本键 > [`DEFAULT_MAX_TOKENS`]）。缺席 / 0 / 负数
/// = 未声明（None），由调用方继续回退。
pub fn resolve_agents_default_max_tokens(cfg: &serde_json::Value) -> Option<i64> {
    cfg.get("agents")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("max_tokens"))
        .and_then(|v| v.as_i64())
        .filter(|n| *n > 0)
}

/// E5（2026-10-15 收尾保障）：`agents.defaults.pre_pause_probe.timeout_secs`
/// 缺席 / 0 / 非法时的默认墙钟（240s——热缓存 cargo check 数秒级，冷启动
/// 大仓留足余量；不设上限内「不限」形态，探针必须中止有界）。
pub const PRE_PAUSE_PROBE_DEFAULT_TIMEOUT_SECS: u64 = 240;

/// E5（2026-10-15 收尾保障）：预算暂停前编译探针的解析结果（单一真相源
/// [`resolve_pre_pause_probe`] 的返回类型）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PrePauseProbeSettings {
    /// 默认开（用户拍板 2026-10-15：诚实树状态注记是默认体验）。
    pub enabled: bool,
    /// 单次探针墙钟上限（秒）。0 / 非法 → 默认 240。
    pub timeout_secs: u64,
}

/// E5：`agents.defaults.pre_pause_probe` 解析（nemesis-agent loop 暂停路径
/// 的单一真相源）。对象缺席 = 默认开 + 默认墙钟；`enabled` 缺席 = 开
/// （只有显式 `false` 关闭）；`timeout_secs` 0 / 负数 / 非数字 → 默认。
pub fn resolve_pre_pause_probe(cfg: &serde_json::Value) -> PrePauseProbeSettings {
    let node = cfg
        .get("agents")
        .and_then(|v| v.get("defaults"))
        .and_then(|v| v.get("pre_pause_probe"));
    PrePauseProbeSettings {
        enabled: node
            .and_then(|v| v.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(true),
        timeout_secs: node
            .and_then(|v| v.get("timeout_secs"))
            .and_then(|v| v.as_u64())
            .filter(|n| *n > 0)
            .unwrap_or(PRE_PAUSE_PROBE_DEFAULT_TIMEOUT_SECS),
    }
}

/// H4 (U16 half): resolve the per-model `reasoning_effort` tier for the
/// active model alias. Mirrors [`resolve_max_output_tokens`]: finds the
/// matching `model_list[]` entry and returns its `reasoning_effort` string
/// ("off"|"low"|"medium"|"high"); None/empty → None (send nothing).
pub fn resolve_reasoning_effort(cfg: &serde_json::Value, active_alias: &str) -> Option<String> {
    let e = find_model_entry(cfg, active_alias)
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
    find_model_entry(cfg, active_alias)?
        .get("context_window")
        .and_then(|v| v.as_i64())
        .filter(|w| *w > 0)
}

/// Per-model 绝对压缩点（2026-10-20 压缩点一等公民化）：`compact_context_tokens`
/// 键——设了就「到点即压」（不乘窗口比例），解决「模型支持 1M 但要在 200K 压
/// 一次」的表达缺口。镜像 [`resolve_context_window`] 的形态；`None` = 未设
/// （调用方回落 窗口×75% 既有行为）。钳制（min(值, 真实窗口)）在组合层
/// （nemesis-agent `resolve_compact_trigger`）做——本函数只读原始键。
pub fn resolve_compact_context_tokens(cfg: &serde_json::Value, active_alias: &str) -> Option<i64> {
    find_model_entry(cfg, active_alias)?
        .get("compact_context_tokens")
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
    find_model_entry(cfg, active_alias)?
        .get("summarizer_prefix_reuse")
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
    let Some(entry) = find_model_entry(cfg, active_alias) else {
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
    find_model_entry(cfg, active_alias)
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
            // A7（2026-09-06）：批量编辑。Mini 不给——edits 数组 schema 对
            // 小模型是参数陷阱（数组内嵌对象），单文件场景 edit_file 已覆盖。
            "multiedit",
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
            // H1（2026-09-05）：todo 清单工具。Mini 不给——小模型把清单
            // 更新和任务执行混在一轮里容易烧轮次。
            "todowrite",
            // B4（2026-09-05）：后台进程三件套。Mini 不给——job_id 跨调用
            // 记账 + offset 分页读输出对 9-35B 模型是纯轮次消耗，exec 的
            // 600s cap 已覆盖小模型的绝大多数场景。
            "background_start",
            "background_output",
            "background_kill",
            // C8（2026-09-06）：构建/测试运行器。Mini 不给——长构建 + 聚焦
            // 回灌的检索（read_file spill）是小模型轮次陷阱；exec 已覆盖。
            "run_checks",
            // F7（2026-09-06）：结构化提问。Mini 不给——向用户发选项卡并
            // 阻塞等答是高信任交互，小模型容易在琐碎选择上滥用。
            "question",
            // 对话生成（2026-09-22）：工作流草稿两件套。Mini 不给——完整
            // DAG 定义生成是长结构输出，小模型 schema 错误率高，且草稿写入
            // 是写文件语义（与 write_file 同档考虑）。
            "workflow_create",
            "workflow_capabilities",
            // N2（2026-10-13）：计划审批卡。Mini 不给——与 question 同款
            // 高信任交互（且批准动作翻转 plan→build 模式），小模型在琐碎
            // 场景滥用审批卡会打断用户。
            "present_plan",
            // N5（2026-10-13）：交付物登记。Mini 不给——结果面产物指引需要
            // 判断「什么值得用户看」，小模型容易把中间产物全量上报稀释价值；
            // 误登记只污染展示面（无副作用），normal/big 放行。
            "present_files",
        ],
        ModelTier::Big | ModelTier::Auto => &[],
    }
}

/// LLM 协议选择器（2026-09-11）：模型条目 `protocol` 字段的 canonical 值集
/// 与归一。单一真相源——写入侧（dashboard models handler / CLI `model add
/// --protocol`）用它校验归一，factory 消费同值集钉死 wire 协议：
/// - `anthropic`（用户可写别名 `claude`）：Claude 消息协议（POST /v1/messages + x-api-key）
/// - `openai`（别名 `chat-completions`）：OpenAI 兼容（POST /chat/completions + Bearer）
/// - `responses`：OpenAI Responses API
/// - `images-openai`（集群专业职能框架 M4）：图像生成（POST /v1/images/generations，
///   b64_json）。**不是对话协议**——factory/默认模型槽/probe 对它 loud 拒绝；
///   唯一消费点 = `generate_image` 工具（`tools.image_gen` 配置）。
///
/// 空输入 = 清除（按 provider 前缀/模型名自动推断，现状行为）；未知值 loud 拒绝。
pub fn normalize_model_protocol(input: &str) -> Result<String, String> {
    match input.trim().to_lowercase().as_str() {
        "" => Ok(String::new()),
        "anthropic" | "claude" => Ok("anthropic".to_string()),
        "openai" | "chat-completions" => Ok("openai".to_string()),
        "responses" => Ok("responses".to_string()),
        "images-openai" => Ok("images-openai".to_string()),
        other => Err(format!(
            "unknown protocol '{other}'. Supported: anthropic | openai | responses | images-openai"
        )),
    }
}

/// 图像协议判定（M4 单一谓词）：四道守卫与 generate_image 消费点共用，
/// 防「图像条目混进对话模型槽」各处散写 `== "images-openai"`。
pub fn is_image_protocol(protocol: &str) -> bool {
    protocol.trim().eq_ignore_ascii_case("images-openai")
}

/// 模型角色声明（2026-10-13 N6）：模型条目 `role` 字段的 canonical 值集
/// 与归一。声明「这个模型是干什么用的」，让 default 对话模型槽的写入侧
/// 有客观依据拒绝错配条目（防杂务模型霸占主对话槽——曾用 `agents.defaults.llm`
/// 指向 8B flash 提速 /compact，首轮对话也跟着掉智）：
/// - `conversation`（缺省）：默认对话模型槽的合法形态（agent loop 主通道）
/// - `utility`：杂务通道（/compact 摘要、会话标题生成等 agents.small_model 消费）
/// - `classifier`：分类/路由/打标（对话形态，但绝不坐主对话槽）
///
/// 空输入 = conversation（缺省，老条目零迁移）；未知值 loud 拒绝。
/// 守卫语义与 images-openai 刻意不同：utility/classifier 是对话形态条目
/// （probe 走得通、messages 可收发），只是**不该当默认对话模型**——所以
/// 四道守卫里 probe 放行、三个 default 槽写入点（CLI --default、唯一模型
/// 自动坐槽、WSAPI models.set_default）+ factory 装配点 loud 拒绝/降级。
pub fn normalize_model_role(input: &str) -> Result<String, String> {
    match input.trim().to_lowercase().as_str() {
        "" | "conversation" => Ok("conversation".to_string()),
        "utility" => Ok("utility".to_string()),
        "classifier" => Ok("classifier".to_string()),
        other => Err(format!(
            "unknown model role '{other}'. Supported: conversation | utility | classifier"
        )),
    }
}

/// 对话角色判定（N6 单一谓词）：默认对话模型槽各守卫共用。
/// 空串（缺省）与 `conversation` 均为对话形态。
pub fn is_conversation_role(role: &str) -> bool {
    let r = role.trim().to_lowercase();
    r.is_empty() || r == "conversation"
}

#[cfg(test)]
mod tests;
