//! F3 (devtool-upgrade 阶段 5): SecurityHandler 的 approvals.list / approvals.clear
//! 命令——「总是允许」规则表管理（纯文件 IO：nemesis-web 不依赖
//! nemesis-security，条目原样透传；路径唯一真相源 nemesis-path）。

use super::*;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;

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
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
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
        session_id: "f3".to_string(),
        chat_id: "chat".to_string(),
        workspace: Some(ws.to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn rules_file(ws: &str) -> std::path::PathBuf {
    nemesis_path::resolve_approval_rules_path_in_workspace(std::path::Path::new(ws))
}

#[tokio::test]
async fn approvals_list_missing_file_is_empty_and_entries_passthrough() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let ctx = make_ctx(&ws);

    // 从未写过规则 → 空表（常态，不报错）。
    let resp = handler
        .handle_cmd("approvals.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["rules"].as_array().unwrap().len(), 0);

    // 落两条规则 → 原样透传（ nemesis-web 不解释条目形状）。
    let path = rules_file(&ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!([
            { "op": "process_exec", "pattern": "cargo test *", "action": "allow", "created_at": "t1" },
            { "op": "file_write", "pattern": "/tmp/a.txt", "action": "allow", "created_at": "t2" }
        ])
        .to_string(),
    )
    .unwrap();
    let resp = handler
        .handle_cmd("approvals.list", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    let rules = resp["rules"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["pattern"], "cargo test *");
    assert_eq!(rules[1]["op"], "file_write");
}

#[tokio::test]
async fn approvals_list_malformed_file_is_loud_error_not_silent_empty() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let path = rules_file(&ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(&path, "{broken").unwrap();

    let handler = SecurityHandler::new();
    let err = handler
        .handle_cmd("approvals.list", None, &make_ctx(&ws))
        .await
        .unwrap_err();
    assert!(err.contains("failed to parse approval rules"), "{err}");
}

#[tokio::test]
async fn approvals_clear_counts_then_empties_file() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let path = rules_file(&ws);
    std::fs::create_dir_all(path.parent().unwrap()).unwrap();
    std::fs::write(
        &path,
        serde_json::json!([
            { "op": "process_exec", "pattern": "cargo test *", "action": "allow", "created_at": "t1" }
        ])
        .to_string(),
    )
    .unwrap();

    let handler = SecurityHandler::new();
    let resp = handler
        .handle_cmd("approvals.clear", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["cleared"], true);
    assert_eq!(resp["removed"], 1);
    assert_eq!(std::fs::read_to_string(&path).unwrap(), "[]\n");

    // 二次 clear：文件已在 → removed 0，幂等不报错。
    let resp = handler
        .handle_cmd("approvals.clear", None, &make_ctx(&ws))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["removed"], 0);
}

#[tokio::test]
async fn approvals_unknown_cmd_still_errors() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let handler = SecurityHandler::new();
    let err = handler
        .handle_cmd("approvals.nope", None, &make_ctx(&ws))
        .await
        .unwrap_err();
    assert!(err.contains("unknown command"), "{err}");
}
