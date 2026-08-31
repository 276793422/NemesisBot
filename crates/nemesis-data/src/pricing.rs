//! Static model pricing table (LiteLLM-derived, compile-time embedded).
//!
//! The table is extracted from LiteLLM's
//! `model_prices_and_context_window.json` (~36 mainstream models covering
//! OpenAI / Anthropic / Google / DeepSeek / GLM / Kimi / Qwen / Grok /
//! Mistral) and embedded at compile time from
//! `assets/model_prices.json`. Extraction date and source are recorded in
//! the JSON itself; refresh by re-running the extraction against an updated
//! LiteLLM table.
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

use serde::Deserialize;

use crate::models::ModelPricing;

/// Compile-time embedded price table (extracted from LiteLLM; see
/// `assets/model_prices.json` for the source/date header).
static PRICES_JSON: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/model_prices.json"));

#[derive(Deserialize)]
struct RawTable {
    #[serde(default)]
    models: Vec<RawEntry>,
}

#[derive(Deserialize)]
struct RawEntry {
    /// Entry payload incl. `aliases` (flattened from the JSON).
    #[serde(flatten)]
    pricing: ModelPricing,
}

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
        let raw: RawTable =
            serde_json::from_str(PRICES_JSON).expect("embedded model_prices.json is valid JSON");
        let mut entries = Vec::with_capacity(raw.models.len());
        let mut by_id = HashMap::new();
        let mut by_alias = HashMap::new();
        for entry in raw.models {
            let idx = entries.len();
            by_id.insert(entry.pricing.model_id.clone(), idx);
            for alias in &entry.pricing.aliases {
                by_alias.insert(alias.clone(), idx);
            }
            entries.push(entry.pricing);
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

/// Look up in the embedded table (convenience wrapper).
pub fn lookup_pricing(model: &str) -> Option<&'static ModelPricing> {
    PricingTable::embedded().lookup(model)
}

#[cfg(test)]
mod tests;
