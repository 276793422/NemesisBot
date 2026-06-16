//! Extra tests for `api_usage` handlers — covers query parsing, JSON shape,
//! and the "DataStore not configured" branch.

#[cfg(test)]
mod api_usage_extra_tests {
    use crate::api_handlers::AppState;
    use crate::api_usage::{
        handle_api_usage_logs, handle_api_usage_summary, handle_api_usage_trends, LogsQuery,
        TrendsQuery, UsageQuery,
    };
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use axum::extract::{Query, State};
    use axum::Json;
    use nemesis_data::DataStore;
    use nemesis_data::RequestLog;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Arc;
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
            internal_cmd_tx: None,
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
            internal_cmd_tx: None,
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
        let q = Query(UsageQuery { start: None, end: None });
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
        let q = Query(UsageQuery { start: None, end: None });
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
        });
        let Json(v) = handle_api_usage_logs(State(state), q).await;
        assert_eq!(v["data"]["total"], 2);
        let logs = v["data"]["logs"].as_array().unwrap();
        assert_eq!(logs.len(), 2);
        // First row should have a model field
        assert!(logs[0]["model"].as_str().unwrap() == "gpt-4" || logs[0]["model"].as_str().unwrap() == "claude");
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
        });
        let Json(v) = handle_api_usage_logs(State(state), q).await;
        assert_eq!(v["status"], "success");
    }

    // -----------------------------------------------------------------------
    // Query-struct construction (cheap sanity)
    // -----------------------------------------------------------------------

    #[test]
    fn usage_query_deserialize() {
    let q: UsageQuery =
        serde_json::from_str(r#"{"start": 100, "end": 200}"#).unwrap();
    assert_eq!(q.start, Some(100));
    assert_eq!(q.end, Some(200));
}

    #[test]
    fn trends_query_deserialize_with_group_by() {
    let q: TrendsQuery =
        serde_json::from_str(r#"{"group_by": "day"}"#).unwrap();
    assert_eq!(q.group_by.as_deref(), Some("day"));
}

    #[test]
    fn logs_query_deserialize_all_fields() {
    let q: LogsQuery =
        serde_json::from_str(r#"{"start": 1, "end": 2, "page": 3, "page_size": 4}"#)
            .unwrap();
    assert_eq!(q.page, Some(3));
    assert_eq!(q.page_size, Some(4));
}

    #[test]
    fn usage_query_empty_json_ok() {
    let q: UsageQuery = serde_json::from_str("{}").unwrap();
    assert!(q.start.is_none());
    assert!(q.end.is_none());
}
}
