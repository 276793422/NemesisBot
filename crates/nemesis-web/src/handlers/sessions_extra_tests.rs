//! Sessions handler 补测（Phase 3 覆盖率，2026-08-25）。
//!
//! 既有 `handlers/tests.rs` 已覆盖 list happy path（含 web 前缀剥离）与
//! delete 级联停用 cron。这里补齐剩余可达缺口：各命令缺参的 bail 臂
//! （rename/delete/clear/export 的 missing session_id / missing title）。
//! create/rename 的 happy path 会经 `chat_log::write_session_meta` 写
//! `default_path_manager()` 单例 home（进程级 OnceLock，web 测试进程会烤到
//! `~/.nemesisbot`）——为不污染真实 home，明确不测（豁免记录 §9.4）。

use super::sessions::SessionsHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

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

#[tokio::test]
async fn rename_bails_on_missing_fields() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SessionsHandler;

    // 缺 session_id（title 在）。
    let err = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({ "title": "t" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");

    // 缺 title（session_id 在）。
    let err = h
        .handle_cmd(
            "rename",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "missing title");

    // data 整个缺。
    let err = h.handle_cmd("rename", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing session_id");
}

#[tokio::test]
async fn delete_clear_export_bail_on_missing_session_id() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let h = SessionsHandler;

    for cmd in ["delete", "clear", "export"] {
        let err = h
            .handle_cmd(cmd, Some(serde_json::json!({ "title": "x" })), &ctx)
            .await
            .unwrap_err();
        assert_eq!(err, "missing session_id", "cmd={cmd}");
    }

    // data 缺（export 路径）。
    let err = h.handle_cmd("export", None, &ctx).await.unwrap_err();
    assert_eq!(err, "missing session_id");
}

#[tokio::test]
async fn unknown_cmd_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = SessionsHandler
        .handle_cmd("nope", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown sessions cmd"), "{err}");
}
