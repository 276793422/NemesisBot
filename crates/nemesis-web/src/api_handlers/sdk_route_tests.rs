// P2-2 (2026-08-24 UI entry gap): HTTP-level oneshot tests for the two SDK
// download routes. The handlers are stateless (no State extractor — static
// embedded artifacts), so these build a minimal stateless router and run
// under every feature combo, including `--no-default-features`. The last
// test additionally exercises the REAL server router (WebServer::build_router)
// to prove the routes are actually registered there.

use axum::routing::get;
use axum::Router;
use tower::ServiceExt;

use super::{handle_sdk_export, handle_sdk_pip};
use crate::sdk_embed::{SDK_EXPORT_ZIP, SDK_SDIST_ZIP, SDK_VERSION};

fn make_sdk_router() -> Router {
    // Mirrors the two routes registered in server.rs; nothing else is needed
    // because the handlers take no state.
    Router::new()
        .route("/api/sdk/export", get(handle_sdk_export))
        .route("/api/sdk/pip", get(handle_sdk_pip))
}

fn is_zip_magic(b: &[u8]) -> bool {
    b.len() >= 4 && &b[..4] == b"PK\x03\x04"
}

async fn oneshot_get(app: Router, path: &str) -> axum::response::Response {
    let req = axum::http::Request::builder()
        .uri(path)
        .body(axum::body::Body::empty())
        .unwrap();
    app.oneshot(req).await.unwrap()
}

#[tokio::test]
async fn test_sdk_export_route_serves_zip() {
    let resp = oneshot_get(make_sdk_router(), "/api/sdk/export").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        cd,
        format!("attachment; filename=\"nemesisbot-sdk-{SDK_VERSION}.zip\"").as_str()
    );
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CACHE_CONTROL)
            .unwrap()
            .to_str()
            .unwrap(),
        "no-store"
    );
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    // Byte-identical to the embedded artifact (no transformation on serve).
    assert_eq!(body.as_ref(), SDK_EXPORT_ZIP);
    assert!(is_zip_magic(&body), "body must start with PK magic");
}

#[tokio::test]
async fn test_sdk_pip_route_serves_sdist_zip() {
    let resp = oneshot_get(make_sdk_router(), "/api/sdk/pip").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );
    let cd = resp
        .headers()
        .get(axum::http::header::CONTENT_DISPOSITION)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(
        cd,
        format!("attachment; filename=\"nemesisbot-sdk-pip-{SDK_VERSION}.zip\"").as_str()
    );
    let body = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024)
        .await
        .unwrap();
    assert_eq!(body.as_ref(), SDK_SDIST_ZIP);
    assert!(is_zip_magic(&body), "body must start with PK magic");
}

/// Prove the routes are wired into the REAL router that the gateway serves
/// (not just the minimal test router above). Same construction as
/// server::tests::test_build_router.
#[tokio::test]
async fn test_sdk_routes_registered_in_full_router() {
    let config = crate::server::WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    };
    let app = crate::server::WebServer::new(config).build_router();
    let resp = oneshot_get(app, "/api/sdk/export").await;
    assert_eq!(resp.status(), 200);
    assert_eq!(
        resp.headers()
            .get(axum::http::header::CONTENT_TYPE)
            .unwrap()
            .to_str()
            .unwrap(),
        "application/zip"
    );
}
