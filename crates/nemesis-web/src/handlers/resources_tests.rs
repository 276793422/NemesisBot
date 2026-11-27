//! 会话资源盘点聚合命令（`session.resources`）协议测试（2026-11-17
//! 会话资源盘点看板 B2）。覆盖：未装配诚实报 / 响应形状 + meta 投影 /
//! 三态诚实（Unavailable 原因透传）/ 空 sid → legacy 键 / sid 清洗 /
//! PerLoop 路由缺席统一 Unavailable。
//!
//! `session.queue_remove`（2026-11-27 等待队列逐条删除）契约：真实
//! AgentLoop + 播种双队列 → 身份对取自 peek（前端同款）→ happy path 双臂
//! （follow_up 中段 + steer）/ 身份过期 Mismatch 诚实拒绝 / 越界
//! NotFound / 未知 queue 值 loud 报错 / 无 loop 透传 resolver 错误 /
//! commands() 清单登记。删除键不需要 registry（headless 形态可删）。
//!
//! registry 是进程级静态槽——全部用例持 [`SESSION_RESOURCES_TEST_LOCK`]
//! 串行；每用例自带 install（下一用例覆盖槽值），槽残留不出测试进程
//! （生产装配在 gateway，测试路径无人触碰）。

use super::resources::{
    ResourcesHandler, SESSION_RESOURCES_TEST_LOCK, set_session_resources_for_test,
};
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_types::resources::{
    CategoryView, GlobalResourceProvider, PerLoopResourceProvider, ProviderMeta, ResourceCtx,
    SessionLoopApi, SessionResourceRegistry,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---- stub providers（纯同步 trait，无 async）----

struct StubOk;
impl GlobalResourceProvider for StubOk {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("stub_ok", "成功类目", 10, false)
    }
    fn collect(&self, _ctx: &ResourceCtx) -> CategoryView {
        CategoryView::ok(2, serde_json::json!({"rows": ["a", "b"]}))
    }
}

struct StubUnavailable;
impl GlobalResourceProvider for StubUnavailable {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("stub_unavail", "缺席类目", 20, true)
    }
    fn collect(&self, _ctx: &ResourceCtx) -> CategoryView {
        CategoryView::unavailable("真相源未装配（测试桩）")
    }
}

struct StubEmpty;
impl GlobalResourceProvider for StubEmpty {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("stub_empty", "空类目", 30, false)
    }
    fn collect(&self, _ctx: &ResourceCtx) -> CategoryView {
        CategoryView::empty()
    }
}

/// PerLoop 桩——collect 只在 loop 在场时可达（handler 侧路由闸先挡）。
struct StubPerLoop;
impl PerLoopResourceProvider for StubPerLoop {
    fn meta(&self) -> ProviderMeta {
        ProviderMeta::new("stub_perloop", "执行态类目", 40, true)
    }
    fn collect(&self, _ctx: &ResourceCtx, _loop: &dyn SessionLoopApi) -> CategoryView {
        CategoryView::ok(1, serde_json::json!({"note": "reached"}))
    }
}

// ---- ctx 构造（background_handler_tests 同款最小 AppState）----

fn make_ctx(workspace: Option<String>) -> RequestContext {
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
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
        mail_plugin: None,
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
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "t".to_string(),
        chat_id: "c-t".to_string(),
        workspace,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

fn install(registry: SessionResourceRegistry) {
    set_session_resources_for_test(Some(Arc::new(registry)));
}

async fn call(sid: Option<&str>) -> Result<Option<serde_json::Value>, String> {
    let mut data = serde_json::Map::new();
    if let Some(s) = sid {
        data.insert("session_id".into(), serde_json::json!(s));
    }
    ResourcesHandler
        .handle_cmd(
            "resources",
            Some(serde_json::Value::Object(data)),
            &make_ctx(Some(std::env::temp_dir().to_string_lossy().into_owned())),
        )
        .await
}

// ---- 用例 ----

#[tokio::test]
async fn without_registry_honest_error() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let err = call(Some("s1")).await.expect_err("must error");
    assert!(err.contains("资源盘点未装配"), "err={err}");
}

#[tokio::test]
async fn response_shape_and_meta_projection() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    let mut reg = SessionResourceRegistry::new();
    reg.register(nemesis_types::resources::ResourceProvider::Global(
        Box::new(StubOk),
    ));
    reg.register(nemesis_types::resources::ResourceProvider::Global(
        Box::new(StubUnavailable),
    ));
    reg.register(nemesis_types::resources::ResourceProvider::Global(
        Box::new(StubEmpty),
    ));
    reg.register(nemesis_types::resources::ResourceProvider::PerLoop(
        Box::new(StubPerLoop),
    ));
    install(reg);

    let resp = call(Some("abc")).await.expect("ok").expect("payload");
    assert_eq!(resp["session_id"], serde_json::json!("abc"));
    assert_eq!(
        resp["session_key"],
        serde_json::json!("agent:main:session:abc")
    );
    // revision = EventHub seq（fresh hub = 0；数值在场即可）。
    assert!(resp["revision"].is_u64());

    let cats = resp["categories"].as_object().expect("categories object");
    assert_eq!(cats.len(), 4, "全部注册类目在场: {cats:?}");

    // Ok 态投影：meta 三件 + status/total/summary，无 reason。
    let ok = &cats["stub_ok"];
    assert_eq!(ok["label"], serde_json::json!("成功类目"));
    assert_eq!(ok["order"], serde_json::json!(10));
    assert_eq!(ok["transient"], serde_json::json!(false));
    assert_eq!(ok["status"], serde_json::json!("ok"));
    assert_eq!(ok["total"], serde_json::json!(2));
    assert_eq!(ok["summary"]["rows"], serde_json::json!(["a", "b"]));
    assert!(ok.get("reason").is_none());

    // Unavailable 态：reason 透传（诚实文案不吞）。
    let un = &cats["stub_unavail"];
    assert_eq!(un["status"], serde_json::json!("unavailable"));
    assert_eq!(un["reason"], serde_json::json!("真相源未装配（测试桩）"));
    assert_eq!(un["transient"], serde_json::json!(true));

    // Empty 态：无 reason 键。
    let empty = &cats["stub_empty"];
    assert_eq!(empty["status"], serde_json::json!("empty"));
    assert!(empty.get("reason").is_none());

    // PerLoop：测试态无 loop → 统一 Unavailable（不炸整命令）。
    let per = &cats["stub_perloop"];
    assert_eq!(per["status"], serde_json::json!("unavailable"));
    assert_eq!(per["reason"], serde_json::json!("会话执行引擎未运行"));
}

#[tokio::test]
async fn empty_sid_maps_to_legacy_key() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    let mut reg = SessionResourceRegistry::new();
    reg.register(nemesis_types::resources::ResourceProvider::Global(
        Box::new(StubOk),
    ));
    install(reg);

    // 无 session_id 字段 + 显式空串同走 legacy 键（口径地雷 2）。
    for sid in [None, Some("")] {
        let resp = call(sid).await.expect("ok").expect("payload");
        assert_eq!(
            resp["session_key"],
            serde_json::json!("agent:main:session:legacy"),
            "sid={sid:?}"
        );
        assert_eq!(resp["session_id"], serde_json::json!(""));
    }
}

#[tokio::test]
async fn session_id_sanitized_in_key() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    let mut reg = SessionResourceRegistry::new();
    reg.register(nemesis_types::resources::ResourceProvider::Global(
        Box::new(StubOk),
    ));
    install(reg);

    // sanitize_session_id：非 [A-Za-z0-9_-] → `_`（sessions.list 同款公式）。
    let resp = call(Some("a:b/c d")).await.expect("ok").expect("payload");
    assert_eq!(
        resp["session_key"],
        serde_json::json!("agent:main:session:a_b_c_d")
    );
    // 原始 sid 原样回传（清洗只作用于键构造）。
    assert_eq!(resp["session_id"], serde_json::json!("a:b/c d"));
}

#[tokio::test]
async fn unknown_cmd_rejected() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    let err = ResourcesHandler
        .handle_cmd("nope", None, &make_ctx(None))
        .await
        .expect_err("unknown cmd must error");
    assert!(err.contains("unknown session command"), "err={err}");
}

// ---- queue_remove（2026-11-27 等待队列逐条删除）契约 ----

use nemesis_agent::inbox::QueuedMessage;
use nemesis_agent::r#loop::{AgentLoop, LlmProvider, LlmResponse};
use nemesis_agent::types::AgentConfig;

struct QrNoopProvider;

#[async_trait::async_trait]
impl LlmProvider for QrNoopProvider {
    async fn chat(
        &self,
        _model: &str,
        _messages: Vec<nemesis_agent::r#loop::LlmMessage>,
        _options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        Ok(LlmResponse {
            finish_reason: None,
            escalated_to: None,
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

/// queue_remove 不读 workspace（无 require_workspace 闸），None 即可。
fn qr_ctx(al: Option<Arc<AgentLoop>>) -> RequestContext {
    let state = Arc::new(AppState {
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
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
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "qr".to_string(),
        chat_id: "qr".to_string(),
        workspace: None,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 播种条目（timestamp 显式可辨——身份复核的验真半边靠它）。
fn qr_seeded(session: &str, content: &str, ts: &str) -> QueuedMessage {
    QueuedMessage {
        msg: nemesis_types::channel::InboundMessage::user_ingress(
            "web".to_string(),
            "tester".to_string(),
            "c1".to_string(),
            content.to_string(),
        )
        .with_session_key(session.to_string())
        .with_metadata_map(Default::default()),
        timestamp: ts.to_string(),
        priority: nemesis_agent::inbox::MessagePriority::Normal,
    }
}

async fn qr_call(
    ctx: &RequestContext,
    data: serde_json::Value,
) -> Result<Option<serde_json::Value>, String> {
    ResourcesHandler
        .handle_cmd("queue_remove", Some(data), ctx)
        .await
}

/// happy path 双臂：follow_up 中段摘除（序保持）+ steer 臂同语义；
/// 删除键不需要 registry（先置 None——headless 形态同源可删）。
#[tokio::test]
async fn queue_remove_removes_follow_up_then_steer_arm() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let lp = Arc::new(AgentLoop::new(
        Box::new(QrNoopProvider),
        AgentConfig::default(),
    ));
    let key = "agent:main:session:qr-1";
    {
        let inbox = lp.inbox_handle();
        // `!` 前缀 → next_step（steer）；无前缀 → next_turn（follow_up）。
        inbox.enqueue(key, qr_seeded(key, "! 插队甲", "t-s0"));
        inbox.enqueue(key, qr_seeded(key, "排队甲", "t-f0"));
        inbox.enqueue(key, qr_seeded(key, "排队乙", "t-f1"));
    }
    let ctx = qr_ctx(Some(lp.clone()));

    // 身份对取自 peek（前端同款流程：快照行原样回传）。
    let snap = lp.inbox_handle().peek(key);
    let victim = &snap.follow_up[1];
    let resp = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-1",
            "queue": "follow_up",
            "index": 1,
            "sender_id": victim.sender_id,
            "timestamp": victim.timestamp,
        }),
    )
    .await
    .expect("ok")
    .expect("payload");
    assert_eq!(resp["removed"], true);
    assert_eq!(resp["session_key"], serde_json::json!(key));
    assert_eq!(resp["queue"], serde_json::json!("follow_up"));
    assert_eq!(resp["follow_up"], 1);
    assert_eq!(resp["steer"], 1);

    // 剩下的正是排队甲（中段摘除序保持）。
    let after = lp.inbox_handle().peek(key);
    assert_eq!(
        after
            .follow_up
            .iter()
            .map(|e| e.content.as_str())
            .collect::<Vec<_>>(),
        ["排队甲"]
    );

    // steer 臂同语义。
    let v = &after.steer[0];
    let resp = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-1",
            "queue": "steer",
            "index": 0,
            "sender_id": v.sender_id,
            "timestamp": v.timestamp,
        }),
    )
    .await
    .expect("ok")
    .expect("payload");
    assert_eq!(resp["queue"], serde_json::json!("steer"));
    assert_eq!(resp["steer"], 0);
    assert_eq!(resp["follow_up"], 1);
}

/// 身份过期（peek→delete 窗口内队列已变化）诚实拒绝，队列零副作用。
#[tokio::test]
async fn queue_remove_mismatch_honest_refusal() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let lp = Arc::new(AgentLoop::new(
        Box::new(QrNoopProvider),
        AgentConfig::default(),
    ));
    let key = "agent:main:session:qr-2";
    {
        let inbox = lp.inbox_handle();
        inbox.enqueue(key, qr_seeded(key, "甲", "t-a"));
        inbox.enqueue(key, qr_seeded(key, "乙", "t-b"));
    }
    let ctx = qr_ctx(Some(lp.clone()));
    let err = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-2",
            "queue": "follow_up",
            "index": 0,
            "sender_id": "tester",
            "timestamp": "stale-ts",
        }),
    )
    .await
    .expect_err("stale identity must refuse");
    assert!(err.contains("队列已变化"), "err={err}");
    let snap = lp.inbox_handle().peek(key);
    assert_eq!(snap.follow_up.len(), 2, "拒绝路径零副作用");
}

/// 越界下标 = NotFound 文案（已被消费/清空同臂）。
#[tokio::test]
async fn queue_remove_not_found_out_of_range() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let lp = Arc::new(AgentLoop::new(
        Box::new(QrNoopProvider),
        AgentConfig::default(),
    ));
    let key = "agent:main:session:qr-3";
    lp.inbox_handle()
        .enqueue(key, qr_seeded(key, "唯一", "t-0"));
    let ctx = qr_ctx(Some(lp));
    let err = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-3",
            "queue": "follow_up",
            "index": 7,
            "sender_id": "tester",
            "timestamp": "t-0",
        }),
    )
    .await
    .expect_err("out-of-range must error");
    assert!(err.contains("已不在队列"), "err={err}");
}

/// 未知 queue 值 loud 报错（不静默纠偏）。
#[tokio::test]
async fn queue_remove_unknown_queue_value_loud() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let ctx = qr_ctx(Some(Arc::new(AgentLoop::new(
        Box::new(QrNoopProvider),
        AgentConfig::default(),
    ))));
    let err = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-x",
            "queue": "nope",
            "index": 0,
            "sender_id": "",
            "timestamp": "",
        }),
    )
    .await
    .expect_err("unknown queue must error");
    assert!(err.contains("unknown queue"), "err={err}");
}

/// 无 loop → resolver 错误透传（resolve_session_loop 单一裁决点语义）。
#[tokio::test]
async fn queue_remove_without_loop_propagates() {
    let _g = SESSION_RESOURCES_TEST_LOCK.lock().await;
    set_session_resources_for_test(None);
    let ctx = qr_ctx(None);
    let err = qr_call(
        &ctx,
        serde_json::json!({
            "session_id": "qr-dead",
            "queue": "follow_up",
            "index": 0,
            "sender_id": "tester",
            "timestamp": "t",
        }),
    )
    .await
    .expect_err("no loop must error");
    assert_eq!(err, "agent loop not running");
}

#[test]
fn queue_remove_registered_in_commands_list() {
    // 注册表纪律：清单外命令在 debug 构建会 warn 漂移——契约测试兜底。
    assert!(ResourcesHandler.commands().contains(&"queue_remove"));
}
