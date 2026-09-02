//! Usage statistics API endpoints.
//!
//! Provides `/api/usage/summary`, `/api/usage/trends`, `/api/usage/logs`,
//! `/api/usage/pricing`（分层价目表）以及价目表管理端点
//! （`update` / `custom` / `custom/remove` / `import`，A2 在线更新）。

use crate::api_handlers::AppState;
use crate::pricing_sync;
use axum::Json;
use axum::extract::{Query, State};
use serde::Deserialize;
use std::collections::HashMap;
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
    /// 模型名子串过滤（A3 请求明细 tab）。
    pub model: Option<String>,
    /// 状态码精确过滤（200=成功；前端「失败」= 非矩阵另说，传具体值）。
    pub status: Option<i32>,
    /// session_key 子串过滤。
    pub session: Option<String>,
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

    let end = params
        .end
        .unwrap_or_else(|| chrono::Local::now().timestamp());
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

    let end = params
        .end
        .unwrap_or_else(|| chrono::Local::now().timestamp());
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

    let end = params
        .end
        .unwrap_or_else(|| chrono::Local::now().timestamp());
    let start = params.start.unwrap_or(end - 86400);
    let page = params.page.unwrap_or(1).max(1);
    let page_size = params.page_size.unwrap_or(20).min(100);
    let filter = nemesis_data::LogFilter {
        model: params.model.clone(),
        status: params.status,
        session_key: params.session.clone(),
    };

    match ds.query_logs(start, end, page, page_size, &filter) {
        Ok((logs, total)) => Json(serde_json::json!({
            "status": "success",
            "data": {
                "logs": logs.iter().map(request_log_json).collect::<Vec<_>>(),
                "total": total,
                "page": page,
                "pageSize": page_size,
            }
        })),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

/// RequestLog → JSON（列表行与详情面板共用同一形状，A3 扩展字段全量带出）。
fn request_log_json(l: &nemesis_data::RequestLog) -> serde_json::Value {
    serde_json::json!({
        "id": l.id,
        "traceId": l.trace_id,
        "model": l.model,
        "providerType": l.provider_type,
        "inputTokens": l.input_tokens,
        "outputTokens": l.output_tokens,
        "cacheCreationTokens": l.cache_creation_tokens,
        "cacheReadTokens": l.cache_read_tokens,
        "totalCostUsd": l.total_cost_usd,
        "inputCostUsd": l.input_cost_usd,
        "outputCostUsd": l.output_cost_usd,
        "cacheCreationCostUsd": l.cache_creation_cost_usd,
        "cacheReadCostUsd": l.cache_read_cost_usd,
        "pricingModel": l.pricing_model,
        "latencyMs": l.latency_ms,
        "firstTokenMs": l.first_token_ms,
        "statusCode": l.status_code,
        "errorMessage": l.error_message,
        "isStreaming": l.is_streaming,
        "sessionKey": l.session_key,
        "createdAt": l.created_at,
    })
}

/// GET /api/usage/logs/{id} — 单条明细（A3 详情面板）。
pub async fn handle_api_usage_log_detail(
    State(state): State<Arc<AppState>>,
    axum::extract::Path(id): axum::extract::Path<i64>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };
    match ds.get_request_log(id) {
        Ok(Some(log)) => Json(serde_json::json!({
            "status": "success",
            "data": request_log_json(&log),
        })),
        Ok(None) => Json(serde_json::json!({"error": format!("请求不存在: {id}")})),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

// ---------------------------------------------------------------------------
// Pricing — layered view + management (A2, 2026-08-31)
// ---------------------------------------------------------------------------

/// Layer of a pricing entry (lookup priority: custom > downloaded > embedded).
const SRC_EMBEDDED: &str = "embedded";
const SRC_DOWNLOADED: &str = "downloaded";
const SRC_CUSTOM: &str = "custom";

fn pricing_entry_json(p: &nemesis_data::ModelPricing, source: &str) -> serde_json::Value {
    serde_json::json!({
        "modelId": p.model_id,
        "displayName": p.display_name,
        "inputCostPerMillion": p.input_cost_per_million,
        "outputCostPerMillion": p.output_cost_per_million,
        "cacheReadCostPerMillion": p.cache_read_cost_per_million,
        "cacheCreationCostPerMillion": p.cache_creation_cost_per_million,
        "maxInputTokens": p.max_input_tokens,
        "maxOutputTokens": p.max_output_tokens,
        "aliases": p.aliases,
        "source": source,
    })
}

/// Effective merged pricing view（查表优先级 自定义 > 下载 > 内置 的列表投
/// 影）：先铺内置层垫底、再下载层、最后自定义层——`overlay` 后到者覆盖同
/// `model_id` 条目 = 高优先层胜出，每条带 `source`。
fn layered_entries(store: Option<&nemesis_data::PricingStore>) -> Vec<serde_json::Value> {
    fn overlay(
        entries: Vec<nemesis_data::ModelPricing>,
        source: &'static str,
        out: &mut Vec<serde_json::Value>,
        idx: &mut HashMap<String, usize>,
    ) {
        for p in entries {
            match idx.get(&p.model_id) {
                Some(&i) => out[i] = pricing_entry_json(&p, source),
                None => {
                    idx.insert(p.model_id.clone(), out.len());
                    out.push(pricing_entry_json(&p, source));
                }
            }
        }
    }

    let mut out: Vec<serde_json::Value> = Vec::new();
    let mut idx: HashMap<String, usize> = HashMap::new();
    // 内置表垫底（永在）；下载层、自定义层按优先级依次覆盖同名条目。
    overlay(
        nemesis_data::all_pricing().to_vec(),
        SRC_EMBEDDED,
        &mut out,
        &mut idx,
    );
    if let Some(s) = store {
        if let Some(dl) = s.list_downloaded() {
            overlay(dl, SRC_DOWNLOADED, &mut out, &mut idx);
        }
        overlay(s.list_custom(), SRC_CUSTOM, &mut out, &mut idx);
    }
    out
}

/// GET /api/usage/pricing
///
/// Layered effective pricing view (custom > downloaded > embedded) plus
/// download meta and the custom-entry list. `data` stays an array of entries
/// (each now with a `source` field) so existing consumers keep working.
pub async fn handle_api_usage_pricing(
    State(state): State<Arc<AppState>>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        // 无 DataStore（测试/早期启动）→ 仅内置表。
        return Json(serde_json::json!({
            "status": "success",
            "data": layered_entries(None),
            "meta": serde_json::Value::Null,
            "custom": [],
        }));
    };
    let pricing = ds.pricing();
    let meta = pricing.meta();
    Json(serde_json::json!({
        "status": "success",
        "data": layered_entries(Some(pricing)),
        "meta": {
            "etag": meta.etag,
            "fetchedAt": meta.fetched_at,
            "sourceUrl": meta.source_url,
            "entryCount": meta.entry_count,
        },
        "custom": pricing.list_custom().iter()
            .map(|p| pricing_entry_json(p, SRC_CUSTOM))
            .collect::<Vec<_>>(),
    }))
}

#[derive(Debug, Deserialize)]
pub struct PricingUpdateBody {
    /// 镜像覆盖（缺省 = LiteLLM 官方 raw 地址）。
    pub url: Option<String>,
}

/// POST /api/usage/pricing/update — 在线拉最新价目表并替换下载层。
/// 失败（网络/解析）→ 旧表保留 + 500 + 错误信息。
pub async fn handle_api_usage_pricing_update(
    State(state): State<Arc<AppState>>,
    body: Option<Json<PricingUpdateBody>>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };
    let url = body.as_ref().and_then(|Json(b)| b.url.clone());
    match pricing_sync::fetch_and_replace(ds.pricing(), url.as_deref()).await {
        Ok(r) => Json(serde_json::json!({"status": "success", "data": r})),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

/// POST /api/usage/pricing/custom — 新增/更新自定义条目（按 modelId 幂等）。
pub async fn handle_api_usage_pricing_custom_upsert(
    State(state): State<Arc<AppState>>,
    Json(entry): Json<nemesis_data::ModelPricing>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };
    if entry.model_id.trim().is_empty() {
        return Json(serde_json::json!({"error": "modelId 不能为空"}));
    }
    match ds.pricing().upsert_custom(entry) {
        Ok(()) => Json(serde_json::json!({"status": "success"})),
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

#[derive(Debug, Deserialize)]
pub struct PricingCustomRemoveBody {
    pub model_id: String,
}

/// POST /api/usage/pricing/custom/remove — 删除自定义条目（modelId 可能含
/// `/`，走 JSON body 避免 URL 编码问题）。
pub async fn handle_api_usage_pricing_custom_remove(
    State(state): State<Arc<AppState>>,
    Json(body): Json<PricingCustomRemoveBody>,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };
    match ds.pricing().remove_custom(&body.model_id) {
        Ok(true) => Json(serde_json::json!({"status": "success", "removed": true})),
        Ok(false) => {
            Json(serde_json::json!({"error": format!("自定义条目不存在: {}", body.model_id)}))
        }
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}

/// POST /api/usage/pricing/import — 离线导入：body 为 LiteLLM 原始
/// `model_prices_and_context_window.json` 全文（路由注册处已放宽 body 上限）。
pub async fn handle_api_usage_pricing_import(
    State(state): State<Arc<AppState>>,
    raw: String,
) -> Json<serde_json::Value> {
    let Some(ref ds) = state.data_store else {
        return Json(serde_json::json!({"error": "DataStore not configured"}));
    };
    match nemesis_data::parse_litellm_json(&raw) {
        Ok(entries) => {
            let entry_count = entries.len();
            let r = ds.pricing().replace_downloaded(
                entries,
                nemesis_data::PricingMeta {
                    etag: None,
                    fetched_at: Some(chrono::Local::now().timestamp()),
                    source_url: Some("manual-import".to_string()),
                    entry_count,
                },
            );
            match r {
                Ok(()) => Json(serde_json::json!({
                    "status": "success",
                    "data": {"entryCount": entry_count}
                })),
                Err(e) => Json(serde_json::json!({"error": e})),
            }
        }
        Err(e) => Json(serde_json::json!({"error": e})),
    }
}
