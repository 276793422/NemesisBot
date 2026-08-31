//! Data storage layer for usage statistics.
//!
//! SQLite-backed storage for LLM request logs, daily rollups, and model pricing.
//! Database file: `{workspace}/data/nemesisbot_data.db`

mod db;
mod models;
mod pricing;
mod pricing_lite;
mod pricing_store;
mod usage_store;
pub mod watcher;

pub use models::{CostBreakdown, DailyRollup, LogFilter, ModelPricing, RequestLog, TrendPoint, UsageSummary};
pub use pricing::{
    all_pricing, compute_cost_usd, cost_breakdown_from_pricing, cost_from_pricing, lookup_pricing,
    PricingTable,
};
pub use pricing_lite::{parse_litellm_json, LITELLM_PRICE_URL};
pub use pricing_store::{PricingMeta, PricingStore};
pub use usage_store::DataStore;
