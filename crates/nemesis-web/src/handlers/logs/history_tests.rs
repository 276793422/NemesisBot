//! T6 (U20) tests for the `logs.history_search` / `logs.history_reindex`
//! WSAPI commands. NOT security-gated: these commands depend only on
//! nemesis-agent (non-optional dep).
//!
//! Isolation note: the STATEFUL e2e (append → search → assert hit) moved to
//! the dedicated integration binary `tests/history_search_e2e.rs` — the lib
//! binary's ~3000 sibling tests bake the `default_path_manager()` singleton
//! home before any redirect could land, and under nextest (per-test process)
//! the in-process IDX_LOCK / HOME_RACE_LOCK below are void, so the e2e was
//! contending cross-process on the real global history_index.db (2026-10-10
//! flake). What remains here either fails before path access (input
//! validation) or can't fail on contention (reindex count is a number
//! regardless of BUSY). Do not move the e2e back — same contract as
//! nemesis-agent/tests/history_search_fts.rs.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

/// Serialize tests touching the global FTS index (mirrors nemesis-agent's
/// history_search/tests.rs IDX_LOCK).
static IDX_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
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
        // AppState dual-declares these two fields (workflow-gated real /
        // `Arc<()>` stub — api_handlers.rs). This module deliberately has NO
        // feature gate (history commands depend only on nemesis-agent), so the
        // fields must be built per-combo: without the workflow feature the
        // stubs get `Arc::new(())` (fixes `cargo test -p nemesis-web`
        // zero-feature compile, broken since this file landed in dd2e522).
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
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn test_history_search_rejects_missing_or_empty_query() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = LogsHandler;

    // Missing data object entirely.
    let err = h
        .handle_cmd("history_search", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"), "got: {err}");

    // Data present but no query field.
    let err = h
        .handle_cmd("history_search", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("query"), "got: {err}");

    // Blank query.
    let err = h
        .handle_cmd(
            "history_search",
            Some(serde_json::json!({"query": "   "})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("empty"), "got: {err}");
}

#[tokio::test]
async fn test_history_reindex_returns_session_count() {
    let _lock = IDX_LOCK.lock();
    let _home = crate::test_home::lock_home();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = LogsHandler;

    let out = h.handle_cmd("history_reindex", None, &ctx).await.unwrap();
    let v = out.expect("reindex returns a payload");
    assert!(
        v.get("reindexed_sessions")
            .and_then(|n| n.as_u64())
            .is_some(),
        "missing reindexed_sessions: {v}"
    );
}
