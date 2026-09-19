//! 桥子路径支持测试（goal：反向桥与多设备汇聚，一期批次二）。
//!
//! 覆盖 goal 测试面锚点：「子路径：HTML 资源引用/WS/路由在 `/d/<node_id>/`
//! 前缀下正确落到目标设备（base 注入生效）；本地直连（无前缀）零回归」。
//!
//! 测试形态：`WebServer::build_router` + `tower::ServiceExt::oneshot` 直发
//! 请求（不 bind 端口），断言路由命中 / 前缀剥离 / base 注入三类语义。

use super::subpath::{BridgeSubpathService, inject_base_tag, strip_self_prefix};
use crate::server::{StaticFiles, WebServer, WebServerConfig};
use axum::body::Body;
use axum::http::{Request, StatusCode};
use std::collections::HashMap;
use std::sync::Arc;
use tower::ServiceExt;

/// 内存静态文件（子路径测试用；index.html 带 `<head>`，chat 壳带
/// `<html>` 无 `<head>` 形态覆盖注入点兜底路径）。
struct InMemStatic {
    files: HashMap<String, Vec<u8>>,
}

impl StaticFiles for InMemStatic {
    fn get_file(&self, path: &str) -> Option<Vec<u8>> {
        self.files.get(path).cloned()
    }
    fn list_files(&self) -> Vec<String> {
        self.files.keys().cloned().collect()
    }
}

fn test_static() -> Arc<InMemStatic> {
    let mut files = HashMap::new();
    files.insert(
        "index.html".to_string(),
        b"<html><head><title>d</title></head><body><script src=\"./assets/app.js\"></script></body></html>"
            .to_vec(),
    );
    files.insert(
        "chat/index.html".to_string(),
        b"<html><body>chat-shell</body></html>".to_vec(),
    );
    Arc::new(InMemStatic { files })
}

fn build_router_with_identity(node_id: Option<&str>) -> axum::Router {
    let config = WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        auth_token: String::new(),
        cors_origins: vec![],
        ws_path: "/ws".to_string(),
        workspace: None,
        home: None,
        version: "test".to_string(),
        static_dir: None,
        static_files: Some(test_static()),
        index_file: "index.html".to_string(),
    };
    let mut server = WebServer::new(config);
    if let Some(id) = node_id {
        server.set_bridge_identity(id.to_string());
    }
    server.build_router()
}

async fn send(router: axum::Router, uri: &str) -> (StatusCode, String) {
    let req = Request::builder()
        .uri(uri)
        .body(Body::empty())
        .expect("request");
    let res = router.oneshot(req).await.expect("service");
    let status = res.status();
    let body = axum::body::to_bytes(res.into_body(), 1024 * 1024)
        .await
        .unwrap_or_default();
    (status, String::from_utf8_lossy(&body).to_string())
}

// ---------------------------------------------------------------------------
// 直连（无前缀）零回归
// ---------------------------------------------------------------------------

#[tokio::test]
async fn direct_request_hits_router_unchanged() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/health").await;
    assert_eq!(status, StatusCode::OK, "直连 /health 应正常命中路由");
    assert!(!body.contains("<base"), "非 HTML 响应不注入 base");
}

#[tokio::test]
async fn direct_html_gets_root_base() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains(r#"<base href="/">"#),
        "直连 HTML 应注入根 base，实际: {body}"
    );
    assert!(
        body.contains(r#"<head><base href="/">"#),
        "注入点应在 <head> 标签闭合后，实际: {body}"
    );
}

/// 直连 `/chat/`（带尾斜杠的子路径入口）：相对构建产物 `./assets/...`
/// 按 base 解析——不注入 base 会解析成 `/chat/assets`（404），注入
/// `<base href="/">` 后解析回 `/assets`。这是「直连也统一注入」的
/// 正确性依据（goal 批次二）。
#[tokio::test]
async fn direct_subpath_entry_html_gets_root_base() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/chat/").await;
    assert_eq!(status, StatusCode::OK, "chat 壳应命中");
    assert!(
        body.contains(r#"<base href="/">"#),
        "子路径入口直连也应注入根 base，实际: {body}"
    );
}

#[tokio::test]
async fn no_identity_still_injects_root_base_for_direct() {
    // --relay 形态（无身份）：直连 HTML 仍注入根 base（相对构建正确性），
    // 但前缀剥离永不命中。
    let router = build_router_with_identity(None);
    let (status, body) = send(router, "/").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"<base href="/">"#));
}

// ---------------------------------------------------------------------------
// 剥前缀（/d/<自身 node_id>/...）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn self_prefix_stripped_router_hits_target() {
    let router = build_router_with_identity(Some("node-a"));
    // 剥前缀后 /health → 注册路由 200（若未剥，会走 fallback 拿到
    // index.html 的 HTML——状态同为 200，所以还要断言响应非 HTML）。
    let (status, body) = send(router, "/d/node-a/health").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("<html"),
        "剥前缀后应命中 /health 路由而非 fallback"
    );
}

#[tokio::test]
async fn self_prefix_html_gets_prefixed_base() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/node-a/").await;
    assert_eq!(status, StatusCode::OK, "剥前缀后 / → index.html");
    assert!(
        body.contains(r#"<base href="/d/node-a/">"#),
        "剥前缀请求的 HTML 应注入带前缀 base，实际: {body}"
    );
}

/// `/d/<id>`（无尾斜杠）剥成 `/`——授权后 redirect 目标形态对齐。
#[tokio::test]
async fn self_prefix_without_trailing_slash_maps_to_root() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/node-a").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains(r#"<base href="/d/node-a/">"#), "实际: {body}");
}

#[tokio::test]
async fn self_prefix_preserves_query() {
    // query 保留：剥前缀后带 query 仍命中注册路由（URI 重写合法性）。
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/node-a/health?probe=1").await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        !body.contains("<html"),
        "带 query 的剥前缀请求应命中 /health"
    );
}

/// 多段子路径 + 静态资源形态：`/d/<id>/assets/app.js` 剥成
/// `/assets/app.js` → fallback 精确匹配（表里无此文件 → 404；带 `.` 不落
/// SPA fallback）。断言 404 即证明路径精确透传。
#[tokio::test]
async fn self_prefix_deep_path_precise() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/node-a/assets/app.js").await;
    assert_eq!(
        status,
        StatusCode::NOT_FOUND,
        "表内无此资源应 404 而非 fallback"
    );
    assert!(!body.contains("<base"), "404 非 HTML 不注入");
}

// ---------------------------------------------------------------------------
// 他人前缀（内置中继的转发请求）不剥不注入
// ---------------------------------------------------------------------------

#[tokio::test]
async fn foreign_prefix_passthrough_no_injection() {
    // 无 relay server：/d/other/... 无路由 → fallback → index.html 200。
    // 断言核心：响应 **不含 base 注入**（转发请求的 HTML 由远程设备自己
    // 注入，本机不得画蛇添足）。
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/other-node/anything").await;
    assert_eq!(status, StatusCode::OK, "fallback 行为不变");
    assert!(
        !body.contains("<base"),
        "他人前缀（转发请求）的响应不得注入 base，实际: {body}"
    );
}

/// 首段为空的畸形前缀 `/d//x`：不剥（守卫 first 非空），不注入。
#[tokio::test]
async fn empty_first_segment_not_stripped() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d//health").await;
    assert_eq!(status, StatusCode::OK, "fallback 行为不变");
    assert!(!body.contains("<base"), "/d/ 开头未剥前缀不注入");
}

// ---------------------------------------------------------------------------
// JSON / 非 HTML 响应不注入
// ---------------------------------------------------------------------------

#[tokio::test]
async fn json_response_not_injected_under_self_prefix() {
    let router = build_router_with_identity(Some("node-a"));
    let (status, body) = send(router, "/d/node-a/api/status").await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body.contains("<base"), "JSON 响应不注入，实际: {body}");
}

// ---------------------------------------------------------------------------
// 纯函数：rewrite_path_and_query / inject_base_tag
// ---------------------------------------------------------------------------

#[test]
fn strip_self_prefix_keeps_query() {
    let mut uri: axum::http::Uri = "/d/a/b?x=1&y=2".parse().expect("uri");
    let inject = strip_self_prefix(&Some("a".to_string()), &mut uri);
    assert_eq!(inject.as_deref(), Some("/d/a/"), "应判定为自身前缀");
    assert_eq!(uri.path(), "/b");
    assert_eq!(uri.query(), Some("x=1&y=2"), "query 必须原样保留");
}

#[test]
fn strip_self_prefix_without_query() {
    let mut uri: axum::http::Uri = "/d/a/b".parse().expect("uri");
    let inject = strip_self_prefix(&Some("a".to_string()), &mut uri);
    assert!(inject.is_some());
    assert_eq!(uri.path(), "/b");
    assert_eq!(uri.query(), None);
}

#[test]
fn strip_self_prefix_foreign_passthrough() {
    let mut uri: axum::http::Uri = "/d/other/x".parse().expect("uri");
    let inject = strip_self_prefix(&Some("a".to_string()), &mut uri);
    assert!(inject.is_none(), "他人前缀不剥不注入");
    assert_eq!(uri.path(), "/d/other/x", "URI 不被改动");
}

#[test]
fn strip_self_prefix_direct_returns_root() {
    let mut uri: axum::http::Uri = "/health".parse().expect("uri");
    let inject = strip_self_prefix(&Some("a".to_string()), &mut uri);
    assert_eq!(inject.as_deref(), Some("/"), "直连注入根 base");
    assert_eq!(uri.path(), "/health");
}

#[test]
fn strip_self_prefix_no_identity_direct_only() {
    // 无身份（--relay）：/d/ 形态一律透传；直连仍注入根 base。
    let mut uri: axum::http::Uri = "/d/x/y".parse().expect("uri");
    assert!(strip_self_prefix(&None, &mut uri).is_none());
    let mut uri2: axum::http::Uri = "/".parse().expect("uri");
    assert_eq!(strip_self_prefix(&None, &mut uri2).as_deref(), Some("/"));
}

#[test]
fn inject_base_tag_after_head() {
    let html = b"<html><head><title>t</title></head><body></body></html>";
    let out = inject_base_tag(html, "/d/x/").expect("inject");
    assert_eq!(
        String::from_utf8(out).expect("utf8"),
        r#"<html><head><base href="/d/x/"><title>t</title></head><body></body></html>"#,
        "注入点应在 <head> 闭合后"
    );
}

#[test]
fn inject_base_tag_falls_back_to_html_marker() {
    let html = b"<html lang=\"en\"><body>x</body></html>";
    let out = inject_base_tag(html, "/").expect("inject");
    let s = String::from_utf8(out).expect("utf8");
    assert!(
        s.starts_with(r#"<html lang="en"><base href="/">"#),
        "无 <head> 时退到 <html> 后，实际: {s}"
    );
}

#[test]
fn inject_base_tag_falls_back_to_front() {
    let html = b"plain text";
    let out = inject_base_tag(html, "/").expect("inject");
    assert!(String::from_utf8_lossy(&out).starts_with(r#"<base href="/">"#));
}

#[test]
fn inject_base_tag_skips_existing_base() {
    let html = b"<html><head><base href=\"z\"></head></html>";
    assert!(inject_base_tag(html, "/").is_none(), "已有 base 不重复注入");
}

// ---------------------------------------------------------------------------
// WS 升级路径：`/d/<id>/ws` 剥成 `/ws` 后升级链路完整
// ---------------------------------------------------------------------------

/// 剥前缀后升级请求命中 handle_websocket_upgrade。oneshot 上下文无真实
/// hyper 连接，axum WebSocketUpgrade 的诚实拒绝是 **426 Upgrade Required**
/// （命中升级路由）；若剥前缀失败会落 fallback 拿到 200 HTML。
#[tokio::test]
async fn self_prefix_ws_path_reaches_upgrade_route() {
    let router = build_router_with_identity(Some("node-a"));
    let req = Request::builder()
        .uri("/d/node-a/ws")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("sec-websocket-version", "13")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(Body::empty())
        .expect("req");
    let res = router.oneshot(req).await.expect("service");
    let status = res.status();
    assert!(
        status == StatusCode::SWITCHING_PROTOCOLS || status == StatusCode::UPGRADE_REQUIRED,
        "剥前缀后 /ws 应命中升级路由（101/426），实际: {status}"
    );
}

/// 外壳 service 冒烟（pub 导出可被 server.rs 引用；Clone 语义成立）。
#[test]
fn service_shell_is_constructible_and_cloneable() {
    let shell = BridgeSubpathService::new(Some("node-a".to_string()), axum::Router::new());
    let _cloned = shell.clone();
}
