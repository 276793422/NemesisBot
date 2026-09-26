use super::*;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx() -> RequestContext {
    let state = Arc::new(crate::api_handlers::AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: None,
        signature_verify: None,
        skills_install_gate: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn close_echoes_session_id() {
    let ctx = make_ctx();
    let h = CanvasHandler;
    let r = h
        .handle_cmd(
            "close",
            Some(serde_json::json!({ "session_id": "sess-42" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["closed"], true);
    assert_eq!(r["session_id"], "sess-42");
}

#[tokio::test]
async fn close_without_session_id_is_honest_error() {
    let ctx = make_ctx();
    let h = CanvasHandler;
    // 缺 session_id / 空串 / 非字符串 / 缺 data 一律诚实报错。
    assert!(
        h.handle_cmd("close", Some(serde_json::json!({})), &ctx)
            .await
            .is_err()
    );
    assert!(
        h.handle_cmd("close", Some(serde_json::json!({ "session_id": "" })), &ctx)
            .await
            .is_err()
    );
    assert!(
        h.handle_cmd("close", Some(serde_json::json!({ "session_id": 3 })), &ctx)
            .await
            .is_err()
    );
    assert!(h.handle_cmd("close", None, &ctx).await.is_err());
}

#[tokio::test]
async fn unknown_command_errors() {
    let ctx = make_ctx();
    let h = CanvasHandler;
    let err = h
        .handle_cmd("open", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("unknown command"), "实际: {}", err);
}
