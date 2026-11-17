//! 会话资源盘点聚合命令（`session.resources`）协议测试（2026-11-17
//! 会话资源盘点看板 B2）。覆盖：未装配诚实报 / 响应形状 + meta 投影 /
//! 三态诚实（Unavailable 原因透传）/ 空 sid → legacy 键 / sid 清洗 /
//! PerLoop 路由缺席统一 Unavailable。
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
