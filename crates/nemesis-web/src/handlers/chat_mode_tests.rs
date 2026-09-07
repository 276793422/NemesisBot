//! F1（2026-09-05）：`chat.set_mode` / `chat.get_mode`（plan/build 双模式）
//! WSAPI 测试——参数校验（缺 mode / 未知值 Err）、翻转生效、ModeChanged
//! 事件发布（带 `web:{session_id}` 路由键）。

use super::chat::ChatHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness（与 m5_session_usage_tests 同款）
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
        home: Some(ws.clone()),
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
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn f1_agent_loop() -> (
    nemesis_agent::r#loop::AgentLoop,
    tokio::sync::broadcast::Receiver<nemesis_types::agent::AgentEvent>,
) {
    let al = nemesis_agent::r#loop::AgentLoop::new(
        Box::new(NoopProvider),
        nemesis_agent::types::AgentConfig::default(),
    );
    let (tx, rx) = tokio::sync::broadcast::channel(16);
    al.set_agent_event_tx(Some(tx));
    (al, rx)
}

// ---------------------------------------------------------------------------
// 参数校验
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_mode_requires_agent_loop() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = ChatHandler
        .handle_cmd(
            "set_mode",
            Some(serde_json::json!({ "session_id": "s1", "mode": "plan" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(err.contains("agent loop not running"));
}

#[tokio::test]
async fn set_mode_rejects_missing_mode() {
    let dir = tempfile::tempdir().unwrap();
    let (al, _rx) = f1_agent_loop();
    let ctx = make_ctx(&dir, Some(Arc::new(al)));
    let err = ChatHandler
        .handle_cmd(
            "set_mode",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("missing mode") && err.contains("plan"),
        "缺 mode 必须明示合法值，实际: {err}"
    );
}

#[tokio::test]
async fn set_mode_rejects_unknown_value() {
    let dir = tempfile::tempdir().unwrap();
    let (al, _rx) = f1_agent_loop();
    let ctx = make_ctx(&dir, Some(Arc::new(al)));
    // 未知值回灌错误（不静默落回默认——与 G1 未知档位同一纪律）。
    let err = ChatHandler
        .handle_cmd(
            "set_mode",
            Some(serde_json::json!({ "session_id": "s1", "mode": "auto" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown mode") && err.contains("auto"),
        "未知值必须诚实拒绝，实际: {err}"
    );
}

// ---------------------------------------------------------------------------
// 翻转 + 事件
// ---------------------------------------------------------------------------

#[tokio::test]
async fn set_mode_flips_and_publishes_routed_event() {
    let dir = tempfile::tempdir().unwrap();
    let (al, mut rx) = f1_agent_loop();
    let ctx = make_ctx(&dir, Some(Arc::new(al)));

    // plan。
    let r = ChatHandler
        .handle_cmd(
            "set_mode",
            Some(serde_json::json!({ "session_id": "s1", "mode": "plan" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["mode"], "plan");
    assert_eq!(r["session_key"], "agent:main:session:s1");
    let ev = rx.try_recv().expect("set_mode 必须发布 ModeChanged");
    match &ev {
        nemesis_types::agent::AgentEvent::ModeChanged { chat_id, mode, .. } => {
            // 路由键 = web:{session_id}（agent 事件 pump 按此前缀投递到会话）。
            assert_eq!(chat_id, "web:s1");
            assert_eq!(mode, "plan");
        }
        other => panic!("必须是 ModeChanged，实际 {other:?}"),
    }

    // build 切回。
    let r = ChatHandler
        .handle_cmd(
            "set_mode",
            Some(serde_json::json!({ "session_id": "s1", "mode": "BUILD" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    // 大小写宽容（AgentMode::parse），线上形态统一小写回。
    assert_eq!(r["mode"], "build");
}

#[tokio::test]
async fn get_mode_reads_loop_state() {
    let dir = tempfile::tempdir().unwrap();
    let (al, _rx) = f1_agent_loop();
    let ctx = make_ctx(&dir, Some(Arc::new(al)));

    // 默认 build。
    let r = ChatHandler
        .handle_cmd(
            "get_mode",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["mode"], "build");

    // 外部翻转后读到 plan。
    {
        let guard = ctx.state.agent_loop.read();
        let al = guard.as_ref().unwrap();
        al.set_mode_with_event(nemesis_agent::types::AgentMode::Plan, "k", "web:s1");
    }
    let r = ChatHandler
        .handle_cmd(
            "get_mode",
            Some(serde_json::json!({ "session_id": "s1" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["mode"], "plan");
}
