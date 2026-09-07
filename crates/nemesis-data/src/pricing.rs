//! Static model pricing table (LiteLLM-derived, compile-time embedded).
//!
//! The table is embedded at compile time from
//! `$OUT_DIR/model_prices_embedded.json` — written by `build.rs`:
//!
//! - default (offline) build: bundled snapshot
//!   `assets/model_prices_litellm.json` (LiteLLM chat/completion entries
//!   filtered down to priced ones + `assets/model_prices_extras.json`
//!   hand-curated bare-name entries merged on top);
//! - `NEMESIS_PRICES_REFRESH=<value>` (set by the official build scripts,
//!   value changes per build so caches don't skip it): fresh download over
//!   the mirror chain, same filter+merge, snapshot fallback on any failure.
//!
//! Format is the raw LiteLLM shape (per-token map) — the same format the
//! runtime download layer uses, so `parse_litellm_json` is the single
//! parsing path for both. `embedded_source()` reports which variant was
//! compiled in (CLI / API display it verbatim).
//!
//! Cost formula (usage-pricing plan):
//!
//! ```text
//! total_cost_usd =
//!     (input - cache_creation - cache_read) * input_price
//!   + output * output_price
//!   + cache_creation * cache_creation_price
//!   + cache_read * cache_read_price        (all prices per million tokens)
//! ```
//!
//! The formula assumes cache tokens are reported as part of `input_tokens`
//! (OpenAI-style normalization, which is what `nemesis-providers` exposes).
//! Providers that report cache tokens separately are handled gracefully by
//! clamping the plain-input portion at zero. Models missing from the table
//! degrade to `0.0` — cost is observability data, never guess.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::models::ModelPricing;

/// Compile-time embedded price table (written by `build.rs`; LiteLLM shape).
static PRICES_JSON: &str =
    include_str!(concat!(env!("OUT_DIR"), "/model_prices_embedded.json"));

/// Provenance of the embedded table (injected by `build.rs`).
static EMBED_SOURCE: &str = env!("NEMESIS_PRICES_EMBED_SOURCE");

/// In-memory lookup index over the embedded pricing entries.
pub struct PricingTable {
    entries: Vec<ModelPricing>,
    /// `model_id` → index into `entries`.
    by_id: HashMap<String, usize>,
    /// alias → index into `entries`.
    by_alias: HashMap<String, usize>,
}

impl PricingTable {
    fn from_embedded_json() -> Self {
        // 内嵌表与下载层同格式同解析（parse_litellm_json 单一路径）；失败
        // 只可能是 build.rs 产出了坏文件——编译期即panic暴露，不留到运行时。
        let parsed = crate::parse_litellm_json(PRICES_JSON)
            .expect("embedded model_prices_embedded.json is valid");
        let mut entries = Vec::with_capacity(parsed.len());
        let mut by_id = HashMap::new();
        let mut by_alias = HashMap::new();
        for entry in parsed {
            let idx = entries.len();
            by_id.insert(entry.model_id.clone(), idx);
            for alias in &entry.aliases {
                by_alias.insert(alias.clone(), idx);
            }
            entries.push(entry);
        }
        Self {
            entries,
            by_id,
            by_alias,
        }
    }

    /// The process-wide embedded table (parsed once).
    pub fn embedded() -> &'static PricingTable {
        static TABLE: OnceLock<PricingTable> = OnceLock::new();
        TABLE.get_or_init(PricingTable::from_embedded_json)
    }

    /// Look up pricing for a configured model name.
    ///
    /// Matching order:
    /// 1. exact `model_id` match;
    /// 2. exact alias match;
    /// 3. bare suffix after the last `/` against `model_id` (handles our
    ///    `provider/model` config names — `deepseek/deepseek-chat` →
    ///    `deepseek-chat`, `zhipu/glm-4.7` → `glm-4.7`);
    /// 4. bare suffix against aliases.
    pub fn lookup(&self, model: &str) -> Option<&ModelPricing> {
        let m = model.trim();
        if m.is_empty() {
            return None;
        }
        if let Some(&i) = self.by_id.get(m) {
            return Some(&self.entries[i]);
        }
        if let Some(&i) = self.by_alias.get(m) {
            return Some(&self.entries[i]);
        }
        let bare = m.rsplit('/').next().unwrap_or(m);
        if let Some(&i) = self.by_id.get(bare) {
            return Some(&self.entries[i]);
        }
        if let Some(&i) = self.by_alias.get(bare) {
            return Some(&self.entries[i]);
        }
        None
    }

    /// All embedded entries (for the `/api/usage/pricing` endpoint).
    pub fn entries(&self) -> &[ModelPricing] {
        &self.entries
    }
}

/// Compute `total_cost_usd` for one LLM request against the embedded table.
/// Unknown model → `0.0` (degrade, never guess).
pub fn compute_cost_usd(
    model: &str,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> f64 {
    let Some(p) = PricingTable::embedded().lookup(model) else {
        return 0.0;
    };
    cost_from_pricing(
        p,
        input_tokens,
        output_tokens,
        cache_creation_tokens,
        cache_read_tokens,
    )
}

/// Pure cost math for a known pricing entry (unit-testable without the table).
pub fn cost_from_pricing(
    p: &ModelPricing,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> f64 {
    // Plain input = total input minus cached portions, clamped at zero for
    // providers that report cache tokens separately from prompt tokens.
    let plain_input = (input_tokens - cache_creation_tokens - cache_read_tokens).max(0);
    (plain_input as f64 * p.input_cost_per_million
        + output_tokens as f64 * p.output_cost_per_million
        + cache_creation_tokens as f64 * p.cache_creation_cost_per_million
        + cache_read_tokens as f64 * p.cache_read_cost_per_million)
        / 1_000_000.0
}

/// [`cost_from_pricing`] 的分项版：同一公式按列拆开，`total` = 分项之和
/// （浮点上直接相加，不做二次舍入——明细表四分项之和恒等于 total）。
pub fn cost_breakdown_from_pricing(
    p: &ModelPricing,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> crate::CostBreakdown {
    let plain_input = (input_tokens - cache_creation_tokens - cache_read_tokens).max(0);
    let input_cost_usd = plain_input as f64 * p.input_cost_per_million / 1_000_000.0;
    let output_cost_usd = output_tokens as f64 * p.output_cost_per_million / 1_000_000.0;
    let cache_creation_cost_usd =
        cache_creation_tokens as f64 * p.cache_creation_cost_per_million / 1_000_000.0;
    let cache_read_cost_usd =
        cache_read_tokens as f64 * p.cache_read_cost_per_million / 1_000_000.0;
    crate::CostBreakdown {
        pricing_model: p.model_id.clone(),
        input_cost_usd,
        output_cost_usd,
        cache_creation_cost_usd,
        cache_read_cost_usd,
        total_cost_usd: input_cost_usd
            + output_cost_usd
            + cache_creation_cost_usd
            + cache_read_cost_usd,
    }
}

/// All embedded entries (convenience wrapper for handlers).
pub fn all_pricing() -> &'static [ModelPricing] {
    PricingTable::embedded().entries()
}

/// Which variant of the embedded table was compiled in（快照 vs 构建期下
/// 载，含来源 URL / 日期 / 条目数）。CLI 与 usage API 原样展示——诚实
/// 来源标记，由 `build.rs` 经 `NEMESIS_PRICES_EMBED_SOURCE` 注入。
pub fn embedded_source() -> &'static str {
    EMBED_SOURCE
}

/// Look up in the embedded table (convenience wrapper).
pub fn lookup_pricing(model: &str) -> Option<&'static ModelPricing> {
    PricingTable::embedded().lookup(model)
}

#[cfg(test)]
mod tests;
