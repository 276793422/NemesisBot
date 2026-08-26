//! S10b (quality-hardening goal 冲刺, web 批次 2): ConfigHandler error arms and
//! CORS stubs the gated `mod tests` skips — missing config.json load failure,
//! invalid save payload, empty-path / invalid-result set_field, and the four
//! "CORS manager not connected" stub commands.

use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::config::ConfigHandler;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state(ws: Option<String>) -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: ws.clone(),
        home: ws.clone(),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    })
}

fn make_ctx(dir: &tempfile::TempDir, with_config: bool) -> RequestContext {
    if with_config {
        let config = nemesis_config::Config::default();
        std::fs::write(
            dir.path().join("config.json"),
            serde_json::to_string_pretty(&config).unwrap(),
        )
        .unwrap();
    }
    let ws = dir.path().to_string_lossy().to_string();
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state: make_state(Some(dir.path().to_string_lossy().to_string())),
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn config_get_with_malformed_config_file_errors() {
    // Missing config.json falls back to defaults (Ok) — a present-but-broken
    // file is what trips the load error arm.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{definitely not json").unwrap();
    let ctx = make_ctx(&dir, false);
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("failed to load config"), "{err}");
}

#[tokio::test]
async fn config_save_rejects_wrong_typed_payload() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();
    let err = handler
        .handle_cmd(
            "save",
            Some(serde_json::json!({ "model_list": "not-an-array" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config data"), "{err}");
}

#[tokio::test]
async fn config_set_field_empty_path_and_invalid_result_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let err = handler
        .handle_cmd("set_field", Some(serde_json::json!({ "path": "" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "empty path");

    // model_list must stay an array — a string breaks re-parse.
    let err = handler
        .handle_cmd(
            "set_field",
            Some(serde_json::json!({ "path": "model_list", "value": "oops" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("invalid config after field update"), "{err}");
}

#[tokio::test]
async fn config_cors_stubs_report_not_connected() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ConfigHandler::new();

    let list = handler.handle_cmd("cors.list", None, &ctx).await.unwrap().unwrap();
    assert_eq!(list["origins"].as_array().unwrap().len(), 0);
    assert!(list["message"].as_str().unwrap().contains("not connected"));

    let added = handler
        .handle_cmd("cors.add", Some(serde_json::json!({ "origin": "https://x.com" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(added["added"], false);

    let removed = handler
        .handle_cmd("cors.remove", Some(serde_json::json!({ "origin": "https://x.com" })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(removed["removed"], false);

    let toggled = handler
        .handle_cmd("cors.toggle", Some(serde_json::json!({ "enabled": true })), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(toggled["toggled"], false);
}
