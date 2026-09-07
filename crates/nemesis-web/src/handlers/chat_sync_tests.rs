//! L2（devtool-upgrade 阶段 6）— `chat.sync` 断线补拉 dispatch 测试。
//!
//! sync 是只读命令且**不依赖 AgentLoop**（提前返回臂）——headless fixture
//! 即可全链路测：record 推帧 → sync 补拉 → 窗口/gap 语义。
//! 与 chat_event_log/tests.rs 共享进程级全局表，session id 用唯一前缀隔离。

use super::chat::ChatHandler;
use crate::api_handlers::AppState;
use crate::chat_event_log;
use crate::chat_event_log::test_support::GLOBAL_TABLE_LOCK;
use crate::events::EventHub;
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn unique_session(tag: &str) -> String {
    static SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    format!(
        "l2sync-{}-{}-{tag}",
        std::process::id(),
        SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    )
}

/// 与 handlers/chat.rs sync 臂同构的环形缓冲键变换（单一真相源是
/// chat.rs 的构造——测试侧镜像它以锚定两侧不漂移）。
fn ring_key(session_id: &str) -> String {
    format!(
        "agent:main:session:{}",
        nemesis_agent::session::SessionStore::sanitize_session_id(session_id)
    )
}

fn make_ctx(session_id: &str) -> RequestContext {
    let dir = tempfile::tempdir().unwrap();
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
        agent_loop: Arc::new(parking_lot::RwLock::new(None)),
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
    // dir 由 state 持字符串拷贝，这里立即 drop 无碍（无文件依赖）。
    drop(dir);
    RequestContext {
        session_id: session_id.to_string(),
        chat_id: format!("c-{session_id}"),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

#[tokio::test]
async fn sync_replays_recorded_frames_with_window_semantics() {
    let _guard = GLOBAL_TABLE_LOCK.lock().await;
    let sid = unique_session("window");
    let ctx = make_ctx("wire-s"); // ctx 默认会话 ≠ 目标会话，验证显式指定优先
    for i in 1..=5 {
        chat_event_log::record(
            &ring_key(&sid),
            "assistant",
            &format!("m{i}"),
            Some("test-model"),
        );
    }
    // after=2 → 补 3/4/5
    let out = ChatHandler
        .handle_cmd(
            "sync",
            Some(serde_json::json!({"session_id": &sid, "after_seq": 2})),
            &ctx,
        )
        .await
        .expect("sync must succeed (no agent loop dependency)")
        .expect("sync returns payload");
    assert_eq!(out["session_id"], sid.as_str());
    assert_eq!(out["session_key"], ring_key(&sid).as_str());
    assert_eq!(out["after_seq"], 2);
    assert_eq!(out["gap"], false);
    let events = out["events"].as_array().unwrap();
    assert_eq!(events.len(), 3);
    assert_eq!(events[0]["seq"], 3);
    assert_eq!(events[2]["content"], "m5");
    assert_eq!(events[0]["model"], "test-model");
    // 追平 → 空、无 gap
    let out = ChatHandler
        .handle_cmd(
            "sync",
            Some(serde_json::json!({"session_id": &sid, "after_seq": 5})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert!(out["events"].as_array().unwrap().is_empty());
    assert_eq!(out["gap"], false);
}

#[tokio::test]
async fn sync_defaults_to_connection_session_and_zero() {
    let _guard = GLOBAL_TABLE_LOCK.lock().await;
    let sid = unique_session("defaults");
    let ctx = make_ctx(&sid); // 不带 session_id → 取 ctx.session_id
    chat_event_log::record(&ring_key(&sid), "user", "hello", None);
    chat_event_log::record(&ring_key(&sid), "assistant", "world", None);
    let out = ChatHandler
        .handle_cmd("sync", None, &ctx) // data 全缺省
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["session_id"], sid.as_str());
    assert_eq!(out["after_seq"], 0);
    assert_eq!(out["events"].as_array().unwrap().len(), 2);
}

#[tokio::test]
async fn sync_reports_honest_gap_for_stale_or_unknown_sessions() {
    let _guard = GLOBAL_TABLE_LOCK.lock().await;
    let ctx = make_ctx("wire-g");
    // 未记录会话 + 持旧 seq（网关重启形态）→ gap=true 空事件
    let unknown = unique_session("unknown");
    let out = ChatHandler
        .handle_cmd(
            "sync",
            Some(serde_json::json!({"session_id": &unknown, "after_seq": 42})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["gap"], true);
    assert!(out["events"].as_array().unwrap().is_empty());
    // from-scratch（after=0）→ 非缺口
    let out = ChatHandler
        .handle_cmd(
            "sync",
            Some(serde_json::json!({"session_id": &unknown, "after_seq": 0})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["gap"], false);
}
