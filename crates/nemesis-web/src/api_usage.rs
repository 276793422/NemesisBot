//! Usage statistics API endpoints.
//!
//! Provides `/api/usage/summary`, `/api/usage/trends`, `/api/usage/logs`.

use crate::api_handlers::AppState;
use axum::extract::{Query, State};
use axum::Json;
use serde::Deserialize;
use std::sync::Arc;

// ---------------------------------------------------------------------------
// Query parameters
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
pub struct UsageQuery {
    /// Start timestamp (unix seconds). Defaults to 24 hours ago.
    pub start: Option<i64>,
    /// End timestamp (unix seconds). Defaults to now.
    pub end: Option<i64>,
}

#[derive(Debug, Deserialize)]
pub struct TrendsQuery {
    pub start: Option<i64>,
    pub end: Option<i64>,
    /// Group by "hour" or "day". Defaults to "hour".
    pub group_by: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct LogsQuery {
    pub start: Option<i64>,
    pub end: Option<i64>,
    pub page: Option<i32>,
    pub page_size: Option<i32>,
}

// ---------------------------------------------------------------------------
// Handlers
// ---------------------------------------------------------------------------

/// GET /api/usage/summary
pub async fn handle_api_usage_summary(
    State(state): State<Arc<AppState>>,
    Query(params): Query<UsageQuery>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };

    let end = params.end.unwrap_or_else(|| chrono::Local::now().timestamp());
    let start = params.start.unwrap_or(end - 86400);

    match ds.query_summary(start, end) {
        Ok(summary) => Json(serde_json::json!({
            "status": "success",
            "data": {
                "totalRequests": summary.total_requests,
                "successCount": summary.success_count,
                "totalInputTokens": summary.total_input_tokens,
                "totalOutputTokens": summary.total_output_tokens,
                "totalCacheCreationTokens": summary.total_cache_creation_tokens,
                "totalCacheReadTokens": summary.total_cache_read_tokens,
                "totalCostUsd": summary.total_cost_usd,
                "avgLatencyMs": (summary.avg_latency_ms as i64),
                "cacheHitRate": (summary.cache_hit_rate * 100.0).round() / 100.0,
            }
        })),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

/// GET /api/usage/trends
pub async fn handle_api_usage_trends(
    State(state): State<Arc<AppState>>,
    Query(params): Query<TrendsQuery>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };

    let end = params.end.unwrap_or_else(|| chrono::Local::now().timestamp());
    let start = params.start.unwrap_or(end - 86400);
    let group_by = params.group_by.as_deref().unwrap_or("hour");

    match ds.query_trends(start, end, group_by) {
        Ok(points) => Json(serde_json::json!({
            "status": "success",
            "data": points.iter().map(|p| serde_json::json!({
                "label": p.label,
                "timestamp": p.timestamp,
                "inputTokens": p.input_tokens,
                "outputTokens": p.output_tokens,
                "cacheCreationTokens": p.cache_creation_tokens,
                "cacheReadTokens": p.cache_read_tokens,
                "requestCount": p.request_count,
                "totalCostUsd": p.total_cost_usd,
            })).collect::<Vec<_>>()
        })),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

/// GET /api/usage/logs
pub async fn handle_api_usage_logs(
    State(state): State<Arc<AppState>>,
    Query(params): Query<LogsQuery>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };

    let end = params.end.unwrap_or_else(|| chrono::Local::now().timestamp());
    let start = params.start.unwrap_or(end - 86400);
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).min(100);

    match ds.query_logs(start, end, page, page_size) {
        Ok((logs, total)) => Json(serde_json::json!({
            "status": "success",
            "data": {
                "logs": logs.iter().map(|l| serde_json::json!({
                    "id": l.id,
                    "traceId": l.trace_id,
                    "model": l.model,
                    "providerType": l.provider_type,
                    "inputTokens": l.input_tokens,
                    "outputTokens": l.output_tokens,
                    "cacheCreationTokens": l.cache_creation_tokens,
                    "cacheReadTokens": l.cache_read_tokens,
                    "totalCostUsd": l.total_cost_usd,
                    "latencyMs": l.latency_ms,
                    "statusCode": l.status_code,
                    "errorMessage": l.error_message,
                    "isStreaming": l.is_streaming,
                    "createdAt": l.created_at,
                })).collect::<Vec<_>>(),
                "total": total,
                "page": page,
                "pageSize": page_size,
            }
        })),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}
