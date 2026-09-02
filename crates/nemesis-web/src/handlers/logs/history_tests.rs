//! T6 (U20) tests for the `logs.history_search` / `logs.history_reindex`
//! WSAPI commands. NOT security-gated: these commands depend only on
//! nemesis-agent (non-optional dep).
//!
//! Isolation note (same discipline as nemesis-agent's history_search tests):
//! `history_search` resolves session_logs through the GLOBAL default path
//! manager — not this handler's `workspace` param — so the e2e test runs
//! against the real global session_logs dir with nanos-unique session keys
//! and cleans up after itself. A module-level lock serializes the tests in
//! this binary; cross-process concurrency with nemesis-agent's own test
//! binary is absorbed by the retry loop (a transient SQLITE_BUSY makes the
//! silent reindex miss rows; re-appending changes mtime so the retry's
//! reindex picks the file up again).

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

fn unique_marker(prefix: &str) -> String {
    format!(
        "zzq{}{}",
        prefix,
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    )
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

/// End-to-end through the real global session_logs dir: append via
/// chat_log, search through the WSAPI handler, assert the hit carries the
/// file-stem session_key that `logs.session_detail` expects.
#[tokio::test]
async fn test_history_search_finds_appended_message_e2e() {
    let _lock = IDX_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = LogsHandler;

    let marker = unique_marker("marker");
    let key = format!("test:hsearch:{}", marker);
    nemesis_agent::chat_log::delete_chat_log(&key);
    nemesis_agent::chat_log::append_chat_log(&key, "user", &format!("please locate {marker} now"));

    // The handler reindexes (mtime-incremental) before searching, so the
    // freshly appended line is findable on the first call. Retry absorbs
    // transient cross-process SQLITE_BUSY on the shared index db: a failed
    // reindex doesn't record mtime; re-appending bumps it so the retry's
    // reindex revisits the file.
    let stem = key.replace(':', "_");
    let mut hits: Vec<serde_json::Value> = Vec::new();
    for _ in 0..3 {
        nemesis_agent::chat_log::append_chat_log(&key, "user", &format!("retry probe {marker}"));
        let out = h
            .handle_cmd(
                "history_search",
                Some(serde_json::json!({"query": marker, "limit": 20})),
                &ctx,
            )
            .await
            .unwrap()
            .expect("search returns a payload");
        let found = out
            .get("hits")
            .and_then(|v| v.as_array())
            .cloned()
            .unwrap_or_default();
        if found
            .iter()
            .any(|hit| hit.get("session_key").and_then(|s| s.as_str()) == Some(stem.as_str()))
        {
            hits = found;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    }
    assert!(
        !hits.is_empty(),
        "marker {marker} must be findable via logs.history_search"
    );

    // Hit shape: stem session_key (== what session_detail expects), role,
    // snippet around the match, plus envelope fields.
    let hit = hits
        .iter()
        .find(|hit| hit.get("session_key").and_then(|s| s.as_str()) == Some(stem.as_str()))
        .unwrap();
    assert_eq!(
        hit.get("role").and_then(|r| r.as_str()),
        Some("user"),
        "hit role: {hit}"
    );
    let snippet = hit.get("snippet").and_then(|s| s.as_str()).unwrap_or("");
    assert!(snippet.contains(&marker), "snippet: {snippet}");
    // No cross-session leak: every hit's session_key must be our stem or at
    // least contain the unique marker (unique per run, so only ours).
    for other in &hits {
        let sk = other
            .get("session_key")
            .and_then(|s| s.as_str())
            .unwrap_or("");
        assert!(
            sk.contains(&marker),
            "foreign session leaked in: {sk} (marker {marker})"
        );
    }

    // limit=1 caps the result count.
    let out = h
        .handle_cmd(
            "history_search",
            Some(serde_json::json!({"query": marker, "limit": 1})),
            &ctx,
        )
        .await
        .unwrap()
        .expect("search returns a payload");
    let capped = out.get("hits").and_then(|v| v.as_array()).map(|a| a.len());
    assert_eq!(capped, Some(1), "limit=1 must cap hits: {out}");
    assert_eq!(
        out.get("query").and_then(|q| q.as_str()),
        Some(marker.as_str())
    );

    // Unknown subcommand still errors.
    let err = h.handle_cmd("history_nope", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command"), "got: {err}");

    nemesis_agent::chat_log::delete_chat_log(&key);
}
