//! Data models for usage statistics.

use serde::{Deserialize, Serialize};

/// Single LLM request log entry.
///
/// A3（2026-08-31）补明细字段：`pricing_model`（实际命中的价目条目名——
/// 配置名 `zhipu/glm-4.7` 与价目条目 `glm-4.7` 分离，排查计价命中用）、
/// 分项成本、`first_token_ms`（`None` = 未测量——provider trait 无流式
/// 通路）、`session_key`（会话过滤/聚合）。`latency_ms` 即该请求耗时
/// 真相源（不另设 duration_ms 列）。
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
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
    /// 实际计价命中的价目条目 model_id（空 = 未命中任何层）。
    #[serde(default)]
    pub pricing_model: String,
    /// 分项成本（usd）：plain input 部分。
    #[serde(default)]
    pub input_cost_usd: f64,
    #[serde(default)]
    pub output_cost_usd: f64,
    #[serde(default)]
    pub cache_creation_cost_usd: f64,
    #[serde(default)]
    pub cache_read_cost_usd: f64,
    /// 首 token 延迟（毫秒）。`None` = 未测量（非流式/旧数据/无流式通路）。
    #[serde(default)]
    pub first_token_ms: Option<i64>,
    /// 会话键（`direct-…` / `rpc:…` / cron / heartbeat 等），过滤与聚合用。
    #[serde(default)]
    pub session_key: String,
}

/// 请求明细查询过滤器（A3 请求明细 tab；时间范围/分页之外的正交条件）。
/// `None`/空串 = 不过滤该维度。`model` 与 `session_key` 做子串匹配
/// （LIKE %v%——排查时按片段定位比精确名更顺手），`status` 精确匹配。
#[derive(Debug, Clone, Default)]
pub struct LogFilter {
    pub model: Option<String>,
    pub status: Option<i32>,
    pub session_key: Option<String>,
}

impl LogFilter {
    /// 任一维度都没给（= 不加 WHERE 条件）。
    pub fn is_empty(&self) -> bool {
        self.model.as_deref().is_none_or(|s| s.is_empty())
            && self.status.is_none()
            && self.session_key.as_deref().is_none_or(|s| s.is_empty())
    }
}

/// 一次 LLM 请求的分项成本（分层查表命中后算出）。
///
/// `total` = 四分项之和（与 [`crate::cost_from_pricing`] 同一套公式拆列）。
/// `pricing_model` 是命中的价目条目名——排查「为什么这条计了 0 成本」时
/// 先看它是空（未命中）还是分项单价为 0（命中了免费条目）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct CostBreakdown {
    pub pricing_model: String,
    pub input_cost_usd: f64,
    pub output_cost_usd: f64,
    pub cache_creation_cost_usd: f64,
    pub cache_read_cost_usd: f64,
    pub total_cost_usd: f64,
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

/// Model pricing entry (embedded table; see `pricing.rs`).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPricing {
    pub model_id: String,
    pub display_name: String,
    pub input_cost_per_million: f64,
    pub output_cost_per_million: f64,
    pub cache_read_cost_per_million: f64,
    pub cache_creation_cost_per_million: f64,
    /// Max context window (input tokens). `None` = unknown.
    #[serde(default)]
    pub max_input_tokens: Option<i64>,
    /// Max output tokens. `None` = unknown.
    #[serde(default)]
    pub max_output_tokens: Option<i64>,
    /// Provider-qualified or historic names resolving to this entry.
    #[serde(default)]
    pub aliases: Vec<String>,
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
