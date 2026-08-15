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
