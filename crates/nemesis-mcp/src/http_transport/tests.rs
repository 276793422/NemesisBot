use super::*;

#[tokio::test]
async fn test_http_transport_name() {
    let transport = HttpTransport::new("http://localhost:8080/mcp");
    assert_eq!(transport.name(), "http");
}

#[tokio::test]
async fn test_http_transport_lifecycle() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    assert!(!transport.is_connected());

    transport.connect().await.unwrap();
    assert!(transport.is_connected());

    transport.close().await.unwrap();
    assert!(!transport.is_connected());
}

#[tokio::test]
async fn test_http_transport_send_not_connected() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    let req = TransportRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "initialize".to_string(),
        params: None,
    };

    let result = transport.send(&req, 1000).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("not connected"));
}

// ---------------------------------------------------------------------------
// SSE parsing unit tests
// ---------------------------------------------------------------------------

/// 旧 `extract_sse_data` 的测试替身（第十四轮：生产解析拆成
/// `sse_data_payload` + `response_payload`，后者过滤无 `id` 的 JSON-RPC
/// 通知）。本替身保持旧测试的提取语义钉死不变：无 data 报错、响应照常
/// 提取、通知/垃圾载荷同样报错（旧测试面没有通知形态输入）。
fn sse_response_for_test(event_text: &str) -> Result<TransportResponse, TransportError> {
    let json_str = sse_data_payload(event_text)
        .ok_or_else(|| TransportError::send_failed("SSE event contains no data field"))?;
    match response_payload(&json_str)? {
        SsePayload::Response(r) => Ok(r),
        SsePayload::Skipped => Err(TransportError::send_failed(
            "SSE event contains no data field",
        )),
    }
}

#[test]
fn test_extract_sse_data_single_line() {
    let event = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(1.into()));
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_no_event_type() {
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(2.into()));
}

#[test]
fn test_extract_sse_data_no_space_after_colon() {
    let event = "data:{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":null}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(3.into()));
}

#[test]
fn test_extract_sse_data_multi_line() {
    let event = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":4,\"result\":{}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    // Multi-line data is joined with \n → valid JSON
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_error_response() {
    let event =
        "data: {\"jsonrpc\":\"2.0\",\"id\":5,\"error\":{\"code\":-32600,\"message\":\"bad\"}}";
    let result = sse_response_for_test(event).unwrap();
    assert!(result.error.is_some());
    assert_eq!(result.error.unwrap().code, -32600);
}

#[test]
fn test_extract_sse_data_no_data_field() {
    let event = "event: message\nid: 123";
    let result = sse_response_for_test(event);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("no data field"));
}

#[test]
fn test_extract_sse_data_ignores_comments() {
    let event = ": this is a comment\ndata: {\"jsonrpc\":\"2.0\",\"id\":6,\"result\":true}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(6.into()));
}

// BUG-20261025-56：id + method 并存 = server→client 请求（JSON-RPC 响应
// 永不携带 method）。旧形态：serde 忽略未知 method 字段 → 被解析成
// result=None 的「响应」返回——id 撞车即被吞成本请求的空成功。
#[test]
fn bug56_sse_server_to_client_request_is_skipped() {
    let req = r#"{"jsonrpc":"2.0","id":9,"method":"sampling/createMessage","params":{}}"#;
    assert!(
        matches!(response_payload(req).unwrap(), SsePayload::Skipped),
        "server-to-client request must be skipped, not returned as response"
    );
    // 对照：无 id 通知照旧 Skipped；带 result 的响应照旧 Response。
    let notif = r#"{"jsonrpc":"2.0","method":"notifications/message","params":{}}"#;
    assert!(matches!(
        response_payload(notif).unwrap(),
        SsePayload::Skipped
    ));
    let resp = r#"{"jsonrpc":"2.0","id":1,"result":{"ok":true}}"#;
    assert!(matches!(
        response_payload(resp).unwrap(),
        SsePayload::Response(_)
    ));
}

// ---------------------------------------------------------------------------
// Session ID handling
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_session_id_cleared_on_close() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    transport.connect().await.unwrap();
    transport.session_id = Some("test-session-123".to_string());

    transport.close().await.unwrap();
    assert!(transport.session_id.is_none());
}

// ---------------------------------------------------------------------------
// Additional SSE parsing edge cases
// ---------------------------------------------------------------------------

#[test]
fn test_extract_sse_data_skips_empty_data_line() {
    // A "data:" line with only whitespace must be ignored, leaving no usable
    // data field → error rather than an empty-string parse.
    let event = "data:   \ndata: {\"jsonrpc\":\"2.0\",\"id\":7,\"result\":true}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(7.into()));
}

#[test]
fn test_extract_sse_data_only_empty_data_is_error() {
    // Every data line is empty/whitespace → treated as no data field.
    let event = "data:\ndata:   ";
    let result = sse_response_for_test(event);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("no data field"));
}

#[test]
fn test_extract_sse_data_ignores_retry_and_id_lines() {
    // SSE control fields retry: and id: must be silently ignored.
    let event = "retry: 5000\nid: abc-123\ndata: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(8.into()));
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_multi_field_concatenation() {
    // Multiple data lines are joined with '\n' to form one JSON document.
    let event = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":9,\ndata: \"result\":{\"v\":1}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(9.into()));
}

#[test]
fn test_extract_sse_data_trims_whitespace_around_data() {
    // Leading/trailing whitespace on each data line is trimmed.
    let event = "data:   {\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{}}   ";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(10.into()));
}

#[test]
fn test_extract_sse_data_string_id() {
    // JSON-RPC id can be a string (MCP allows string|number|null).
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":\"req-xyz\",\"result\":{\"ok\":true}}";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::String("req-xyz".to_string()));
}

#[test]
fn test_extract_sse_data_empty_event_text_is_error() {
    assert!(sse_response_for_test("").is_err());
    assert!(sse_response_for_test("   ").is_err());
}

#[test]
fn test_extract_sse_data_blank_lines_between_fields() {
    // Real SSE can have blank separators; lines() drops empty lines, and the
    // data: field is still extracted.
    let event = "\nevent: message\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{}}\n";
    let result = sse_response_for_test(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(11.into()));
}

#[test]
fn test_extract_sse_data_comment_only_is_error() {
    // A comment line (starting with ':') and nothing else → no data field.
    let event = ": keep-alive comment";
    let result = sse_response_for_test(event);
    assert!(result.is_err());
}

#[test]
fn test_extract_sse_data_case_sensitive_prefix() {
    // The "data:" prefix is case-sensitive — "Data:" must not be treated as a
    // data field, so this event has no usable data.
    let event = "Data: {\"jsonrpc\":\"2.0\",\"id\":12,\"result\":{}}";
    let result = sse_response_for_test(event);
    assert!(result.is_err());
}

#[test]
fn test_extract_sse_data_error_with_data_field() {
    // A data field carrying a JSON-RPC error should parse successfully and
    // expose the error object.
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":13,\"error\":{\"code\":-32700,\"message\":\"parse error\"}}";
    let result = sse_response_for_test(event).unwrap();
    let err = result.error.expect("error should be present");
    assert_eq!(err.code, -32700);
    assert_eq!(err.message, "parse error");
}

// ---------------------------------------------------------------------------
// Transport lifecycle / not-connected paths
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_close_before_connect_is_ok() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    // close() without connect() must succeed and leave disconnected.
    transport.close().await.unwrap();
    assert!(!transport.is_connected());
}

#[tokio::test]
async fn test_send_after_close_fails() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    transport.connect().await.unwrap();
    transport.close().await.unwrap();

    let req = TransportRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "ping".to_string(),
        params: None,
    };
    let result = transport.send(&req, 1000).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("not connected"));
}

#[tokio::test]
async fn test_connect_sets_connected_flag() {
    let mut transport = HttpTransport::new("http://localhost:8080/mcp");
    assert!(!transport.is_connected());
    transport.connect().await.unwrap();
    assert!(transport.is_connected());
}

// ===========================================================================
// W4c 补测（2026-08-25）：send() 真实 HTTP 往返（wiremock）——JSON 分支 /
// 空体 / 解析失败 / SSE 流 / 会话 ID 回传 / 202 / 5xx / 连接失败 / 缺 content-type
// ===========================================================================

use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn w4c_request(method: &str, id: i64) -> TransportRequest {
    TransportRequest {
        jsonrpc: "2.0".to_string(),
        id: Some(serde_json::Value::Number(id.into())),
        method: method.to_string(),
        params: None,
    }
}

#[tokio::test]
async fn test_w4c_http_send_json_response_round_trip() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "jsonrpc": "2.0", "id": 7, "result": {"tools": [{"name": "t1"}]}
        })))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("tools/list", 7), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(7.into()));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["tools"][0]["name"], "t1");
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_empty_body_returns_synthetic_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_string(""))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("anything", 3), 5000).await.unwrap();
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_invalid_json_maps_send_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_string("<<<not-json"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 1), 5000).await.unwrap_err();
    assert!(err.message.contains("Failed to parse JSON response"));
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_sse_stream_parsed() {
    let server = MockServer::start().await;
    let body =
        "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"ok\":true}}\r\n\r\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("initialize", 9), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(9.into()));
    assert_eq!(resp.result.unwrap()["ok"], true);
    t.close().await.unwrap();
}

#[tokio::test]
async fn bug56_http_sse_server_request_before_response_is_skipped() {
    // BUG-20261025-56：SSE 流里 server→client 请求（id+method 并存，id=9
    // 与我方请求撞车）先于真响应。旧形态：被解析成 result=None 的「响应」
    // 返回 → 断言 unwrap panic；修复后：跳过，第二个事件正常返回。
    let server = MockServer::start().await;
    let body = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"method\":\"sampling/createMessage\",\"params\":{}}\r\n\r\n\
                event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"ok\":true}}\r\n\r\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("initialize", 9), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(9.into()));
    assert_eq!(resp.result.unwrap()["ok"], true);
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_sse_stream_without_data_is_error() {
    let server = MockServer::start().await;
    let body = "event: message\r\n: only a comment\r\n\r\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 4), 5000).await.unwrap_err();
    // 流结束（EOF）→ buffer 里只剩注释行 → no data field
    assert!(err.message.contains("no data field") || err.message.contains("ended without data"));
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_202_returns_synthetic_result() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(202))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t
        .send(&w4c_request("notifications/initialized", 5), 5000)
        .await
        .unwrap();
    assert!(resp.result.is_some());
    assert!(resp.error.is_none());
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_error_status_maps_send_failed() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(500).set_body_string("server exploded"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 1), 5000).await.unwrap_err();
    assert!(err.message.contains("HTTP 500"));
    assert!(err.message.contains("server exploded"));
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_send_connection_failure_maps_send_failed() {
    // 指向必然拒绝连接的端口
    let mut t = HttpTransport::new("http://127.0.0.1:1/mcp");
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 1), 2000).await.unwrap_err();
    assert!(err.message.contains("HTTP request failed"));
}

#[tokio::test]
async fn test_w4c_http_session_id_echoed_on_subsequent_requests() {
    let server = MockServer::start().await;
    // wiremock 按挂载顺序取第一个匹配的 mock：带 header 的请求必须先挂
    // 第二个请求：必须带上 Mcp-Session-Id: sid-abc
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("Mcp-Session-Id", "sid-abc"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"echo":true}})),
        )
        .mount(&server)
        .await;
    // 第一个请求（无 session header 落到这里）：响应头里发 Mcp-Session-Id
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("mcp-session-id", "sid-abc")
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})),
        )
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let r1 = t.send(&w4c_request("initialize", 1), 5000).await.unwrap();
    assert!(r1.result.is_some());
    assert_eq!(t.session_id.as_deref(), Some("sid-abc"));

    let r2 = t.send(&w4c_request("tools/list", 2), 5000).await.unwrap();
    assert_eq!(r2.result.unwrap()["echo"], true);
    t.close().await.unwrap();
    // close 清掉 session id
    assert!(t.session_id.is_none());
}

#[tokio::test]
async fn test_w4c_http_send_without_content_type_still_parses_json() {
    // 无 content-type 头 → 按空串处理 → 走 JSON 解析分支
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_string(r#"{"jsonrpc":"2.0","id":11,"result":{"plain":1}}"#),
        )
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("x", 11), 5000).await.unwrap();
    assert_eq!(resp.result.unwrap()["plain"], 1);
    t.close().await.unwrap();
}

// ===========================================================================
// S1 补测（2026-08-26）：SSE LF 分隔（"\n\n"）/ 空流 EOF / 响应体截断读取失败
// ===========================================================================

#[tokio::test]
async fn test_s1_http_send_sse_lf_only_delimiter() {
    // Existing SSE tests use CRLF bodies, so the `buffer.find("\n\n")` fast
    // path never fires. LF-only framing must hit it.
    let server = MockServer::start().await;
    let body = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":31,\"result\":{\"lf\":true}}\n\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let resp = t.send(&w4c_request("initialize", 31), 5000).await.unwrap();
    assert_eq!(resp.result.unwrap()["lf"], true);
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_s1_http_send_sse_empty_stream_maps_ended_without_data() {
    // Truly empty SSE body → EOF with an empty buffer → the dedicated
    // "stream ended without data" error (not the no-data-field parse error).
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw("", "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 32), 5000).await.unwrap_err();
    assert!(
        err.message.contains("ended without data"),
        "got: {}",
        err.message
    );
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_s1_http_send_truncated_body_maps_read_failure() {
    // Raw socket server that advertises Content-Length: 100 but sends 5 bytes
    // and closes → reqwest text() fails mid-body → "Failed to read response".
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    let server_thread = std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 8192];
            let _ = sock.read(&mut buf); // drain the request (best effort)
            let resp = b"HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort";
            let _ = sock.write_all(resp);
            let _ = sock.flush();
            let _ = sock.shutdown(std::net::Shutdown::Both);
        }
    });

    let mut t = HttpTransport::new(format!("http://{}/mcp", addr));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 33), 5000).await.unwrap_err();
    assert!(
        err.message.contains("Failed to read response"),
        "got: {}",
        err.message
    );
    server_thread.join().unwrap();
}

// ===========================================================================
// 收官批 B 段（2026-10-15）：McpServerConfig.headers 消费化——解析规则 /
// vault:/env: 引用现查 / 保留头过滤 / Debug 掩码 / wiremock 头实发。
// 全局 vault 解析器槽位是进程单例：与 stdio 测试共用 VAULT_SLOT_LOCK。
// ===========================================================================

use crate::stdio_transport::tests::VAULT_SLOT_LOCK;

#[test]
fn test_b_header_pairs_parse_rules() {
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    // 字面量原样 + 值两侧 trim + 头名 trim。
    let out = resolved_header_pairs(&["Authorization: Bearer abc".into(), "X-A:  v1  ".into()]);
    assert_eq!(
        out,
        vec![
            ("Authorization".into(), "Bearer abc".into()),
            ("X-A".into(), "v1".into())
        ]
    );

    // 非法行跳过：缺冒号 / 头名空 / 头名含空白。
    let out = resolved_header_pairs(&[
        "no-colon-line".into(),
        ": value-only".into(),
        "Bad Name: v".into(),
        "Good: v".into(),
    ]);
    assert_eq!(out, vec![("Good".into(), "v".into())]);
}

#[test]
fn test_b_header_pairs_same_name_later_wins() {
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    // 同名头（大小写不敏感）后行胜——reqwest 追加语义下鉴权头两行必坏，
    // 必须在列表里归一为单条。
    let out = resolved_header_pairs(&[
        "authorization: old".into(),
        "Authorization: new".into(),
        "X-Other: keep".into(),
    ]);
    assert_eq!(
        out,
        vec![
            ("authorization".into(), "new".into()),
            ("X-Other".into(), "keep".into())
        ],
        "鉴权头必须只剩一条且取后值"
    );
}

#[test]
fn test_b_header_pairs_reserved_session_id_skipped() {
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    // Mcp-Session-Id 是协议保留头（server 指派）——配置行 WARN 跳过。
    let out = resolved_header_pairs(&[
        "Mcp-Session-Id: forged".into(),
        "mcp-session-id: forged2".into(),
        "X-Keep: v".into(),
    ]);
    assert_eq!(out, vec![("X-Keep".into(), "v".into())]);
}

#[test]
fn test_b_header_pairs_failed_reference_skips_row_only() {
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    // 引用解析失败只跳过该行，不毒化其余行（语义同 stdio env）。
    let out = resolved_header_pairs(&[
        "X-Bad: vault:definitely-missing-alias-b1".into(),
        "X-Good: plain".into(),
    ]);
    assert_eq!(out, vec![("X-Good".into(), "plain".into())]);
}

#[test]
fn test_b_header_pairs_vault_reference_resolved() {
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    // vault: 引用：装测试解析器后现查（镜像 stdio resolved_env 先例）。
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "mcp-hdr-token" {
            Ok("vaulted-hdr-value".into())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    let out = resolved_header_pairs(&["X-Auth: vault:mcp-hdr-token".into()]);
    nemesis_config::clear_global_vault_resolver();
    assert_eq!(out, vec![("X-Auth".into(), "vaulted-hdr-value".into())]);
}

#[tokio::test]
// 锁必须横跨 await（serialize 的正是 connect/send 期间的进程 env 态）；
// 锁内只 await 自方 wiremock/transport，无其他测试会反向等这把锁——无死锁面。
#[allow(clippy::await_holding_lock)]
async fn test_b_headers_env_reference_e2e_wiremock() {
    // env: 引用全链路：解析 → connect 现查 → send 实发 → wiremock 断言头到达。
    // 进程 env 是全局态：唯一变量名 + VAULT_SLOT_LOCK 串行（set_var 在
    // edition 2024 是 unsafe——先例 s9_tests.rs 同款）。
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    const VAR: &str = "NB_MCP_HDR_E2E_KEY";
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("X-Api-Key", "e2e-hdr-secret"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":41,"result":{"ok":true}})),
        )
        .mount(&server)
        .await;

    // SAFETY: 锁内唯一变量名设/还原；并行测试不读该变量。
    unsafe {
        std::env::set_var(VAR, "e2e-hdr-secret");
    }
    let mut t = HttpTransport::with_headers(
        format!("{}/mcp", server.uri()),
        vec![format!("X-Api-Key: env:{VAR}")],
    );
    // connect 现查发生在 env 变量仍在场时——时序即语义。
    t.connect().await.unwrap();
    let result = t.send(&w4c_request("ping", 41), 5000).await;
    unsafe {
        std::env::remove_var(VAR);
    }

    let resp = result.expect("env: 引用的头应实发并拿到 200");
    assert_eq!(resp.result.unwrap()["ok"], true);
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_b_headers_literal_e2e_and_session_id_not_forged() {
    // 字面量头实发 + 保留头不伪造会话身份：配置里写 Mcp-Session-Id 必须被
    // 过滤，第二跳只带 server 指派的真 session id（received_requests 全量
    // 校验，防 reqwest 追加语义产出重复头）。
    let server = MockServer::start().await;
    // 挂载顺序即优先级（wiremock 取第一个匹配）：带会话头的第二跳 mock 先挂，
    // 通用 mock（指派 session id）后挂——否则第二跳也落进通用 mock。
    // 第一跳：回 set-cookie 式指派 session id。
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("X-Tenant", "acme"))
        .and(header("Mcp-Session-Id", "sid-real"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"echo":true}})),
        )
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .and(header("X-Tenant", "acme"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("mcp-session-id", "sid-real")
                .set_body_json(serde_json::json!({"jsonrpc":"2.0","id":1,"result":{}})),
        )
        .mount(&server)
        .await;

    let mut t = HttpTransport::with_headers(
        format!("{}/mcp", server.uri()),
        vec![
            "X-Tenant: acme".into(),
            "Mcp-Session-Id: forged-by-config".into(),
        ],
    );
    t.connect().await.unwrap();
    let r1 = t.send(&w4c_request("initialize", 1), 5000).await.unwrap();
    assert!(r1.result.is_some());
    let r2 = t.send(&w4c_request("tools/list", 2), 5000).await.unwrap();
    assert_eq!(r2.result.unwrap()["echo"], true);
    t.close().await.unwrap();

    // 全量校验：每个请求的 Mcp-Session-Id 头值都只能是 server 指派的真值。
    let received = server.received_requests().await.expect("received requests");
    assert_eq!(received.len(), 2, "两跳都应到达");
    for (i, req) in received.iter().enumerate() {
        let values: Vec<String> = req
            .headers
            .get_all("mcp-session-id")
            .iter()
            .filter_map(|v| std::str::from_utf8(v.as_bytes()).ok())
            .map(|s| s.to_ascii_lowercase())
            .collect();
        if i == 0 {
            assert!(values.is_empty(), "第一跳不该带会话头: {values:?}");
        } else {
            assert_eq!(values, vec!["sid-real"], "第二跳只带真会话 id");
        }
    }
}

#[tokio::test]
async fn test_b_debug_masks_resolved_header_values() {
    // 掩码锁：解析后的头值（凭据真值）永不进 Debug 输出——日志/错误路径
    // 即使 {:?} 整个 transport 也不泄露字节。
    let mut t = HttpTransport::with_headers(
        "http://127.0.0.1:9/mcp",
        vec!["Authorization: Bearer super-secret-value".into()],
    );
    t.connect().await.unwrap();
    let dbg = format!("{:?}", t);
    assert!(
        !dbg.contains("super-secret-value"),
        "Debug 不得含头值: {dbg}"
    );
    assert!(dbg.contains("<redacted:"), "Debug 应显式掩码: {dbg}");
    assert!(dbg.contains("Authorization"), "头名可以出现（值掩码）");
    t.close().await.unwrap();
    // close 后 resolved_headers 清空——Debug 亦无残留。
    let dbg2 = format!("{:?}", t);
    assert!(!dbg2.contains("<redacted:"), "close 应清空解析头");
}

#[tokio::test]
// 锁必须横跨 await（serialize 的正是 connect 期间的进程级全局解析器槽位）；
// 锁内只 await 自方 transport，无其他测试会反向等这把锁——无死锁面。
#[allow(clippy::await_holding_lock)]
async fn test_b_close_clears_resolved_headers_and_reconnect_refreshes() {
    // close 清空解析值；重连现查——引用轮换后重连拿到新值（现查语义）。
    let _guard = VAULT_SLOT_LOCK.lock().unwrap();
    let mut t = HttpTransport::with_headers(
        "http://127.0.0.1:9/mcp",
        vec!["X-Token: vault:b-refresh-alias".into()],
    );
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "b-refresh-alias" {
            Ok("v1".into())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    t.connect().await.unwrap();
    assert_eq!(t.resolved_headers, vec![("X-Token".into(), "v1".into())]);

    // 轮换 vault 值 → close → 重连 → 新值。
    nemesis_config::set_global_vault_resolver(std::sync::Arc::new(|alias| {
        if alias == "b-refresh-alias" {
            Ok("v2-rotated".into())
        } else {
            Err(format!("别名不存在: {alias}"))
        }
    }));
    t.close().await.unwrap();
    assert!(t.resolved_headers.is_empty(), "close 应清空解析头");
    t.connect().await.unwrap();
    assert_eq!(
        t.resolved_headers,
        vec![("X-Token".into(), "v2-rotated".into())],
        "重连必须现查到轮换后的新值"
    );
    nemesis_config::clear_global_vault_resolver();
    t.close().await.unwrap();
}

// ===========================================================================
// 第十二轮复查（2026-10-15）：SSE 事件边界 CRLF + 无 data 事件跳过
// ===========================================================================

// -- take_complete_event 单元（边界逻辑直接钉死）--

#[test]
fn test_take_complete_event_lf_boundary() {
    let (event, rest) = take_complete_event("data: 1\n\nrest").unwrap();
    assert_eq!(event, "data: 1");
    assert_eq!(rest, "rest");
}

#[test]
fn test_take_complete_event_crlf_boundary() {
    let (event, rest) = take_complete_event("data: 1\r\n\r\nrest").unwrap();
    assert_eq!(event, "data: 1");
    assert_eq!(rest, "rest");
}

#[test]
fn test_take_complete_event_none_when_incomplete() {
    assert!(take_complete_event("data: 1\n").is_none());
    assert!(take_complete_event("data: 1\r\n\r").is_none());
    assert!(take_complete_event("").is_none());
}

#[test]
fn test_take_complete_event_earliest_boundary_wins() {
    // CRLF 边界在前 → CRLF 胜（否则 LF 匹配会切在 CRLF 边界之后）
    let (event, rest) = take_complete_event("a\r\n\r\nb\n\nc").unwrap();
    assert_eq!(event, "a");
    assert_eq!(rest, "b\n\nc");
    // LF 边界在前 → LF 胜
    let (event, rest) = take_complete_event("a\n\nb\r\n\r\nc").unwrap();
    assert_eq!(event, "a");
    assert_eq!(rest, "b\r\n\r\nc");
}

// -- sse_data_payload 单元 --

#[test]
fn test_sse_data_payload_comment_only_is_none() {
    assert!(sse_data_payload(": keepalive").is_none());
    assert!(sse_data_payload("event: ping\r\n: x").is_none());
}

#[test]
fn test_sse_data_payload_data_lines_joined() {
    assert_eq!(sse_data_payload("data: a").unwrap(), "a");
    assert_eq!(sse_data_payload("data: a\ndata: b").unwrap(), "a\nb");
}

// -- wiremock 端到端（差异咬合：旧实现首个事件是注释时硬报 "no data field"）--

#[tokio::test]
async fn test_w4c_http_sse_keepalive_comment_then_data_lf() {
    let server = MockServer::start().await;
    let body = ": keepalive\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":5,\"result\":{\"ok\":1}}\n\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    // 旧实现：find("\n\n") 命中注释事件 → extract_sse_data 硬报错整体失败；
    // 新实现：跳过无 data 事件继续读 → 取到 data 事件。
    let resp = t.send(&w4c_request("x", 5), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(5.into()));
    assert_eq!(resp.result.unwrap()["ok"], 1);
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_http_sse_crlf_comment_then_data() {
    let server = MockServer::start().await;
    let body = ": ping\r\n\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":6,\"result\":{\"ok\":2}}\r\n\r\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    // 回归锁：CRLF 行尾的事件边界照常切分 + 注释事件照常跳过。
    let resp = t.send(&w4c_request("x", 6), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(6.into()));
    assert_eq!(resp.result.unwrap()["ok"], 2);
    t.close().await.unwrap();
}

// ------------------------- R14（2026-10-16）：JSON-RPC 通知过滤 ---------

#[test]
fn r14_response_payload_classification() {
    // 无 id 的通知 → Skipped（MCP 标准行为：响应前先推 progress/log）
    let notification = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}";
    assert!(matches!(
        response_payload(notification).unwrap(),
        SsePayload::Skipped
    ));
    let progress = "{\"jsonrpc\":\"2.0\",\"method\":\"notifications/progress\",\"params\":{}}";
    assert!(matches!(
        response_payload(progress).unwrap(),
        SsePayload::Skipped
    ));
    // 非 JSON 垃圾 → Skipped
    assert!(matches!(
        response_payload("not json").unwrap(),
        SsePayload::Skipped
    ));
    // 带 id 的响应 → Response（错误响应也算）
    let ok = "{\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{}}";
    assert!(matches!(
        response_payload(ok).unwrap(),
        SsePayload::Response(_)
    ));
    // id 在但形状非法（缺 jsonrpc）→ 硬报错（协议违例保留旧失败面）
    let malformed = "{\"id\":9,\"result\":{}}";
    let err = response_payload(malformed).unwrap_err();
    assert!(
        err.message.contains("Failed to parse SSE data"),
        "实际: {}",
        err.message
    );
}

#[tokio::test]
async fn r14_test_w4c_http_sse_notification_then_data() {
    let server = MockServer::start().await;
    // 服务器响应前先推一条 notifications/message（MCP 标准 progress/log 行为）。
    let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{\"level\":\"info\",\"data\":\"starting\"}}\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{\"ok\":11}}\n\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    // 旧实现：首个 data 载荷直接 from_str::<TransportResponse>，通知无 id
    // → "missing field `id`" 整个请求硬失败；新实现：通知跳过 → 取到响应。
    let resp = t.send(&w4c_request("x", 11), 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(11.into()));
    assert_eq!(resp.result.unwrap()["ok"], 11);
    t.close().await.unwrap();
}

#[tokio::test]
async fn r14_test_w4c_http_sse_notification_only_ended_without_data() {
    let server = MockServer::start().await;
    // 流里只有通知（响应永不到来，如服务器崩在处理中）：EOF 后诚实报
    // "ended without data"，不再是通知反序列化的 "missing field id" 伪失败。
    let body = "data: {\"jsonrpc\":\"2.0\",\"method\":\"notifications/message\",\"params\":{}}\n\n";
    Mock::given(method("POST"))
        .and(path("/mcp"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;

    let mut t = HttpTransport::new(format!("{}/mcp", server.uri()));
    t.connect().await.unwrap();
    let err = t.send(&w4c_request("x", 12), 5000).await.unwrap_err();
    assert!(
        err.message.contains("SSE stream ended without data"),
        "实际: {}",
        err.message
    );
    t.close().await.unwrap();
}

// ---------------------------------------------------------------------------
// sanitize_url_for_log（2026-10-21 自查 #1：query 凭据不得进错误串）
// ---------------------------------------------------------------------------

#[test]
fn sanitize_url_strips_query_key() {
    // Smithery 风格 `?key=` 端点——reqwest Display 原样带全 URL 的时代，
    // 连接失败会把 key 泄进错误串（日志 + mcp_discover 回灌模型双通路）。
    assert_eq!(
        super::sanitize_url_for_log("https://gateway.smithery.ai/mcp?key=secret-key-123"),
        "https://gateway.smithery.ai/mcp?<redacted>"
    );
}

#[test]
fn sanitize_url_plain_url_untouched() {
    assert_eq!(
        super::sanitize_url_for_log("http://127.0.0.1:8080/mcp"),
        "http://127.0.0.1:8080/mcp"
    );
    assert_eq!(
        super::sanitize_url_for_log("https://example.com/a/b/c"),
        "https://example.com/a/b/c"
    );
}

#[test]
fn sanitize_url_masks_userinfo() {
    assert_eq!(
        super::sanitize_url_for_log("https://user:hunter2@example.com/mcp"),
        "https://***@example.com/mcp"
    );
    assert_eq!(
        super::sanitize_url_for_log("https://user:hunter2@example.com/mcp?key=k"),
        "https://***@example.com/mcp?<redacted>"
    );
}

#[test]
fn sanitize_url_path_at_sign_is_not_userinfo() {
    // '@' 出现在路径段（authority 之后含 '/'）不是 userinfo，不掩码。
    assert_eq!(
        super::sanitize_url_for_log("https://example.com/u@a/resource"),
        "https://example.com/u@a/resource"
    );
}
