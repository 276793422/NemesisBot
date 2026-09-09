//! 资产下载端点测试（tower ServiceExt 一次性请求驱动，不拉起真 server）：
//! 配置缺失/参数缺失/未登记 ref/白名单拦截/token 篡改与过期 → 各诚实状态码；
//! 合法 bundle → 200 + 字节一致 + sha256 header。

use super::*;
use crate::api_handlers::AppState;
use crate::events::EventHub;
use crate::session::SessionManager;
use axum::http::StatusCode;
use axum::body::Body;
use http_body_util::BodyExt;
use tower::ServiceExt;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::Instant;

const SECRET: &[u8] = b"asset-endpoint-test-secret-0123456789";

/// 带资产服务的最小 AppState（真实 BoardStore + assets 目录落盘）。
struct AssetFixture {
    state: std::sync::Arc<AppState>,
    dir: std::path::PathBuf,
}

impl Drop for AssetFixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

fn make_state(with_asset_cfg: bool) -> AssetFixture {
    let n = ASSET_TEST_SEQ.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    let dir = std::env::temp_dir().join(format!(
        "nemesis-web-asset-ep-{}-{n}",
        std::process::id()
    ));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("board")).expect("create board dir");

    let store = nemesis_board::BoardStore::open(&dir.join("board").join("board.db"), "NB")
        .expect("open store");
    let mut service =
        nemesis_board::BoardService::new(std::sync::Arc::new(store), nemesis_types::cluster::NodeRole::Coordinator);
    if with_asset_cfg {
        let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(&dir);
        std::fs::create_dir_all(&assets_dir).expect("create assets dir");
        service = service
            .with_asset_secret(SECRET.to_vec())
            .with_assets_dir(assets_dir);
    }

    let state = std::sync::Arc::new(AppState {
        auth_token: String::new(),
        session_count: std::sync::Arc::new(AtomicUsize::new(0)),
        workspace: Some(dir.to_string_lossy().to_string()),
        home: Some(dir.to_string_lossy().to_string()),
        version: "test".to_string(),
        start_time: Instant::now(),
        model_name: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_base: std::sync::Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: std::sync::Arc::new(AtomicBool::new(false)),
        event_hub: std::sync::Arc::new(EventHub::new()),
        running: std::sync::Arc::new(AtomicBool::new(true)),
        session_manager: std::sync::Arc::new(SessionManager::with_default_timeout()),
        inbound_tx: None,
        streaming_provider: None,
        ws_router: None,
        agent_service: None,
        data_store: None,
        memory_manager: None,
        forge: None,
        agent_loop: std::sync::Arc::new(parking_lot::RwLock::new(None)),
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
        webhook_rate_limiter: std::sync::Arc::new(crate::handlers::workflow::WebhookRateLimiter::new()),
        #[cfg(not(feature = "workflow"))]
        webhook_rate_limiter: std::sync::Arc::new(()),
        internal_cmd_tx: None,
        estop: None,
        cron: None,
        board: Some(service),
    });
    AssetFixture { state, dir }
}

static ASSET_TEST_SEQ: AtomicUsize = AtomicUsize::new(0);

/// 向端点发一次 oneshot 请求，返回（status, 全量 body 字节, sha256 header）。
async fn request(
    state: &std::sync::Arc<AppState>,
    ref_name: &str,
    query: &str,
) -> (StatusCode, bytes::Bytes, Option<String>) {
    let router = axum::Router::new().route(
        "/api/board/asset/{ref}",
        axum::routing::get(handle_board_asset_download),
    ).with_state(state.clone());
    let uri = format!("/api/board/asset/{ref_name}{query}");
    let resp = router
        .oneshot(
            axum::http::Request::builder()
                .uri(&uri)
                .body(Body::empty())
                .unwrap(),
        )
        .await
        .expect("oneshot response");
    let status = resp.status();
    let sha = resp
        .headers()
        .get("x-asset-sha256")
        .and_then(|v| v.to_str().ok())
        .map(str::to_string);
    let body = resp.into_body().collect().await.expect("collect body").to_bytes();
    (status, body, sha)
}

fn signed_query(ref_name: &str, expires_at: i64) -> String {
    let token = nemesis_board::sign_asset_token(SECRET, ref_name, expires_at);
    format!("?asset_token={token}&expires_at={expires_at}")
}

/// 登记 + 落盘一个资产实体；返回 (ref, 内容字节)。
fn register_and_write(fx: &AssetFixture, ref_name: &str, content: &[u8]) {
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(content);
    let sha = format!("{:x}", hasher.finalize());
    let assets_dir = nemesis_path::resolve_board_assets_dir_in_workspace(&fx.dir);
    std::fs::create_dir_all(&assets_dir).expect("create assets dir");
    std::fs::write(assets_dir.join(ref_name), content).expect("write asset file");
    fx.state
        .board
        .as_ref()
        .unwrap()
        .store()
        .register_asset(nemesis_board::NewAsset {
            ref_name: ref_name.to_string(),
            origin_issue: Some(1),
            sha256: sha,
            size: content.len() as i64,
        })
        .expect("register asset");
}

#[tokio::test]
async fn missing_config_and_params_are_honest() {
    // 未配资产服务（board 有、secret 无）→ 503。
    let fx = make_state(false);
    register_and_write(&fx, "plain.txt", b"x");
    let (status, _, _) = request(&fx.state, "plain.txt", "?asset_token=t&expires_at=1").await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    drop(fx);

    // 配好但 query 缺 token / 缺 expires_at / expires_at 非数字 → 400。
    let fx = make_state(true);
    register_and_write(&fx, "a.txt", b"aa");
    let (s1, _, _) = request(&fx.state, "a.txt", "?expires_at=99").await;
    assert_eq!(s1, StatusCode::BAD_REQUEST);
    let (s2, _, _) = request(&fx.state, "a.txt", "?asset_token=t").await;
    assert_eq!(s2, StatusCode::BAD_REQUEST);
    let (s3, _, _) = request(&fx.state, "a.txt", "?asset_token=t&expires_at=zzz").await;
    assert_eq!(s3, StatusCode::BAD_REQUEST);

    // 路径穿越 / 非白名单字符（..、分隔符、冒号）→ 400，不查表不读盘。
    for bad in ["..%2Fescape", "sub%2Fdir.txt", "a%3Ab.txt"] {
        let (s, _, _) = request(&fx.state, bad, "?asset_token=t&expires_at=1").await;
        assert_eq!(s, StatusCode::BAD_REQUEST, "traversal {bad} must 400");
    }
}

#[tokio::test]
async fn unregistered_ref_and_missing_file_404() {
    let fx = make_state(true);
    // 表里没有的 ref → 404（token 签了也不行——白名单以表为准）。
    let far = 4_102_444_800i64;
    let (status, body, _) = request(&fx.state, "ghost.txt", &signed_query("ghost.txt", far)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("not found"));

    // 表登记了但盘上没有 → 404（表有盘无诚实报错）。
    use sha2::Digest;
    let mut hasher = sha2::Sha256::new();
    hasher.update(b"missing");
    fx.state
        .board
        .as_ref()
        .unwrap()
        .store()
        .register_asset(nemesis_board::NewAsset {
            ref_name: "phantom.txt".to_string(),
            origin_issue: Some(1),
            sha256: format!("{:x}", hasher.finalize()),
            size: 7,
        })
        .unwrap();
    let (status, body, _) =
        request(&fx.state, "phantom.txt", &signed_query("phantom.txt", far)).await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert!(String::from_utf8_lossy(&body).contains("missing on disk"));
}

#[tokio::test]
async fn tampered_and_expired_tokens_403() {
    let fx = make_state(true);
    register_and_write(&fx, "spec.md", b"# spec");
    let far = 4_102_444_800i64;
    let past = 1i64;

    // 篡改 token → 403 invalid。
    let (status, body, _) =
        request(&fx.state, "spec.md", &format!("?asset_token=deadbeef&expires_at={far}")).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(String::from_utf8_lossy(&body).contains("invalid"));

    // 过期 → 403 expired（提示重拿引用）。
    let (status, body, _) = request(&fx.state, "spec.md", &signed_query("spec.md", past)).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
    assert!(String::from_utf8_lossy(&body).contains("expired"));
}

#[tokio::test]
async fn valid_bundle_streams_content_with_sha() {
    let fx = make_state(true);
    let content = b"hello asset content \xf0\x9f\x98\x80 binary-safe".to_vec();
    register_and_write(&fx, "report-v1.md", &content);
    let far = 4_102_444_800i64;
    let (status, body, sha) =
        request(&fx.state, "report-v1.md", &signed_query("report-v1.md", far)).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body.as_ref(), content.as_slice(), "bytes must round-trip");
    assert_eq!(sha.unwrap().len(), 64, "sha256 header must be 64 hex");
}

#[tokio::test]
async fn token_signed_for_other_ref_rejected() {
    let fx = make_state(true);
    register_and_write(&fx, "one.txt", b"1");
    register_and_write(&fx, "two.txt", b"2");
    let far = 4_102_444_800i64;
    // one.txt 的 token 拿去拉 two.txt → 403（ref 进签名内容）。
    let query = signed_query("one.txt", far);
    let (status, _, _) = request(&fx.state, "two.txt", &query).await;
    assert_eq!(status, StatusCode::FORBIDDEN);
}
