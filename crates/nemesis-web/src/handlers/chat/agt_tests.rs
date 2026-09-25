//! chat.rs AGT 覆盖率批次（2026-09-25）。与既有 handlers/tests 互补，聚焦
//! todo_get 的两个确定性 bail 臂：
//! - 非 sync 命令缺 session_id → "missing session_id"（81）
//! - session_id 在但 workspace 未配置且无项目归属 → "workspace not
//!   configured"（93-94：project_root None → or_else ctx.workspace None）

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn agt_ctx(workspace: Option<String>) -> RequestContext {
    let state = std::sync::Arc::new(AppState {
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: workspace.clone(),
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: std::sync::Arc::new(parking_lot::Mutex::new("m".to_string())),
        model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
        event_hub: std::sync::Arc::new(EventHub::new()),
        running: std::sync::Arc::new(AtomicBool::new(true)),
        session_manager: std::sync::Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
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
        webhook_rate_limiter: std::sync::Arc::new(
            crate::handlers::workflow::WebhookRateLimiter::new(),
        ),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn agt_todo_get_missing_session_id_bails() {
    let ctx = agt_ctx(Some("/tmp/agt-ws".to_string()));
    let err = ChatHandler
        .handle_cmd("todo_get", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
}

#[tokio::test]
async fn agt_todo_get_without_workspace_bails() {
    // bridge 未装配 → project_root None → or_else ctx.workspace None → Err。
    let ctx = agt_ctx(None);
    let err = ChatHandler
        .handle_cmd(
            "todo_get",
            Some(serde_json::json!({ "session_id": "agt-miss" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "workspace not configured");
}
