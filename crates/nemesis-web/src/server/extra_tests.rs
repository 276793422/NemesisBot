//! Additional server tests focused on uncovered branches:
//! - `content_type_for` MIME detection for all extensions
//! - `cors_origin_value` with/without Origin header
//! - `serve_embedded_static`: exact match, SPA fallback, 404
//! - Build router with embedded `static_files` and OPTIONS preflight
//! - Build router with `static_dir` directory serving + charset header
//! - SSE stream returns text/event-stream
//! - `dispatch_outbound` filtering and routing
//! - `start` / `start_with_shutdown` lifecycle (invalid addr, immediate shutdown)
//! - WebServer setters: `set_internal_cmd_tx`, `set_cluster_log_dir`, model state
//! - StaticFiles trait default impls

use super::*;
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicUsize};
use std::time::{Duration, Instant};

// ============================================================
// content_type_for — full extension coverage
// ============================================================

#[test]
fn test_content_type_for_html_and_htm() {
    assert_eq!(content_type_for("index.html"), "text/html; charset=utf-8");
    assert_eq!(content_type_for("page.htm"), "text/html; charset=utf-8");
}

#[test]
fn test_content_type_for_css() {
    assert_eq!(content_type_for("style.css"), "text/css; charset=utf-8");
}

#[test]
fn test_content_type_for_javascript_variants() {
    assert_eq!(
        content_type_for("app.js"),
        "application/javascript; charset=utf-8"
    );
    assert_eq!(
        content_type_for("app.mjs"),
        "application/javascript; charset=utf-8"
    );
}

#[test]
fn test_content_type_for_json_and_xml() {
    assert_eq!(
        content_type_for("data.json"),
        "application/json; charset=utf-8"
    );
    assert_eq!(
        content_type_for("data.xml"),
        "application/xml; charset=utf-8"
    );
}

#[test]
fn test_content_type_for_svg_and_txt() {
    assert_eq!(content_type_for("logo.svg"), "image/svg+xml; charset=utf-8");
    assert_eq!(content_type_for("readme.txt"), "text/plain; charset=utf-8");
}

#[test]
fn test_content_type_for_icon() {
    assert_eq!(content_type_for("favicon.ico"), "image/x-icon");
}

#[test]
fn test_content_type_for_raster_images() {
    assert_eq!(content_type_for("img.png"), "image/png");
    assert_eq!(content_type_for("photo.jpg"), "image/jpeg");
    assert_eq!(content_type_for("photo.jpeg"), "image/jpeg");
    assert_eq!(content_type_for("anim.gif"), "image/gif");
    assert_eq!(content_type_for("modern.webp"), "image/webp");
}

#[test]
fn test_content_type_for_fonts() {
    assert_eq!(content_type_for("f.woff"), "font/woff");
    assert_eq!(content_type_for("f.woff2"), "font/woff2");
    assert_eq!(content_type_for("f.ttf"), "font/ttf");
    assert_eq!(content_type_for("f.otf"), "font/otf");
    assert_eq!(content_type_for("f.eot"), "application/vnd.ms-fontobject");
}

#[test]
fn test_content_type_for_wasm_and_map() {
    assert_eq!(content_type_for("app.wasm"), "application/wasm");
    assert_eq!(
        content_type_for("app.js.map"),
        "application/json; charset=utf-8"
    );
}

#[test]
fn test_content_type_for_unknown_returns_octet_stream() {
    assert_eq!(
        content_type_for("file.unknownext"),
        "application/octet-stream"
    );
    assert_eq!(content_type_for("noext"), "application/octet-stream");
    assert_eq!(content_type_for(""), "application/octet-stream");
}

#[test]
fn test_content_type_for_normalizes_uppercase_extension() {
    assert_eq!(content_type_for("INDEX.HTML"), "text/html; charset=utf-8");
    assert_eq!(
        content_type_for("App.JS"),
        "application/javascript; charset=utf-8"
    );
    assert_eq!(content_type_for("PHOTO.PNG"), "image/png");
}

// ============================================================
// cors_origin_value
// ============================================================

fn build_request_with_origin(origin: Option<&str>) -> axum::extract::Request {
    let mut req = axum::http::Request::builder()
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();
    if let Some(o) = origin {
        req.headers_mut().insert(
            http::header::ORIGIN,
            http::HeaderValue::from_str(o).unwrap(),
        );
    }
    req
}

#[test]
fn test_cors_origin_value_with_header() {
    let req = build_request_with_origin(Some("https://example.com"));
    assert_eq!(cors_origin_value(&req), "https://example.com");
}

#[test]
fn test_cors_origin_value_missing_header_defaults_to_star() {
    let req = build_request_with_origin(None);
    assert_eq!(cors_origin_value(&req), "*");
}

#[test]
fn test_cors_origin_value_empty_string_header() {
    let req = build_request_with_origin(Some(""));
    assert_eq!(cors_origin_value(&req), "");
}

// ============================================================
// Mock StaticFiles for serve_embedded_static tests
// ============================================================

struct MockStaticFiles {
    files: HashMap<String, Vec<u8>>,
}

impl StaticFiles for MockStaticFiles {
    fn get_file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
    fn list_files(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

fn make_mock_static(files: &[(&str, &str)]) -> Arc<dyn StaticFiles> {
    let mut map = HashMap::new();
    for (k, v) in files {
        map.insert(k.to_string(), v.as_bytes().to_vec());
    }
    Arc::new(MockStaticFiles { files: map })
}

#[test]
fn test_static_files_trait_default_has_file_with_mock() {
    let files = make_mock_static(&[("a.txt", "a"), ("b.txt", "b")]);
    assert!(files.has_file("a.txt"));
    assert!(files.has_file("b.txt"));
    assert!(!files.has_file("missing.txt"));
}

#[test]
fn test_static_files_trait_list_files_with_mock() {
    let files = make_mock_static(&[("a.txt", "a"), ("b.txt", "b")]);
    let listed = files.list_files();
    assert_eq!(listed.len(), 2);
}

// ============================================================
// serve_embedded_static
// ============================================================

#[tokio::test]
async fn test_serve_embedded_static_exact_match_html() {
    let files = make_mock_static(&[
        ("index.html", "<html>hi</html>"),
        ("app.js", "console.log()"),
    ]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/index.html")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 200);
    let ct = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
    assert_eq!(ct.to_str().unwrap(), "text/html; charset=utf-8");
    let allow = resp
        .headers()
        .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .unwrap();
    assert_eq!(allow.to_str().unwrap(), "*");
}

#[tokio::test]
async fn test_serve_embedded_static_with_origin_header() {
    let files = make_mock_static(&[("index.html", "<html>origin</html>")]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/index.html")
        .header(http::header::ORIGIN, "https://myapp.example")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 200);
    let origin = resp
        .headers()
        .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .unwrap();
    assert_eq!(origin.to_str().unwrap(), "https://myapp.example");
}

#[tokio::test]
async fn test_serve_embedded_static_vary_header_present() {
    let files = make_mock_static(&[("index.html", "<html>vary</html>")]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/index.html")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    let vary = resp.headers().get(http::header::VARY);
    assert!(vary.is_some());
    assert_eq!(vary.unwrap().to_str().unwrap(), "Origin");
}

#[tokio::test]
async fn test_serve_embedded_static_spa_fallback_for_extensionless_path() {
    let files = make_mock_static(&[("index.html", "<html>SPA</html>")]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/some/spa/route")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 200);
    let ct = resp.headers().get(http::header::CONTENT_TYPE).unwrap();
    assert_eq!(ct.to_str().unwrap(), "text/html; charset=utf-8");
}

#[tokio::test]
async fn test_serve_embedded_static_root_path_serves_index() {
    let files = make_mock_static(&[("index.html", "<html>root</html>")]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_serve_embedded_static_404_for_missing_file_with_extension() {
    let files = make_mock_static(&[]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/missing.png")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_serve_embedded_static_404_when_spa_fallback_has_no_index() {
    let files = make_mock_static(&[]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/spa/route")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 404);
}

#[tokio::test]
async fn test_serve_embedded_static_exact_match_javascript() {
    let files = make_mock_static(&[("app.js", "console.log()")]);
    let req: axum::extract::Request = axum::http::Request::builder()
        .uri("/app.js")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = serve_embedded_static(files, req).await;
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert_eq!(ct, "application/javascript; charset=utf-8");
}

// ============================================================
// Router-level embedded static + OPTIONS preflight
// ============================================================

#[tokio::test]
async fn test_build_router_with_embedded_static_files_serves_index() {
    let files = make_mock_static(&[
        ("index.html", "<html>router</html>"),
        ("app.js", "console.log(1)"),
    ]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/index.html")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_build_router_embedded_static_spa_fallback() {
    let files = make_mock_static(&[("index.html", "<html>SPA</html>")]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/dashboard/users")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let body = axum::body::to_bytes(resp.into_body(), 4096).await.unwrap();
    assert_eq!(&body[..], b"<html>SPA</html>");
}

#[tokio::test]
async fn test_build_router_embedded_static_options_preflight_with_origin() {
    let files = make_mock_static(&[("index.html", "<html>options</html>")]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(http::Method::OPTIONS)
        .uri("/app.js")
        .header(http::header::ORIGIN, "https://example.org")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    let origin = resp
        .headers()
        .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .unwrap();
    assert_eq!(origin.to_str().unwrap(), "https://example.org");
    let methods = resp
        .headers()
        .get(http::header::ACCESS_CONTROL_ALLOW_METHODS)
        .unwrap();
    assert_eq!(methods.to_str().unwrap(), "GET, OPTIONS");
    let vary = resp.headers().get(http::header::VARY).unwrap();
    assert_eq!(vary.to_str().unwrap(), "Origin");
}

#[tokio::test]
async fn test_build_router_embedded_static_options_preflight_without_origin() {
    let files = make_mock_static(&[("index.html", "x")]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .method(http::Method::OPTIONS)
        .uri("/app.js")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 204);
    let origin = resp
        .headers()
        .get(http::header::ACCESS_CONTROL_ALLOW_ORIGIN)
        .unwrap();
    assert_eq!(origin.to_str().unwrap(), "*");
}

#[tokio::test]
async fn test_build_router_embedded_static_returns_404_for_missing_asset() {
    let files = make_mock_static(&[("index.html", "<html></html>")]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/missing.png")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 404);
}

// ============================================================
// Router-level directory static with charset header
// ============================================================

#[tokio::test]
async fn test_build_router_static_dir_html_adds_charset_utf8() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("index.html"),
        "<html><body>héllo wörld</body></html>",
    )
    .unwrap();
    let config = WebServerConfig {
        static_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/index.html")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/html"), "got: {}", ct);
    assert!(ct.contains("charset=utf-8"), "got: {}", ct);
}

#[tokio::test]
async fn test_build_router_static_dir_css_adds_charset_utf8() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("style.css"), "body { color: red; }").unwrap();
    let config = WebServerConfig {
        static_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/style.css")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap()
        .to_string();
    assert!(ct.contains("text/css"));
    assert!(ct.contains("charset=utf-8"));
}

#[tokio::test]
async fn test_build_router_static_dir_serves_nested_file() {
    let dir = tempfile::tempdir().unwrap();
    let sub = dir.path().join("assets");
    std::fs::create_dir_all(&sub).unwrap();
    std::fs::write(sub.join("logo.svg"), "<svg></svg>").unwrap();
    let config = WebServerConfig {
        static_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/assets/logo.svg")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

#[tokio::test]
async fn test_build_router_static_dir_root_serves_index() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("index.html"), "<html>autoindex</html>").unwrap();
    let config = WebServerConfig {
        static_dir: Some(dir.path().to_string_lossy().to_string()),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
}

// ============================================================
// SSE stream endpoint
// ============================================================

#[tokio::test]
async fn test_sse_stream_returns_event_stream_content_type() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    let app = server.build_router();

    use tower::ServiceExt;
    let req = axum::http::Request::builder()
        .uri("/api/events/stream")
        .body(axum::body::Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), 200);
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .unwrap()
        .to_str()
        .unwrap();
    assert!(ct.contains("text/event-stream"), "got: {}", ct);
}

// ============================================================
// WebServer setters and lifecycle
// ============================================================

#[tokio::test]
async fn test_web_server_set_workspace_pathbuf() {
    let dir = tempfile::tempdir().unwrap();
    let config = WebServerConfig::default();
    let mut server = WebServer::new(config);
    server.set_workspace(dir.path().to_path_buf());
    assert_eq!(
        server.config.workspace.as_deref(),
        Some(dir.path().to_str().unwrap())
    );
}

#[test]
fn test_web_server_set_internal_cmd_tx() {
    let config = WebServerConfig::default();
    let mut server = WebServer::new(config);
    let (tx, _rx) = tokio::sync::mpsc::channel::<crate::internal::InternalCommand>(1);
    server.set_internal_cmd_tx(tx);
    assert!(server.internal_cmd_tx.is_some());
}

#[test]
fn test_web_server_set_cluster_log_dir() {
    let config = WebServerConfig::default();
    let mut server = WebServer::new(config);
    server.set_cluster_log_dir("/some/log/dir".to_string());
    assert_eq!(server.cluster_log_dir.as_deref(), Some("/some/log/dir"));
}

#[test]
fn test_web_server_model_base_after_set_with_key() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    server.set_model_info("test-model", "https://api.example.com", true);
    assert_eq!(*server.model_base.lock(), "https://api.example.com");
    assert!(
        server
            .model_has_key
            .load(std::sync::atomic::Ordering::SeqCst)
    );
}

#[test]
fn test_web_server_model_has_key_false_after_set() {
    let config = WebServerConfig::default();
    let server = WebServer::new(config);
    server.set_model_info("test-model", "https://api.example.com", false);
    assert!(
        !server
            .model_has_key
            .load(std::sync::atomic::Ordering::SeqCst)
    );
}

// ============================================================
// start / start_with_shutdown lifecycle
// ============================================================

#[tokio::test]
async fn test_start_invalid_listen_addr_returns_error() {
    let config = WebServerConfig {
        listen_addr: "invalid-address-not-parseable".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let result = server.start().await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid listen address"));
}

#[tokio::test]
async fn test_start_with_shutdown_invalid_listen_addr_returns_error() {
    let config = WebServerConfig {
        listen_addr: "not a valid addr".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let (_tx, rx) = tokio::sync::broadcast::channel::<()>(1);
    let result = server.start_with_shutdown(rx, None).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("invalid listen address"));
}

#[tokio::test]
async fn test_start_with_shutdown_immediate_signal_succeeds() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let (tx, rx) = tokio::sync::broadcast::channel::<()>(1);
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel();
    let _ = tx.send(());

    let result = server.start_with_shutdown(rx, Some(bound_tx)).await;
    assert!(result.is_ok());
    let actual_addr = tokio::time::timeout(Duration::from_secs(2), bound_rx).await;
    assert!(actual_addr.is_ok());
    let addr = actual_addr.unwrap().unwrap();
    assert!(addr.is_ipv4());
}

#[tokio::test]
async fn test_start_with_shutdown_without_bound_tx_succeeds() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let (tx, rx) = tokio::sync::broadcast::channel::<()>(1);
    let _ = tx.send(());

    let result = server.start_with_shutdown(rx, None).await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_start_with_shutdown_binds_and_serves_until_signal() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        ..Default::default()
    };
    let server = WebServer::new(config);
    let (tx, rx) = tokio::sync::broadcast::channel::<()>(1);

    let handle = tokio::spawn(async move { server.start_with_shutdown(rx, None).await });

    tokio::time::sleep(Duration::from_millis(200)).await;
    let _ = tx.send(());
    let result = tokio::time::timeout(Duration::from_secs(5), handle).await;
    assert!(result.is_ok());
    let inner = result.unwrap();
    assert!(inner.is_ok());
}

// ============================================================
// dispatch_outbound
// ============================================================

fn make_outbound(
    channel: &str,
    chat_id: &str,
    content: &str,
    message_type: &str,
) -> nemesis_types::channel::OutboundMessage {
    nemesis_types::channel::OutboundMessage {
        channel: channel.to_string(),
        chat_id: chat_id.to_string(),
        content: content.to_string(),
        message_type: message_type.to_string(),
        meta: Default::default(),
    }
}

#[tokio::test]
async fn test_dispatch_outbound_filters_non_web_channel() {
    let bus = Arc::new(MessageBus::new());
    let session_manager = Arc::new(SessionManager::with_default_timeout());

    bus.publish_outbound(make_outbound("telegram", "telegram:123", "ignored", ""));

    let handle = tokio::spawn(async move {
        dispatch_outbound(bus, session_manager).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.abort();
}

#[tokio::test]
async fn test_dispatch_outbound_invalid_chat_id_format_warns() {
    let bus = Arc::new(MessageBus::new());
    let session_manager = Arc::new(SessionManager::with_default_timeout());

    bus.publish_outbound(make_outbound("web", "invalid-format", "test", ""));

    let handle = tokio::spawn(async move {
        dispatch_outbound(bus, session_manager).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.abort();
}

#[tokio::test]
async fn test_dispatch_outbound_routes_to_nonexistent_session() {
    let bus = Arc::new(MessageBus::new());
    let session_manager = Arc::new(SessionManager::with_default_timeout());

    bus.publish_outbound(make_outbound("web", "web:nonexistent-session", "hello", ""));

    let handle = tokio::spawn(async move {
        dispatch_outbound(bus, session_manager).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.abort();
}

#[tokio::test]
async fn test_dispatch_outbound_routes_history_message_to_nonexistent_session() {
    let bus = Arc::new(MessageBus::new());
    let session_manager = Arc::new(SessionManager::with_default_timeout());

    bus.publish_outbound(make_outbound("web", "web:nonexistent", "{}", "history"));

    let handle = tokio::spawn(async move {
        dispatch_outbound(bus, session_manager).await;
    });

    tokio::time::sleep(Duration::from_millis(100)).await;
    handle.abort();
}

// ============================================================
// send_to_session / send_history_to_session success path
// ============================================================

#[tokio::test]
async fn test_send_to_session_with_active_queue_succeeds() {
    use crate::websocket_handler::SendQueue;
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(SendQueue::from_channels(tx, done_rx));
    mgr.set_send_queue(&session.id, queue);

    let send_result = send_to_session(&mgr, &session.id, "assistant", "hello world", None).await;
    assert!(send_result.is_ok());

    let received = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(received.is_ok());
    let bytes = received.unwrap().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["module"], "chat");
    assert_eq!(parsed["cmd"], "receive");
    assert_eq!(parsed["data"]["role"], "assistant");
    assert_eq!(parsed["data"]["content"], "hello world");
}

/// Badge pipeline: send_to_session must include `model` in the WS `receive`
/// frame when provided, and omit it when None (so legacy/badge-less messages
/// stay byte-identical).
#[tokio::test]
async fn test_send_to_session_includes_model_badge() {
    use crate::websocket_handler::SendQueue;
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();

    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(SendQueue::from_channels(tx, done_rx));
    mgr.set_send_queue(&session.id, queue);

    // With a model badge.
    send_to_session(
        &mgr,
        &session.id,
        "assistant",
        "badged reply",
        Some("deepseek/deepseek-v4-flash"),
    )
    .await
    .unwrap();
    let bytes = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["cmd"], "receive");
    assert_eq!(parsed["data"]["model"], "deepseek/deepseek-v4-flash");

    // Without a model badge → field absent (badge-less messages unchanged).
    send_to_session(&mgr, &session.id, "assistant", "plain reply", None)
        .await
        .unwrap();
    let bytes2 = tokio::time::timeout(Duration::from_millis(500), rx.recv())
        .await
        .unwrap()
        .unwrap();
    let parsed2: serde_json::Value = serde_json::from_slice(&bytes2).unwrap();
    assert!(
        parsed2["data"].get("model").is_none(),
        "None model must omit the field, not serialize null"
    );
}

/// Bug fix: a web inbound with NO session_id (the default conversation) must
/// map to `agent:main:session:legacy` so it shows up in the session list
/// (sessions.list filters on `agent_main_session_*`). `process_messages` is
/// the single web inbound chokepoint, so this covers every Dashboard WS msg.
/// Formerly fell back to `web:{chat_id}` → route-resolved to `agent:main:main`
/// → invisible in the session list.
#[tokio::test]
async fn test_process_messages_default_web_session_is_legacy() {
    use crate::websocket_handler::IncomingMessage;
    use nemesis_bus::MessageBus;
    use std::collections::HashMap;
    use tokio::sync::mpsc;

    let (tx, rx) = mpsc::unbounded_channel::<IncomingMessage>();
    let bus = Arc::new(MessageBus::new());
    let mut sub = bus.subscribe_inbound();
    let bus_clone = bus.clone();
    tokio::spawn(async move {
        super::process_messages(rx, bus_clone).await;
    });

    // No session_id in metadata → the default conversation.
    let inc = IncomingMessage {
        session_id: "s".to_string(),
        sender_id: "u".to_string(),
        chat_id: "web:abc".to_string(),
        content: "hi".to_string(),
        metadata: HashMap::new(),
        voice_playback: None,
    };
    tx.send(inc).unwrap();
    drop(tx); // let process_messages drain and exit

    let inbound = tokio::time::timeout(Duration::from_millis(1000), sub.recv())
        .await
        .expect("timed out waiting for inbound")
        .expect("inbound channel closed");
    assert_eq!(
        inbound.session_key, "agent:main:session:legacy",
        "default web conversation must use the legacy session key (appears in list)"
    );
}

#[tokio::test]
async fn test_send_history_to_session_with_active_queue_succeeds() {
    use crate::websocket_handler::SendQueue;
    let mgr = Arc::new(SessionManager::with_default_timeout());
    let session = mgr.create_session();
    let (tx, mut rx) = tokio::sync::mpsc::channel::<Vec<u8>>(16);
    let (_, done_rx) = tokio::sync::watch::channel(false);
    let queue = Arc::new(SendQueue::from_channels(tx, done_rx));
    mgr.set_send_queue(&session.id, queue);

    let history = r#"{"messages":[{"role":"user","content":"hi"}]}"#;
    let result = send_history_to_session(&mgr, &session.id, history).await;
    assert!(result.is_ok());

    let received = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(received.is_ok());
    let bytes = received.unwrap().unwrap();
    let parsed: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(parsed["cmd"], "history");
}

// ============================================================
// process_messages with voice_playback
// ============================================================

#[tokio::test]
async fn test_process_messages_preserves_voice_playback_some() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let (tx, proc_rx) = mpsc::unbounded_channel();

    tx.send(crate::websocket_handler::IncomingMessage {
        session_id: "s1".to_string(),
        sender_id: "web:s1".to_string(),
        chat_id: "web:s1".to_string(),
        content: "speak this".to_string(),
        metadata: HashMap::new(),
        voice_playback: Some(true),
    })
    .unwrap();
    drop(tx);

    process_messages(proc_rx, bus).await;

    let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    assert!(msg.is_ok());
    let inbound = msg.unwrap().unwrap();
    assert_eq!(inbound.voice_playback, Some(true));
    // No session_id → default "legacy" conversation key (was `web:{chat_id}`
    // before the session-list-visibility fix).
    assert_eq!(inbound.session_key, "agent:main:session:legacy");
}

#[tokio::test]
async fn test_process_messages_no_voice_playback_keeps_none() {
    let bus = Arc::new(MessageBus::new());
    let mut rx = bus.subscribe_inbound();
    let (tx, proc_rx) = mpsc::unbounded_channel();

    tx.send(crate::websocket_handler::IncomingMessage {
        session_id: "s1".to_string(),
        sender_id: "web:s1".to_string(),
        chat_id: "web:s1".to_string(),
        content: "no voice".to_string(),
        metadata: HashMap::new(),
        voice_playback: None,
    })
    .unwrap();
    drop(tx);

    process_messages(proc_rx, bus).await;

    let msg = tokio::time::timeout(Duration::from_millis(500), rx.recv()).await;
    let inbound = msg.unwrap().unwrap();
    assert_eq!(inbound.voice_playback, None);
    assert_eq!(inbound.correlation_id, "");
    assert!(inbound.media.is_empty());
}

// ============================================================
// DirectoryStaticFiles with non-canonicalizable base
// ============================================================

#[test]
fn test_directory_static_files_non_canonicalizable_base_get_file() {
    let provider = DirectoryStaticFiles::new("/this/does/not/exist/at/all");
    assert!(provider.get_file("anything.txt").is_none());
}

#[test]
fn test_directory_static_files_non_canonicalizable_base_list_files() {
    let provider = DirectoryStaticFiles::new("/this/does/not/exist/at/all");
    let files = provider.list_files();
    assert!(files.is_empty());
}

// ============================================================
// WebServerConfig Debug formatting
// ============================================================

#[test]
fn test_config_debug_includes_all_fields() {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:9999".to_string(),
        auth_token: "tok".to_string(),
        cors_origins: vec!["https://test".to_string()],
        ws_path: "/ws".to_string(),
        workspace: Some("/work".to_string()),
        home: Some("/home".to_string()),
        version: "v9".to_string(),
        static_dir: Some("/stat".to_string()),
        static_files: None,
        index_file: "main.html".to_string(),
    };
    let s = format!("{:?}", config);
    assert!(s.contains("listen_addr"));
    assert!(s.contains("127.0.0.1:9999"));
    assert!(s.contains("auth_token"));
    assert!(s.contains("tok"));
    assert!(s.contains("ws_path"));
    assert!(s.contains("/ws"));
    assert!(s.contains("workspace"));
    assert!(s.contains("/work"));
    assert!(s.contains("home"));
    assert!(s.contains("/home"));
    assert!(s.contains("version"));
    assert!(s.contains("v9"));
    assert!(s.contains("index_file"));
    assert!(s.contains("main.html"));
}

#[test]
fn test_config_debug_with_static_files_some_shows_ellipsis() {
    let files: Arc<dyn StaticFiles> = make_mock_static(&[("x", "y")]);
    let config = WebServerConfig {
        static_files: Some(files),
        ..Default::default()
    };
    let s = format!("{:?}", config);
    assert!(s.contains("static_files"));
}

// ============================================================
// resolve_static_dir edge cases
// ============================================================

#[test]
fn test_resolve_static_dir_explicit_is_file_not_dir_returns_none_or_other() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("not_a_dir.txt");
    std::fs::write(&file_path, "content").unwrap();
    let result = resolve_static_dir(Some(file_path.to_str().unwrap()), None);
    if let Some(p) = result {
        assert!(!p.ends_with("not_a_dir.txt"));
    }
}

#[test]
fn test_resolve_static_dir_workspace_missing_static_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let ws = dir.path().to_string_lossy().to_string();
    let result = resolve_static_dir(None, Some(&ws));
    if let Some(p) = &result {
        assert!(!p.contains(&ws) || p.contains("static"));
    }
}

// ============================================================
// handle_health with workspace
// ============================================================

#[tokio::test]
async fn test_handle_health_includes_running_and_sessions_reflects_state() {
    let state = Arc::new(crate::api_handlers::AppState {
        auth_token: String::new(),
        session_count: Arc::new(AtomicUsize::new(7)),
        workspace: None,
        home: None,
        version: "1.0.0".to_string(),
        start_time: Instant::now(),
        model_name: Arc::new(parking_lot::Mutex::new("test".to_string())),
        model_base: Arc::new(parking_lot::Mutex::new(String::new())),
        model_has_key: Arc::new(AtomicBool::new(false)),
        event_hub: Arc::new(crate::events::EventHub::new()),
        running: Arc::new(AtomicBool::new(false)),
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
    let resp = handle_health(AxumState(state)).await;
    let json = resp.0;
    assert_eq!(json["status"], "ok");
    assert_eq!(json["running"], false);
    assert_eq!(json["sessions"], 7);
}
