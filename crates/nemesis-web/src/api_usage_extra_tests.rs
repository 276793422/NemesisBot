//! Extra tests for `api_usage` handlers — covers query parsing, JSON shape,
//! and the "DataStore not configured" branch.

use crate::api_handlers::AppState;
use crate::api_usage::{
    LogsQuery, TrendsQuery, UsageQuery, handle_api_usage_logs, handle_api_usage_pricing,
    handle_api_usage_summary, handle_api_usage_trends,
};
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::Json;
use axum::extract::{Query, State};
use nemesis_data::DataStore;
use nemesis_data::RequestLog;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// -----------------------------------------------------------------------
// Helpers
// -----------------------------------------------------------------------

fn make_state_no_data_store() -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

fn make_state_with_store(ds: Arc<DataStore>) -> Arc<AppState> {
    let s = AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: Some(ds),
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    };
    Arc::new(s)
}

fn open_store() -> (tempfile::TempDir, Arc<DataStore>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("usage.db");
    let ds = DataStore::open(&db_path).expect("open store");
    (dir, Arc::new(ds))
}

fn sample_log(trace: &str, model: &str, cost: f64, tokens: i64, ts: i64) -> RequestLog {
    RequestLog {
        id: 0,
        trace_id: trace.to_string(),
        model: model.to_string(),
        provider_type: "openai".to_string(),
        input_tokens: tokens,
        output_tokens: tokens * 2,
        cache_creation_tokens: 0,
        cache_read_tokens: 0,
        total_cost_usd: cost,
        latency_ms: 100,
        status_code: 200,
        error_message: None,
        is_streaming: false,
        created_at: ts,
        pricing_model: model.to_string(),
        input_cost_usd: cost * 0.6,
        output_cost_usd: cost * 0.4,
        cache_creation_cost_usd: 0.0,
        cache_read_cost_usd: 0.0,
        first_token_ms: None,
        session_key: "sess-a".to_string(),
    }
}

fn now_ts() -> i64 {
    chrono::Local::now().timestamp()
}

// -----------------------------------------------------------------------
// Summary — DataStore absent
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_no_data_store_returns_error() {
    let state = make_state_no_data_store();
    let q = Query(UsageQuery {
        start: None,
        end: None,
    });
    let Json(v) = handle_api_usage_summary(State(state), q).await;
    assert_eq!(v["error"], "DataStore not configured");
}

// -----------------------------------------------------------------------
// Summary — empty store
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_empty_store_returns_zeros() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let now = now_ts();
    let q = Query(UsageQuery {
        start: Some(now - 3600),
        end: Some(now),
    });
    let Json(v) = handle_api_usage_summary(State(state), q).await;
    assert_eq!(v["status"], "success");
    assert_eq!(v["data"]["totalRequests"], 0);
    assert_eq!(v["data"]["successCount"], 0);
    assert_eq!(v["data"]["totalInputTokens"], 0);
    assert_eq!(v["data"]["totalOutputTokens"], 0);
    assert_eq!(v["data"]["totalCostUsd"], 0.0);
}

// -----------------------------------------------------------------------
// Summary — with inserted log
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_with_log_aggregates() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    ds.insert_request_log(&sample_log("t1", "gpt-4", 0.01, 100, now - 60))
        .unwrap();
    let state = make_state_with_store(ds);
    let q = Query(UsageQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
    });
    let Json(v) = handle_api_usage_summary(State(state), q).await;
    assert_eq!(v["status"], "success");
    assert_eq!(v["data"]["totalRequests"], 1);
    assert_eq!(v["data"]["successCount"], 1);
    assert_eq!(v["data"]["totalInputTokens"], 100);
    assert_eq!(v["data"]["totalOutputTokens"], 200);
    assert_eq!(v["data"]["totalCostUsd"], 0.01);
}

// -----------------------------------------------------------------------
// Summary — default range (start/end None)
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_default_range_no_panic() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let q = Query(UsageQuery {
        start: None,
        end: None,
    });
    let Json(v) = handle_api_usage_summary(State(state), q).await;
    assert_eq!(v["status"], "success");
}

// -----------------------------------------------------------------------
// Summary — out-of-range log excluded
// -----------------------------------------------------------------------

#[tokio::test]
async fn summary_excludes_out_of_range_logs() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    // Old log far outside the requested window
    ds.insert_request_log(&sample_log("old", "gpt-4", 0.5, 999, now - 10_000_000))
        .unwrap();
    let state = make_state_with_store(ds);
    let q = Query(UsageQuery {
        start: Some(now - 60),
        end: Some(now + 60),
    });
    let Json(v) = handle_api_usage_summary(State(state), q).await;
    assert_eq!(v["data"]["totalRequests"], 0);
}

// -----------------------------------------------------------------------
// Trends
// -----------------------------------------------------------------------

#[tokio::test]
async fn trends_no_data_store_returns_error() {
    let state = make_state_no_data_store();
    let q = Query(TrendsQuery {
        start: None,
        end: None,
        group_by: None,
    });
    let Json(v) = handle_api_usage_trends(State(state), q).await;
    assert_eq!(v["error"], "DataStore not configured");
}

#[tokio::test]
async fn trends_empty_store_returns_empty_array() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let now = now_ts();
    let q = Query(TrendsQuery {
        start: Some(now - 3600),
        end: Some(now),
        group_by: Some("hour".to_string()),
        });
        let Json(v) = handle_api_usage_trends(State(state), q).await;
        assert_eq!(v["status"], "success");
    assert!(v["data"].is_array());
}

#[tokio::test]
async fn trends_with_log_hour_grouping() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    ds.insert_request_log(&sample_log("t1", "gpt-4", 0.02, 50, now - 120))
        .unwrap();
    let state = make_state_with_store(ds);
    let q = Query(TrendsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        group_by: Some("hour".to_string()),
        });
        let Json(v) = handle_api_usage_trends(State(state), q).await;
        assert_eq!(v["status"], "success");
    let arr = v["data"].as_array().unwrap();
    assert!(!arr.is_empty(), "expected at least one trend bucket");
    // Find the bucket with inputTokens > 0
    let hit = arr
        .iter()
        .find(|p| p["inputTokens"].as_i64().unwrap_or(0) > 0);
    assert!(hit.is_some(), "expected a non-empty bucket");
}

#[tokio::test]
async fn trends_with_day_grouping() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    ds.insert_request_log(&sample_log("t1", "gpt-4", 0.02, 50, now - 60))
        .unwrap();
    let state = make_state_with_store(ds);
    let q = Query(TrendsQuery {
        start: Some(now - 86400 * 2),
        end: Some(now + 60),
        group_by: Some("day".to_string()),
    });
    let Json(v) = handle_api_usage_trends(State(state), q).await;
    assert_eq!(v["status"], "success");
    assert!(v["data"].is_array());
}

#[tokio::test]
async fn trends_default_group_by_is_hour() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let q = Query(TrendsQuery {
        start: None,
        end: None,
        group_by: None,
    });
    let Json(v) = handle_api_usage_trends(State(state), q).await;
    assert_eq!(v["status"], "success");
}

// -----------------------------------------------------------------------
// Logs
// -----------------------------------------------------------------------

#[tokio::test]
async fn logs_no_data_store_returns_error() {
    let state = make_state_no_data_store();
    let q = Query(LogsQuery {
        start: None,
        end: None,
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["error"], "DataStore not configured");
}

#[tokio::test]
async fn logs_empty_store_returns_empty_list() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let now = now_ts();
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now),
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["status"], "success");
    assert_eq!(v["data"]["total"], 0);
    assert!(v["data"]["logs"].is_array());
    assert_eq!(v["data"]["page"], 1);
    assert_eq!(v["data"]["pageSize"], 20);
}

#[tokio::test]
async fn logs_with_inserted_entries() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    ds.insert_request_log(&sample_log("t1", "gpt-4", 0.01, 10, now - 60))
        .unwrap();
    ds.insert_request_log(&sample_log("t2", "claude", 0.02, 20, now - 30))
        .unwrap();
    let state = make_state_with_store(ds);
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: Some(1),
        page_size: Some(10),
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["data"]["total"], 2);
    let logs = v["data"]["logs"].as_array().unwrap();
    assert_eq!(logs.len(), 2);
    // First row should have a model field
    assert!(
        logs[0]["model"].as_str().unwrap() == "gpt-4"
            || logs[0]["model"].as_str().unwrap() == "claude"
    );
}

#[tokio::test]
async fn logs_page_size_clamped_to_100() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    let state = make_state_with_store(ds);
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now),
        page: None,
        page_size: Some(500),
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["data"]["pageSize"], 100);
}

#[tokio::test]
async fn logs_page_below_one_becomes_one() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    let state = make_state_with_store(ds);
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now),
        page: Some(-3),
        page_size: None,
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["data"]["page"], 1);
}

#[tokio::test]
async fn logs_default_range_used() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds);
    let q = Query(LogsQuery {
        start: None,
        end: None,
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["status"], "success");
}

// -----------------------------------------------------------------------
// Query-struct construction (cheap sanity)
// -----------------------------------------------------------------------

#[test]
fn usage_query_deserialize() {
    let q: UsageQuery = serde_json::from_str(r#"{"start": 100, "end": 200}"#).unwrap();
    assert_eq!(q.start, Some(100));
    assert_eq!(q.end, Some(200));
}

#[test]
fn trends_query_deserialize_with_group_by() {
    let q: TrendsQuery = serde_json::from_str(r#"{"group_by": "day"}"#).unwrap();
    assert_eq!(q.group_by.as_deref(), Some("day"));
}

#[test]
fn logs_query_deserialize_all_fields() {
    let q: LogsQuery =
        serde_json::from_str(r#"{"start": 1, "end": 2, "page": 3, "page_size": 4}"#).unwrap();
    assert_eq!(q.page, Some(3));
    assert_eq!(q.page_size, Some(4));
}

#[test]
fn usage_query_empty_json_ok() {
    let q: UsageQuery = serde_json::from_str("{}").unwrap();
    assert!(q.start.is_none());
    assert!(q.end.is_none());
}

// -----------------------------------------------------------------------
// Pricing — layered table endpoint (A2)
// -----------------------------------------------------------------------

#[tokio::test]
async fn pricing_returns_embedded_table() {
    let Json(v) = handle_api_usage_pricing(State(make_state_no_data_store())).await;
    assert_eq!(v["status"], "success");
    // 无 DataStore → 仅内置表 + meta null。
    assert!(v["meta"].is_null());
    let entries = v["data"].as_array().expect("data array");
    assert!(entries.len() >= 30, "expected ~36 entries, got {}", entries.len());
    assert!(entries.iter().all(|e| e["source"] == "embedded"));

    let gpt = entries
        .iter()
        .find(|e| e["modelId"] == "gpt-4o")
        .expect("gpt-4o present");
    assert_eq!(gpt["inputCostPerMillion"], 2.5);
    assert_eq!(gpt["outputCostPerMillion"], 10.0);
    assert_eq!(gpt["displayName"], "GPT-4o");
    // Aliases field is always present (may be empty for OpenAI entries).
    assert!(gpt["aliases"].is_array());

    let ds = entries
        .iter()
        .find(|e| e["modelId"] == "deepseek-chat")
        .expect("deepseek-chat present");
    assert_eq!(ds["cacheReadCostPerMillion"], 0.03);
    let aliases = ds["aliases"].as_array().unwrap();
    assert!(aliases.iter().any(|a| a == "deepseek/deepseek-chat"));

    // Optional token limits round-trip as null or number.
    for e in entries {
        assert!(e["maxInputTokens"].is_null() || e["maxInputTokens"].is_i64());
        assert!(e["maxOutputTokens"].is_null() || e["maxOutputTokens"].is_i64());
    }
}

// -----------------------------------------------------------------------
// Pricing — management endpoints (A2: custom CRUD / import / update gate)
// -----------------------------------------------------------------------

use crate::api_usage::{
    PricingCustomRemoveBody, PricingUpdateBody, handle_api_usage_pricing_custom_remove,
    handle_api_usage_pricing_custom_upsert, handle_api_usage_pricing_import,
    handle_api_usage_pricing_update,
};

fn custom_entry(model_id: &str, input: f64) -> nemesis_data::ModelPricing {
    nemesis_data::ModelPricing {
        model_id: model_id.to_string(),
        display_name: "custom".to_string(),
        input_cost_per_million: input,
        output_cost_per_million: input * 2.0,
        cache_read_cost_per_million: 0.0,
        cache_creation_cost_per_million: 0.0,
        max_input_tokens: None,
        max_output_tokens: None,
        aliases: Vec::new(),
    }
}

#[tokio::test]
async fn pricing_layered_view_with_store_and_custom_override() {
    let (_dir, ds) = open_store();
    // 预置自定义条目覆盖内置 gpt-4o。
    ds.pricing()
        .upsert_custom(custom_entry("gpt-4o", 123.0))
        .unwrap();
    let state = make_state_with_store(ds);

    let Json(v) = handle_api_usage_pricing(State(state)).await;
    assert_eq!(v["status"], "success");
    let entries = v["data"].as_array().unwrap();
    // 总数 = 内置 36 + 1 个自定义新条目……（gpt-4o 是覆盖不是新增）
    // 自定义只覆盖了内置条目 → 数量不变。
    assert_eq!(entries.len(), 36, "override must not duplicate the entry");
    let gpt = entries.iter().find(|e| e["modelId"] == "gpt-4o").unwrap();
    assert_eq!(gpt["source"], "custom");
    assert_eq!(gpt["inputCostPerMillion"], 123.0);

    let custom_list = v["custom"].as_array().unwrap();
    assert_eq!(custom_list.len(), 1);
    assert_eq!(custom_list[0]["modelId"], "gpt-4o");

    // meta 有字段（尚未下载 → 值为 null）。
    assert!(v["meta"].is_object());
    assert!(v["meta"]["entryCount"].is_i64() || v["meta"]["entryCount"].is_null());
}

#[tokio::test]
async fn pricing_custom_upsert_and_remove_endpoints() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds.clone());

    // upsert（含 provider/ 形状 modelId —— JSON body 免 URL 编码）。
    let Json(v) = handle_api_usage_pricing_custom_upsert(
        State(state.clone()),
        Json(custom_entry("zhipu/glm-4.7", 6.0)),
    )
    .await;
    assert_eq!(v["status"], "success");
    assert_eq!(ds.pricing().list_custom().len(), 1);

    // 幂等 upsert。
    let Json(v) = handle_api_usage_pricing_custom_upsert(
        State(state.clone()),
        Json(custom_entry("zhipu/glm-4.7", 7.0)),
    )
    .await;
    assert_eq!(v["status"], "success");
    assert_eq!(ds.pricing().list_custom().len(), 1);
    assert_eq!(ds.pricing().list_custom()[0].input_cost_per_million, 7.0);

    // 空 modelId 拒绝。
    let Json(v) = handle_api_usage_pricing_custom_upsert(
        State(state.clone()),
        Json(custom_entry("  ", 1.0)),
    )
    .await;
    assert!(v["error"].is_string());

    // remove 存在 / 不存在。
    let Json(v) = handle_api_usage_pricing_custom_remove(
        State(state.clone()),
        Json(PricingCustomRemoveBody {
            model_id: "zhipu/glm-4.7".to_string(),
        }),
    )
    .await;
    assert_eq!(v["removed"], true);
    let Json(v) = handle_api_usage_pricing_custom_remove(
        State(state),
        Json(PricingCustomRemoveBody {
            model_id: "zhipu/glm-4.7".to_string(),
        }),
    )
    .await;
    assert!(v["error"].is_string(), "removing missing entry must error");
}

#[tokio::test]
async fn pricing_import_parses_litellm_body() {
    let (_dir, ds) = open_store();
    let state = make_state_with_store(ds.clone());

    let body = r#"{
      "gpt-4o": {
        "input_cost_per_token": 2.5e-06,
        "output_cost_per_token": 1e-05,
        "litellm_provider": "openai",
        "mode": "chat"
      },
      "text-embedding-3-small": {
        "input_cost_per_token": 2e-08,
        "output_cost_per_token": 0.0,
        "mode": "embedding"
      }
    }"#;
    let Json(v) = handle_api_usage_pricing_import(State(state.clone()), body.to_string()).await;
    assert_eq!(v["status"], "success");
    assert_eq!(v["data"]["entryCount"], 1, "embedding entry filtered out");
    let dl = ds.pricing().list_downloaded().unwrap();
    assert_eq!(dl.len(), 1);
    assert_eq!(dl[0].model_id, "gpt-4o");
    assert!((dl[0].input_cost_per_million - 2.5).abs() < 1e-9);

    // 坏 body → error，下载层保留。
    let Json(v) = handle_api_usage_pricing_import(State(state), "{broken".to_string()).await;
    assert!(v["error"].is_string());
    assert_eq!(ds.pricing().list_downloaded().unwrap().len(), 1);
}

#[tokio::test]
async fn pricing_update_requires_data_store() {
    let state = make_state_no_data_store();
    let Json(v) = handle_api_usage_pricing_update(
        State(state),
        Some(Json(PricingUpdateBody { url: None })),
    )
    .await;
    assert_eq!(v["error"], "DataStore not configured");
}

// -----------------------------------------------------------------------
// Log detail endpoint + A3 filters (A3, 2026-08-31)
// -----------------------------------------------------------------------

use crate::api_usage::handle_api_usage_log_detail;

/// 列表响应包含 A3 新字段（分项成本 / pricingModel / firstTokenMs / sessionKey）。
#[tokio::test]
async fn logs_json_includes_a3_fields() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    let mut log = sample_log("t-a3", "glm-4.7", 0.02, 50, now - 60);
    log.first_token_ms = Some(320);
    log.status_code = 500;
    log.error_message = Some("boom".to_string());
    ds.insert_request_log(&log).unwrap();
    let state = make_state_with_store(ds);

    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    let row = &v["data"]["logs"][0];
    assert_eq!(row["pricingModel"], "glm-4.7");
    assert_eq!(row["sessionKey"], "sess-a");
    assert_eq!(row["firstTokenMs"], 320);
    assert_eq!(row["inputCostUsd"], 0.012);
    assert_eq!(row["outputCostUsd"], 0.008);
    assert_eq!(row["errorMessage"], "boom");
}

/// LogsQuery 的 model/status/session 过滤参数经 handler 生效。
#[tokio::test]
async fn logs_filters_via_query_params() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    let mut err = sample_log("t-err", "gpt-4", 0.01, 10, now - 60);
    err.status_code = 500;
    ds.insert_request_log(&err).unwrap();
    ds.insert_request_log(&sample_log("t-ok", "claude-3", 0.02, 20, now - 30))
        .unwrap();
    let state = make_state_with_store(ds);

    // model 子串。
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: None,
        page_size: None,
        model: Some("gpt".to_string()),
        status: None,
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state.clone()), q).await;
    assert_eq!(v["data"]["total"], 1);

    // status 精确。
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: None,
        page_size: None,
        model: None,
        status: Some(500),
        session: None,
    });
    let Json(v) = handle_api_usage_logs(State(state.clone()), q).await;
    assert_eq!(v["data"]["total"], 1);
    assert_eq!(v["data"]["logs"][0]["traceId"], "t-err");

    // session 子串：两行 session 都是 sess-a → 都命中；换不存在的 → 0。
    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: Some("sess-a".to_string()),
    });
    let Json(v) = handle_api_usage_logs(State(state.clone()), q).await;
    assert_eq!(v["data"]["total"], 2);

    let q = Query(LogsQuery {
        start: Some(now - 3600),
        end: Some(now + 60),
        page: None,
        page_size: None,
        model: None,
        status: None,
        session: Some("no-such-sess".to_string()),
    });
    let Json(v) = handle_api_usage_logs(State(state), q).await;
    assert_eq!(v["data"]["total"], 0);
}

/// 详情端点：命中 → 全字段 data；未命中 → error；无 DataStore → error。
#[tokio::test]
async fn log_detail_found_missing_and_no_store() {
    let (_dir, ds) = open_store();
    let now = now_ts();
    let mut log = sample_log("t-detail", "glm-4.7", 0.03, 70, now - 60);
    log.first_token_ms = Some(180);
    ds.insert_request_log(&log).unwrap();
    // 拿真实 id（AUTOINCREMENT 从 1 起）。
    let id = ds.query_logs(0, now + 60, 1, 10, &nemesis_data::LogFilter::default())
        .unwrap()
        .0[0]
        .id;
    let state = make_state_with_store(ds);

    // 命中：data 带 A3 字段。
    let Json(v) = handle_api_usage_log_detail(State(state.clone()), axum::extract::Path(id)).await;
    assert_eq!(v["status"], "success");
    assert_eq!(v["data"]["traceId"], "t-detail");
    assert_eq!(v["data"]["pricingModel"], "glm-4.7");
    assert_eq!(v["data"]["sessionKey"], "sess-a");
    assert_eq!(v["data"]["firstTokenMs"], 180);

    // 未命中。
    let Json(v) =
        handle_api_usage_log_detail(State(state.clone()), axum::extract::Path(999_999)).await;
    assert!(v["error"].is_string(), "missing id must error");

    // 无 DataStore。
    let Json(v) =
        handle_api_usage_log_detail(State(make_state_no_data_store()), axum::extract::Path(1)).await;
    assert_eq!(v["error"], "DataStore not configured");
}
