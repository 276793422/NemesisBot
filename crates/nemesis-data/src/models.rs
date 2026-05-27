//! Data models for usage statistics.

use serde::{Deserialize, Serialize};

/// Single LLM request log entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RequestLog {
    pub id: i64,
    pub trace_id: String,
    pub model: String,
    pub provider_type: String,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_cost_usd: f64,
    pub latency_ms: i64,
    pub status_code: i32,
    pub error_message: Option<String>,
    pub is_streaming: bool,
    pub created_at: i64,
}

/// Daily aggregated statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DailyRollup {
    pub date: String,
    pub model: String,
    pub request_count: i64,
    pub success_count: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
}

/// Model pricing entry (reserved, not used yet).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
}

/// Aggregated usage summary for a time range.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct UsageSummary {
    pub total_requests: i64,
    pub success_count: i64,
    pub total_input_tokens: i64,
    pub total_output_tokens: i64,
    pub total_cache_creation_tokens: i64,
    pub total_cache_read_tokens: i64,
    pub total_cost_usd: f64,
    pub avg_latency_ms: f64,
    pub cache_hit_rate: f64,
}

/// A single point in a trend chart.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TrendPoint {
    /// Time bucket label (ISO 8601 or formatted string).
    pub label: String,
    /// Unix timestamp of the bucket start.
    pub timestamp: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub cache_creation_tokens: i64,
    pub cache_read_tokens: i64,
    pub request_count: i64,
    pub total_cost_usd: f64,
}
