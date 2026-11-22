//! BUG-20261122-016 回归锁：web_search 生产实现（loop_tools::web）的响应
//! 状态闸行为级测试。
//!
//! 防线本体：search_brave / search_duckduckgo 非 2xx 必须报错而非进解析。
//! 缺陷形态：Brave 4xx/429 错误体经 `#[serde(default)]` typed struct 静默
//! 解析成「零结果成功」（配额耗尽被伪装成「网上没有结果」，agent 无从触发
//! 重试/降级）；DDG 403 验证码页正则零命中静默返回「No results found or
//! extraction failed」。同款修复（BUG-20261115-017）曾落在 nemesis-tools::web
//! 孪生实现——该 web 模块全仓零消费点，生产注册路径（loop_tools::registry）
//! 从未被修，本次移植。
//! 测试经 `search_brave_at` / `search_duckduckgo_at` 端点注入打真实 HTTP
//! 往返（loopback，BUG-20261115-009 测试同款基建）。

use crate::loop_tools::web::{WebSearchConfig, WebSearchTool};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;

/// 读一次 HTTP 请求并写出响应（同 web_search_redirect_tests 基建形态）。
async fn serve_one(listener: &TcpListener, response: &str) -> String {
    let (mut sock, _) = listener.accept().await.expect("accept");
    let mut buf = [0u8; 2048];
    let n = AsyncReadExt::read(&mut sock, &mut buf).await.unwrap_or(0);
    AsyncWriteExt::write_all(&mut sock, response.as_bytes())
        .await
        .expect("write response");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// 构造 Brave 启用的配置（端点注入测试用；其余 provider 关闭与本测试无关）。
fn brave_cfg() -> WebSearchConfig {
    WebSearchConfig {
        brave_enabled: true,
        brave_api_key: Some("test-key".to_string()),
        brave_max_results: 5,
        duckduckgo_enabled: false,
        duckduckgo_max_results: 5,
        perplexity_enabled: false,
        perplexity_api_key: None,
        perplexity_max_results: 5,
    }
}

/// Brave 429 + JSON 错误体必须 Err 并携带状态码与 body 预览
/// （缺陷形态：`#[serde(default)]` 静默解析成「零结果成功」）。
#[tokio::test]
async fn brave_429_error_body_is_err_not_silent_empty() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let body = r#"{"error":[{"id":"RATE_LIMITED"}]}"#;
    let resp = format!(
        "HTTP/1.1 429 Too Many Requests\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = tokio::spawn(async move { serve_one(&l, &resp).await });

    let tool = WebSearchTool::new(brave_cfg());
    let out = tool.search_brave_at(&base, "rust async").await;
    let _req = server.await.unwrap();

    let err = out.expect_err("429 must be an error, not silent no-results");
    assert!(
        err.contains("Brave HTTP 429"),
        "error must carry status: {err}"
    );
    assert!(
        err.contains("RATE_LIMITED"),
        "error must carry body preview: {err}"
    );
}

/// Brave 200 + 空 results 保持「No results」成功形态（状态闸不改变合法语义：
/// 真正的零结果仍是成功字符串，不是 Err）。
#[tokio::test]
async fn brave_200_empty_results_still_success_string() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let body = r#"{"web":{"results":[]}}"#;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = tokio::spawn(async move { serve_one(&l, &resp).await });

    let tool = WebSearchTool::new(brave_cfg());
    let out = tool.search_brave_at(&base, "obscure query").await;
    let _req = server.await.unwrap();

    let ok = out.expect("200 empty results must stay a success string");
    assert!(ok.contains("No results for:"), "got: {ok}");
}

/// DDG 403 验证码页必须 Err 并携带状态码
/// （缺陷形态：正文零命中静默返回「No results found or extraction failed」）。
#[tokio::test]
async fn ddg_403_challenge_page_is_err_not_silent_empty() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let body = "<html><body>anomaly detected, complete the captcha</body></html>";
    let resp = format!(
        "HTTP/1.1 403 Forbidden\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = tokio::spawn(async move { serve_one(&l, &resp).await });

    let tool = WebSearchTool::new(brave_cfg());
    let out = tool.search_duckduckgo_at(&base, "rust async").await;
    let _req = server.await.unwrap();

    let err = out.expect_err("403 challenge page must be an error, not silent no-results");
    assert!(
        err.contains("DuckDuckGo HTTP 403"),
        "error must carry status: {err}"
    );
}

/// DDG 200 + 正常结果页保持解析成功（状态闸不改变合法语义：抓取链路完好）。
#[tokio::test]
async fn ddg_200_with_results_still_parses_links() {
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://127.0.0.1:{}", l.local_addr().unwrap().port());
    let body = r#"<html><a class="result__a" href="https://example.com/doc">Doc Title</a></html>"#;
    let resp = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
        body.len()
    );
    let server = tokio::spawn(async move { serve_one(&l, &resp).await });

    let tool = WebSearchTool::new(brave_cfg());
    let out = tool.search_duckduckgo_at(&base, "rust async").await;
    let _req = server.await.unwrap();

    let ok = out.expect("200 with results must still parse");
    assert!(ok.contains("https://example.com/doc"), "got: {ok}");
    assert!(ok.contains("Doc Title"), "got: {ok}");
    assert!(ok.contains("via DuckDuckGo"), "got: {ok}");
}
