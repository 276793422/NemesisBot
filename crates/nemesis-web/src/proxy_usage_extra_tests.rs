//! `proxy_usage` 单测（docs/PLAN/2026-11-03_代理请求用量记账.md §4）：
//! 落账字段管线（model/session 前缀/status/token/cost）、错误路径、
//! usage=None 全 0、data_store 缺席 no-op、执行模型解析优先级。
//! AppState 构造依赖 workflow 的 ChatSecretStore，feature 闸同
//! `api_usage_extra_tests`。

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::proxy_usage::{ProxyUsageRecord, proxy_execution_model, record_proxy_usage};
use crate::session::SessionManager;
use nemesis_data::{DataStore, LogFilter};
use nemesis_providers::types::UsageInfo;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// -----------------------------------------------------------------------
// Helpers（形态对齐 api_usage_extra_tests）
// -----------------------------------------------------------------------

fn make_state(ds: Option<Arc<DataStore>>) -> Arc<AppState> {
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
        data_store: ds,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        quic_relay: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    })
}

fn open_store() -> (tempfile::TempDir, Arc<DataStore>) {
    let dir = tempfile::tempdir().unwrap();
    let db_path = dir.path().join("usage.db");
    let ds = DataStore::open(&db_path).expect("open store");
    (dir, Arc::new(ds))
}

fn proxy_logs(ds: &DataStore, session_substr: &str) -> Vec<nemesis_data::RequestLog> {
    let filter = LogFilter {
        session_key: Some(session_substr.to_string()),
        ..Default::default()
    };
    let (rows, _) = ds
        .query_logs(0, chrono::Local::now().timestamp() + 60, 1, 100, &filter)
        .expect("query logs");
    rows
}

// -----------------------------------------------------------------------
// 落账字段管线
// -----------------------------------------------------------------------

#[test]
fn test_records_success_row() {
    let (_dir, ds) = open_store();
    let usage = UsageInfo {
        prompt_tokens: 100,
        completion_tokens: 50,
        total_tokens: 150,
        cached_tokens: None,
        cache_creation_tokens: Some(10),
        cache_read_tokens: Some(20),
    };
    record_proxy_usage(
        Some(ds.as_ref()),
        ProxyUsageRecord {
            execution_model: "no-such-pricing-model",
            usage: Some(&usage),
            latency_ms: 123,
            is_streaming: false,
            status_code: 200,
            error_message: None,
            session_key: "proxy:openai-api:abc123".to_string(),
            trace_id: "openai-proxy-abc123".to_string(),
        },
    );
    let rows = proxy_logs(&ds, "proxy:openai-api:");
    assert_eq!(rows.len(), 1, "success row must land in the ledger");
    let r = &rows[0];
    // 账本不变量：model = 实际执行模型。
    assert_eq!(r.model, "no-such-pricing-model");
    assert_eq!(r.trace_id, "openai-proxy-abc123");
    assert_eq!(r.session_key, "proxy:openai-api:abc123");
    assert_eq!(r.status_code, 200);
    assert_eq!(r.error_message, None);
    assert!(!r.is_streaming);
    assert_eq!(r.input_tokens, 100);
    assert_eq!(r.output_tokens, 50);
    assert_eq!(r.cache_creation_tokens, 10);
    assert_eq!(r.cache_read_tokens, 20);
    assert_eq!(r.latency_ms, 123);
    // 价目表未命中 → 诚实全 0 + 空条目名（可区分「未命中」与「免费条目」）。
    assert_eq!(r.total_cost_usd, 0.0);
    assert_eq!(r.pricing_model, "");
    // organ 5 同口径：provider_type 恒空串。
    assert_eq!(r.provider_type, "");
    assert_eq!(r.first_token_ms, None);
}

#[test]
fn test_cache_read_falls_back_to_cached_tokens() {
    let (_dir, ds) = open_store();
    let usage = UsageInfo {
        prompt_tokens: 10,
        completion_tokens: 5,
        total_tokens: 15,
        // OpenAI prompt_cache_hit_tokens 别名落点：cache_read 缺席时回落。
        cached_tokens: Some(7),
        cache_creation_tokens: None,
        cache_read_tokens: None,
    };
    record_proxy_usage(
        Some(ds.as_ref()),
        ProxyUsageRecord {
            execution_model: "no-such-pricing-model",
            usage: Some(&usage),
            latency_ms: 1,
            is_streaming: true,
            status_code: 200,
            error_message: None,
            session_key: "proxy:chat-stream:x".to_string(),
            trace_id: "chat-stream-proxy-x".to_string(),
        },
    );
    let rows = proxy_logs(&ds, "proxy:chat-stream:");
    assert_eq!(rows.len(), 1);
    assert_eq!(rows[0].cache_read_tokens, 7);
    assert_eq!(rows[0].cache_creation_tokens, 0);
    assert!(rows[0].is_streaming);
}

#[test]
fn test_records_error_row() {
    let (_dir, ds) = open_store();
    let err_msg = "upstream 503: overloaded".to_string();
    record_proxy_usage(
        Some(ds.as_ref()),
        ProxyUsageRecord {
            execution_model: "m",
            usage: None,
            latency_ms: 45,
            is_streaming: true,
            status_code: 500,
            error_message: Some(&err_msg),
            session_key: "proxy:openai-api:err".to_string(),
            trace_id: "openai-proxy-err".to_string(),
        },
    );
    let rows = proxy_logs(&ds, "proxy:openai-api:");
    assert_eq!(rows.len(), 1, "error row must land in the ledger too");
    assert_eq!(rows[0].status_code, 500);
    assert_eq!(
        rows[0].error_message.as_deref(),
        Some("upstream 503: overloaded")
    );
    assert_eq!(rows[0].input_tokens, 0);
    assert_eq!(rows[0].output_tokens, 0);
    assert_eq!(rows[0].total_cost_usd, 0.0);
}

#[test]
fn test_usage_none_zero_tokens() {
    let (_dir, ds) = open_store();
    record_proxy_usage(
        Some(ds.as_ref()),
        ProxyUsageRecord {
            execution_model: "m",
            usage: None,
            latency_ms: 9,
            is_streaming: false,
            status_code: 200,
            error_message: None,
            session_key: "proxy:openai-api:nu".to_string(),
            trace_id: "openai-proxy-nu".to_string(),
        },
    );
    let rows = proxy_logs(&ds, "proxy:openai-api:");
    assert_eq!(rows.len(), 1);
    let r = &rows[0];
    assert_eq!(r.input_tokens, 0);
    assert_eq!(r.output_tokens, 0);
    assert_eq!(r.cache_creation_tokens, 0);
    assert_eq!(r.cache_read_tokens, 0);
    assert_eq!(r.total_cost_usd, 0.0);
}

#[test]
fn test_no_data_store_is_noop() {
    // usage-store 关/未装配：调用不炸、不写（没有 store 可断言，跑过即过）。
    record_proxy_usage(
        None,
        ProxyUsageRecord {
            execution_model: "m",
            usage: None,
            latency_ms: 1,
            is_streaming: false,
            status_code: 200,
            error_message: None,
            session_key: "proxy:openai-api:none".to_string(),
            trace_id: "openai-proxy-none".to_string(),
        },
    );
}

// -----------------------------------------------------------------------
// 执行模型解析
// -----------------------------------------------------------------------

#[test]
fn test_execution_model_requested_wins() {
    let state = make_state(None);
    assert_eq!(
        proxy_execution_model(&state, Some("caller-model")),
        "caller-model"
    );
    // 纯空白点名视同未点名。
    assert_eq!(proxy_execution_model(&state, Some("  ")), "m");
}

#[test]
fn test_execution_model_falls_back_to_active() {
    let state = make_state(None);
    assert_eq!(proxy_execution_model(&state, None), "m");
    assert_eq!(proxy_execution_model(&state, Some("")), "m");
}
