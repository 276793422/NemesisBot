//! scanner.rs 纯逻辑覆盖（Phase 3 批次 18，2026-08-25）。
//!
//! 子模块（非 sibling）以访问私有项。覆盖：`cmd_cancel` 三态匹配（精确 /
//! update-db 前缀 / 无活跃操作）、`install_engine_inner` 与 `update_db_inner`
//! 的**网络前**早期错误臂、`mark_op_started` 去重、`format_bytes` 边界、
//! `make_download_progress_cb` 的 SSE 事件发布。
//!
//! 结构性豁免（台账 §9.4）：真实下载（install/download）、freshclam 病毒库
//! 更新、clamd 引擎创建与扫描——需要真网络/真引擎，走既有 graceful-failure
//! 测试（scanner_more_tests::test_cmd_test_clamav_engine_creation_fails_gracefully）。

use super::*;
use crate::api_handlers::AppState;
use crate::session::SessionManager;
use crate::ws_router::RequestContext;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

fn make_ctx(dir: &tempfile::TempDir) -> RequestContext {
    let ws = dir.path().to_string_lossy().to_string();
    let state = Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: Some(ws.clone()),
        home: Some(ws.clone()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test-model".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
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

fn write_cfg(ws: &std::path::Path, engines: serde_json::Value) {
    let dir = ws.join("config");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("config.scanner.json"),
        serde_json::json!({ "enabled": [], "engines": engines }).to_string(),
    )
    .unwrap();
}

fn noop_progress() -> Arc<dyn Fn(u64, u64) + Send + Sync> {
    Arc::new(|_written: u64, _total: u64| {})
}

// -----------------------------------------------------------------------
// format_bytes
// -----------------------------------------------------------------------

#[test]
fn format_bytes_boundaries() {
    assert_eq!(format_bytes(0), "0 B");
    assert_eq!(format_bytes(512), "512 B");
    assert_eq!(format_bytes(1023), "1023 B");
    assert_eq!(format_bytes(1024), "1.0 KB");
    assert_eq!(format_bytes(1536), "1.5 KB");
    assert_eq!(format_bytes(1024 * 1024), "1.0 MB");
    assert_eq!(format_bytes(1024 * 1024 + 512 * 1024), "1.5 MB");
}

// -----------------------------------------------------------------------
// mark_op 去重
// -----------------------------------------------------------------------

#[tokio::test]
async fn mark_op_dedup_then_finish_allows_restart() {
    let key = "wweb2-dup";
    assert!(mark_op_started(key).await.is_some(), "first start ok");
    assert!(mark_op_started(key).await.is_none(), "second start must be rejected");
    mark_op_finished(key).await;
    assert!(mark_op_started(key).await.is_some(), "after finish, restart ok");
    mark_op_finished(key).await;
}

// -----------------------------------------------------------------------
// cmd_cancel 三态
// -----------------------------------------------------------------------

#[tokio::test]
async fn cmd_cancel_no_active_op_errors() {
    let err = ScannerHandler::new()
        .cmd_cancel(&serde_json::json!({ "name": "wweb2-never" }))
        .await
        .unwrap_err();
    assert!(err.contains("no active operation for wweb2-never"), "err: {err}");
}

#[tokio::test]
async fn cmd_cancel_exact_key_cancels_token() {
    let key = "wweb2-exact";
    let token = CancellationToken::new();
    active_ops().lock().await.insert(key.to_string(), token.clone());
    let r = ScannerHandler::new()
        .cmd_cancel(&serde_json::json!({ "name": key }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cancelled"], true);
    assert_eq!(r["engine"], key);
    assert!(token.is_cancelled(), "token must be cancelled");
    active_ops().lock().await.remove(key);
}

#[tokio::test]
async fn cmd_cancel_update_db_prefix_key_cancels() {
    let key = "wweb2-eng-update-db";
    let token = CancellationToken::new();
    active_ops().lock().await.insert(key.to_string(), token.clone());
    // 用户只传 engine 名，前缀匹配到 update-db 操作键
    let r = ScannerHandler::new()
        .cmd_cancel(&serde_json::json!({ "name": "wweb2-eng" }))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(r["cancelled"], true);
    assert!(token.is_cancelled());
    active_ops().lock().await.remove(key);
}

#[tokio::test]
async fn cmd_cancel_missing_name_field() {
    let err = ScannerHandler::new()
        .cmd_cancel(&serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(err.contains("missing field: name"), "err: {err}");
}

// -----------------------------------------------------------------------
// install / update_db 命令层
// -----------------------------------------------------------------------

#[tokio::test]
async fn cmd_install_missing_name_field() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();
    let err = ScannerHandler::new()
        .cmd_install(&ws, &serde_json::json!({}), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("missing field: name"), "err: {err}");
}

#[tokio::test]
async fn cmd_install_rejects_while_op_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();
    let key = "wweb2-busy";
    active_ops().lock().await.insert(key.to_string(), CancellationToken::new());
    let err = ScannerHandler::new()
        .cmd_install(&ws, &serde_json::json!({ "name": key }), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("already in progress"), "err: {err}");
    active_ops().lock().await.remove(key);
}

#[tokio::test]
async fn cmd_update_db_rejects_while_op_in_progress() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let ws = dir.path().to_string_lossy().to_string();
    let key = "wweb2-busy2-update-db";
    active_ops().lock().await.insert(key.to_string(), CancellationToken::new());
    let err = ScannerHandler::new()
        .cmd_update_db(&ws, &serde_json::json!({ "name": "wweb2-busy2" }), &ctx)
        .await
        .unwrap_err();
    assert!(err.contains("database update already in progress"), "err: {err}");
    active_ops().lock().await.remove(key);
}

// -----------------------------------------------------------------------
// install_engine_inner / update_db_inner 网络前早期错误臂
// -----------------------------------------------------------------------

#[tokio::test]
async fn install_engine_inner_early_errors() {
    let hub = crate::events::EventHub::new();
    let token = CancellationToken::new();
    let cb = noop_progress();

    // 1) 配置文件存在但 JSON 损坏（load_scanner_config 对缺失文件有三层
    //    回退不报错，parse 失败才是可达臂）
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.scanner.json"), "{broken").unwrap();
    let err = install_engine_inner(&ws, "clamav", false, None, &hub, &token, &cb)
        .await
        .unwrap_err();
    assert!(err.starts_with("load config:"), "err: {err}");

    // 2) 引擎不存在（合法空配置）
    write_cfg(dir.path(), serde_json::json!({}));
    let err = install_engine_inner(&ws, "ghost", false, None, &hub, &token, &cb)
        .await
        .unwrap_err();
    assert_eq!(err, "engine 'ghost' not found");

    // 3) 已安装且未 force
    write_cfg(
        dir.path(),
        serde_json::json!({ "clamav": { "state": { "install_status": "installed" } } }),
    );
    let err = install_engine_inner(&ws, "clamav", false, None, &hub, &token, &cb)
        .await
        .unwrap_err();
    assert!(err.contains("already installed"), "err: {err}");

    // 4) 未安装但 URL 为空（网络前的最后一道校验）
    write_cfg(
        dir.path(),
        serde_json::json!({ "clamav": { "url": "", "clamav_path": "" } }),
    );
    let err = install_engine_inner(&ws, "clamav", false, None, &hub, &token, &cb)
        .await
        .unwrap_err();
    assert_eq!(err, "no download URL configured");
}

#[tokio::test]
async fn update_db_inner_early_errors() {
    let hub = crate::events::EventHub::new();
    let token = CancellationToken::new();

    // 1) 配置文件存在但 JSON 损坏（同上：缺失文件会回退不报错）
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let cfg_dir = dir.path().join("config");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    std::fs::write(cfg_dir.join("config.scanner.json"), "{broken").unwrap();
    let err = update_db_inner(&ws, "clamav", &hub, &token).await.unwrap_err();
    assert!(err.starts_with("load config:"), "err: {err}");

    // 2) 引擎不存在（合法空配置）
    write_cfg(dir.path(), serde_json::json!({}));
    let err = update_db_inner(&ws, "ghost", &hub, &token).await.unwrap_err();
    assert_eq!(err, "engine 'ghost' not found");

    // 3) 未安装（无 clamav_path）→ 不进入 freshclam
    write_cfg(
        dir.path(),
        serde_json::json!({ "clamav": { "clamav_path": "" } }),
    );
    let err = update_db_inner(&ws, "clamav", &hub, &token).await.unwrap_err();
    assert_eq!(err, "engine not installed (no clamav_path)");
}

// -----------------------------------------------------------------------
// make_download_progress_cb（SSE 事件）
// -----------------------------------------------------------------------

#[tokio::test]
async fn download_progress_cb_publishes_percentage_event() {
    let hub = Arc::new(crate::events::EventHub::new());
    let mut rx = hub.subscribe();
    let cb = make_download_progress_cb(hub.clone(), "wweb2-eng");
    cb(512, 2048);
    let ev = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("event within 500ms")
        .expect("event present");
    assert_eq!(ev.event_type, "scanner-progress");
    assert_eq!(ev.data["engine"], "wweb2-eng");
    assert_eq!(ev.data["phase"], "downloading");
    assert_eq!(ev.data["progress"], 25);
    let msg = ev.data["message"].as_str().unwrap();
    assert!(msg.contains("25%"), "msg: {msg}");
    assert!(msg.contains("512 B"), "msg: {msg}");
    assert!(msg.contains("2.0 KB"), "msg: {msg}");
}

#[tokio::test]
async fn download_progress_cb_zero_total_uses_bytes_arm() {
    let hub = Arc::new(crate::events::EventHub::new());
    let mut rx = hub.subscribe();
    let cb = make_download_progress_cb(hub.clone(), "wweb2-eng");
    cb(300, 0);
    let ev = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("event within 500ms")
        .expect("event present");
    assert_eq!(ev.data["progress"], 0);
    let msg = ev.data["message"].as_str().unwrap();
    assert!(msg.contains("300 B"), "msg: {msg}");
    assert!(!msg.contains('%'), "no percentage in unknown-total arm: {msg}");
}

#[tokio::test]
async fn download_progress_cb_caps_at_100_percent() {
    let hub = Arc::new(crate::events::EventHub::new());
    let mut rx = hub.subscribe();
    let cb = make_download_progress_cb(hub.clone(), "wweb2-eng");
    // written > total（超发/未知总量的防御）→ 不超过 100
    cb(9999, 100);
    let ev = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv())
        .await
        .expect("event within 500ms")
        .expect("event present");
    assert_eq!(ev.data["progress"], 100);
}
