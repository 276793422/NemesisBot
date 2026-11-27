//! BUG-20261128-016 红证+回归锁：forge 反思（reflect）/ 技能行为测试（test）
//! 两条烧 token 的 lane 在急停（estop）期间必须冻结。
//! - reflect = judge LLM 调用（resolve_judge_caller 现场造 provider 裸调）；
//! - test = forge_test_cmd → K1 runner（独立装配自带新鲜 estop，gateway 急停
//!   传导不到它）——闸必须在 WSAPI 入口挡住，不能指望 runner 内部。
//!
//! 既有先例：learn.run（forge.rs 同文件闸）+ cluster tasks.submit（EST-04）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::ModuleHandler;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn forge_make_ctx(
    dir: &tempfile::TempDir,
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
) -> crate::ws_router::RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop,
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    crate::ws_router::RequestContext {
        session_id: "forge-estop".to_string(),
        chat_id: "forge-estop".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn forge_reflect_frozen_under_estop() {
    let dir = tempfile::tempdir().unwrap();
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let ctx = forge_make_ctx(&dir, Some(estop));
    let err = ForgeHandler::new()
        .handle_cmd("reflect", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "err: {err}");
}

#[tokio::test]
async fn forge_test_frozen_under_estop() {
    let dir = tempfile::tempdir().unwrap();
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    estop.trigger();
    let ctx = forge_make_ctx(&dir, Some(estop));
    let err = ForgeHandler::new()
        .handle_cmd(
            "test",
            Some(serde_json::json!({ "name": "no-such-skill" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("急停"), "err: {err}");
}
