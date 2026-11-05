//! images lane 单测（wiremock 假端点；集群专业职能框架 M4）。

use super::*;
use wiremock::matchers::{body_string_contains, header, method, path};
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

// ---------------------------------------------------------------------------
// BUG-20261106-008：gpt-image-1 系拒绝 response_format 参数（HTTP 400
// unknown parameter）且该族恒回 b64——端点点名抱怨该参数时剥掉重试一次。
// ---------------------------------------------------------------------------

const GPT_IMAGE_400: &str = r#"{"error":{"message":"Unknown parameter: 'response_format'.","type":"invalid_request_error","param":"response_format","code":"unknown_parameter"}}"#;

#[tokio::test]
async fn gpt_image_1_rejects_response_format_then_retry_succeeds() {
    // 官方 400 形态 → 剥参重试成功；重试请求体不含该参数、其余字段原样。
    let server = MockServer::start().await;
    // 兜底臂（priority 10）：无 response_format 的请求 → 正常 b64 响应。
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "data": [{"b64_json": "QUJD"}],
        })))
        .with_priority(10)
        .mount(&server)
        .await;
    // 首试臂（priority 1）：带 response_format 的请求 → 官方 400 形态。
    Mock::given(method("POST"))
        .and(path("/v1/images/generations"))
        .and(body_string_contains("\"response_format\""))
        .respond_with(ResponseTemplate::new(400).set_body_string(GPT_IMAGE_400))
        .with_priority(1)
        .mount(&server)
        .await;

    let out = generate_image(
        &format!("{}/v1", server.uri()),
        "sk-test",
        &req("一只猫"),
        30,
    )
    .await
    .expect("剥参重试必须成功");
    assert_eq!(out.b64_json, "QUJD");

    let reqs = server.received_requests().await.expect("请求记录");
    assert_eq!(reqs.len(), 2, "恰一次重试");
    let first: serde_json::Value = serde_json::from_slice(&reqs[0].body).unwrap();
    let second: serde_json::Value = serde_json::from_slice(&reqs[1].body).unwrap();
    assert_eq!(
        first["response_format"], "b64_json",
        "首试带参数（现状 wire 形态不变）"
    );
    assert!(
        second.get("response_format").is_none(),
        "重试请求体必须剥掉 response_format：{second}"
    );
    assert_eq!(second["model"], "img-model", "其余字段重试时保留");
    assert_eq!(second["prompt"], "一只猫");
    assert_eq!(second["size"], "1024x1024");
    assert_eq!(second["n"], 1);
}

#[tokio::test]
async fn retry_still_failing_is_honest_error_once() {
    // 剥参重试后仍 400 → 末次响应诚实透传，恰两次请求不循环。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string(GPT_IMAGE_400))
        .mount(&server)
        .await;

    let err = generate_image(&server.uri(), "sk-test", &req("x"), 30)
        .await
        .expect_err("两连 400 → Err");
    assert!(err.contains("400"), "{err}");
    assert!(err.contains("response_format"), "末次 body 尾部透传：{err}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        2,
        "恰一次重试"
    );
}

#[tokio::test]
async fn content_policy_400_does_not_retry() {
    // 非 response_format 抱怨的 400（内容策略）→ 不触发剥参重试，恰一次请求。
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(400).set_body_string(
            r#"{"error":{"message":"Your request was rejected as a result of our safety system.","code":"content_policy_violation"}}"#,
        ))
        .mount(&server)
        .await;

    let err = generate_image(&server.uri(), "sk-test", &req("x"), 30)
        .await
        .expect_err("内容策略 400 → Err");
    assert!(err.contains("safety system"), "{err}");
    assert_eq!(
        server.received_requests().await.unwrap().len(),
        1,
        "非参数抱怨不重试"
    );
}

#[test]
fn response_format_complaint_predicate_scope() {
    // 点名抱怨族（激活）。
    assert!(complains_about_response_format(GPT_IMAGE_400));
    assert!(complains_about_response_format(
        "response_format is not supported for this model"
    ));
    assert!(complains_about_response_format(
        "Unrecognized request argument: response_format"
    ));
    // 非点名/非抱怨（不激活）。
    assert!(!complains_about_response_format(
        r#"{"error":{"code":"content_policy_violation","message":"rejected by safety system"}}"#
    ));
    assert!(!complains_about_response_format(
        r#"{"error":{"message":"Insufficient quota"}}"#
    ));
    assert!(!complains_about_response_format(""));
}
