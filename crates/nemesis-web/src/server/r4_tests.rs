//! R4 覆盖率（2026-08-27）：server.rs 可达臂——
//! - `serve_embedded_static` 的 workflow/chat/ 路径前缀规则（standalone 页壳）
//! - `start()` 端口被占时的 bind-failed 错误路径

use super::*;
use axum::body::Body;
use http::Request;
use tower::ServiceExt;

fn base_config(listen_addr: &str) -> WebServerConfig {
    WebServerConfig {
        listen_addr: listen_addr.to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: String::new(),
        static_dir: None,
        static_files: None,
        index_file: "index.html".to_string(),
    }
}

#[tokio::test]
async fn workflow_chat_path_prefix_serves_standalone_html() {
    // Production passes the embedded provider; the prefix rule under test is
    // provider-independent, so a directory provider with the same layout is
    // equivalent (workflow-chat/index.html shell).
    let dir = tempfile::tempdir().unwrap();
    let wc = dir.path().join("workflow-chat");
    std::fs::create_dir_all(&wc).unwrap();
    std::fs::write(wc.join("index.html"), "<!doctype html><html>shell</html>").unwrap();

    let mut config = base_config("127.0.0.1:0");
    config.static_files = Some(std::sync::Arc::new(DirectoryStaticFiles::new(dir.path())));
    let server = WebServer::new(config);
    let app = server.build_router();

    // 8-hex id lives in the path; any workflow/chat/<id> must serve the
    // same standalone HTML shell (client resolves the id from the URL).
    let req = Request::builder()
        .uri("/workflow/chat/abcd1234")
        .body(Body::empty())
        .unwrap();
    let resp = app.oneshot(req).await.unwrap();
    assert_eq!(resp.status(), axum::http::StatusCode::OK);
    let ct = resp
        .headers()
        .get(http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or_default()
        .to_string();
    assert!(ct.starts_with("text/html"), "unexpected content type: {ct}");
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    let body = String::from_utf8_lossy(&bytes);
    assert!(body.contains("<"), "expected HTML shell, got: {}", &body[..body.len().min(80)]);
}

#[tokio::test]
async fn start_returns_bind_failed_when_port_already_bound() {
    // Occupy a port first, then point the server at it → bind must fail with
    // the dedicated error string (not a panic, not a hang).
    let blocker = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = blocker.local_addr().unwrap();

    let server = WebServer::new(base_config(&addr.to_string()));
    let err = server.start().await.unwrap_err();
    assert!(
        err.starts_with("bind failed:"),
        "expected bind-failed error, got: {err}"
    );
}
