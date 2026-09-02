//! S10b (quality-hardening goal 冲刺, web 批次 2): api_handlers helpers +
//! narrow handler arms not reached by the existing test modules, all offline:
//!
//! - `handle_api_models` api_key masking arms (short key → "****", long key
//!   → prefix+"****", multibyte key → char-boundary floor, no-key untouched)
//! - `find_latest_request_summary` newest-dir/newest-md walk + empty-dir None
//! - `is_daily_nemesisbot_log` strict `nemesisbot.YYYY-MM-DD` matcher
//! - `read_log_entries` tail window (small file full-read, >64KB seek path
//!   with first-partial-line drop, garbage-line filtering, missing file)
//! - `sanitize_map` sensitive-key masking + nested recursion + non-string
//! - `first_line_trunc` first-non-empty-line + ellipsis + multibyte
//! - `resolve_fork_store` home-missing 503 arm + fallback Ok arm
//! - turns endpoint: leading pre-user rows (`leading` counter) arm
//! - fork endpoint: `title` → `write_session_meta` arm
//!
//! chat_log-touching tests follow the fork_route_tests house pattern:
//! nanos-unique session keys + `delete_chat_log` cleanup (the chat log root
//! is the process-global path manager and must not be redirected here).

use super::*;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::Router;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};
use tower::ServiceExt;

fn make_state(home: Option<&std::path::Path>) -> Arc<AppState> {
    let ws = home
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
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
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc(()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    })
}

// ---------------------------------------------------------------------------
// handle_api_models api_key masking
// ---------------------------------------------------------------------------

#[tokio::test]
async fn models_config_masks_api_keys_in_all_arms() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = serde_json::json!({
        "model_list": [
            { "model_name": "long",  "model": "x/y", "api_key": "abcd1234efgh" },
            { "model_name": "short", "model": "x/y", "api_key": "ab" },
            { "model_name": "cjk",   "model": "x/y", "api_key": "密钥很长很长" },
            { "model_name": "bare",  "model": "x/y" }
        ],
        "agents": { "defaults": { "llm": "long" } }
    });
    std::fs::write(dir.path().join("config.json"), cfg.to_string()).unwrap();

    let state = make_state(Some(dir.path()));
    let Json(v) = handle_api_models(State(state)).await.expect("ok");
    let models = v["models"].as_array().unwrap();
    let by_name = |n: &str| {
        models
            .iter()
            .find(|m| m["model_name"] == n)
            .unwrap_or_else(|| panic!("missing model {}", n))
            .clone()
    };
    assert_eq!(by_name("long")["api_key"], "abcd****");
    assert_eq!(by_name("short")["api_key"], "****");
    // Multibyte: the 4-byte cut floors to the 3-byte char boundary → "密****".
    let cjk = by_name("cjk")["api_key"].as_str().unwrap().to_string();
    assert!(
        cjk.starts_with("密") && cjk.ends_with("****"),
        "got {}",
        cjk
    );
    // No api_key field → entry untouched.
    assert!(by_name("bare").get("api_key").is_none());
}

// ---------------------------------------------------------------------------
// pure helpers
// ---------------------------------------------------------------------------

#[test]
fn find_latest_request_summary_walks_newest_dir_then_newest_md() {
    let dir = tempfile::tempdir().unwrap();
    // Older dir A, then (after an mtime gap) newer dir B.
    std::fs::create_dir_all(dir.path().join("A")).unwrap();
    std::fs::write(dir.path().join("A/a-old.md"), "a").unwrap();
    std::thread::sleep(Duration::from_millis(60));
    std::fs::create_dir_all(dir.path().join("B")).unwrap();
    std::fs::write(dir.path().join("B/b1.md"), "b1").unwrap();
    std::thread::sleep(Duration::from_millis(60));
    std::fs::write(dir.path().join("B/b2-newest.md"), "b2").unwrap();
    std::fs::write(dir.path().join("B/ignored.txt"), "x").unwrap();
    std::fs::create_dir_all(dir.path().join("B/nested-dir")).unwrap();

    let got = find_latest_request_summary(dir.path()).expect("some md");
    assert!(
        got.ends_with("b2-newest.md"),
        "newest md in newest dir, got {}",
        got
    );

    // Empty base dir → the `latest_dir?` None arm.
    let empty = tempfile::tempdir().unwrap();
    assert!(find_latest_request_summary(empty.path()).is_none());
}

#[test]
fn is_daily_nemesisbot_log_strict_match() {
    assert!(is_daily_nemesisbot_log("nemesisbot.2026-08-25"));
    assert!(!is_daily_nemesisbot_log("nemesisbot.log"));
    assert!(!is_daily_nemesisbot_log("nemesisbot.2026-8-25")); // not 10 chars
    assert!(!is_daily_nemesisbot_log("nemesisbot.2026-08-2")); // not 10 chars
    assert!(!is_daily_nemesisbot_log("nemesisbot.2026-AB-25")); // non-digits
    assert!(!is_daily_nemesisbot_log("app.2026-08-25")); // wrong prefix
}

#[test]
fn read_log_entries_tail_big_file_and_missing() {
    let dir = tempfile::tempdir().unwrap();

    // Small file (< 64KB): full read, no first-line drop, garbage filtered.
    let small = dir.path().join("small.jsonl");
    let mut body = String::new();
    for i in 0..3 {
        body.push_str(&format!("{{\"id\":{}}}\n", i));
    }
    body.push_str("not json at all\n");
    std::fs::write(&small, body).unwrap();
    // Tail window is over RAW lines, then parse-filtered — the garbage tail
    // line consumes one window slot, so n=2 yields only 1 parsed entry.
    let entries = read_log_entries(small.to_str().unwrap(), 2);
    assert_eq!(entries.len(), 1, "garbage line eats a window slot");
    assert_eq!(entries[0]["id"], 2);
    // n covering the whole file → all 3 valid lines, garbage dropped.
    let entries = read_log_entries(small.to_str().unwrap(), 10);
    assert_eq!(entries.len(), 3);
    assert_eq!(entries[0]["id"], 0);
    assert_eq!(entries[2]["id"], 2);

    // Big file (> 64KB): seek-to-tail path drops the first (partial) line.
    let big = dir.path().join("big.jsonl");
    let mut body = String::new();
    for i in 0..800 {
        // ~150 bytes/line × 800 ≈ 120KB > 64KB window.
        body.push_str(&format!(
            "{{\"id\":{},\"pad\":\"{}\"}}\n",
            i,
            "p".repeat(130)
        ));
    }
    std::fs::write(&big, body).unwrap();
    let entries = read_log_entries(big.to_str().unwrap(), 5);
    assert_eq!(entries.len(), 5);
    assert_eq!(entries[4]["id"], 799, "last line kept");
    assert_eq!(entries[0]["id"], 795, "exactly the 5-entry tail window");

    // Missing file → empty vec (open-error arm).
    let missing = dir.path().join("missing.jsonl");
    assert!(read_log_entries(missing.to_str().unwrap(), 5).is_empty());
}

#[test]
fn sanitize_map_masks_sensitive_keys_and_recurses() {
    let mut v = serde_json::json!({
        "model": "gpt-x",
        "api_key": "abcd1234",
        "auth_token": "ab",
        "password": "猎人密码很长",
        "my_secret": 12345,
        "credentials": { "client_token": "xyz987", "note": "keep" }
    });
    let map = v.as_object_mut().unwrap();
    sanitize_map(map);

    assert_eq!(v["model"], "gpt-x", "non-sensitive untouched");
    assert_eq!(v["api_key"], "abcd****");
    assert_eq!(v["auth_token"], "****", "short value fully masked");
    let pwd = v["password"].as_str().unwrap();
    assert!(
        pwd.starts_with("猎") && pwd.ends_with("****"),
        "multibyte floor, got {}",
        pwd
    );
    assert_eq!(v["my_secret"], 12345, "non-string value left as-is");
    // Nested object under a sensitive key recurses.
    assert_eq!(v["credentials"]["client_token"], "xyz9****");
    assert_eq!(v["credentials"]["note"], "keep");
}

#[test]
fn first_line_trunc_first_non_empty_line_and_ellipsis() {
    assert_eq!(
        first_line_trunc("\n\n  hello world  \nsecond", 20),
        "  hello world  "
    );
    assert_eq!(first_line_trunc("short", 10), "short");
    assert_eq!(first_line_trunc("0123456789A", 5), "01234…");
    assert_eq!(first_line_trunc("", 5), "");
    assert_eq!(first_line_trunc("   \n  \n", 5), "");
    // Multibyte: truncation never splits a character.
    let cjk = first_line_trunc(&"忆".repeat(80), 60);
    assert_eq!(cjk.chars().count(), 61, "60 chars + ellipsis");
    assert!(cjk.ends_with('…'));
}

// ---------------------------------------------------------------------------
// resolve_fork_store
// ---------------------------------------------------------------------------

#[test]
fn resolve_fork_store_without_home_is_503_and_with_home_is_ok() {
    // home = None → the SERVICE_UNAVAILABLE arm.
    let state_none = crate::api_handlers::AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "t".into(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".into())),
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
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        #[cfg(not(feature = "workflow"))]
        #[cfg(feature = "workflow")]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: std::sync::Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
    };
    match resolve_fork_store(&state_none) {
        Err((StatusCode::SERVICE_UNAVAILABLE, body)) => {
            assert_eq!(body.0["error"], "home not configured");
        }
        other => panic!("expected 503, got Ok: {}", other.is_ok()),
    }

    let dir = tempfile::tempdir().unwrap();
    let state_ok = make_state(Some(dir.path()));
    let store = resolve_fork_store(&state_ok).expect("fallback store");
    drop(store);
}

// ---------------------------------------------------------------------------
// turns / fork HTTP arms (house pattern: unique keys + cleanup)
// ---------------------------------------------------------------------------

fn make_router(state: Arc<AppState>) -> Router {
    Router::new()
        .route(
            "/api/chat/sessions/{id}/turns",
            axum::routing::get(handle_api_chat_session_turns),
        )
        .route(
            "/api/chat/sessions/{id}/fork",
            axum::routing::post(handle_api_chat_session_fork),
        )
        .with_state(state)
}

fn unique_sid(prefix: &str) -> String {
    format!(
        "{}s10b{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn turns_counts_leading_pre_user_rows_into_kept_messages() {
    let sid = unique_sid("lead");
    let key = chat_session_key(&sid);
    // A pre-user assistant row → the `leading` counter arm (1099-1101).
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "开场白（首条非 user 行）");
    nemesis_agent::chat_log::append_chat_log(&key, "user", "第一问");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "收尾回答");

    let state = make_state(None); // home unused by the turns read path
    let app = make_router(state);
    let req = axum::http::Request::builder()
        .uri(format!("/api/chat/sessions/{}/turns", sid))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;

    assert_eq!(v["total_turns"], 1);
    assert_eq!(v["total_messages"], 3);
    let turns = v["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1);
    // The projected (non-empty assistant) row wins end_preview over the user row.
    assert_eq!(turns[0]["preview"], "第一问");
    assert_eq!(turns[0]["end_preview"], "收尾回答");
    assert_eq!(turns[0]["turn_messages"], 2);
    // leading=1 is folded into the cumulative kept count.
    assert_eq!(turns[0]["kept_messages"], 3);

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn fork_with_title_writes_session_meta_sidecar() {
    let sid = unique_sid("titled");
    let key = chat_session_key(&sid);
    nemesis_agent::chat_log::append_chat_log(&key, "user", "第一问");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "回答一");
    nemesis_agent::chat_log::append_chat_log(&key, "user", "第二问");
    nemesis_agent::chat_log::append_chat_log(&key, "assistant", "回答二");

    let dir = tempfile::tempdir().unwrap();
    let state = make_state(Some(dir.path()));
    let app = make_router(state);
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/chat/sessions/{}/fork", sid))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(
            serde_json::json!({ "at_turn": 1, "title": "新分支标题" }).to_string(),
        ))
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let v = body_json(resp).await;
    assert_eq!(v["forked"], true);
    assert_eq!(v["chat_log_lines"], 2, "at_turn=1 keeps only turn 1");

    // The title arm wrote the sidecar meta for the NEW session.
    let new_key = v["new_key"].as_str().unwrap();
    assert_eq!(
        nemesis_agent::chat_log::read_session_meta(new_key).as_deref(),
        Some("新分支标题")
    );

    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::delete_chat_log(new_key);
}
