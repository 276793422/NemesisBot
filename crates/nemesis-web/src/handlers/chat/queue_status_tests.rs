//! P4（能力扩展 WS8）：`chat.queue_status` WSAPI 契约测试。
//!
//! 契约面：真实 AgentLoop + 预置两条队列的条目 → 响应字段与计数逐项断言
//! （steer/followUp/capacity/busy/mode/session_key 回显）；空队列零值形态；
//! 缺 session_id 的守卫臂；commands() 清单登记（debug 构建漂移 warn 兜底）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_agent::inbox::QueuedMessage;
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

struct NoopProvider;

#[async_trait::async_trait]
impl LlmProvider for NoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            content: "ok".to_string(),
            tool_calls: vec![],
            finished: true,
            reasoning_content: None,
            usage: None,
            raw_request_body: None,
            raw_response_body: None,
        })
    }
}

fn make_ctx_with_loop(al: Option<Arc<AgentLoop>>) -> RequestContext {
    let state = Arc::new(AppState {
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
        agent_loop: Arc::new(parking_lot::RwLock::new(al)),
        cluster: None,
        cluster_service: None,
        cluster_log_dir: None,
        workflow_engine: None,
        #[cfg(feature = "workflow")]
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
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
        session_id: "p4".to_string(),
        chat_id: "p4".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// handler 侧 session_key 构造规则的同构镜像（与 chat.rs E6 顶部一致）。
fn session_key_of(sid: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(sid)
    )
}

fn queued(session: &str, content: &str) -> QueuedMessage {
    QueuedMessage {
        msg: nemesis_types::channel::InboundMessage {
            channel: "web".to_string(),
            sender_id: "tester".to_string(),
            chat_id: "c1".to_string(),
            content: content.to_string(),
            media: vec![],
            session_key: session.to_string(),
            correlation_id: String::new(),
            metadata: Default::default(),
            voice_playback: None,
        },
        timestamp: String::new(),
    }
}

#[tokio::test]
async fn queue_status_reports_steer_and_follow_up_counts() {
    let lp = Arc::new(AgentLoop::new(
        Box::new(NoopProvider),
        AgentConfig::default(),
    ));
    let key = session_key_of("p4-qs");
    // 1 条插队（`!` 前缀 → next_step）+ 2 条排队（无前缀 → next_turn）。
    // 经 doc-hidden 测试支撑访问器播种（inbox 字段对 web crate 不可见）。
    let inbox = lp.inbox_handle();
    inbox.enqueue(&key, queued(&key, "! 插队消息"));
    inbox.enqueue(&key, queued(&key, "排队一"));
    inbox.enqueue(&key, queued(&key, "排队二"));

    let ctx = make_ctx_with_loop(Some(lp));
    let resp = ChatHandler
        .handle_cmd(
            "queue_status",
            Some(serde_json::json!({ "session_id": "p4-qs" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();

    assert_eq!(resp["available"], true);
    assert_eq!(resp["session_id"], "p4-qs");
    assert_eq!(resp["session_key"], key);
    assert_eq!(resp["steer"], 1, "`!` 前缀消息计进 steer");
    assert_eq!(resp["followUp"], 2, "无前缀消息计进 followUp");
    assert!(resp["capacity"].as_u64().unwrap() >= 3, "共享容量回显");
    assert_eq!(resp["busy"], false);
    // mode 是后端并发模式字符串（steer/queue/reject 之一）。
    assert!(resp["mode"].is_string());
}

#[tokio::test]
async fn queue_status_empty_queues_report_zeroes() {
    let ctx = make_ctx_with_loop(Some(Arc::new(AgentLoop::new(
        Box::new(NoopProvider),
        AgentConfig::default(),
    ))));
    let resp = ChatHandler
        .handle_cmd(
            "queue_status",
            Some(serde_json::json!({ "session_id": "p4-empty" })),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(resp["steer"], 0);
    assert_eq!(resp["followUp"], 0);
    assert_eq!(resp["available"], true);
}

#[tokio::test]
async fn queue_status_without_loop_propagates_resolve_error() {
    let ctx = make_ctx_with_loop(None);
    let err = ChatHandler
        .handle_cmd(
            "queue_status",
            Some(serde_json::json!({ "session_id": "p4-dead" })),
            &ctx,
        )
        .await
        .unwrap_err();
    assert_eq!(err, "agent loop not running");
}

#[tokio::test]
async fn queue_status_missing_session_id_bails() {
    let ctx = make_ctx_with_loop(None);
    let err = ChatHandler
        .handle_cmd("queue_status", Some(serde_json::json!({})), &ctx)
        .await
        .unwrap_err();
    assert_eq!(err, "missing session_id");
}

#[test]
fn queue_status_registered_in_commands_list() {
    // L1 注册表纪律：清单外命令在 debug 构建会 warn 漂移——契约测试兜底。
    assert!(ChatHandler.commands().contains(&"queue_status"));
}
