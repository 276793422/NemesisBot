//! R4 覆盖率（2026-08-27）：server.rs 可达臂——
//! - `serve_embedded_static` 的 workflow/chat/ 路径前缀规则（standalone 页壳）
//! - bind 端口走查（2026-08-30 重写：49000~50000 带内循环往复 + 49001 让位
//!   WebSocket 通道 + 带外线性小步，`bind_with_port_walk` / `port_walk_sequence`。
//!   旧「占一端口期望 bind 失败」断言在走查引入后前提失效——walk 落到邻端口
//!   成功 serve 导致测试永久挂死，已按新语义重写）。

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

// ============================================================
// port_walk_sequence（纯函数：走查序列生成）
// ============================================================

#[test]
fn port_walk_sequence_in_range_starts_at_base_and_skips_49001() {
    let seq = port_walk_sequence(49000);
    // 1001 个带内端口去掉 49001 → 1000 个尝试
    assert_eq!(seq.len(), 1000);
    assert_eq!(seq[0], 49000);
    assert_eq!(seq[1], 49002, "49001 must be skipped (WebSocket channel port)");
    assert_eq!(*seq.last().unwrap(), 50000);
    assert!(!seq.contains(&49001));
}

#[test]
fn port_walk_sequence_wraps_from_top_back_to_bottom() {
    let seq = port_walk_sequence(49999);
    assert_eq!(seq[0], 49999);
    assert_eq!(seq[1], 50000);
    assert_eq!(seq[2], 49000, "must wrap back to range start (循环往复)");
    assert_eq!(seq[3], 49002, "wrap also skips 49001");
    assert_eq!(*seq.last().unwrap(), 49998, "full circle ends just before base");
    assert_eq!(seq.len(), 1000);
    assert!(!seq.contains(&49001));
}

#[test]
fn port_walk_sequence_zero_is_single_os_random_bind() {
    // 端口 0 = OS 随机分配：单次 bind，不进走查。
    assert_eq!(port_walk_sequence(0), vec![0]);
}

#[test]
fn port_walk_sequence_out_of_range_walks_linear_no_wrap() {
    // 带外低段：线性 +1，20 次封顶，不回绕。
    let seq = port_walk_sequence(21000);
    let expected: Vec<u16> = (21000..21020).collect();
    assert_eq!(seq, expected);

    // 带外高段：同样线性，不落入 49000 带。
    let seq = port_walk_sequence(60000);
    let expected: Vec<u16> = (60000..60020).collect();
    assert_eq!(seq, expected);

    // 线性段若扫过 49001 仍须让位（48990+11 = 49001 被剔除 → 19 个）。
    let seq = port_walk_sequence(48990);
    assert_eq!(seq.len(), 19, "linear walk passing 49001 must skip it");
    assert!(!seq.contains(&49001));
}

// ============================================================
// bind_with_port_walk / start_with_shutdown（真实 bind 行为）
// ============================================================

#[tokio::test]
async fn start_walks_to_next_free_port_when_first_in_range_occupied() {
    // 带内 4999x 段（远离生产端口 49000/49001）：占前两个 → 应落到第三个。
    let b1 = tokio::net::TcpListener::bind("127.0.0.1:49990").await.unwrap();
    let b2 = tokio::net::TcpListener::bind("127.0.0.1:49991").await.unwrap();
    let config = base_config("127.0.0.1:49990");
    let server = WebServer::new(config);
    let (tx, rx) = tokio::sync::broadcast::channel::<()>(1);
    let _ = tx.send(()); // 预发关闭信号：bind 成功后 select 立即返回，serve 不阻塞
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel();

    let result = tokio::time::timeout(
        Duration::from_secs(5),
        server.start_with_shutdown(rx, Some(bound_tx)),
    )
    .await;
    drop(b1);
    drop(b2);

    result.unwrap().unwrap();
    let addr = tokio::time::timeout(Duration::from_secs(2), bound_rx)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(addr.port(), 49992, "walk should land on the next free port");
}

#[tokio::test]
async fn start_loud_fails_when_all_linear_fallback_ports_busy() {
    // 带外 2100x 段（低于 Windows 49152+ / Linux 32768+ 动态端口区间，
    // 避开系统临时分配）：20 个全部占住 → 线性走查耗尽 → loud 失败。
    let mut blockers = Vec::new();
    for port in 21000..21020u16 {
        blockers.push(tokio::net::TcpListener::bind(("127.0.0.1", port)).await.unwrap());
    }
    let config = base_config("127.0.0.1:21000");
    let server = WebServer::new(config);
    let (tx, rx) = tokio::sync::broadcast::channel::<()>(1);
    let _ = tx.send(());

    let result = server.start_with_shutdown(rx, None).await;
    drop(blockers);

    let err = result.unwrap_err();
    assert!(
        err.starts_with("bind failed:"),
        "expected loud bind failure, got: {err}"
    );
    assert!(err.contains("21000"), "error should mention base port: {err}");
}
