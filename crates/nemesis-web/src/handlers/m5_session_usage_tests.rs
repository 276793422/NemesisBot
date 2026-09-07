//! M5（2026-09-05）：会话级用量 WSAPI 测试——`logs.session_usage`、
//! `backfill_session_usage`（logs.session_list / sessions.list 共用回填）
//! 与 `chat.context_status` 三条链路。

use super::chat::ChatHandler;
use super::logs::{LogsHandler, backfill_session_usage};
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_data::{DataStore, RequestLog};
use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness
// ---------------------------------------------------------------------------

/// 与 unit_tests / m5_session_usage_tests（nemesis-data）同款唯一 db 路径
/// （pid + 进程内单调计数器；时间戳在 Windows 上有同窗口碰撞前科）。
fn temp_db_path() -> std::path::PathBuf {
    static SEQ: AtomicUsize = AtomicUsize::new(0);
    let mut path = std::env::temp_dir();
    path.push(format!(
        "nb_web_m5_usage_{}_{}.db",
        std::process::id(),
        SEQ.fetch_add(1, Ordering::Relaxed),
    ));
    let _ = std::fs::remove_file(&path);
    path
}

struct NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

/// AppState 全量字面量（chat_secret_store / webhook_rate_limiter 按
/// workflow feature 双臂——与 sessions_s10b_tests 同款）。
fn make_ctx(
    dir: &tempfile::TempDir,
    data_store: Option<Arc<DataStore>>,
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
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
        data_store,
        memory_manager: None,
        forge: None,
        agent_loop: Arc::new(parking_lot::RwLock::new(agent_loop)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn log(session_key: &str, input: i64, cache_read: i64, output: i64, cost: f64) -> RequestLog {
    RequestLog {
        trace_id: format!("m5-web-{session_key}"),
        model: "test-model".to_string(),
        provider_type: "test".to_string(),
        input_tokens: input,
        output_tokens: output,
        cache_read_tokens: cache_read,
        total_cost_usd: cost,
        status_code: 200,
        session_key: session_key.to_string(),
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// logs.session_usage
// ---------------------------------------------------------------------------

#[tokio::test]
async fn session_usage_happy_path() {
    let dir = tempfile::tempdir().unwrap();
    let ds = Arc::new(DataStore::open(&temp_db_path()).unwrap());
    ds.insert_request_log(&log("agent:main:session:s1", 100, 50, 30, 0.01))
        .unwrap();
    ds.insert_request_log(&log("agent:main:session:s1", 200, 0, 40, 0.02))
        .unwrap();
    let ctx = make_ctx(&dir, Some(ds), None);

    let r = LogsHandler
        .handle_cmd(
            "session_usage",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["session_key"], "agent:main:session:s1");
    assert_eq!(r["requests"], 2);
    assert_eq!(r["input_tokens"], 350, "input 口径含 cache_read");
    assert_eq!(r["output_tokens"], 70);
    assert!((r["total_cost_usd"].as_f64().unwrap() - 0.03).abs() < 1e-9);
}

#[tokio::test]
async fn session_usage_prefers_explicit_session_key() {
    let dir = tempfile::tempdir().unwrap();
    let ds = Arc::new(DataStore::open(&temp_db_path()).unwrap());
    // rpc 会话键不落 `agent:main:session:` 形态——显式键直查。
    ds.insert_request_log(&log("rpc:node-b/task-7", 500, 0, 90, 0.5))
        .unwrap();
    let ctx = make_ctx(&dir, Some(ds), None);

    let r = LogsHandler
        .handle_cmd(
            "session_usage",
            Some(serde_json::json!({ "session_key": "rpc:node-b/task-7" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["session_key"], "rpc:node-b/task-7");
    assert_eq!(r["requests"], 1);
    assert_eq!(r["input_tokens"], 500);
}

#[tokio::test]
async fn session_usage_without_store_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None, None);
    let err = LogsHandler
        .handle_cmd(
            "session_usage",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("usage data store not available"));
}

#[tokio::test]
async fn session_usage_requires_identifier() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None, None);
    // 键解析在 store 检查之前：两个标识都缺 → 错误落在键上。
    let err = LogsHandler
        .handle_cmd("session_usage", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("session_id or session_key is required"));
}

// ---------------------------------------------------------------------------
// backfill_session_usage
// ---------------------------------------------------------------------------

#[test]
fn backfill_fills_tokens_and_cost_only_for_keyed_entries() {
    let dir = tempfile::tempdir().unwrap();
    let ds = Arc::new(DataStore::open(&temp_db_path()).unwrap());
    ds.insert_request_log(&log("agent:main:session:has-key", 1000, 0, 100, 0.25))
        .unwrap();
    let ctx = make_ctx(&dir, Some(ds), None);

    let mut sessions = vec![
        serde_json::json!({ "id": "has-key", "session_key": "agent:main:session:has-key" }),
        serde_json::json!({ "id": "no-key", "firstMessage": "hi" }),
    ];
    backfill_session_usage(&ctx, &mut sessions);

    assert_eq!(sessions[0]["tokens"], 1000);
    assert!((sessions[0]["cost"].as_f64().unwrap() - 0.25).abs() < 1e-9);
    // 无 session_key 的条目保持原样（前端按缺省不渲染）。
    assert!(sessions[1].get("tokens").is_none());
    assert!(sessions[1].get("cost").is_none());
}

#[test]
fn backfill_without_store_is_noop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None, None);
    let mut sessions = vec![serde_json::json!({
        "id": "x", "session_key": "agent:main:session:x"
    })];
    backfill_session_usage(&ctx, &mut sessions);
    assert!(sessions[0].get("tokens").is_none(), "无 store = 静默跳过");
}

// ---------------------------------------------------------------------------
// chat.context_status
// ---------------------------------------------------------------------------

#[tokio::test]
async fn context_status_requires_agent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None, None);
    let err = ChatHandler
        .handle_cmd(
            "context_status",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"));
}

#[tokio::test]
async fn context_status_returns_snapshot() {
    let dir = tempfile::tempdir().unwrap();
    let mut al = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    let store = Arc::new(nemesis_agent::session::SessionStore::new_in_memory());
    let key = "agent:main:session:ctx-s";
    store.get_or_create(key);
    store.add_message(key, "user", "hello there, measuring context");
    al.set_session_store(store);
    let ctx = make_ctx(&dir, None, Some(Arc::new(al)));

    let r = ChatHandler
        .handle_cmd(
            "context_status",
            Some(serde_json::json!({ "session_id": "ctx-s" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["session_key"], key);
    let status = &r["context"];
    assert_eq!(status["history_len"], 1);
    assert!(status["used_tokens"].as_u64().unwrap() > 0);
    assert_eq!(status["window"], 128_000, "无 config/价目表 → 实例默认");
    assert!(status["pct"].as_u64().unwrap() <= 100);
}
