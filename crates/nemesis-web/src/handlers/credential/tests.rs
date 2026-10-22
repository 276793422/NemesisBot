//! B4 — CredentialHandler 测试：槽空诚实报错 / 载荷校验 / 透传回执。
//!
//! 静态槽是进程级单例——用例持 `TEST_LOCK`（tokio Mutex，await 安全，
//! vault handler 测试同族纪律）串行，`clear_broker` 还原，互不污染。
//! ctx 走 approval/question tests 同款 AppState literal（handler 不消费
//! ctx，全 None 即可）。

use super::CredentialHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::credential::{clear_broker, set_broker};
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::agent::CredentialResponder;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

static TEST_LOCK: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("m".to_string())),
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
        signature_verify: None,
        skills_install_gate: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "s".to_string(),
        chat_id: "c".to_string(),
        workspace: Some(dir.path().to_string_lossy().to_string()),
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 记录型 FakeResponder：respond 记录透传并回可编程回执，pending 返回固定条目。
struct FakeResponder {
    respond_receipt: Result<serde_json::Value, String>,
    seen: parking_lot::Mutex<Vec<(String, String)>>,
}

impl FakeResponder {
    fn ok() -> Arc<Self> {
        Arc::new(Self {
            respond_receipt: Ok(serde_json::json!({
                "stored": true, "key_name": "connector/x/api_key", "rotated": false,
            })),
            seen: parking_lot::Mutex::new(Vec::new()),
        })
    }
}

impl CredentialResponder for FakeResponder {
    fn respond(&self, request_id: &str, value: &str) -> Result<serde_json::Value, String> {
        self.seen
            .lock()
            .push((request_id.to_string(), value.to_string()));
        self.respond_receipt.clone()
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        vec![serde_json::json!({"request_id": "cred-9", "service": "svc"})]
    }
}

#[tokio::test]
async fn not_wired_is_honest_error() {
    let _g = TEST_LOCK.lock().await;
    clear_broker();
    let dir = tempfile::tempdir().unwrap();
    let err = CredentialHandler
        .handle_cmd("pending", None, &make_ctx(&dir))
        .await
        .unwrap_err();
    assert!(err.contains("not wired"), "got: {err}");
}

#[tokio::test]
async fn respond_passes_through_and_returns_receipt() {
    let _g = TEST_LOCK.lock().await;
    let fake = FakeResponder::ok();
    set_broker(fake.clone());
    let dir = tempfile::tempdir().unwrap();
    let out = CredentialHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({"request_id": "cred-7", "value": "sk-secret"})),
            &make_ctx(&dir),
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["stored"], serde_json::json!(true));
    let seen = fake.seen.lock();
    assert_eq!(seen.len(), 1);
    assert_eq!(seen[0], ("cred-7".into(), "sk-secret".into()));
    clear_broker();
}

#[tokio::test]
async fn respond_missing_fields_rejected() {
    let _g = TEST_LOCK.lock().await;
    set_broker(FakeResponder::ok());
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let err = CredentialHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({"request_id": "x"})),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing value"), "got: {err}");
    let err = CredentialHandler
        .handle_cmd("respond", Some(serde_json::json!({"value": "v"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing request_id"), "got: {err}");
    clear_broker();
}

#[tokio::test]
async fn pending_lists_cards() {
    let _g = TEST_LOCK.lock().await;
    set_broker(FakeResponder::ok());
    let dir = tempfile::tempdir().unwrap();
    let out = CredentialHandler
        .handle_cmd("pending", None, &make_ctx(&dir))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["pending"][0]["request_id"], "cred-9");
    clear_broker();
}

#[tokio::test]
async fn unknown_command_rejected() {
    let _g = TEST_LOCK.lock().await;
    set_broker(FakeResponder::ok());
    let dir = tempfile::tempdir().unwrap();
    let err = CredentialHandler
        .handle_cmd("nope", None, &make_ctx(&dir))
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: credential.nope"),
        "got: {err}"
    );
    clear_broker();
}
