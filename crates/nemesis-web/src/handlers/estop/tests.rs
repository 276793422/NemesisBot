use super::*;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(estop: Option<Arc<nemesis_agent::estop::EstopState>>) -> RequestContext {
    let state = Arc::new(crate::api_handlers::AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop,
        estop_audit: None,
        background_registry: None,
        cron: None,
        board: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
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
async fn trigger_release_status_roundtrip() {
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    let ctx = make_ctx(Some(estop.clone()));
    let h = EstopHandler;

    // status → 未触发
    let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["engaged"], false);

    // trigger → 触发
    let r = h.handle_cmd("trigger", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["engaged"], true);
    assert!(estop.is_engaged());

    // status → 已触发
    let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["engaged"], true);

    // release → 释放
    let r = h.handle_cmd("release", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["engaged"], false);
    assert!(!estop.is_engaged());
}

#[tokio::test]
async fn status_when_not_available_reports_false() {
    // estop 未接线（None）→ status 仍可查，返回 engaged:false（不报错）。
    let ctx = make_ctx(None);
    let h = EstopHandler;
    let r = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert_eq!(r["engaged"], false);
}

#[tokio::test]
async fn trigger_release_error_when_not_available() {
    let ctx = make_ctx(None);
    let h = EstopHandler;
    assert!(h.handle_cmd("trigger", None, &ctx).await.is_err());
    assert!(h.handle_cmd("release", None, &ctx).await.is_err());
}

#[tokio::test]
async fn unknown_command_errors() {
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    let ctx = make_ctx(Some(estop));
    let h = EstopHandler;
    let err = h.handle_cmd("frobnicate", None, &ctx).await.unwrap_err();
    assert!(err.contains("unknown command"), "实际: {}", err);
}

// ---------------------------------------------------------------------------
// BUG-20261118-096：急停触发/释放落审计钩子（Merkle 审计链接线面）
// ---------------------------------------------------------------------------

/// 带审计钩子的 ctx：闭包把 (operation, source) 记进共享 Vec（互斥锁）。
fn make_ctx_with_audit(
    estop: Option<Arc<nemesis_agent::estop::EstopState>>,
) -> (
    RequestContext,
    Arc<parking_lot::Mutex<Vec<(String, String)>>>,
) {
    let calls: Arc<parking_lot::Mutex<Vec<(String, String)>>> =
        Arc::new(parking_lot::Mutex::new(Vec::new()));
    let sink = Arc::clone(&calls);
    let hook: EstopAuditHook = Arc::new(move |operation: &str, source: &str| {
        sink.lock()
            .push((operation.to_string(), source.to_string()));
    });
    let state = Arc::new(crate::api_handlers::AppState {
        estop,
        estop_audit: Some(hook),
        // 其余字段与 make_ctx 同型（Rust 无结构体字面量 rest 语法，逐字段
        // 重复是既有测试形态的代价；estop_audit 显式置 Some 是本测试重点）。
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        background_registry: None,
        cron: None,
        board: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
    });
    (
        RequestContext {
            session_id: "s".to_string(),
            chat_id: "c".to_string(),
            workspace: None,
            home: None,
            state,
            auth_method: crate::session::AuthMethod::default(),
        },
        calls,
    )
}

#[tokio::test]
async fn trigger_release_invoke_audit_hook_status_does_not() {
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    let (ctx, calls) = make_ctx_with_audit(Some(estop));
    let h = EstopHandler;

    // status 只读，不落审计。
    let _ = h.handle_cmd("status", None, &ctx).await.unwrap().unwrap();
    assert!(
        calls.lock().is_empty(),
        "status 不得触发审计钩子: {:?}",
        calls.lock()
    );

    // trigger → ("engage", "dashboard")。
    let _ = h.handle_cmd("trigger", None, &ctx).await.unwrap().unwrap();
    // release → ("release", "dashboard")。
    let _ = h.handle_cmd("release", None, &ctx).await.unwrap().unwrap();

    let got = calls.lock().clone();
    assert_eq!(
        got,
        vec![
            ("engage".to_string(), "dashboard".to_string()),
            ("release".to_string(), "dashboard".to_string()),
        ],
        "审计钩子调用序列: {got:?}"
    );
}

#[tokio::test]
async fn audit_hook_absent_is_honest_noop() {
    // estop_audit: None（headless/测试装配）→ trigger/release 照常执行，
    // 不 panic（审计是记录面不是执行闸）。
    let estop = Arc::new(nemesis_agent::estop::EstopState::new());
    let ctx = make_ctx(Some(estop.clone()));
    let h = EstopHandler;
    let _ = h.handle_cmd("trigger", None, &ctx).await.unwrap().unwrap();
    assert!(estop.is_engaged());
    let _ = h.handle_cmd("release", None, &ctx).await.unwrap().unwrap();
    assert!(!estop.is_engaged());
}
