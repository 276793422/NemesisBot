//! Tests: spin up a local mock "real endpoint" (axum), start the proxy
//! pointing at it, and verify pass-through + auth substitution.

use axum::body::Body;
use axum::http::{Request, StatusCode};
use axum::routing::{get, post};
use axum::Router;

#[tokio::test]
async fn proxy_forwards_and_swaps_auth() {
    // Mock upstream: echoes back the Authorization header it received.
    let upstream = Router::new().route(
        "/v1/chat/completions",
        post(|headers: axum::http::HeaderMap| async move {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<none>")
                .to_string();
            axum::Json(serde_json::json!({ "upstream_auth": auth }))
        }),
    );
    let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_port = upstream_listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(upstream_listener, upstream).await;
    });

    // Start the proxy pointing at the mock with a "real" key.
    let handle = crate::start(
        format!("http://127.0.0.1:{upstream_port}"),
        "sk-REAL-KEY".to_string(),
    )
    .await
    .unwrap();

    // Client call with a FAKE key through the proxy.
    let client = reqwest::Client::new();
    let resp = client
        .post(format!("http://127.0.0.1:{}/v1/chat/completions", handle.port))
        .header("authorization", "Bearer eval-fake-key")
        .json(&serde_json::json!({ "model": "test" }))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let body: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(
        body["upstream_auth"], "Bearer sk-REAL-KEY",
        "proxy must swap the fake key for a Bearer real key"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn proxy_passes_query_and_path_verbatim() {
    let upstream = Router::new().route(
        "/v1/anything",
        get(|| async { "PATH_OK" }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, upstream).await;
    });

    let handle = crate::start(
        format!("http://127.0.0.1:{port}"),
        "k".to_string(),
    )
    .await
    .unwrap();

    let resp = reqwest::get(format!("http://127.0.0.1:{}/v1/anything?x=1&y=2", handle.port))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    assert_eq!(resp.text().await.unwrap(), "PATH_OK");

    handle.shutdown().await;
}

#[tokio::test]
async fn proxy_streaming_body_passthrough() {
    // Upstream returns a chunked body; the proxy must stream it through.
    let upstream = Router::new().route(
        "/v1/stream",
        post(|| async {
            Body::from_stream(futures::stream::iter(
                (0..5).map(|i| Ok::<_, std::io::Error>(format!("chunk{i};"))),
            ))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, upstream).await;
    });

    let handle = crate::start(
        format!("http://127.0.0.1:{port}"),
        "k".to_string(),
    )
    .await
    .unwrap();

    let resp = reqwest::Client::new()
        .post(format!("http://127.0.0.1:{}/v1/stream", handle.port))
        .header("authorization", "Bearer fake")
        .body("")
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let text = resp.text().await.unwrap();
    assert_eq!(text, "chunk0;chunk1;chunk2;chunk3;chunk4;");

    handle.shutdown().await;
}

// ==================== 补覆盖：api_base / 上游不可达 502 分支 / x-api-key 裸 key 替换 ====================
// 对应 llvm-cov 未达行：64-66（api_base 格式）、136-141（forward 失败 → 502）、
// 182（x-api-key 分支携带【裸】真 key）。全部走本地回环，无外网依赖。

#[tokio::test]
async fn api_base_points_at_local_proxy_v1() {
    // start() 是纯被动代理，构造时不连上游；上游地址给个不可达值也无妨
    let handle = crate::start("http://127.0.0.1:1".to_string(), "k".to_string())
        .await
        .unwrap();
    assert!(handle.port > 0, "proxy should listen on a real port");
    assert_eq!(
        handle.api_base(),
        format!("http://127.0.0.1:{}/v1", handle.port),
        "api_base must point back at the local proxy /v1"
    );
    handle.shutdown().await;
}

#[tokio::test]
async fn unreachable_upstream_returns_bad_gateway() {
    // 挑一个保证无监听的回环端口：bind :0 后立刻 drop → 转发必被拒绝（确定性失败）
    let dead = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = dead.local_addr().unwrap().port();
    drop(dead);

    let handle = crate::start(format!("http://127.0.0.1:{dead_port}"), "k".to_string())
        .await
        .unwrap();

    let resp = reqwest::get(format!("http://127.0.0.1:{}/v1/whatever", handle.port))
        .await
        .unwrap();
    assert_eq!(resp.status(), StatusCode::BAD_GATEWAY);
    let body = resp.text().await.unwrap();
    assert!(
        body.starts_with("eval-proxy:"),
        "error body should carry the proxy prefix, got: {body}"
    );

    handle.shutdown().await;
}

#[tokio::test]
async fn x_api_key_swapped_to_bare_real_key() {
    // 上游回显它实际收到的两个 auth 头，验证替换语义：
    // authorization → "Bearer <real>"；x-api-key → <real>（裸 key，非 Bearer）
    let upstream = Router::new().route(
        "/v1/echo",
        post(|headers: axum::http::HeaderMap| async move {
            let auth = headers
                .get("authorization")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<none>")
                .to_string();
            let xkey = headers
                .get("x-api-key")
                .and_then(|v| v.to_str().ok())
                .unwrap_or("<none>")
                .to_string();
            axum::Json(serde_json::json!({ "auth": auth, "xkey": xkey }))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        let _ = axum::serve(listener, upstream).await;
    });

    let handle = crate::start(format!("http://127.0.0.1:{port}"), "sk-REAL".to_string())
        .await
        .unwrap();
    let url = format!("http://127.0.0.1:{}/v1/echo", handle.port);
    let client = reqwest::Client::new();

    // ① 只带 x-api-key：换成裸真 key（非 Bearer），且不额外注入 authorization
    let body: serde_json::Value = client
        .post(&url)
        .header("x-api-key", "fake-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["xkey"], "sk-REAL", "x-api-key must carry the bare real key");
    assert_eq!(body["auth"], "<none>", "no authorization should be injected");

    // ② 两个头都带：authorization → Bearer 真 key；x-api-key → 裸真 key
    let body: serde_json::Value = client
        .post(&url)
        .header("authorization", "Bearer fake")
        .header("x-api-key", "fake-key")
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["auth"], "Bearer sk-REAL");
    assert_eq!(body["xkey"], "sk-REAL");

    // ③ 一个 auth 头都不带：注入 authorization: Bearer 真 key
    let body: serde_json::Value = client
        .post(&url)
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(body["auth"], "Bearer sk-REAL");
    assert_eq!(body["xkey"], "<none>");

    handle.shutdown().await;
}
