//! LiteLLM 价目表解析器（A2 在线更新，2026-08-31）。
//!
//! LiteLLM `model_prices_and_context_window.json` 是价目表在线更新的事实
//! 标准源（3,400+ 条 / MIT / raw.githubusercontent 直拉）。它的形状是
//! `model_name → {input_cost_per_token, output_cost_per_token, ...}`
//! 的扁平 map，**per-token 计价**——本项目 [`ModelPricing`] 是 per-million，
//! 转换时 ×1e6。
//!
//! 解析纪律（遵循 `feedback_verify_disk_format` 纪律）：
//! - 只收 `mode == "chat"`（completion 并入 chat 语义；embedding/audio/
//!   image/moderation/rerank 与本项目的 token 计价模型不匹配，跳过）；
//! - `input_cost_per_token` / `output_cost_per_token` 缺失或非数值 → 跳过
//!   该条（成本可观测性永不猜测——对齐 pricing.rs 的 degrade 原则）；
//! - cache 两项缺失 → 0.0（大量模型没有独立 cache 价）；
//! - `max_input_tokens` / `max_tokens`（LiteLLM 的输出上限字段）缺失 → None；
//! - 非 chat 之外的未知字段一律忽略（LiteLLM 每个版本都会加字段，宽容
//!   解析保证旧解析器吃新表不炸）。

use serde::Deserialize;

use crate::models::ModelPricing;

/// LiteLLM 表的默认下载地址（raw 直拉；ETag 增量）。运行时可通过
/// CLI `--url` / WSAPI `url` 参数覆盖（镜像场景——受限网络下指向可达的
/// 镜像端点）。
pub const LITELLM_PRICE_URL: &str =
    "https://raw.githubusercontent.com/BerriAI/litellm/main/model_prices_and_context_window.json";

#[derive(Deserialize)]
struct LiteLLMEntry {
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    input_cost_per_token: Option<f64>,
    #[serde(default)]
    output_cost_per_token: Option<f64>,
    #[serde(default)]
    cache_read_input_token_cost: Option<f64>,
    #[serde(default)]
    cache_creation_input_token_cost: Option<f64>,
    #[serde(default)]
    max_input_tokens: Option<i64>,
    #[serde(default)]
    max_tokens: Option<i64>,
    #[serde(default)]
    litellm_provider: Option<String>,
}

/// 解析 LiteLLM 价目表 JSON → 本项目 [`ModelPricing`] 列表。
///
/// `raw` 必须是顶层 `map<string, entry>`；解析出 **0 条** chat 条目也报错
/// ——真表有数千条，0 条 = 传错文件（如我们自己的 `{models: [...]}` 形状
/// 会被宽容反序列化成全默认条目再全被跳过），由调用方降级保留旧表。
pub fn parse_litellm_json(raw: &str) -> Result<Vec<ModelPricing>, String> {
    let map: std::collections::BTreeMap<String, LiteLLMEntry> =
        serde_json::from_str(raw).map_err(|e| format!("LiteLLM price table parse failed: {e}"))?;

    let mut out = Vec::new();
    for (name, e) in map {
        // 只收 chat/completion；embedding 等按 token 计价语义不符。
        match e.mode.as_deref() {
            Some("chat") | Some("completion") => {}
            _ => continue,
        }
        // 双基础价齐全才收（缺任一 = 无法诚实计价）。
        let (Some(input), Some(output)) = (e.input_cost_per_token, e.output_cost_per_token) else {
            continue;
        };
        if !(input.is_finite() && output.is_finite()) {
            continue;
        }
        let provider = e.litellm_provider.unwrap_or_default();
        out.push(ModelPricing {
            // model_id 保留 LiteLLM 原名（lookup 的 bare-suffix 匹配天然
            // 处理我们的 `provider/model` 配置名）。
            model_id: name,
            display_name: provider,
            input_cost_per_million: input * 1_000_000.0,
            output_cost_per_million: output * 1_000_000.0,
            cache_read_cost_per_million: e.cache_read_input_token_cost.unwrap_or(0.0) * 1_000_000.0,
            cache_creation_cost_per_million: e.cache_creation_input_token_cost.unwrap_or(0.0)
                * 1_000_000.0,
            max_input_tokens: e.max_input_tokens,
            max_output_tokens: e.max_tokens,
            // LiteLLM 顶层键即权威名；同义别名靠 lookup 的后缀匹配兜底。
            aliases: Vec::new(),
        });
    }
    // 0 条 = 传错文件/形状不对（真表数千条）——报错让调用方降级保留旧表。
    if out.is_empty() {
        return Err(
            "LiteLLM price table parsed to 0 chat entries — wrong file or shape?".to_string(),
        );
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
