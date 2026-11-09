//! 跨主机重定向终止策略真机行为验证：纯函数裁决由模块 tests 钉死，本
//! 套件验证 reqwest `Policy::custom` 闭包接线与 `stop()` 透传语义——
//! 跨源 302 响应原样透传（不跟随、请求自带头不出首跳主机）、同源 302
//! 正常跟随、自指环到跳数上限诚实终止不挂起。

#![cfg(feature = "http")]

use wiremock::matchers::path;
use wiremock::{Mock, MockServer, ResponseTemplate};

fn client() -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(nemesis_utils::http_redirect::no_cross_host_redirect_policy())
        .build()
        .expect("client build")
}

#[tokio::test]
async fn cross_origin_redirect_is_stopped_and_passed_through() {
    let target = MockServer::start().await;
    // 目标主机若被跟随会拿到 200/leaked——用它断言「从未发生第二跳」。
    Mock::given(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("leaked"))
        .mount(&target)
        .await;

    let origin = MockServer::start().await;
    Mock::given(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/final", target.uri())),
        )
        .mount(&origin)
        .await;

    let resp = client()
        .get(format!("{}/redir", origin.uri()))
        .header("x-api-key", "secret-key")
        .send()
        .await
        .unwrap();
    // stop() 透传：302 即最终响应（若跟随会拿到 200）。
    assert_eq!(resp.status().as_u16(), 302);
    assert_eq!(
        resp.headers().get("location").unwrap(),
        format!("{}/final", target.uri()).as_str()
    );
}

#[tokio::test]
async fn same_origin_redirect_is_followed() {
    let origin = MockServer::start().await;
    Mock::given(path("/final"))
        .respond_with(ResponseTemplate::new(200).set_body_string("done"))
        .mount(&origin)
        .await;
    Mock::given(path("/redir"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/final", origin.uri())),
        )
        .mount(&origin)
        .await;

    let resp = client()
        .get(format!("{}/redir", origin.uri()))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status().as_u16(), 200);
    assert_eq!(resp.text().await.unwrap(), "done");
}

#[tokio::test]
async fn self_loop_terminates_at_hop_cap() {
    let origin = MockServer::start().await;
    Mock::given(path("/loop"))
        .respond_with(
            ResponseTemplate::new(302).insert_header("Location", format!("{}/loop", origin.uri())),
        )
        .mount(&origin)
        .await;

    let resp = client()
        .get(format!("{}/loop", origin.uri()))
        .send()
        .await
        .unwrap();
    // 同源但跳数耗尽 → 终停透传（与 reqwest 默认上限同语义，不挂起不无限跟随）。
    assert_eq!(resp.status().as_u16(), 302);
}
