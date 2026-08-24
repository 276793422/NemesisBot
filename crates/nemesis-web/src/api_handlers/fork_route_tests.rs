// P3-1 (2026-08-24 UI entry gap): HTTP-level tests for the session-fork
// dialog endpoints (`GET /api/chat/sessions/{id}/turns`,
// `POST /api/chat/sessions/{id}/fork`).
//
// The handlers depend only on nemesis-agent (non-optional dep), so this module
// has NO feature gate — it runs under every combo including
// `--no-default-features`.
//
// Isolation (house pattern from handlers/logs/history_tests.rs +
// nemesis-agent session_fork/tests.rs): `agent_loop` is None in the test
// state, so requests exercise the FALLBACK store branch over a tempdir home
// (this is the Z1-shaped path — fresh store loads session files from disk at
// construction). chat_log writes resolve through the GLOBAL default path
// manager, so tests use nanos-unique session keys and delete_chat_log for
// cleanup.

use super::*;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::Router;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tower::ServiceExt;

fn make_state(
    dir: &tempfile::TempDir,
    auth_token: &str,
) -> Arc<AppState> {
    let ws = dir.path().to_string_lossy().to_string();
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
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
        // None → resolve_fork_store takes the fallback branch (fresh store
        // over <home>/workspace/sessions), which is the branch under test.
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        // AppState dual-declares these two fields (workflow-gated real /
        // `Arc<()>` stub — api_handlers.rs); build per-combo so this module
        // compiles without the workflow feature too.
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
    })
}

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

fn unique_sid() -> String {
    format!(
        "p31fork{}",
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
}

fn msg(role: &str, content: &str, timestamp: &str) -> nemesis_agent::session::StoredMessage {
    nemesis_agent::session::StoredMessage {
        role: role.to_string(),
        content: content.to_string(),
        tool_calls: vec![],
        tool_call_id: None,
        timestamp: timestamp.to_string(),
        reasoning_content: None,
        tool_name: None,
        tool_result_projection: None,
    }
}

/// Seed a 3-turn session (system + user/assistant exchanges, one tool hop in
/// turn 2) into `<home>/workspace/sessions` — the directory the fallback
/// store reads — plus matching chat_log lines under the global log dir.
/// Layout: [sys, u1, a1, u2, tool, a2, u3, a3] = 8 messages.
fn seed_session(home: &std::path::Path, key: &str) {
    let store = nemesis_agent::session::SessionStore::new_with_storage(
        home.join("workspace").join("sessions"),
    );
    store.get_or_create(key);
    store.set_history(
        key,
        vec![
            msg("system", "You are Nemesis.", "t0"),
            msg("user", "第一问：你好\n第二行不进预览", "t1"),
            msg("assistant", "回答一", "t2"),
            msg("user", "第二问：查天气", "t3"),
            msg("tool", "tool result", "t4"),
            msg("assistant", "回答二", "t5"),
            msg("user", "第三问：再见", "t6"),
            msg("assistant", "回答三", "t7"),
        ],
    );
    store.save(key).unwrap();

    nemesis_agent::chat_log::append_chat_log(key, "user", "第一问：你好");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "回答一");
    nemesis_agent::chat_log::append_chat_log(key, "user", "第二问：查天气");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "回答二");
    nemesis_agent::chat_log::append_chat_log(key, "user", "第三问：再见");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "回答三");
}

async fn oneshot(app: Router, req: axum::http::Request<axum::body::Body>) -> axum::response::Response {
    app.oneshot(req).await.unwrap()
}

async fn body_json(resp: axum::response::Response) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 8 * 1024 * 1024)
        .await
        .unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn test_turns_table_counts_and_previews() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();
    let key = chat_session_key(&sid);
    seed_session(dir.path(), &key);

    let req = axum::http::Request::builder()
        .uri(format!("/api/chat/sessions/{sid}/turns"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "")), req).await;
    assert_eq!(resp.status(), 200);
    let v = body_json(resp).await;

    assert_eq!(v["session_id"], sid);
    assert_eq!(v["total_turns"], 3);
    assert_eq!(v["total_messages"], 8);

    let turns = v["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 3);
    // Turn rows: preview = first line of the USER message; kept_messages is
    // cumulative INCLUDING the leading system prompt (fork-cut semantics).
    // end_preview = the turn's last non-empty user/assistant first line —
    // what a fork at that turn ends on (2026-08-25 fix contract).
    assert_eq!(turns[0]["turn"], 1);
    assert_eq!(turns[0]["preview"], "第一问：你好");
    assert_eq!(turns[0]["end_preview"], "回答一");
    assert_eq!(turns[0]["time"], "t1");
    assert_eq!(turns[0]["turn_messages"], 2);
    assert_eq!(turns[0]["kept_messages"], 3); // sys + u1 + a1
    assert_eq!(turns[1]["turn"], 2);
    assert_eq!(turns[1]["turn_messages"], 3); // u2 + tool + a2
    assert_eq!(turns[1]["kept_messages"], 6);
    assert_eq!(turns[1]["end_preview"], "回答二"); // tool rows never preview
    assert_eq!(turns[2]["turn"], 3);
    assert_eq!(turns[2]["preview"], "第三问：再见");
    assert_eq!(turns[2]["end_preview"], "回答三");
    assert_eq!(turns[2]["kept_messages"], 8); // whole history

    nemesis_agent::chat_log::delete_chat_log(&key);
}

#[tokio::test]
async fn test_turns_unknown_session_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();
    let req = axum::http::Request::builder()
        .uri(format!("/api/chat/sessions/{sid}/turns"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "")), req).await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_turns_and_fork_require_auth_token() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();

    let req = axum::http::Request::builder()
        .uri(format!("/api/chat/sessions/{sid}/turns"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "secret")), req).await;
    assert_eq!(resp.status(), 401);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/chat/sessions/{sid}/fork"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "secret")), req).await;
    assert_eq!(resp.status(), 401);
}

#[tokio::test]
async fn test_fork_creates_new_session_and_switch_target() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();
    let key = chat_session_key(&sid);
    seed_session(dir.path(), &key);

    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/chat/sessions/{sid}/fork"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from(r#"{"at_turn": 2}"#))
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "")), req).await;
    assert_eq!(resp.status(), 200);
    let v = body_json(resp).await;

    assert_eq!(v["forked"], true);
    assert_eq!(v["at_turn"], 2);
    assert_eq!(v["kept_messages"], 6); // sys + turns 1..2 complete
    assert_eq!(v["dropped_messages"], 2); // u3 + a3 stay only in the source
    assert_eq!(v["source_session_id"], sid);
    let new_key = v["new_key"].as_str().unwrap().to_string();
    let new_sid = v["session_id"].as_str().unwrap().to_string();
    assert!(new_key.starts_with(&format!("{key}__fork")), "new_key={new_key}");
    assert!(chat_session_key(&new_sid).ends_with(&new_key), "session_id must map back onto new_key");

    // New session file exists on disk under the fallback-store dir and its
    // history is exactly the 6-message prefix (Z1 fork semantics).
    let sessions_dir = dir.path().join("workspace").join("sessions");
    let safe = new_key.replace(':', "_");
    assert!(sessions_dir.join(format!("{safe}.json")).exists(), "fork file missing");
    let fresh = nemesis_agent::session::SessionStore::new_with_storage(&sessions_dir);
    let history = fresh.get_history(&new_key);
    assert_eq!(history.len(), 6);
    assert_eq!(history.last().unwrap().content, "回答二");

    // Source untouched: full 8-message history still on disk.
    let source_history = fresh.get_history(&key);
    assert_eq!(source_history.len(), 8);

    // chat_log projected FROM THE STORE PREFIX (2026-08-25 fix): the
    // user/assistant rows of messages[..cut], timestamps from the store.
    // seed layout [sys,u1,a1,u2,tool,a2,u3,a3] cut at 6 → 4 rows, ending
    // on the picked turn's final assistant reply "回答二" (ts t5).
    assert_eq!(v["chat_log_lines"].as_u64().unwrap(), 4);
    let (forked_log, _n, _m, _o) = nemesis_agent::chat_log::read_chat_log(&new_key, 50, None);
    assert_eq!(forked_log.len(), 4);
    // Full store content is projected verbatim (the live append path logs
    // full message bodies too — the seed's hand-written log lines above
    // only carried the first line, which is a fixture shortcut, not the
    // real format).
    assert_eq!(forked_log[0]["content"], "第一问：你好\n第二行不进预览");
    assert_eq!(
        forked_log.last().unwrap()["content"], "回答二",
        "fork must END on the picked turn's final assistant reply"
    );
    assert_eq!(forked_log.last().unwrap()["timestamp"], "t5");

    // Cleanup the global-dir artifacts (nanos-unique keys).
    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::delete_chat_log(&new_key);
}

#[tokio::test]
async fn test_fork_unknown_session_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();
    let req = axum::http::Request::builder()
        .method("POST")
        .uri(format!("/api/chat/sessions/{sid}/fork"))
        .header("content-type", "application/json")
        .body(axum::body::Body::from("{}"))
        .unwrap();
    let resp = oneshot(make_router(make_state(&dir, "")), req).await;
    assert_eq!(resp.status(), 404);
}

/// Prove the routes are wired into the REAL router the gateway serves (same
/// construction as server::tests / sdk_route_tests).
#[tokio::test]
async fn test_fork_routes_registered_in_full_router() {
    let config = crate::server::WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let app = crate::server::WebServer::new(config).build_router();
    let sid = unique_sid();
    let req = axum::http::Request::builder()
        .uri(format!("/api/chat/sessions/{sid}/turns"))
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = oneshot(app, req).await;
    // The full router's internal AppState has home=None, so the handler's
    // resolve_fork_store takes the SERVICE_UNAVAILABLE branch — which is
    // exactly the proof we want: OUR handler produced the response (a
    // missing route would fall through to the SPA fallback 200 HTML).
    assert_eq!(resp.status(), 503);
}
