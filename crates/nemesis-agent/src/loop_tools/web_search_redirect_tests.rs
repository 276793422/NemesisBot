//! BUG-20261115-009 回归锁：带凭据外呼 search client 的重定向策略行为级测试。
//!
//! 防线本体（跨源终停/同源跟随的裁决语义）由 `nemesis-utils::http_redirect`
//! 决策纯函数测试钉死；本文件在行为层（真实 HTTP 往返，本地 loopback 双
//! 监听）钉住消费面挂线：`WebSearchTool::cred_search_client` 构造的 client
//! 必须跨源 stop（自定义凭据头不出站）、同源 follow（CDN/网关合规场景
//! 不断链）。若回归摘掉 `.redirect(no_cross_host_redirect_policy())`
//! （reqwest 默认 = 跨源也跟随、自定义头原样带去目标主机），跨源测试当轮红。

use crate::loop_tools::web::WebSearchTool;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 读一次 HTTP 请求（请求头 < 2KB，一段 read 足够）并写出响应。
async fn serve_one(listener: &TcpListener, response: &str) -> String {
    let (mut sock, _) = listener.accept().await.expect("accept");
    let mut buf = [0u8; 2048];
    let n = AsyncReadExt::read(&mut sock, &mut buf).await.unwrap_or(0);
    AsyncWriteExt::write_all(&mut sock, response.as_bytes())
        .await
        .expect("write response");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// 跨源 302 必须终停：3xx 透传为最终响应，第二跳主机不得收到任何请求
/// （自定义凭据头 X-Subscription-Token 不出站）。
#[tokio::test]
async fn cred_search_client_stops_cross_host_redirect() {
    let l_a = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let l_b = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port_b = l_b.local_addr().unwrap().port();
    let url_a = format!(
        "http://127.0.0.1:{}/search",
        l_a.local_addr().unwrap().port()
    );

    // 第二跳哨兵：策略生效时永远等不到连接（500ms 超时 = 诚实通过）；
    // 策略漏挂时 reqwest 立即跟随，哨兵捕获带凭据头的请求 → 断言红。
    let sentry_b = tokio::spawn(async move {
        let resp = "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok";
        serve_one(&l_b, resp).await
    });
    let hop_a = tokio::spawn(async move {
        let target = format!("http://127.0.0.1:{port_b}/evil");
        let resp = format!(
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        serve_one(&l_a, &resp).await
    });

    let client = WebSearchTool::cred_search_client(5).unwrap();
    let resp = client
        .get(&url_a)
        .header("X-Subscription-Token", "secret-token")
        .send()
        .await
        .expect("first hop send");

    assert_eq!(
        resp.status().as_u16(),
        302,
        "跨源 302 必须透传为最终响应（no_cross_host_redirect_policy 终停）"
    );
    let _ = hop_a.await;
    let leaked = tokio::time::timeout(std::time::Duration::from_millis(500), sentry_b).await;
    if let Ok(Ok(second_hop_req)) = leaked {
        panic!(
            "BUG-20261115-009：跨源重定向把请求（含自定义凭据头）带去了第三方主机，第二跳收到: {second_hop_req}"
        );
        // 超时/哨兵被丢 = 第二跳无连接 = 凭据未出站 ✓
    }
}

/// 同源 302 必须跟随：单监听两次 accept，第二跳命中同源新路径
/// （CDN/网关合规场景不断链——策略不是一刀切 Policy::none()）。
#[tokio::test]
async fn cred_search_client_follows_same_origin_redirect() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = l.local_addr().unwrap().port();
    let url = format!("http://127.0.0.1:{port}/search");

    let server = tokio::spawn(async move {
        let target = format!("http://127.0.0.1:{port}/second");
        let first_resp = format!(
            "HTTP/1.1 302 Found\r\nLocation: {target}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
        );
        let _first_req = serve_one(&l, &first_resp).await;
        serve_one(
            &l,
            "HTTP/1.1 200 OK\r\nContent-Length: 2\r\nConnection: close\r\n\r\nok",
        )
        .await
    });

    let client = WebSearchTool::cred_search_client(5).unwrap();
    let resp = client.get(&url).send().await.expect("follow send");

    assert_eq!(
        resp.status().as_u16(),
        200,
        "同源重定向必须跟随（策略只终停跨源）"
    );
    let second_req = server.await.expect("server join");
    assert!(
        second_req.starts_with("GET /second"),
        "第二跳应命中同源新路径: {second_req}"
    );
}
