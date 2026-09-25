//! upload.rs AGT 覆盖率批次（2026-09-25）。与 tests 互补，聚焦仍缺的确定性臂：
//! - uploads 目录创建失败 → 500（`<home>/workspace/uploads` 预置成普通
//!   文件 → create_dir_all 必败，Windows/Unix 同构）
//! - 成功落盘路径的 tracing 字段求值（进程内无 subscriber 时 info! 宏在
//!   字段求值前短路——装一个最小 INFO subscriber 使字段行被覆盖）
//! - sweep 的非文件项跳过臂（目录混进 uploads）+ removed>0 日志字段
//!
//! 结构性豁免（见报告）：
//! - 138-145 写盘失败臂：dest 文件名 = `web_{millis}_{seq:04x}.{ext}`，
//!   毫秒 + 进程内原子序号在测试里不可预置占位（并行测试共享 seq，竞态
//!   不可确定性复现；无平台无关的目录只读手段）。
//! - 238 周期清扫臂：6 小时 interval 的循环体，结构性等不到。

#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use axum::routing::post;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tower::ServiceExt;

fn agt_home_lock() -> parking_lot::ReentrantMutexGuard<'static, ()> {
    crate::test_home::lock_home()
}

/// RAII：重定向单例 home，drop 时恢复（同 tests::RedirectHomeGuard 形状）。
struct AgtRedirectHome(std::path::PathBuf);
impl AgtRedirectHome {
    fn to(dir: &std::path::Path) -> Self {
        let old = nemesis_path::default_path_manager().home_dir();
        nemesis_path::default_path_manager().set_home_dir(dir.to_path_buf());
        Self(old)
    }
}
impl Drop for AgtRedirectHome {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.0.clone());
    }
}

fn agt_state() -> Arc<AppState> {
    Arc::new(AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(0)),
        workspace: None,
        home: None,
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(true)),
        session_manager: Arc::new(crate::session::SessionManager::with_default_timeout()),
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
        chat_secret_store: Arc::new(nemesis_workflow::chat_secrets::ChatSecretStore::in_memory()),
        #[cfg(not(feature = "workflow"))]
        chat_secret_store: Arc::new(()),
        #[cfg(feature = "workflow")]
        webhook_rate_limiter: Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        signature_verify: None,
        cron: None,
        board: None,
    })
}

fn agt_router(state: Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route(
            "/api/upload/image",
            post(handle_upload_image).layer(axum::extract::DefaultBodyLimit::max(
                UPLOAD_BODY_LIMIT_BYTES,
            )),
        )
        .with_state(state)
}

fn agt_png() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(b"agt-png-payload");
    v
}

async fn agt_post(app: axum::Router, body: Vec<u8>) -> axum::http::Response<axum::body::Body> {
    let req = axum::http::Request::builder()
        .method(axum::http::Method::POST)
        .uri("/api/upload/image?name=agt.png")
        .header("content-type", "application/octet-stream")
        .body(axum::body::Body::from(body))
        .unwrap();
    app.oneshot(req).await.unwrap()
}

/// 最小 tracing subscriber：放行 INFO 及以上事件，使 info! 宏的字段表达式
/// 真正求值（进程内无 subscriber 时宏在字段求值前短路）。
struct AgtLogSubscriber;
impl tracing::Subscriber for AgtLogSubscriber {
    fn enabled(&self, meta: &tracing::Metadata<'_>) -> bool {
        meta.level() <= &tracing::Level::INFO
    }
    fn new_span(&self, _attrs: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

// ---------------------------------------------------------------------------
// uploads 目录创建失败
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_upload_dir_create_failure_rejected() {
    let _lock = agt_home_lock();
    let dir = tempfile::tempdir().unwrap();
    let _home = AgtRedirectHome::to(dir.path());
    let ws = dir.path().join("workspace");
    std::fs::create_dir_all(&ws).unwrap();
    // uploads 位置预置成普通文件 → create_dir_all 必败。
    std::fs::write(ws.join("uploads"), b"not a dir").unwrap();

    let resp = agt_post(agt_router(agt_state()), agt_png()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::INTERNAL_SERVER_ERROR);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(v["error"], "upload_dir_create_failed", "{v}");
}

// ---------------------------------------------------------------------------
// 成功落盘 + 日志字段求值
// ---------------------------------------------------------------------------

#[tokio::test]
async fn agt_upload_success_emits_log_fields() {
    let _ = tracing::subscriber::set_global_default(AgtLogSubscriber);
    let _lock = agt_home_lock();
    let dir = tempfile::tempdir().unwrap();
    let _home = AgtRedirectHome::to(dir.path());

    let resp = agt_post(agt_router(agt_state()), agt_png()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX)
        .await
        .unwrap();
    let v: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(v["id"].as_str().unwrap().ends_with(".png"), "{v}");
    assert_eq!(v["size"], agt_png().len() as u64, "{v}");
    // 落盘文件真实存在。
    let stored = v["path"].as_str().unwrap();
    assert!(std::path::Path::new(stored).is_file(), "path={stored}");
}

// ---------------------------------------------------------------------------
// sweep：非文件项跳过 + removed 日志
// ---------------------------------------------------------------------------

#[test]
fn agt_sweep_skips_directories_and_logs_removal() {
    let _ = tracing::subscriber::set_global_default(AgtLogSubscriber);
    let dir = tempfile::tempdir().unwrap();
    // 旧文件：mtime 拨回 8 天前。
    let old = dir.path().join("old.png");
    std::fs::write(&old, b"old").unwrap();
    let old_mtime = std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    {
        let f = std::fs::OpenOptions::new().write(true).open(&old).unwrap();
        f.set_modified(old_mtime).unwrap();
    }
    // 新文件留存。
    let fresh = dir.path().join("fresh.png");
    std::fs::write(&fresh, b"fresh").unwrap();
    // 子目录（内含文件）必须整体跳过。
    let subdir = dir.path().join("not-a-file.png");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("inner.txt"), b"keep").unwrap();

    assert_eq!(sweep_uploads_older_than(dir.path(), UPLOADS_TTL), 1);
    assert!(!old.exists(), "过期文件必须被删");
    assert!(fresh.exists(), "新文件必须留存");
    assert!(subdir.is_dir(), "目录项必须跳过不删");
    assert!(subdir.join("inner.txt").exists(), "目录内容不动");
}
