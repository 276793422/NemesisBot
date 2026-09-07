//! M7（2026-09-06）：`approval.respond` / `approval.pending` WSAPI 测试——
//! 未装配诚实错、参数校验、wired 链路触达 FakeResponder、pending 列表。

use super::ApprovalHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::agent::ApprovalResponder;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（与 chat_mode_tests 同款 AppState literal）
// ---------------------------------------------------------------------------

struct NoopProvider;

#[async_trait::async_trait]
impl nemesis_agent::r#loop::LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _: &str,
        _: Vec<nemesis_agent::r#loop::LlmMessage>,
        _: Option<nemesis_agent::types::ChatOptions>,
        _: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<nemesis_agent::r#loop::LlmResponse, String> {
        Ok(nemesis_agent::r#loop::LlmResponse {
            content: String::new(),
            tool_calls: Vec::new(),
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx(
    dir: &tempfile::TempDir,
    agent_loop: Option<Arc<nemesis_agent::r#loop::AgentLoop>>,
) -> RequestContext {
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
        agent_loop: Arc::new(parking_lot::RwLock::new(agent_loop)),
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

fn bare_loop() -> nemesis_agent::r#loop::AgentLoop {
    nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    )
}

/// 记录型 FakeResponder：respond 记录裁决并回显，pending 返回固定条目。
struct FakeResponder {
    calls: parking_lot::Mutex<Vec<(String, bool, bool, Option<String>)>>,
}

impl ApprovalResponder for FakeResponder {
    fn respond(
        &self,
        request_id: &str,
        approved: bool,
        always: bool,
        note: Option<String>,
    ) -> Result<bool, String> {
        self.calls
            .lock()
            .push((request_id.to_string(), approved, always, note));
        Ok(approved)
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        vec![serde_json::json!({
            "request_id": "fake-1",
            "operation": "process_exec",
            "risk_level": "HIGH",
        })]
    }
}

// ---------------------------------------------------------------------------
// 未装配 / 无 loop：诚实错
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_without_agent_loop_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r", "approved": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"), "err: {err}");
}

#[tokio::test]
async fn respond_without_responder_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, Some(Arc::new(bare_loop())));
    let err = ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r", "approved": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("approval manager not wired"), "err: {err}");
}

// ---------------------------------------------------------------------------
// 参数校验
// ---------------------------------------------------------------------------

fn wired_ctx(dir: &tempfile::TempDir) -> (RequestContext, Arc<FakeResponder>) {
    let al = bare_loop();
    let fake = Arc::new(FakeResponder {
        calls: parking_lot::Mutex::new(Vec::new()),
    });
    al.set_approval_responder(fake.clone());
    (make_ctx(dir, Some(Arc::new(al))), fake)
}

#[tokio::test]
async fn respond_rejects_missing_request_id() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "approved": true })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing request_id"), "err: {err}");
}

#[tokio::test]
async fn respond_rejects_missing_approved() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing approved"), "err: {err}");
}

#[tokio::test]
async fn unknown_cmd_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = ApprovalHandler
        .handle_cmd("whatever", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: approval.whatever"),
        "err: {err}"
    );
}

// ---------------------------------------------------------------------------
// wired 链路：respond 触达 + pending 列表
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_delivers_verdict_to_responder() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r1", "approved": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["request_id"], "r1");
    assert_eq!(out["approved"], true);
    assert_eq!(out["delivered"], true);
    assert_eq!(
        *fake.calls.lock(),
        vec![("r1".to_string(), true, false, None)]
    );
}

#[tokio::test]
async fn respond_passes_always_flag_through() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r2", "approved": true, "always": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // F3: always 透传给 responder（规则写入是 responder 侧职责）。
    assert_eq!(
        *fake.calls.lock(),
        vec![("r2".to_string(), true, true, None)]
    );
}

#[tokio::test]
async fn respond_defaults_missing_always_to_false() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r3", "approved": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // 缺 always 键 = 旧前端兼容路径，按 false。
    assert_eq!(
        *fake.calls.lock(),
        vec![("r3".to_string(), true, false, None)]
    );
}

#[tokio::test]
async fn respond_passes_note_through_and_defaults_to_none() {
    // F6: note 键透传给 responder；缺键按 None（旧前端兼容）。
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({
                "request_id": "r4",
                "approved": false,
                "note": "别动生产配置",
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    ApprovalHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "request_id": "r5", "approved": true })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        *fake.calls.lock(),
        vec![
            (
                "r4".to_string(),
                false,
                false,
                Some("别动生产配置".to_string())
            ),
            ("r5".to_string(), true, false, None),
        ]
    );
}

#[tokio::test]
async fn pending_lists_responder_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = ApprovalHandler
        .handle_cmd("pending", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["pending"][0]["request_id"], "fake-1");
    assert_eq!(out["pending"][0]["risk_level"], "HIGH");
    assert_eq!(fake.calls.lock().len(), 0, "pending must not consume calls");
}
