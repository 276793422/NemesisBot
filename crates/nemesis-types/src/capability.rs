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
        ],
        ModelTier::Big | ModelTier::Auto => &[],
    }
}

#[cfg(test)]
mod tests;
