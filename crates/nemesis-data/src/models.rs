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

/// Prompt-length 阶梯价单档（L3，2026-10-12 内部专项 T0）：`input_tokens`
/// 达到 `min_prompt_tokens` 后整套价目切换到本档；`None` 的价目字段继承
/// 基础价（部分覆盖场景——只调输出价、其余沿用）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PriceTier {
    /// 生效阈值：`input_tokens >= min_prompt_tokens` 即命中；多条命中取
    /// `min_prompt_tokens` 最大的一档（pi S3 语义：长上下文按段计价）。
    pub min_prompt_tokens: i64,
    /// 阶梯输入价（$/M）。`None` = 继承基础价。
    #[serde(default)]
    pub input_cost_per_million: Option<f64>,
    /// 阶梯输出价（$/M）。`None` = 继承基础价。
    #[serde(default)]
    pub output_cost_per_million: Option<f64>,
    /// 阶梯缓存读价（$/M）。`None` = 继承基础价。
    #[serde(default)]
    pub cache_read_cost_per_million: Option<f64>,
    /// 阶梯缓存写价（$/M）。`None` = 继承基础价。
    #[serde(default)]
    pub cache_creation_cost_per_million: Option<f64>,
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
    /// Prompt-length 阶梯价（L3）。`None`/缺省 = 单一价（旧行为，零迁移）。
    /// 解析语义见 [`PriceTier`]：命中档价目覆盖基础价，`None` 字段继承。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub price_tiers: Option<Vec<PriceTier>>,
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

/// 单会话的用量聚合（M5 会话级 cost/context 常驻）。
///
/// `input_tokens` = 完整输入口径（prompt + cache_creation + cache_read，
/// R2 2026-10-22 补全——prompt 列是 provider 非缓存纯输入，聚合 SQL 把
/// 缓存两列加回）；`requests` 是落库的 LLM 请求条数。
/// 会话无任何请求时全零（不是错误——`logs.session_usage` 对
/// 空/外部通道会话诚实返回零值）。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct SessionUsageAgg {
    pub requests: i64,
    pub input_tokens: i64,
    pub output_tokens: i64,
    pub total_cost_usd: f64,
}

/// 单模型的工具健康聚合（P2B `models.health`，2026-09-12 NB-15 根修配套）。
///
/// `failure_rate` = validation_failures / tool_calls（无调用 = 0.0）。
/// `hint` 是消费侧（WSAPI handler）按阈值追加的处置建议——数据层只出
/// 事实（计数），不出结论。
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct ModelToolHealth {
    pub model: String,
    pub tool_calls: i64,
    pub validation_failures: i64,
    pub failure_rate: f64,
    /// A2（2026-10-21 DeepInfer 思路引入）：窗口内 loop-guard 事件数
    /// （⑥/④/⑤/⑤′/⑦/⑧/T1）。`#[serde(default)]` 兼容旧前端/旧调用方。
    #[serde(default)]
    pub guard_events: i64,
    pub hint: Option<String>,
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

#[cfg(test)]
mod tests;
