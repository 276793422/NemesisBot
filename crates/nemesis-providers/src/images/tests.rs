//! images lane 单测（wiremock 假端点；集群专业职能框架 M4）。

use super::*;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn req(prompt: &str) -> ImageRequest {
    ImageRequest {
        model: "img-model".into(),
        prompt: prompt.into(),
        size: Some("1024x1024".into()),
    }
}

#[tokio::test]
async fn happy_path_returns_b64() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(header("Authorization", "Bearer sk-test"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"b64_json": "QUJD"}],
        })))
        .mount(&server)
        .await;

    let out = generate_image(
        &format!("{}/v1", server.uri()),
        "sk-test",
        &req("一只猫"),
        30,
    )
    .await
    .expect("成功臂");
    assert_eq!(
        out,
        ImageResult {
            b64_json: "QUJD".into()
        }
    );
}

#[tokio::test]
async fn http_error_surfaces_body_tail() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(
            ResponseTemplate::new(402).set_body_string(r#"{"error":"insufficient quota"}"#),
        )
        .mount(&server)
        .await;

    let err = generate_image(&server.uri(), "sk-test", &req("x"), 30)
        .await
        .expect_err("4xx → Err");
    assert!(err.contains("402"), "{err}");
    assert!(err.contains("insufficient quota"), "{err}");
}

#[tokio::test]
async fn url_only_response_is_honest_error_no_download() {
    // 端点只回 url → 不做二次下载（SSRF 供给面刻意为零），诚实报错。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"url": "http://127.0.0.1:1/x.png"}],
        })))
        .mount(&server)
        .await;

    let err = generate_image(&server.uri(), "sk-test", &req("x"), 30)
        .await
        .expect_err("url-only → Err");
    assert!(err.contains("b64_json"), "{err}");
}

#[tokio::test]
async fn malformed_responses_are_honest_errors() {
    // data 缺失。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not-json"))
        .mount(&server)
        .await;
    assert!(
        generate_image(&server.uri(), "k", &req("x"), 30)
            .await
            .unwrap_err()
            .contains("非 JSON")
    );

    // data 空数组。
    let server2 = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"data": []})))
        .mount(&server2)
        .await;
    assert!(
        generate_image(&server2.uri(), "k", &req("x"), 30)
            .await
            .unwrap_err()
            .contains("空数组")
    );

    // 非法入参：空 base / 空 key / 空 prompt（不打网络）。
    let r = req("x");
    assert!(generate_image("", "k", &r, 30).await.is_err());
    assert!(generate_image(&server2.uri(), " ", &r, 30).await.is_err());
    assert!(
        generate_image(&server2.uri(), "k", &req("  "), 30)
            .await
            .is_err()
    );
}
