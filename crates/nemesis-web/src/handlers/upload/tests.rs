//! （多模态 goal 2026-09-03）上传端点 + media 引用解析 + TTL 清扫测试。
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
/// 并入全 crate 共享的 `test_home::HOME_RACE_LOCK`——模块内
/// 私有锁挡不住跨模块 chat_log 单例读写的竞态窗口。
fn uploads_state_lock() -> parking_lot::ReentrantMutexGuard<'static, ()> {
    crate::test_home::lock_home()
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
        cors_manager: None,
        events_snapshot: None,
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
        quic_relay: None,
        mail_plugin: None,
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
        estop_audit: None,
        background_registry: None,
        signature_verify: None,
        skills_install_gate: None,
        session_stats: None,
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

/// 最小合法 JPEG：FF FF 前缀（sniff 只看前缀）。
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
    post_upload_with_session(app, token, name, None, body).await
}

/// 会话资产区变体：`?session_id=` 走会话 home/uploads 落盘。
async fn post_upload_with_session(
    app: axum::Router,
    token: Option<&str>,
    name: Option<&str>,
    session_id: Option<&str>,
    body: Vec<u8>,
) -> axum::http::Response<axum::body::Body> {
    let mut uri = match name {
        Some(n) => format!("/api/upload/image?name={}", n),
        None => "/api/upload/image".to_string(),
    };
    if let Some(sid) = session_id {
        uri.push_str(&format!("&session_id={}", sid));
    }
    let mut builder = axum::http::Request::builder().method("POST").uri(uri);
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
    let _lock = uploads_state_lock();
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

    // 文件落盘且字节逐位一致（写后即落盘：std::fs:write 关 fd 即 flush）。
    assert_eq!(std::fs::read(&stored).unwrap(), payload);

    // 上传返回的 id 可被 resolve_media_ref 解析回同一路径（WSAPI 闭环）。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), None).as_deref(),
        Some(stored.to_string_lossy().as_ref())
    );
}

// F-C（四轮盲审）：落盘扩展名按**内容**定，不信 `?name=` 声明——
// JPEG 字节命名.png 若按 name 落盘，下游水合按扩展名定 media_type 时
// content/type 失配。
#[tokio::test]
async fn fc_upload_stored_extension_follows_content_not_name() {
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());

    // JPEG 字节 +.png 声明 → 落盘.jpg（内容说了算）。
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

    // .jpeg 声明同样归一为内容判定的.jpg（与 URL 下载命名一致）。
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

    // PNG 字节 +.jpg 声明 → 反向同样按内容落.png。
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
            resolve_media_ref(&serde_json::json!({ "id": bad }), None),
            None,
            "id={bad:?}"
        );
    }
    // 未知形状 / 空路径 passthrough 拒绝。
    assert_eq!(resolve_media_ref(&serde_json::json!({}), None), None);
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "path": "  " }), None),
        None
    );
    // path 为用户点名语义：非空即原样返回。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "path": "C:/pics/cat.jpg" }), None).as_deref(),
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

// --- 会话资产区（2026-11-12）：session_id 上传落 home/uploads + media 解析 ---

#[tokio::test]
async fn upload_with_session_id_lands_in_session_home() {
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());

    // 会话绑定（meta 写进重定向 home 的 session_logs；workspace 取单例）。
    let workspace = nemesis_path::default_path_manager().workspace();
    let session_id = format!("sesshome-{}", uuid::Uuid::new_v4().simple());
    let session_key = format!("agent:main:session:{}", session_id);
    nemesis_agent::session_home::assign_session_home(&workspace, &session_key)
        .expect("assign session home");

    // 上传带 session_id → 落会话 home/uploads（非全局）。
    let resp = post_upload_with_session(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.png"),
        Some(&session_id),
        png_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = json["id"].as_str().unwrap().to_string();
    let stored = std::path::PathBuf::from(json["path"].as_str().unwrap());

    let home =
        nemesis_agent::session_home::resolve_session_home(&workspace, &session_key).expect("bound");
    assert!(
        stored.starts_with(home.join("uploads")),
        "上传必须落会话 home/uploads: {stored:?}"
    );
    assert!(
        !stored.starts_with(nemesis_path::resolve_uploads_dir_in_workspace(&workspace)),
        "绑定会话上传不得落全局 uploads"
    );

    // media 解析：带会话键命中 home；不带会话键（None）→ 全局兜底 miss。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), Some(session_key.as_str())).as_deref(),
        Some(stored.to_string_lossy().as_ref())
    );
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), None),
        None,
        "home 内文件不得被全局解析命中"
    );
}

#[tokio::test]
async fn upload_without_session_id_stays_global_and_resolves() {
    // 存量行为回归：缺省上传落全局 uploads，media 解析（None 会话键）命中。
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());

    let resp = post_upload(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.png"),
        png_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = json["id"].as_str().unwrap().to_string();
    let stored = std::path::PathBuf::from(json["path"].as_str().unwrap());

    let workspace = nemesis_path::default_path_manager().workspace();
    assert!(
        stored.starts_with(nemesis_path::resolve_uploads_dir_in_workspace(&workspace)),
        "无 session_id 上传落全局: {stored:?}"
    );
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), None).as_deref(),
        Some(stored.to_string_lossy().as_ref())
    );
}

#[tokio::test]
async fn upload_with_unknown_session_id_falls_back_to_global() {
    // 未绑定会话（存量 lenient）：session_id 解析不出 home → 落全局，行为
    // 与缺省一致。
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());

    let unknown_sid = format!("ghost-{}", uuid::Uuid::new_v4().simple());
    let resp = post_upload_with_session(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.png"),
        Some(&unknown_sid),
        png_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let stored = std::path::PathBuf::from(json["path"].as_str().unwrap());

    let workspace = nemesis_path::default_path_manager().workspace();
    assert!(
        stored.starts_with(nemesis_path::resolve_uploads_dir_in_workspace(&workspace)),
        "未绑定会话上传落全局: {stored:?}"
    );
}

// --- 2026-11-16 污染修复回归：项目会话上传/media/fork 恒主 workspace ---
// 前史（BUG-20261112-117）：项目会话资产区根一度拍板为项目目录，上传端点
// 与 resolve_media_ref 曾随 project_root_for_session 取根。2026-11-16 反转
// 拍板：`agents/<id>/home` 树落用户真实工作树是污染（checkpoint 影子库
// 「项目目录零污染」同族先例），根恒主 workspace——sessions.create /
// upload / resolve_media_ref / fork / admission / hydrate 全链同源。

#[tokio::test]
async fn project_session_upload_and_media_ref_use_main_workspace() {
    let _home_lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    let workspace = nemesis_path::default_path_manager().workspace();
    let project_dir = dir.path().join("proj");
    std::fs::create_dir_all(&project_dir).unwrap();

    let session_id = format!("projhome-{}", uuid::Uuid::new_v4().simple());
    let session_key = format!("agent:main:session:{}", session_id);
    // 项目会话形态：资产区绑定 + 项目归属烧入 meta（2026-11-12 前分配点
    // 同款状态），但树恒物化在主 workspace（修复后唯一合法位置）。
    let bound_id = nemesis_agent::session_home::assign_session_home(&workspace, &session_key)
        .expect("assign main-workspace session home");
    nemesis_agent::chat_log::write_session_project(
        &session_key,
        "proj-1",
        &project_dir.to_string_lossy(),
    );

    // 上传带 session_id → 必须落主 workspace home/uploads（非项目目录、
    // 非全局）。
    let resp = post_upload_with_session(
        test_router(make_state("")),
        Some("ignored"),
        Some("photo.png"),
        Some(&session_id),
        png_bytes(),
    )
    .await;
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), 65536).await.unwrap();
    let json: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    let id = json["id"].as_str().unwrap().to_string();
    let stored = std::path::PathBuf::from(json["path"].as_str().unwrap());

    let main_home = nemesis_agent::session_home::resolve_session_home(&workspace, &session_key)
        .expect("session bound in main workspace");
    assert_eq!(
        nemesis_path::resolve_agent_home_in_workspace(&workspace, &bound_id),
        main_home,
        "绑定解析锚主 workspace"
    );
    assert!(
        stored.starts_with(main_home.join("uploads")),
        "项目会话上传必须落主 workspace home/uploads: {stored:?}"
    );
    assert!(
        !stored.starts_with(&project_dir),
        "上传不得落项目目录（污染回归锁）: {stored:?}"
    );
    assert!(
        !stored.starts_with(nemesis_path::resolve_uploads_dir_in_workspace(&workspace)),
        "绑定会话上传不得落全局 uploads: {stored:?}"
    );

    // media 解析（带会话键）→ 主 workspace 命中；不带会话键 → 全局兜底
    // miss（None）。
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), Some(session_key.as_str())).as_deref(),
        Some(stored.to_string_lossy().as_ref())
    );
    assert_eq!(
        resolve_media_ref(&serde_json::json!({ "id": id }), None),
        None
    );

    // fork copy-on-fork 同根：源/新 home 都在主 workspace 树下。
    let new_key = format!(
        "agent:main:session:projhome-fork-{}",
        uuid::Uuid::new_v4().simple()
    );
    let forked =
        nemesis_agent::session_home::fork_copy_session_home(&workspace, &session_key, &new_key)
            .expect("fork copy should succeed")
            .expect("fork copies home");
    let new_home = nemesis_path::resolve_agent_home_in_workspace(&workspace, &forked);
    let copied = new_home.join("uploads").join(&id);
    assert!(copied.is_file(), "fork 复制必须带走上传资产: {copied:?}");
    assert!(
        !new_home.starts_with(&project_dir),
        "fork 产物不得落项目目录: {new_home:?}"
    );
}

// --- BUG-20261119-208：uploads sweeper 终态令牌换装（BUG-20261115-023 家族
// --- nemesis-web 成员）回归锁。局部令牌——全局令牌在测试里绝不可 cancel
// --- （shutdown_tests 纪律）。

/// 预取消形态：令牌先取消再 spawn → 前置 is_cancelled 检查零工作退出
/// （连目录枚举都不做）。墙钟 <2s：无 select 的裸 6h park 必超时红。
#[tokio::test]
async fn uploads_sweeper_pre_cancelled_exits_immediately() {
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    let token = tokio_util::sync::CancellationToken::new();
    token.cancel(); // 预取消：spawn 前已终态
    let handle = spawn_uploads_sweeper(token);
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("预取消形态必须在 2s 内退出（裸 park 形态 park 满 6h 超时红）")
        .expect("sweeper 任务不得 panic（JoinError）");
}

/// 启动清扫后 park 期间取消抢占：活令牌 spawn → 启动首轮清扫在 tempdir
/// 内真实执行（过期文件被删 = 清扫真实发生的正面证据）→ cancel → 6h park
/// 被抢占退出。裸 park 形态要等满 6h（超时红）。
#[tokio::test]
async fn uploads_sweeper_cancel_during_park_exits_after_startup_sweep() {
    let _lock = uploads_state_lock();
    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    // 播一个 8 天前的过期文件进 uploads 目录（TTL 7 天 → 启动清扫应删）。
    let uploads = nemesis_path::resolve_uploads_dir_in_workspace(
        &nemesis_path::default_path_manager().workspace(),
    );
    std::fs::create_dir_all(&uploads).unwrap();
    let expired = uploads.join("expired.png");
    std::fs::write(&expired, b"x").unwrap();
    let old_mtime = std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(&expired)
        .unwrap()
        .set_modified(old_mtime)
        .unwrap();

    let token = tokio_util::sync::CancellationToken::new();
    let handle = spawn_uploads_sweeper(token.clone());
    // 启动首轮清扫（spawn 后同步开跑的第一拍）：过期文件应被删。
    for _ in 0..50 {
        if !expired.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(!expired.exists(), "启动清扫应删除过期文件");
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("park 期间取消必须抢占退出（裸 park 形态 park 满 6h 超时红）")
        .expect("sweeper 任务不得 panic（JoinError）");
}

// --- BUG-20261127-037：uploads 清扫树枚举臂链接穿透（BUG-20261127-033 同
// --- 威胁模型漏网）回归锁。agents/<x> 条目或 <x>/home 是 junction 时，旧
// --- 形态派生 home/uploads 路径穿透链接枚举外部真实文件并按 7 天 TTL 删除
// --- （数据破坏面）。修复后三层守卫（条目 file_type / home
// --- symlink_metadata / sweep 根位）整树跳过。
//
// 链接种植：Windows `mklink /J`（junction，无特权，与 033 回归测试同款）/
// unix symlink。
fn plant_link(at: &std::path::Path, name: &str, target: &std::path::Path) {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("cmd")
            .args(["/C", "mklink", "/J"])
            .arg(at.join(name))
            .arg(target)
            .output()
            .expect("run mklink");
        assert!(
            out.status.success(),
            "mklink /J failed: {}",
            String::from_utf8_lossy(&out.stdout)
        );
    }
    #[cfg(unix)]
    std::os::unix::fs::symlink(target, at.join(name)).expect("symlink");
}

/// 8 天前 mtime（TTL 7 天之上，保证按龄判定过期）。
fn backdate_old(path: &std::path::Path) {
    let old = std::time::SystemTime::now() - Duration::from_secs(8 * 24 * 3600);
    std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_modified(old)
        .unwrap();
}

/// 根位链接锁：uploads_dir 本身是 junction → 整目录跳过（返回 0），外部
/// 真实过期文件原样留存（穿链接删除 = 数据破坏）。
#[test]
fn sweep_uploads_dir_link_root_not_traversed() {
    let external = tempfile::tempdir().unwrap();
    let target = external.path().join("real-uploads");
    std::fs::create_dir_all(&target).unwrap();
    let old_file = target.join("old.png");
    std::fs::write(&old_file, b"x").unwrap();
    backdate_old(&old_file);

    let holder = tempfile::tempdir().unwrap();
    plant_link(holder.path(), "uplink", &target);

    let removed = sweep_uploads_older_than(&holder.path().join("uplink"), UPLOADS_TTL);
    assert_eq!(removed, 0, "链接根必须整目录跳过");
    assert!(
        old_file.is_file(),
        "外部真实过期文件必须原样留存（数据破坏锁）"
    );
}

/// 条目链接锁（spawn_uploads_sweeper 全链路）：agents/<x> 本身是 junction
/// → 整 home 跳过，外部 home/uploads 的过期文件留存。确定性控制：全局
/// uploads 过期文件被删 = 启动清扫确实跑过（真扫了的正面证据）。
#[tokio::test]
async fn sweeper_home_tree_entry_link_skipped() {
    let _lock = uploads_state_lock();
    let external = tempfile::tempdir().unwrap();
    let ext_up = external.path().join("home").join("uploads");
    std::fs::create_dir_all(&ext_up).unwrap();
    let old_file = ext_up.join("old.png");
    std::fs::write(&old_file, b"x").unwrap();
    backdate_old(&old_file);

    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    let workspace = nemesis_path::default_path_manager().workspace();
    let agents = nemesis_path::resolve_agents_root_in_workspace(&workspace);
    std::fs::create_dir_all(&agents).unwrap();
    plant_link(&agents, "deadbeef", external.path());

    let uploads = nemesis_path::resolve_uploads_dir_in_workspace(&workspace);
    std::fs::create_dir_all(&uploads).unwrap();
    let control = uploads.join("control.png");
    std::fs::write(&control, b"x").unwrap();
    backdate_old(&control);

    let token = tokio_util::sync::CancellationToken::new();
    let handle = spawn_uploads_sweeper(token.clone());
    for _ in 0..200 {
        if !control.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !control.exists(),
        "控制文件应被启动清扫删除（清扫已运行的正面证据）"
    );
    assert!(
        old_file.is_file(),
        "agents 条目链接整树跳过：外部真实过期文件必须原样留存（数据破坏锁）"
    );
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("取消后 2s 内退出")
        .expect("sweeper 任务不得 panic（JoinError）");
}

/// home 链接锁：<x> 真实目录但 home 是 junction → 派生 home/uploads 穿透
/// 枚举外部树。守卫后整 home 跳过，外部过期文件留存。控制同上。
#[tokio::test]
async fn sweeper_home_dir_link_skipped() {
    let _lock = uploads_state_lock();
    let external = tempfile::tempdir().unwrap();
    let ext_up = external.path().join("uploads");
    std::fs::create_dir_all(&ext_up).unwrap();
    let old_file = ext_up.join("old.png");
    std::fs::write(&old_file, b"x").unwrap();
    backdate_old(&old_file);

    let dir = tempfile::tempdir().unwrap();
    let _guard = RedirectHomeGuard::to(dir.path());
    let workspace = nemesis_path::default_path_manager().workspace();
    let node_dir = nemesis_path::resolve_agents_root_in_workspace(&workspace).join("realnode");
    std::fs::create_dir_all(&node_dir).unwrap();
    plant_link(&node_dir, "home", external.path());

    let uploads = nemesis_path::resolve_uploads_dir_in_workspace(&workspace);
    std::fs::create_dir_all(&uploads).unwrap();
    let control = uploads.join("control.png");
    std::fs::write(&control, b"x").unwrap();
    backdate_old(&control);

    let token = tokio_util::sync::CancellationToken::new();
    let handle = spawn_uploads_sweeper(token.clone());
    for _ in 0..200 {
        if !control.exists() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
    }
    assert!(
        !control.exists(),
        "控制文件应被启动清扫删除（清扫已运行的正面证据）"
    );
    assert!(
        old_file.is_file(),
        "home 链接整树跳过：外部真实过期文件必须原样留存（数据破坏锁）"
    );
    token.cancel();
    tokio::time::timeout(std::time::Duration::from_secs(2), handle)
        .await
        .expect("取消后 2s 内退出")
        .expect("sweeper 任务不得 panic（JoinError）");
}
