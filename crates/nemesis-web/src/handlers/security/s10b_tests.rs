//! S10b (quality-hardening goal 冲刺, web 批次 2): SecurityHandler arms the
//! gated `mod tests` skips — audit offset paging + malformed-line skipping +
//! decision normalization (approved→allow), direct flatten/extract fallback
//! arms, and config load/save error propagation.

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

fn audit_line(event_id: &str, decision: &str, danger: &str, ts: &str) -> String {
    serde_json::json!({
        "event_id": event_id,
        "request": { "op_type": "file_write", "danger_level": danger, "target": "C:/x.txt" },
        "decision": decision,
        "reason": "policy",
        "timestamp": ts,
    })
    .to_string()
}

fn make_ctx(ws: &str) -> RequestContext {
    use crate::api_handlers::AppState;
    use crate::events::EventHub;
    use crate::session::SessionManager;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::time::Instant;

    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.to_string()),
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
        board: None,
    });
    RequestContext {
        session_id: "s10b".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn security_audit_pages_skips_malformed_and_normalizes_decisions() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let log_dir = dir.path().join("logs/security_logs");
    std::fs::create_dir_all(&log_dir).unwrap();
    std::fs::write(
        log_dir.join("audit.jsonl"),
        format!(
            "{}\n{}\n{}\nnot-json-garbage\n{}\n",
            audit_line("e1", "denied", "HIGH", "2026-01-01T10:00:00Z"),
            audit_line("e2", "allowed", "MEDIUM", "2026-01-02T10:00:00Z"),
            audit_line("e3", "approved", "LOW", "2026-01-03T10:00:00Z"),
            audit_line("e4", "blocked", "CRITICAL", "2026-01-04T10:00:00Z"),
        ),
    )
    .unwrap();

    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);
    let resp = handler
        .handle_cmd("audit", Some(serde_json::json!({ "limit": 1, "offset": 1 })), &ctx)
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resp["total"], 4, "garbage line skipped, 4 valid entries");
    let entries = resp["entries"].as_array().unwrap();
    assert_eq!(entries.len(), 1, "limit=1 window");
    // Sorted desc by timestamp → offset 1 lands on e3.
    assert_eq!(entries[0]["id"], "e3");
    assert_eq!(entries[0]["result"], "allow", "approved normalizes to allow");
    assert_eq!(entries[0]["decision"], "approved");
    assert_eq!(entries[0]["operation"], "file_write");

    // First page (offset 0) is the newest entry with a deny-family decision.
    let first = handler
        .handle_cmd("audit", Some(serde_json::json!({ "limit": 2 })), &ctx)
        .await
        .unwrap()
        .unwrap();
    let first_entries = first["entries"].as_array().unwrap();
    assert_eq!(first_entries[0]["id"], "e4");
    assert_eq!(first_entries[0]["result"], "deny", "blocked → deny");
    assert_eq!(first_entries[1]["id"], "e3");
}

#[test]
fn flatten_audit_entry_and_extract_risk_level_fallback_arms() {
    // Missing request object → empty operation/target defaults.
    let flat = flatten_audit_entry(&serde_json::json!({
        "event_id": "e", "decision": "Allowed", "timestamp": "t"
    }));
    assert_eq!(flat["operation"], "");
    assert_eq!(flat["result"], "allow", "case-insensitive prefix");
    assert_eq!(flat["raw"]["event_id"], "e");

    // Empty decision → deny.
    let flat = flatten_audit_entry(&serde_json::json!({ "event_id": "e2" }));
    assert_eq!(flat["result"], "deny");
    assert_eq!(flat["policy"], "");

    // extract_risk_level: nested request wins, top-level fallback, unknown default.
    assert_eq!(
        extract_risk_level(&serde_json::json!({
            "request": { "danger_level": "HIGH" }, "risk_level": "LOW"
        })),
        "HIGH"
    );
    assert_eq!(
        extract_risk_level(&serde_json::json!({ "risk_level": "MEDIUM" })),
        "MEDIUM"
    );
    assert_eq!(extract_risk_level(&serde_json::json!({})), "unknown");
}

#[tokio::test]
async fn security_config_malformed_file_and_invalid_save_error() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);

    // Missing file → defaults (Ok); a present-but-broken file errors.
    std::fs::create_dir_all(dir.path().join("config")).unwrap();
    std::fs::write(dir.path().join("config/config.security.json"), "{nope").unwrap();
    let err = handler.handle_cmd("config.get", None, &ctx).await.unwrap_err();
    assert!(err.contains("failed to load security config"), "{err}");

    let err = handler
        .handle_cmd("config.save", Some(serde_json::Value::Null), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("invalid security config"), "{err}");
}
