//! T8（多模态 goal 2026-09-03）上传端点 + media 引用解析 + TTL 清扫测试。
//!
//! 覆盖 goal 验证清单：未鉴权 401 / 非白名单扩展名 400 / magic 伪装 415 /
//! 成功 200 + 文件落盘可读回 / 超限 413 / `resolve_media_ref` 穿越拒绝 /
//! `sweep_uploads_older_than` 按 mtime 清旧留新。
//!
//! 竞态纪律（env-test-race-lock-pattern）：成功路径与 resolve 正例会把
//! `default_path_manager()` 单例 home 重定向到临时目录（进程全局副作用），
//! 这些测试必须先拿同一把模块级锁串行；RAII guard 在 panic 展开时也会恢复
//! 原 home，不污染同进程其它测试。

// 刻意设计：#[tokio::test] 每测试独立 current_thread runtime，持 std Mutex
// guard 跨 await 不会死锁（持锁方在自己线程上恢复运行）。测试域统一豁免。
#![allow(clippy::await_holding_lock)]

use super::*;
use crate::api_handlers::AppState;
use axum::routing::post;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;
use tower::ServiceExt;

/// 模块级共享锁：串行化所有重定向 PathManager 单例的测试。
fn uploads_state_lock() -> &'static std::sync::Mutex<()> {
    static LOCK: std::sync::OnceLock<std::sync::Mutex<()>> = std::sync::OnceLock::new();
    LOCK.get_or_init(|| std::sync::Mutex::new(()))
}

/// RAII：重定向单例 home，drop 时恢复（panic 展开也恢复）。
struct RedirectHomeGuard(std::path::PathBuf);
impl RedirectHomeGuard {
    fn to(dir: &std::path::Path) -> Self {
        let old = nemesis_path::default_path_manager().home_dir();
        nemesis_path::default_path_manager().set_home_dir(dir.to_path_buf());
        Self(old)
    }
}
impl Drop for RedirectHomeGuard {
    fn drop(&mut self) {
        nemesis_path::default_path_manager().set_home_dir(self.0.clone());
    }
}

fn make_state(auth_token: &str) -> std::sync::Arc<AppState> {
    Arc::new(AppState {
        auth_token: auth_token.to_string(),
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
        cron: None,
        board: None,
    })
}

/// 与 server.rs 相同形状的最小路由（DefaultBodyLimit 与生产一致）。
fn test_router(state: std::sync::Arc<AppState>) -> axum::Router {
    axum::Router::new()
        .route(
            "/api/upload/image",
            post(handle_upload_image).layer(axum::extract::DefaultBodyLimit::max(
                UPLOAD_BODY_LIMIT_BYTES,
            )),
        )
        .with_state(state)
}

/// 最小合法 PNG：8 字节签名 + IHDR 长度占位（sniff 只看前缀）。
fn png_bytes() -> Vec<u8> {
    let mut v = vec![0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A];
    v.extend_from_slice(b"fake-png-payload");
    v
}

/// 最小合法 JPEG：FF D8 FF 前缀（sniff 只看前缀）。
fn jpeg_bytes() -> Vec<u8> {
    let mut v = vec![0xFF, 0xD8, 0xFF, 0xE0];
    v.extend_from_slice(b"fake-jpeg-payload");
    v
}

async fn post_upload(
    app: axum::Router,
    token: Option<&str>,
    name: Option<&str>,
    body: Vec<u8>,
) -> axum::http::Response<axum::body::Body> {
    let mut builder = axum::http::Request::builder()
        .method("POST")
        .uri(match name {
            Some(n) => format!("/api/upload/image?name={}", n),
            None => "/api/upload/image".to_string(),
        });
    if let Some(t) = token {
        builder = builder.header("X-Auth-Token", t);
    }
    app.oneshot(builder.body(axum::body::Body::from(body)).unwrap())
        .await
        .unwrap()
}

async fn error_body(resp: axum::http::Response<axum::body::Body>) -> serde_json::Value {
    let bytes = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    serde_json::from_slice(&bytes).unwrap()
}

#[tokio::test]
async fn upload_requires_auth() {
    let state = make_state("secret-token");
    let app = test_router(state);

    // 无 header → 401。
    let resp = post_upload(app.clone(), None, Some("a.png"), png_bytes()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
    assert_eq!(error_body(resp).await["error"], "unauthorized");

    // 错 token → 401。
    let resp = post_upload(app, Some("wrong"), Some("a.png"), png_bytes()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::UNAUTHORIZED);
}

#[tokio::test]
async fn upload_rejects_unsupported_extension() {
    let state = make_state("");
    let app = test_router(state);

    for name in ["a.txt", "noext", "a.svg"] {
        let resp = post_upload(app.clone(), Some("ignored"), Some(name), png_bytes()).await;
        assert_eq!(
            resp.status(),
            axum::http::StatusCode::BAD_REQUEST,
            "name={name}"
        );
        assert_eq!(error_body(resp).await["error"], "unsupported_extension");
    }

    // 缺 ?name= 同样 400。
    let resp = post_upload(app, Some("ignored"), None, png_bytes()).await;
    assert_eq!(resp.status(), axum::http::StatusCode::BAD_REQUEST);
}

#[tokio::test]
async fn upload_rejects_text_masquerading_as_png() {
    let state = make_state("");
    let app = test_router(state);

    let resp = post_upload(
        app,
        Some("ignored"),
        Some("fake.png"),
        b"just text, not a png".to_vec(),
    )
    .await;
    assert_eq!(
        resp.status(),
        axum::http::StatusCode::UNSUPPORTED_MEDIA_TYPE
    );
    assert_eq!(error_body(resp).await["error"], "not_an_image");
}

#[tokio::test]
async fn upload_rejects_oversized_body() {
    let state = make_state("");
    let app = test_router(state);

    // 上限 + 1 字节：低于路由 DefaultBodyLimit(26MB)，命中 handler 的
    // MAX_IMAGE_BYTES 检查 → 413。magic 无所谓（大小检查在前）。
    let body = vec![0u8; UPLOAD_MAX_BYTES as usize + 1];
    let resp = post_upload(app, Some("ignored"), Some("big.png"), body).await;
    assert_eq!(resp.status(), axum::http::StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(error_body(resp).await["error"], "too_large");
}

#[tokio::test]
async fn upload_stores_file_and_refs_resolve() {
    let _lock = uploads_state_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    let uploads_dir = nemesis_path::resolve_uploads_dir_in_workspace(
        &nemesis_path::default_path_manager().workspace(),
    );

    let payload = png_bytes();
    let resp = post_upload(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.PNG"),
        payload.clone(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);

    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();

    // id 形如 web_{millis}.png；path 落在 tempdir 的 uploads 下。
    let id = json["id"].as_str().unwrap();
    assert!(id.starts_with("web_") && id.ends_with(".png"), "id={id}");
    assert_eq!(json["size"], payload.len());
    let stored = std::path::PathBuf::from(json["path"].as_str().unwrap());
    assert!(stored.starts_with(&uploads_dir));

    // 文件落盘且字节逐位一致（写后即落盘：std::fs::write 关 fd 即 flush）。
    assert_eq!(std::fs::read(&stored).unwrap(), payload);

    // 上传返回的 id 可被 resolve_media_ref 解析回同一路径（WSAPI 闭环）。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id })).as_deref(),
        Some(stored.to_string_lossy().as_ref())
    );
}

// F-C（2026-09-04 四轮盲审）：落盘扩展名按**内容**定，不信 `?name=` 声明——
// JPEG 字节命名 .png 若按 name 落盘，下游水合按扩展名定 media_type 时
// content/type 失配。
#[tokio::test]
async fn fc_upload_stored_extension_follows_content_not_name() {
    let _lock = uploads_state_lock().lock().unwrap();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());

    // JPEG 字节 + .png 声明 → 落盘 .jpg（内容说了算）。
    let resp = post_upload(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.png"),
        jpeg_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = json["id"].as_str().unwrap();
    assert!(id.ends_with(".jpg"), "JPEG 字节必须落盘为 .jpg: id={id}");

    // .jpeg 声明同样归一为内容判定的 .jpg（与 C3 URL 下载命名一致）。
    let resp = post_upload(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.jpeg"),
        jpeg_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["id"].as_str().unwrap().ends_with(".jpg"),
        "jpeg 声明归一为 .jpg"
    );

    // PNG 字节 + .jpg 声明 → 反向同样按内容落 .png。
    let resp = post_upload(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.jpg"),
        png_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert!(
        json["id"].as_str().unwrap().ends_with(".png"),
        "PNG 字节落盘为 .png"
    );
}

#[test]
fn resolve_media_ref_rejects_bad_refs() {
    // 路径穿越 / 分隔符 / 空 / 非法字符——在目录解析前即拒绝（无需重定向单例）。
    for bad in [
        "../evil.png",
        "a/b.png",
        "a\\b.png",
        "",
        "a..b.png",
        "img pdf.png",
    ] {
        assert_eq!(
            resolve_media_ref(&serde_json::json!({ "id": bad })),
            None,
            "id={bad:?}"
        );
    }
    // 未知形状 / 空路径 passthrough 拒绝。
    assert_eq!(resolve_media_ref(&serde_json::json!({})), None);
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "path": "  " })),
        None
    );
    // path 为用户点名语义：非空即原样返回。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "path": "C:/pics/cat.jpg" })).as_deref(),
        Some("C:/pics/cat.jpg")
    );
}

#[test]
fn sweep_removes_expired_files_only() {
    let dir = tempfile::tempdir().unwrap();
    let old_path = dir.path().join("old.png");
    let new_path = dir.path().join("new.png");
    std::fs::write(&old_path, b"old").unwrap();
    std::fs::write(&new_path, b"new").unwrap();

    // 把 old 的 mtime 拨回 8 天前（std File::set_modified，Rust 1.75+）。
    let old_mtime = std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    {
        let f = std::fs::OpenOptions::new()
            .write(true)
            .open(&old_path)
            .unwrap();
        f.set_modified(old_mtime).unwrap();
    }

    // TTL 7 天：old 删、new 留。
    assert_eq!(sweep_uploads_older_than(dir.path(), UPLOADS_TTL), 1);
    assert!(!old_path.exists());
    assert!(new_path.exists());

    // 幂等：再扫一次没有可删的。
    assert_eq!(sweep_uploads_older_than(dir.path(), UPLOADS_TTL), 0);
}

#[test]
fn sweep_missing_dir_is_zero() {
    let dir = tempfile::tempdir().unwrap();
    let absent = dir.path().join("no_such_uploads");
    assert_eq!(sweep_uploads_older_than(&absent, UPLOADS_TTL), 0);
}
