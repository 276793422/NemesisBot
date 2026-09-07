//! Data storage layer for usage statistics.
//!
//! SQLite-backed storage for LLM request logs, daily rollups, and model pricing.
//! Database file: `{workspace}/data/nemesisbot_data.db`

mod db;
mod models;
mod pricing;
mod pricing_filter;
mod pricing_lite;
mod pricing_store;
mod usage_store;
pub mod watcher;

pub use models::{
    CostBreakdown, DailyRollup, LogFilter, ModelPricing, RequestLog, SessionUsageAgg, TrendPoint,
    UsageSummary,
};
pub use pricing::{
    PricingTable, all_pricing, compute_cost_usd, cost_breakdown_from_pricing, cost_from_pricing,
    embedded_source, lookup_pricing,
};
// 过滤/合并/镜像链公开导出：build.rs 经 #[path] 共享 pricing_filter.rs；
// 集成测试经这组导出走与 build.rs 完全相同的代码路径。
pub use pricing_filter::{
    PRICE_MIRROR_URLS, filter_litellm_table, merge_extras, validate_filtered_table,
};
pub use pricing_lite::{LITELLM_PRICE_URL, parse_litellm_json};
pub use pricing_store::{PricingMeta, PricingStore};
pub use usage_store::DataStore;
