//! browser 模块测试（纯函数面——JS 片段构造 / 哨兵判别 / 截断纪律；
//! chromium/CDP 通路是 VM 内形态，宿主测试环境诚实不覆盖，真机面归
//! cluster-uat T-VM 浏览器场景）。

use super::*;

// ---------------------------------------------------------------------------
// JS 片段构造：注入安全（selector/text 经 JSON 字符串字面量嵌入）
// ---------------------------------------------------------------------------

#[test]
fn js_str_escapes_quotes_and_backslashes() {
    // 引号/反斜杠/换行都被 JSON 转义吃掉——无法逃出字符串字面量。
    let s = js_str("he said \"hi\" \\ path\nnext");
    assert!(s.starts_with('"') && s.ends_with('"'));
    assert!(s.contains("\\\""));
    assert!(s.contains("\\\\"));
    assert!(s.contains("\\n"));
    // 产出必须是合法 JSON 字符串（往返一致 = 嵌入安全）。
    let round: String = serde_json::from_str(&s).unwrap();
    assert_eq!(round, "he said \"hi\" \\ path\nnext");
}

#[test]
fn js_str_handles_single_quote_and_unicode() {
    // 单引号在 JS 双引号字面量里无需转义——原样保留合法；unicode 直通。
    let s = js_str("it's 中文 🎉");
    let round: String = serde_json::from_str(&s).unwrap();
    assert_eq!(round, "it's 中文 🎉");
}

#[test]
fn js_click_hostile_selector_cannot_escape() {
    // 逃逸尝试：先闭合双引号再注入代码——双引号被 JSON 转义阻断。
    let hostile = "a\"); alert(1); (\"";
    let s = js_str(hostile);
    assert!(s.contains("\\\""), "双引号必须被转义");
    let round: String = serde_json::from_str(&s).unwrap();
    assert_eq!(round, hostile);
}

#[test]
fn js_click_embeds_selector_and_checks_missing() {
    let js = js_click("#login-btn");
    assert!(js.contains("document.querySelector(\"#login-btn\")"));
    assert!(js.contains("ERROR:no-element"));
    assert!(js.contains("el.click()"));
}

#[test]
fn js_type_uses_native_setter_and_dispatches_events() {
    let js = js_type("#user", "p@ss\"word");
    assert!(js.contains("HTMLTextAreaElement.prototype"));
    assert!(js.contains("HTMLInputElement.prototype"));
    assert!(js.contains("new Event('input'"));
    assert!(js.contains("new Event('change'"));
    // 文本经 JSON 转义嵌入：内嵌双引号不破坏字面量。
    assert!(js.contains("p@ss\\\"word"));
}

#[test]
fn js_extract_with_and_without_selector() {
    let with = js_extract(Some(".main"));
    assert!(with.contains("document.querySelector(\".main\")"));
    assert!(with.contains("ERROR:no-element"));
    let without = js_extract(None);
    assert!(without.contains("document.body.innerText"));
    assert!(!without.contains("ERROR:no-element"));
}

#[test]
fn js_exists_form() {
    assert_eq!(
        js_exists("#done"),
        "document.querySelector(\"#done\") !== null"
    );
}

// ---------------------------------------------------------------------------
// 哨兵判别与截断
// ---------------------------------------------------------------------------

#[test]
fn check_element_result_sentinel_and_passthrough() {
    assert_eq!(
        check_element_result(&serde_json::json!("ERROR:no-element")).unwrap_err(),
        "element not found (selector matched nothing)"
    );
    assert_eq!(
        check_element_result(&serde_json::json!("ok")).unwrap(),
        "ok"
    );
    // 非字符串形态（数字等）转字符串透传。
    assert_eq!(check_element_result(&serde_json::json!(42)).unwrap(), "42");
}

#[test]
fn truncate_head_tail_short_passes_through() {
    assert_eq!(truncate_head_tail("short"), "short");
}

#[test]
fn truncate_head_tail_long_marks_middle() {
    let long = "x".repeat(10_000);
    let out = truncate_head_tail(&long);
    assert!(out.contains("truncated"));
    // head+tail 保留量：4000+4000 字符。
    let kept = out.chars().filter(|c| *c == 'x').count();
    assert_eq!(kept, 8000);
}

// ---------------------------------------------------------------------------
// 动作参数校验（handle 的错误面——不触 chromium 的早退路径）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn handle_requires_action() {
    let err = handle(&serde_json::json!({}), Path::new("/tmp/ws"))
        .await
        .unwrap_err();
    assert!(err.contains("Missing 'action'"));
}

#[tokio::test]
async fn handle_rejects_unknown_action_before_browser_probe() {
    // 未知 action 在任何 chromium/CDP 接触前诚实拒绝。
    let err = handle(
        &serde_json::json!({"action": "hover"}),
        Path::new("/tmp/ws"),
    )
    .await
    .unwrap_err();
    assert!(err.contains("unknown browser action: hover"));
    assert!(err.contains("navigate, click, type"));
}

#[tokio::test]
async fn navigate_requires_url() {
    // 参数校验先行（先于 page_target 的 CDP 接触）：缺 url = 明确错误。
    let val = serde_json::json!({"action": "navigate"});
    let err = super::navigate(&val, Path::new("/tmp/ws"))
        .await
        .unwrap_err();
    assert!(err.contains("missing 'url'"), "got: {err}");
}

#[tokio::test]
async fn click_requires_selector() {
    let val = serde_json::json!({"action": "click"});
    let err = super::click(&val).await.unwrap_err();
    assert!(err.contains("missing 'selector'"), "got: {err}");
}

#[tokio::test]
async fn type_requires_selector_and_text() {
    let err = super::type_text(&serde_json::json!({"action": "type"}))
        .await
        .unwrap_err();
    assert!(err.contains("missing 'selector'"), "got: {err}");
    let err = super::type_text(&serde_json::json!({"action": "type", "selector": "#a"}))
        .await
        .unwrap_err();
    assert!(err.contains("missing 'text'"), "got: {err}");
}

#[tokio::test]
async fn fill_form_requires_fields_array() {
    let err = super::fill_form(&serde_json::json!({"action": "fill_form"}))
        .await
        .unwrap_err();
    assert!(err.contains("missing 'fields'"), "got: {err}");
    let err = super::fill_form(&serde_json::json!(
        {"action": "fill_form", "fields": [{"selector": "#a"}]}
    ))
    .await
    .unwrap_err();
    assert!(err.contains("each field needs"), "got: {err}");
}

#[tokio::test]
async fn wait_for_requires_selector() {
    let err = super::wait_for(&serde_json::json!({"action": "wait_for"}))
        .await
        .unwrap_err();
    assert!(err.contains("missing 'selector'"), "got: {err}");
}

#[test]
fn cdp_request_is_http_1_1() {
    // 回归锁：chromium 155 DevTools server 明拒 HTTP/1.0（「Cannot handle
    // request with protocol: HTTP/1.0」）——曾把 cdp_alive 打成永久 false，
    // chromium 3s 就绪却白烧满 120s 就绪预算报 not ready（VM-BROWSER-1
    // 三连红的根因）。请求行必须 HTTP/1.1。
    let req = super::cdp_request("GET", "/json/version");
    assert!(
        req.starts_with("GET /json/version HTTP/1.1\r\n"),
        "请求行必须是 HTTP/1.1: {req:?}"
    );
    assert!(req.contains("Host: 127.0.0.1:9222\r\n"), "got: {req:?}");
    assert!(req.ends_with("Connection: close\r\n\r\n"), "got: {req:?}");
}

#[test]
fn extract_response_headers_happy_path() {
    // 框架读形态（VM-BROWSER-1 续）：状态码 + Content-Length 提取。
    let head = "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 42\r\n\r\n";
    let (status, cl) = super::extract_status_and_content_length(head).unwrap();
    assert_eq!(status, 200);
    assert_eq!(cl, Some(42));
}

#[test]
fn extract_response_headers_case_insensitive_and_missing_cl() {
    // 头名大小写不敏感；CL 缺席 = None（调用方诚实报错，不做无限等待——
    // keep-alive 形态下「读到 EOF」会永久挂死，forensic8 实证）。
    let head = "HTTP/1.1 404 Not Found\r\ncontent-length: 7\r\n\r\n";
    let (status, cl) = super::extract_status_and_content_length(head).unwrap();
    assert_eq!(status, 404);
    assert_eq!(cl, Some(7));

    let head = "HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\n";
    let (status, cl) = super::extract_status_and_content_length(head).unwrap();
    assert_eq!(status, 200);
    assert_eq!(cl, None);
}

#[test]
fn extract_response_headers_malformed_status_line() {
    // 无状态行 / 非 HTTP 响应 = 诚实报错（旧实现会把这种吞成空 body）。
    assert!(super::extract_status_and_content_length("garbage\r\n\r\n").is_err());
    assert!(super::extract_status_and_content_length("").is_err());
}
