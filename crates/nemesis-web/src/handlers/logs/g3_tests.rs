//! G3（spill 状态卡，2026-08-28）：logs.spill_status / logs.spill_cleanup。
//!
//! 夹具：home=workspace 的 tempdir；spill 树手工构造（set_modified 制造
//! 过期/新鲜文件，与 nemesis-agent spill/tests.rs 同法）。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
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
        chat_secret_store: std::sync::Arc::new(
            nemesis_workflow::chat_secrets::ChatSecretStore::in_memory(),
        ),
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
    });
    RequestContext {
        session_id: "test-session".to_string(),
        chat_id: "test-chat".to_string(),
        workspace: Some(ws.clone()),
        home: Some(ws),
        state,
        auth_method: crate::session::AuthMethod::default(),
    }
}

/// 造一个 spill 文件并把 mtime 拨回 `age_secs` 前（std File::set_modified）。
fn make_spill_file(root: &std::path::Path, session: &str, name: &str, age_secs: u64, content: &str) {
    let dir = root.join(session);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join(name);
    std::fs::write(&path, content).unwrap();
    let past = std::time::SystemTime::now() - std::time::Duration::from_secs(age_secs);
    let f = std::fs::OpenOptions::new().write(true).open(&path).unwrap();
    f.set_modified(past).unwrap();
}

#[test]
fn spill_status_reports_tree_and_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let root = dir.path().join("logs/spill");
    make_spill_file(&root, "s1", "newer.txt", 24 * 3600, "0123456789"); // 10 B
    make_spill_file(&root, "s2", "older.txt", 24 * 3600, "ab"); // 2 B

    let out = LogsHandler.spill_status(&ctx).unwrap().unwrap();
    assert_eq!(out["files"], 2, "out = {}", out);
    assert_eq!(out["bytes"], 12);
    assert!(out["oldest"].is_string());
    // 测试进程未装 live ConfigStore → 回退默认保留期 7 天。
    assert_eq!(out["retention_days"], 7);
    assert_eq!(out["threshold_chars"], 65_536);
    assert!(out["root"].as_str().unwrap().ends_with("spill"));
}

#[test]
fn spill_cleanup_deletes_expired_and_returns_fresh_status() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);
    let root = dir.path().join("logs/spill");
    make_spill_file(&root, "s1", "old.txt", 10 * 24 * 3600, "old!"); // 4 B, 过期
    make_spill_file(&root, "s1", "fresh.txt", 24 * 3600, "ok"); // 2 B, 保留

    let out = LogsHandler.spill_cleanup(&ctx).unwrap().unwrap();
    assert_eq!(out["deleted"], 1, "out = {}", out);
    assert_eq!(out["retention_days"], 7);
    assert_eq!(out["files"], 1, "fresh file remains");
    assert_eq!(out["bytes"], 2);
    assert!(!root.join("s1").join("old.txt").exists());
    assert!(root.join("s1").join("fresh.txt").exists());
}

#[test]
fn spill_status_missing_tree_reports_zero() {
    let dir = tempfile::tempdir().unwrap();
    let ctx = make_ctx(&dir);

    let out = LogsHandler.spill_status(&ctx).unwrap().unwrap();
    assert_eq!(out["files"], 0);
    assert_eq!(out["bytes"], 0);
    assert!(out["oldest"].is_null());
}
