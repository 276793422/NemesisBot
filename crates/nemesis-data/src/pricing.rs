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
//! Cost formula (usage-pricing plan; 2026-10-20 口径统一后语义):
//!
//! ```text
//! total_cost_usd =
//!     input * input_price                       (非缓存纯输入)
//!   + output * output_price
//!   + cache_creation * cache_creation_price
//!   + cache_read * cache_read_price             (all prices per million tokens)
//! ```
//!
//! `input_tokens` is the **non-cached plain input** (Anthropic-style
//! semantics) — the three outbound lanes normalize before feeding the
//! ledger; cache read/write are priced in their own columns and are never
//! re-charged at the plain-input rate. Two distinct token口径 share this
//! module:
//!
//! - **计费口径** = the four columns above (each priced separately);
//! - **选档口径** = full prompt length for `price_tiers` threshold
//!   matching, computed by [`tier_selection_tokens`] (= plain input +
//!   cache creation + cache read) — cache tokens are real context and do
//!   trigger long-context tiers.
//!
//! Providers that report cache tokens separately are handled gracefully by
//! clamping the plain-input portion at zero. Models missing from the table
//! degrade to `0.0` — cost is observability data, never guess.

use std::collections::HashMap;
use std::sync::OnceLock;

use crate::models::ModelPricing;

/// Compile-time embedded price table (written by `build.rs`; LiteLLM shape).
/// pricing-embed 关时不编译（build.rs 同步跳过 OUT_DIR 产物生成）。
#[cfg(feature = "pricing-embed")]
static PRICES_JSON: &str = include_str!(concat!(env!("OUT_DIR"), "/model_prices_embedded.json"));

/// Provenance of the embedded table (injected by `build.rs`).
#[cfg(feature = "pricing-embed")]
static EMBED_SOURCE: &str = env!("NEMESIS_PRICES_EMBED_SOURCE");
/// pricing-embed 关的诚实来源标记（CLI / usage API 原样展示）。
#[cfg(not(feature = "pricing-embed"))]
const EMBED_SOURCE: &str = "not embedded (pricing-embed off)";

/// In-memory lookup index over the embedded pricing entries.
pub struct PricingTable {
    entries: Vec<ModelPricing>,
    /// `model_id` → index into `entries`.
    by_id: HashMap<String, usize>,
    /// alias → index into `entries`.
    by_alias: HashMap<String, usize>,
}

impl PricingTable {
    #[cfg(feature = "pricing-embed")]
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

    /// pricing-embed 关的空表（lookup 恒 None → cost 恒 0.0 诚实降级）。
    #[cfg(not(feature = "pricing-embed"))]
    fn empty() -> Self {
        Self {
            entries: Vec::new(),
            by_id: HashMap::new(),
            by_alias: HashMap::new(),
        }
    }

    /// The process-wide embedded table (parsed once).
    pub fn embedded() -> &'static PricingTable {
        static TABLE: OnceLock<PricingTable> = OnceLock::new();
        #[cfg(feature = "pricing-embed")]
        return TABLE.get_or_init(PricingTable::from_embedded_json);
        #[cfg(not(feature = "pricing-embed"))]
        return TABLE.get_or_init(PricingTable::empty);
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
///
/// `input_tokens` 语义（账本不变量，2026-10-20 修复；R2 2026-10-22 补全
/// 口径注记）：**非缓存纯输入**（Anthropic 语义）。三条出站 lane 的
/// provider 层统一归一后喂入；缓存读写单列各自计价。合并形态的账本聚合
/// = `SUM(input_tokens + cache_creation_tokens + cache_read_tokens)`
/// （R2 前漏 creation 列）；分列聚合（UsageSummary / TrendPoint /
/// daily_rollups）三列各自独立，消费方按需相加。此前「input 含缓存
/// 总量、公式内做减法」的旧语义已废弃。
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

/// 阶梯选档口径的单一真相源（F3，2026-10-21）：`min_prompt_tokens` 的语义
/// 是**完整 prompt 长度**（含缓存部分——缓存读写就是真实上下文，长会话的
/// 阶梯价正该由它触发），在当前统一口径（`input_tokens` = 非缓存纯输入）
/// 下 = 非缓存输入 + 缓存创建 + 缓存读取。各分量防御性钳 0 + 饱和加法
/// （上游报数异常不 panic、不产生负长度）。**计费口径**仍按三类各自分列
/// （缓存不按普通输入价重复计费）——选档看完整长度，计费按列。
pub fn tier_selection_tokens(
    input_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> i64 {
    input_tokens
        .max(0)
        .saturating_add(cache_creation_tokens.max(0))
        .saturating_add(cache_read_tokens.max(0))
}

/// Prompt-length 阶梯解析（L3，2026-10-12 内部专项 T0）：`prompt_tokens`
/// 达到某档 `min_prompt_tokens` 即该档生效；多条命中取阈值最大的一档；
/// 档内 `None` 价目字段继承基础价。**单一真相源**——`cost_from_pricing`
/// 与 `cost_breakdown_from_pricing` 共同消费，两条口径永不分叉。
///
/// `prompt_tokens` = 完整 prompt 长度（[`tier_selection_tokens`] 的产物，
/// 含缓存），非账本的 `input_tokens`（非缓存纯输入）——两口径勿混。
fn effective_prices(p: &ModelPricing, prompt_tokens: i64) -> EffectivePrices {
    let Some(tiers) = &p.price_tiers else {
        return EffectivePrices {
            input: p.input_cost_per_million,
            output: p.output_cost_per_million,
            cache_read: p.cache_read_cost_per_million,
            cache_creation: p.cache_creation_cost_per_million,
        };
    };
    // 命中 = prompt_tokens >= min_prompt_tokens；多条命中取阈值最大的一档
    // （max_by_key 平票取后者，但等阈值档语义上应同价——取哪条都一致）。
    let tier = tiers
        .iter()
        .filter(|t| prompt_tokens >= t.min_prompt_tokens)
        .max_by_key(|t| t.min_prompt_tokens);
    match tier {
        None => EffectivePrices {
            input: p.input_cost_per_million,
            output: p.output_cost_per_million,
            cache_read: p.cache_read_cost_per_million,
            cache_creation: p.cache_creation_cost_per_million,
        },
        Some(t) => EffectivePrices {
            input: t.input_cost_per_million.unwrap_or(p.input_cost_per_million),
            output: t
                .output_cost_per_million
                .unwrap_or(p.output_cost_per_million),
            cache_read: t
                .cache_read_cost_per_million
                .unwrap_or(p.cache_read_cost_per_million),
            cache_creation: t
                .cache_creation_cost_per_million
                .unwrap_or(p.cache_creation_cost_per_million),
        },
    }
}

/// [`effective_prices`] 的解析产物（四列价目，已含继承回填）。
struct EffectivePrices {
    input: f64,
    output: f64,
    cache_read: f64,
    cache_creation: f64,
}

/// Pure cost math for a known pricing entry (unit-testable without the table).
///
/// `input_tokens` = 非缓存纯输入（见 [`compute_cost_usd`] 语义注记）；
/// **四列全部**防御性钳 0（账本不变量：上游报数异常——provider 响应的
/// 负 token 数——不得产生负成本；`tier_selection_tokens` 同款纪律）。
pub fn cost_from_pricing(
    p: &ModelPricing,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> f64 {
    // 选档 = 完整 prompt 长度（缓存参与）；计费列三类各自分列。
    let prices = effective_prices(
        p,
        tier_selection_tokens(input_tokens, cache_creation_tokens, cache_read_tokens),
    );
    let plain_input = input_tokens.max(0);
    let output = output_tokens.max(0);
    let cache_creation = cache_creation_tokens.max(0);
    let cache_read = cache_read_tokens.max(0);
    (plain_input as f64 * prices.input
        + output as f64 * prices.output
        + cache_creation as f64 * prices.cache_creation
        + cache_read as f64 * prices.cache_read)
        / 1_000_000.0
}

/// [`cost_from_pricing`] 的分项版：同一公式按列拆开，`total` = 分项之和
/// （浮点上直接相加，不做二次舍入——明细表四分项之和恒等于 total）。
/// 阶梯解析与总价版同源（[`effective_prices`]），命中档时分项列按阶梯价。
pub fn cost_breakdown_from_pricing(
    p: &ModelPricing,
    input_tokens: i64,
    output_tokens: i64,
    cache_creation_tokens: i64,
    cache_read_tokens: i64,
) -> crate::CostBreakdown {
    // 选档与总价版同源：完整 prompt 长度（缓存参与），见 [`tier_selection_tokens`]。
    // 四列防御性钳 0（与 [`cost_from_pricing`] 同一不变量：分项与 total
    // 同钳，四分项之和恒等于 total 的形态保持）。
    let prices = effective_prices(
        p,
        tier_selection_tokens(input_tokens, cache_creation_tokens, cache_read_tokens),
    );
    let plain_input = input_tokens.max(0);
    let output = output_tokens.max(0);
    let cache_creation = cache_creation_tokens.max(0);
    let cache_read = cache_read_tokens.max(0);
    let input_cost_usd = plain_input as f64 * prices.input / 1_000_000.0;
    let output_cost_usd = output as f64 * prices.output / 1_000_000.0;
    let cache_creation_cost_usd = cache_creation as f64 * prices.cache_creation / 1_000_000.0;
    let cache_read_cost_usd = cache_read as f64 * prices.cache_read / 1_000_000.0;
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

// 覆盖率补充测试（bare-suffix alias 第 4 级匹配）。
#[cfg(test)]
mod cov_tests;

// Wave6B 覆盖率收尾：见 cov_wave6b_tests.rs 头注释。
#[cfg(test)]
mod cov_wave6b_tests;

// L3（2026-10-12 内部专项 T0）：prompt-length 阶梯价解析/计价/透传测试。
#[cfg(test)]
mod tier_tests;
