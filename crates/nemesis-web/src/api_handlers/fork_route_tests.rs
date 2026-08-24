// P3-1 (2026-08-24 UI entry gap): HTTP-level tests for the session-fork
// dialog endpoints (`GET /api/chat/sessions/{id}/turns`,
// `POST /api/chat/sessions/{id}/fork`).
//
// ROUND-3 CONTRACT (2026-08-25 fork 第三轮): the chat_log jsonl is the
// single source of truth for turn semantics — the turns table counts jsonl
// rows and the fork copies them verbatim; the SessionStore side is derived
// from the same rows. The fixtures deliberately seed a DIVERGENT, polluted
// store (compaction-truncated August content + tool junk) next to the
// clean jsonl, so any regression back to store-coordinate counting fails
// these tests loudly (previews/counts would show the August content).
//
// The handlers depend only on nemesis-agent (non-optional dep), so this
// module has NO feature gate — it runs under every combo including
// `--no-default-features`.
//
// Isolation (house pattern from handlers/logs/history_tests.rs +
// nemesis-agent session_fork/tests.rs): `agent_loop` is None in the test
// state, so the fork WRITE path exercises the FALLBACK store branch over a
// tempdir home. chat_log writes resolve through the GLOBAL default path
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

/// Seed a session with a DELIBERATELY DIVERGENT pair of stores:
/// - chat_log jsonl = the TRUTH: 3 clean turns (u,a) × 3 = 6 rows, one
///   assistant row carrying a model badge (verbatim-copy proof). Written
///   through the GLOBAL log dir (nanos-unique keys).
/// - SessionStore = the polluted cache (round-3 regression shape): a
///   system row + tool rows + DIFFERENT ("august") content, written under
///   `<home>/workspace/sessions` — the directory the fallback store reads.
/// Any endpoint still counting/copying from the store would surface the
/// august content and fail the assertions.
fn seed_session(home: &std::path::Path, key: &str) {
    nemesis_agent::chat_log::append_chat_log(key, "user", "第一问：你好\n第二行不进预览");
    nemesis_agent::chat_log::append_chat_log_with_model(
        key,
        "assistant",
        "回答一",
        Some("zhipu/glm-4.7"),
    );
    nemesis_agent::chat_log::append_chat_log(key, "user", "第二问：查天气");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "回答二");
    nemesis_agent::chat_log::append_chat_log(key, "user", "第三问：再见");
    nemesis_agent::chat_log::append_chat_log(key, "assistant", "回答三");

    let store = nemesis_agent::session::SessionStore::new_with_storage(
        home.join("workspace").join("sessions"),
    );
    store.get_or_create(key);
    store.set_history(
        key,
        vec![
            msg("system", "You are Nemesis.", "2026-08-05T00:00:00+08:00"),
            msg("user", "august question 1", "2026-08-05T00:01:00+08:00"),
            msg("assistant", "让我先查一下……", "2026-08-05T00:02:00+08:00"),
            msg("tool", "tool junk", "2026-08-05T00:03:00+08:00"),
            msg("assistant", "", "2026-08-05T00:04:00+08:00"),
            msg("assistant", "august answer 1", "2026-08-05T00:05:00+08:00"),
        ],
    );
    store.save(key).unwrap();
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
async fn test_turns_table_counts_jsonl_not_the_divergent_store() {
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
    // ROUND 3: total/kept/turn_messages are JSONL ROW counts (jsonl never
    // contains the system prompt or tool rows — 6 rows, 2 per turn). The
    // divergent store's 6 messages (incl. system/tool/empty) must not show
    // up anywhere in these numbers.
    assert_eq!(v["total_messages"], 6);

    let (src_rows, _, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 50, None);
    let turns = v["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 3);
    // Turn rows: preview = first line of the USER row; kept_messages is
    // cumulative over jsonl rows (fork-cut semantics). end_preview = the
    // turn's last projected row — what a fork at that turn ends on.
    assert_eq!(turns[0]["turn"], 1);
    assert_eq!(turns[0]["preview"], "第一问：你好");
    assert_eq!(turns[0]["end_preview"], "回答一");
    assert_eq!(turns[0]["time"], src_rows[0]["timestamp"]);
    assert_eq!(turns[0]["turn_messages"], 2);
    assert_eq!(turns[0]["kept_messages"], 2);
    assert_eq!(turns[1]["turn"], 2);
    assert_eq!(turns[1]["preview"], "第二问：查天气");
    assert_eq!(turns[1]["end_preview"], "回答二");
    assert_eq!(turns[1]["turn_messages"], 2);
    assert_eq!(turns[1]["kept_messages"], 4);
    assert_eq!(turns[2]["turn"], 3);
    assert_eq!(turns[2]["preview"], "第三问：再见");
    assert_eq!(turns[2]["end_preview"], "回答三");
    assert_eq!(turns[2]["kept_messages"], 6); // whole log
    // No august/store content anywhere in the turn table.
    let raw = v.to_string();
    assert!(!raw.contains("august"), "store content leaked into turns: {raw}");

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

/// A store-only session (no jsonl) is NOT forkable/listable in the dialog —
/// round 3 defines existence by the jsonl. The store's presence must not
/// make the turns endpoint serve store-coordinate content.
#[tokio::test]
async fn test_turns_store_only_session_is_404() {
    let dir = tempfile::tempdir().unwrap();
    let sid = unique_sid();
    let key = chat_session_key(&sid);
    // Seed ONLY the store side (no jsonl).
    let store = nemesis_agent::session::SessionStore::new_with_storage(
        dir.path().join("workspace").join("sessions"),
    );
    store.get_or_create(&key);
    store.set_history(
        &key,
        vec![
            msg("system", "s", "t0"),
            msg("user", "store-only question", "t1"),
            msg("assistant", "store-only answer", "t2"),
        ],
    );
    store.save(&key).unwrap();

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
async fn test_fork_copies_jsonl_verbatim_and_mirrors_store() {
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
    assert_eq!(v["kept_messages"], 4); // turns 1..2 complete = 4 jsonl rows
    assert_eq!(v["dropped_messages"], 2); // u3 + a3 stay only in the source
    assert_eq!(v["summary_kept"], false); // round-3 rule: never carried
    assert_eq!(v["source_session_id"], sid);
    let new_key = v["new_key"].as_str().unwrap().to_string();
    let new_sid = v["session_id"].as_str().unwrap().to_string();
    assert!(new_key.starts_with(&format!("{key}__fork")), "new_key={new_key}");
    assert!(chat_session_key(&new_sid).ends_with(&new_key), "session_id must map back onto new_key");

    // ROUND 3: the new chat_log is a VERBATIM copy of the source jsonl's
    // first 4 rows — content, timestamps AND the model badge preserved;
    // none of the divergent store's august content may appear.
    assert_eq!(v["chat_log_lines"].as_u64().unwrap(), 4);
    let (src_rows, _, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 50, None);
    let (forked_log, n, _m, _o) = nemesis_agent::chat_log::read_chat_log(&new_key, 50, None);
    assert_eq!(n, 4);
    for (i, v) in forked_log.iter().enumerate() {
        assert_eq!(v["content"], src_rows[i]["content"], "forked row {i} content");
        assert_eq!(v["timestamp"], src_rows[i]["timestamp"], "forked row {i} ts");
    }
    assert_eq!(forked_log[1]["model"], "zhipu/glm-4.7", "model badge must survive verbatim copy");
    assert_eq!(
        forked_log.last().unwrap()["content"], "回答二",
        "fork must END on the picked turn's final assistant reply"
    );
    let raw = serde_json::to_string(&forked_log).unwrap();
    assert!(!raw.contains("august"), "store content leaked into fork: {raw}");

    // New session's STORE (model context) is mirrored from the same 4 rows
    // via the shared self-heal mapping — no system/tool rows, real ts.
    let sessions_dir = dir.path().join("workspace").join("sessions");
    let safe = new_key.replace(':', "_");
    assert!(sessions_dir.join(format!("{safe}.json")).exists(), "fork store file missing");
    let fresh = nemesis_agent::session::SessionStore::new_with_storage(&sessions_dir);
    let history = fresh.get_history(&new_key);
    assert_eq!(history.len(), 4);
    assert!(history.iter().all(|m| m.role == "user" || m.role == "assistant"));
    assert_eq!(history.last().unwrap().content, "回答二");
    assert_eq!(history[1].timestamp, src_rows[1]["timestamp"].as_str().unwrap());
    // The divergent SOURCE store is untouched.
    assert_eq!(fresh.get_history(&key).len(), 6);
    // Source jsonl untouched: still 6 rows.
    let (after, total, _, _) = nemesis_agent::chat_log::read_chat_log(&key, 50, None);
    assert_eq!(total, 6);
    assert_eq!(after.len(), 6);

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
    // ROUND 3: the turns endpoint reads jsonl directly and no longer needs
    // a store/home, so a home-less full router now answers 404 (no jsonl
    // for a nanos-unique sid) — still proof OUR handler produced the
    // response (a missing route would fall through to the SPA fallback
    // 200 HTML).
    assert_eq!(resp.status(), 404);
}
