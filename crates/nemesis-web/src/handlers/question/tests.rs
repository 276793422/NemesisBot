//! F7（2026-09-06）：`question.respond` / `question.pending` WSAPI 测试——
//! 未装配诚实错、参数校验（selected 非字符串数组）、wired 链路触达
//! FakeResponder（selected 原样透传）、pending 列表。

use super::QuestionHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::agent::QuestionResponder;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（与 approval tests 同款 AppState literal）
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
            finish_reason: None,
            escalated_to: None,
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

fn bare_loop() -> nemesis_agent::r#loop::AgentLoop {
    nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    )
}

/// 记录型 FakeResponder：respond 记录作答回显送达，pending 返回固定条目。
/// B8：form_calls 记录表单作答（respond_form 通路）。
struct FakeResponder {
    calls: parking_lot::Mutex<Vec<(String, Vec<String>)>>,
    form_calls: parking_lot::Mutex<Vec<(String, std::collections::BTreeMap<String, String>)>>,
}

impl QuestionResponder for FakeResponder {
    fn respond(&self, question_id: &str, selected: Vec<String>) -> Result<bool, String> {
        self.calls.lock().push((question_id.to_string(), selected));
        Ok(true)
    }
    fn respond_form(
        &self,
        question_id: &str,
        values: &std::collections::BTreeMap<String, String>,
    ) -> Result<bool, String> {
        self.form_calls
            .lock()
            .push((question_id.to_string(), values.clone()));
        Ok(true)
    }
    fn pending(&self) -> Vec<serde_json::Value> {
        vec![serde_json::json!({
            "question_id": "fake-q1",
            "question": "用哪个包管理器?",
            "multi": false,
        })]
    }
}

fn wired_ctx(dir: &tempfile::TempDir) -> (RequestContext, Arc<FakeResponder>) {
    let al = bare_loop();
    let fake = Arc::new(FakeResponder {
        calls: parking_lot::Mutex::new(Vec::new()),
        form_calls: parking_lot::Mutex::new(Vec::new()),
    });
    al.set_question_responder(fake.clone());
    (make_ctx(dir, Some(Arc::new(al))), fake)
}

// ---------------------------------------------------------------------------
// 未装配 / 无 loop：诚实错
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_without_agent_loop_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q", "selected": ["a"] })),
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
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q", "selected": ["a"] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("question broker not wired"), "err: {err}");
}

// ---------------------------------------------------------------------------
// 参数校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_rejects_missing_question_id() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "selected": ["a"] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing question_id"), "err: {err}");
}

#[tokio::test]
async fn respond_rejects_missing_selected() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("missing selected"), "err: {err}");
}

#[tokio::test]
async fn respond_rejects_non_string_selected_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q1", "selected": ["a", 3] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("array of strings"), "err: {err}");
    assert!(
        fake.calls.lock().is_empty(),
        "rejected payloads must not reach the broker"
    );
}

#[tokio::test]
async fn unknown_cmd_is_honest_error() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd("whatever", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: question.whatever"),
        "err: {err}"
    );
}

// ---------------------------------------------------------------------------
// wired 链路：respond 触达 + pending 列表
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_delivers_selection_to_responder() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q1", "selected": ["pnpm", "npm"] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["question_id"], "q1");
    assert_eq!(out["delivered"], true);
    assert_eq!(
        *fake.calls.lock(),
        vec![(
            "q1".to_string(),
            vec!["pnpm".to_string(), "npm".to_string()]
        )]
    );
}

#[tokio::test]
async fn respond_delivers_empty_selection_shape_to_broker_validation() {
    // 传输形态合法（空数组也是字符串数组）——空选择的业务校验在 broker 侧
    // （校验失败留 pending 可重试）；handler 只透传，不重复判。
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q2", "selected": [] })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["delivered"], true);
    assert_eq!(
        *fake.calls.lock(),
        vec![("q2".to_string(), Vec::<String>::new())]
    );
}

#[tokio::test]
async fn pending_lists_responder_entries() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = QuestionHandler
        .handle_cmd("pending", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["pending"][0]["question_id"], "fake-q1");
    assert_eq!(out["pending"][0]["multi"], false);
    assert_eq!(fake.calls.lock().len(), 0, "pending must not consume calls");
}

// ---------------------------------------------------------------------------
// B8（提问两形态）：values（表单作答）分支
// ---------------------------------------------------------------------------

#[tokio::test]
async fn respond_values_delivers_form_to_responder() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({
                "question_id": "q1",
                "values": { "env": "prod", "note": "hello" }
            })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["delivered"], true);
    let calls = fake.form_calls.lock();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0].0, "q1");
    assert_eq!(calls[0].1.get("env").map(String::as_str), Some("prod"));
    assert_eq!(calls[0].1.get("note").map(String::as_str), Some("hello"));
    assert!(
        fake.calls.lock().is_empty(),
        "form branch must not touch the selection channel"
    );
}

#[tokio::test]
async fn respond_rejects_values_and_selected_together() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({
                "question_id": "q1",
                "selected": ["a"],
                "values": { "env": "prod" }
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("not both"), "err: {err}");
    assert!(fake.calls.lock().is_empty());
    assert!(fake.form_calls.lock().is_empty());
}

#[tokio::test]
async fn respond_rejects_non_string_values() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({
                "question_id": "q1",
                "values": { "count": 3 }
            })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("must be a string"), "err: {err}");
    assert!(fake.form_calls.lock().is_empty());
}

#[tokio::test]
async fn respond_values_empty_object_is_transport_legal() {
    // 空表单也是合法传输形态（必填校验在 broker 侧）——handler 只透传。
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let out = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q2", "values": {} })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["delivered"], true);
    assert_eq!(fake.form_calls.lock()[0].1.len(), 0);
}

#[tokio::test]
async fn respond_rejects_non_object_values() {
    let dir = tempfile::tempdir().unwrap();
    let (ctx, fake) = wired_ctx(&dir);
    let err = QuestionHandler
        .handle_cmd(
            "respond",
            Some(serde_json::json!({ "question_id": "q1", "values": ["env"] })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("must be an object"), "err: {err}");
    assert!(fake.form_calls.lock().is_empty());
}
