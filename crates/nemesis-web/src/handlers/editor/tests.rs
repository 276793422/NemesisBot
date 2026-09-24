//! `editor.get` / `editor.set` WSAPI 测试(2026-09-20 Full Access 用户裁决)。
//!
//! 覆盖:未装配诚实错 / get 初始双关 / set 开关 + SSE 广播 / 服务端联动
//! 收口(ext=true ⇒ full=true)/ 双键同关 / 部分更新缺键保当前 / 非.bool
//! 响亮报错 / 无 workspace + 无 bridge 降级不炸。进程级槽共享 → 全文件
//! 持 [`EDITOR_TEST_LOCK`] 串行。
// 家规先例(handlers/upload/tests.rs 等):async 测试持 parking_lot 锁跨
// await 合法——锁只为测试串行化,单测试任务内无死锁面。
#![allow(clippy::await_holding_lock)]

use super::EDITOR_TEST_LOCK;
use super::EditorHandler;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::handlers::editor::{install_editor_access, set_editor_access_for_test};
use crate::session::SessionManager;
use crate::ws_router::{ModuleHandler, RequestContext};
use nemesis_security::editor_access::EditorAccessState;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

// ---------------------------------------------------------------------------
// Harness(approval/tests.rs 同款 AppState literal,无 loop 依赖)
// ---------------------------------------------------------------------------

fn make_ctx(dir: &tempfile::TempDir, workspace: Option<String>) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: workspace.clone(),
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
        cron: None,
        board: None,
    });
    RequestContext {
        session_id: "sess-editor".to_string(),
        chat_id: "web:conn-editor".to_string(),
        workspace,
        home: None,
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 装好状态槽的 ctx(+ 返回状态 Arc 供断言)。调用方持 EDITOR_TEST_LOCK。
fn wired_ctx(dir: &tempfile::TempDir) -> (RequestContext, Arc<EditorAccessState>) {
    let state = EditorAccessState::new();
    install_editor_access(state.clone());
    (
        make_ctx(dir, Some(dir.path().to_string_lossy().to_string())),
        state,
    )
}

// ---------------------------------------------------------------------------
// 未装配:诚实错
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_without_install_is_honest_error() {
    let _guard = EDITOR_TEST_LOCK.lock();
    set_editor_access_for_test(None);
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = EditorHandler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("编辑器放行未装配"), "err: {err}");
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_without_install_is_honest_error() {
    let _guard = EDITOR_TEST_LOCK.lock();
    set_editor_access_for_test(None);
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir, None);
    let err = EditorHandler
        .handle_cmd("set", Some(serde_json::json!({"full_access": true})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("编辑器放行未装配"), "err: {err}");
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn unknown_cmd_is_honest_error() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _state) = wired_ctx(&dir);
    let err = EditorHandler
        .handle_cmd("whatever", None, &ctx)
        .await
        .unwrap_err();
    assert!(
        err.contains("unknown command: editor.whatever"),
        "err: {err}"
    );
    set_editor_access_for_test(None);
}

// ---------------------------------------------------------------------------
// get / set 主链路
// ---------------------------------------------------------------------------

#[tokio::test]
async fn get_returns_initial_double_off() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _state) = wired_ctx(&dir);
    let out = EditorHandler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], false);
    assert_eq!(out["external_write"], false);
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_publishes_sse_and_returns_effective_values() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, state) = wired_ctx(&dir);

    // 订阅先于 publish(broadcast 无订阅者即丢)。
    let mut rx = ctx.state.event_hub.subscribe();

    let out = EditorHandler
        .handle_cmd("set", Some(serde_json::json!({"full_access": true})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], true);
    assert_eq!(out["external_write"], false);
    assert_eq!(state.snapshot(), (true, false));

    let ev = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out waiting for SSE event")
        .expect("hub closed");
    assert_eq!(ev.event_type, "editor-mode");
    assert_eq!(ev.data["full_access"], true);
    assert_eq!(ev.data["external_write"], false);
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_external_write_alone_implies_full_server_side() {
    // 服务端联动收口:{"external_write": true} 单键 → snapshot (true, true)
    // (UI 禁用联动只是体验,不变量在服务端)。
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, state) = wired_ctx(&dir);
    let out = EditorHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({"external_write": true})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], true, "ext=true 必须联动 full=true");
    assert_eq!(out["external_write"], true);
    assert_eq!(state.snapshot(), (true, true));
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_both_false_turns_everything_off() {
    // 关 FA 时前端随关 ext(双键 false)→ 回到双关。
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, state) = wired_ctx(&dir);
    EditorHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({"full_access": true, "external_write": true})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let out = EditorHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({"full_access": false, "external_write": false})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], false);
    assert_eq!(out["external_write"], false);
    assert_eq!(state.snapshot(), (false, false));
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_partial_update_keeps_missing_key() {
    // 部分更新契约:只发 full_access,external_write 保持当前值。
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, state) = wired_ctx(&dir);
    EditorHandler
        .handle_cmd(
            "set",
            Some(serde_json::json!({"full_access": true, "external_write": true})),
            &ctx,
        )
        .await
        .unwrap()
        .unwrap();
    let out = EditorHandler
        .handle_cmd("set", Some(serde_json::json!({"full_access": false})), &ctx)
        .await
        .unwrap()
        .unwrap();
    // full=false 缺 ext → set_flags(false, true) → 联动仍 full=true。
    assert_eq!(out["full_access"], true);
    assert_eq!(out["external_write"], true);
    assert_eq!(state.snapshot(), (true, true));
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_rejects_non_bool_loud() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _state) = wired_ctx(&dir);
    let err = EditorHandler
        .handle_cmd("set", Some(serde_json::json!({"full_access": "yes"})), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("must be a bool"), "响亮报错: {err}");
    set_editor_access_for_test(None);
}

#[tokio::test]
async fn set_without_data_is_honest_error() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let (ctx, _state) = wired_ctx(&dir);
    let err = EditorHandler
        .handle_cmd("set", None, &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing data"), "err: {err}");
    set_editor_access_for_test(None);
}

// ---------------------------------------------------------------------------
// roots 刷新降级:无 workspace + 无 bridge 不炸(get/set 照常)
// ---------------------------------------------------------------------------

#[tokio::test]
async fn no_workspace_and_no_bridge_degrades_gracefully() {
    let _guard = EDITOR_TEST_LOCK.lock();
    let dir = tempfile::tempdir().unwrap();
    let state = EditorAccessState::new();
    install_editor_access(state.clone());
    // workspace=None + bridge 未装配 → refresh_roots no-op(保留旧 roots)。
    let ctx = make_ctx(&dir, None);
    let out = EditorHandler
        .handle_cmd("get", None, &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], false);
    let out = EditorHandler
        .handle_cmd("set", Some(serde_json::json!({"full_access": true})), &ctx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(out["full_access"], true);
    set_editor_access_for_test(None);
}
