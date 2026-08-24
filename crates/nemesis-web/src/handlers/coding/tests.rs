//! Tests for `CodingHandler` (P2-1).
//!
//! `lsp_status` asserts structure, NOT availability — which servers are on
//! PATH is machine-dependent, so a test asserting `available == false` would
//! break on a dev box with rust-analyzer installed (and vice versa). The
//! registry table itself (5 languages, commands) is compile-time data.

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::RequestContext as Ctx;
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
    });
    Ctx {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn lsp_status_lists_five_languages_with_probe_flags() {
    let handler = CodingHandler;
    let r = handler.lsp_status().unwrap().unwrap();
    let langs = r["languages"].as_array().expect("languages array");
    assert_eq!(langs.len(), 5, "registry has exactly 5 language rows");
    let labels: Vec<&str> = langs.iter().filter_map(|l| l["label"].as_str()).collect();
    assert_eq!(
        labels,
        vec!["rust", "go", "typescript/javascript", "python", "c/c++"],
    );
    for l in langs {
        assert!(l["command"].is_string(), "command present: {l}");
        assert!(l["available"].is_boolean(), "available is bool: {l}");
    }
    let count = r["available_count"].as_u64().expect("count") as usize;
    // Internal consistency: count matches the per-language flags.
    let flagged = langs
        .iter()
        .filter(|l| l["available"].as_bool().unwrap())
        .count();
    assert_eq!(count, flagged);
    assert_eq!(r["tool_would_register"], count > 0);
}

#[tokio::test]
async fn coding_config_reads_defaults_from_fresh_home() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = CodingHandler;
    let r = handler.config(&ctx).unwrap().unwrap();
    // Defaults per nemesis-config: all three tools off, empty mode strings.
    assert_eq!(r["lsp"]["enabled"], false);
    assert_eq!(r["claude_code"]["enabled"], false);
    assert_eq!(r["codex"]["enabled"], false);
    assert_eq!(r["claude_code"]["permission_mode"], "");
    assert_eq!(r["codex"]["sandbox"], "");
}

#[tokio::test]
async fn coding_config_reads_written_sections() {
    let dir = tempfile::tempdir().unwrap();
    let home = dir.path();
    // config.json with claude_code/codex/lsp sections — the shape
    // config.set_field produces after the UI saves (disk fallback path of
    // load_config; the live ConfigStore is not set in tests).
    let cfg = serde_json::json!({
        "agents": {
            "claude_code_tool": { "enabled": true, "permission_mode": "plan" },
            "codex_tool": { "enabled": true, "sandbox": "workspace_write" },
            "lsp_tool": { "enabled": true },
        }
    });
    std::fs::write(home.join("config.json"), serde_json::to_string(&cfg).unwrap()).unwrap();
    let ctx = make_ctx(&dir);
    let handler = CodingHandler;
    let r = handler.config(&ctx).unwrap().unwrap();
    assert_eq!(r["lsp"]["enabled"], true);
    assert_eq!(r["claude_code"]["enabled"], true);
    assert_eq!(r["claude_code"]["permission_mode"], "plan");
    assert_eq!(r["codex"]["enabled"], true);
    assert_eq!(r["codex"]["sandbox"], "workspace_write");
}

#[tokio::test]
async fn coding_unknown_command_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let handler = CodingHandler;
    let r = handler.handle_cmd("bogus", None, &ctx).await;
    assert!(r.is_err());
}
