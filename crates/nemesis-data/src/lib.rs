//! Data storage layer for usage statistics.
//!
//! SQLite-backed storage for LLM request logs, daily rollups, and model pricing.
//! Database file: `{workspace}/data/nemesisbot_data.db`

mod db;
mod models;
mod usage_store;

pub use models::{DailyRollup, ModelPricing, RequestLog, TrendPoint, UsageSummary};
pub use usage_store::DataStore;
