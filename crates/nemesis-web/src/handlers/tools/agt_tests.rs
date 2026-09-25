//! tools.rs AGT 覆盖率批次（2026-09-25）：显式 session_id 契约臂（71-76）——
//! 带 session_id 的工具命令走 resolve_session_loop 归属解析（bridge 未装配
//! + 主 loop 未装配 → "agent loop not running" 诚实上抛）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

#[tokio::test]
async fn agt_tools_with_session_id_resolves_loop_and_bails_without_one() {
    let state = std::sync::Arc::new(AppState {
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: None,
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
    let ctx = RequestContext {
        session_id: "agt".to_string(),
        chat_id: "agt".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    };

    let err = ToolsHandler
        .handle_cmd(
            "list",
            Some(serde_json::json!({ "session_id": "agt-s1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "agent loop not running");
}
