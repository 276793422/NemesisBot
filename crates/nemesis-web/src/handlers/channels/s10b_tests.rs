//! S10b (quality-hardening goal 冲刺, web 批次 2): ChannelsHandler error arms
//! and sensitive-field masking the gated `mod tests` skips — missing
//! config.json load failure, update without a config payload, update whose
//! payload fails channels re-parse, and `mask_sensitive_fields` recursion.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_state() -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
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
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: None,
        home: Some(dir.path().to_string_lossy().to_string()),
        state: make_state(),
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn channels_list_with_malformed_config_file_errors() {
    // Missing config.json falls back to defaults (Ok) — a broken file trips
    // the load error arm.
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("config.json"), "{definitely not json").unwrap();
    let ctx = make_ctx(&dir, false);
    let handler = ChannelsHandler::new();
    let err = handler.handle_cmd("list", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to load config"), "{err}");
}

#[tokio::test]
async fn channels_update_missing_config_field_and_bad_reparse_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, true);
    let handler = ChannelsHandler::new();

    // "web" exists in default channels config, but the payload has no
    // "config" sub-object.
    let err = handler
        .handle_cmd("update", Some(serde_json::json!({ "name": "web" })), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing config field");

    // `enabled` must stay a bool — a string breaks the channels re-parse.
    let err = handler
        .handle_cmd(
            "update",
            Some(serde_json::json!({
                "name": "web",
                "config": { "enabled": "yes" }
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("failed to parse updated channels"), "{err}");
}

#[test]
fn mask_sensitive_fields_masks_sensitive_and_recurses() {
    let v = serde_json::json!({
        "name": "web",
        "token": "short",
        "api_key": "sk-1234567890abcdef",
        "empty_token": "",
        "nested": {
            "password": "hunter2password-long",
            "plain": "visible"
        },
        "list": [{ "secret": "0123456789abc" }],
    });
    let out = mask_sensitive_fields(v);
    assert_eq!(out["name"], "web");
    assert_eq!(out["token"], "****", "short values fully masked");
    assert_eq!(out["api_key"], "sk-1****cdef");
    assert_eq!(out["empty_token"], "", "empty sensitive values left empty");
    assert_eq!(out["nested"]["password"], "hunt****long");
    assert_eq!(out["nested"]["plain"], "visible");
    assert_eq!(out["list"][0]["secret"], "0123****9abc");
}
