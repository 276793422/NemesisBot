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

#[test]
fn test_extract_sse_data_single_line() {
    let event = "event: message\ndata: {\"jsonrpc\":\"2.0\",\"id\":1,\"result\":{\"ok\":true}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(1.into()));
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_no_event_type() {
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":2,\"result\":{}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(2.into()));
}

#[test]
fn test_extract_sse_data_no_space_after_colon() {
    let event = "data:{\"jsonrpc\":\"2.0\",\"id\":3,\"result\":null}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(3.into()));
}

#[test]
fn test_extract_sse_data_multi_line() {
    let event = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":4,\"result\":{}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    // Multi-line data is joined with \n → valid JSON
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_error_response() {
    let event =
        "data: {\"jsonrpc\":\"2.0\",\"id\":5,\"error\":{\"code\":-32600,\"message\":\"bad\"}}";
    let result = extract_sse_data(event).unwrap();
    assert!(result.error.is_some());
    assert_eq!(result.error.unwrap().code, -32600);
}

#[test]
fn test_extract_sse_data_no_data_field() {
    let event = "event: message\nid: 123";
    let result = extract_sse_data(event);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("no data field"));
}

#[test]
fn test_extract_sse_data_ignores_comments() {
    let event = ": this is a comment\ndata: {\"jsonrpc\":\"2.0\",\"id\":6,\"result\":true}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(6.into()));
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
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(7.into()));
}

#[test]
fn test_extract_sse_data_only_empty_data_is_error() {
    // Every data line is empty/whitespace → treated as no data field.
    let event = "data:\ndata:   ";
    let result = extract_sse_data(event);
    assert!(result.is_err());
    assert!(result.unwrap_err().message.contains("no data field"));
}

#[test]
fn test_extract_sse_data_ignores_retry_and_id_lines() {
    // SSE control fields retry: and id: must be silently ignored.
    let event = "retry: 5000\nid: abc-123\ndata: {\"jsonrpc\":\"2.0\",\"id\":8,\"result\":{}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(8.into()));
    assert!(result.result.is_some());
}

#[test]
fn test_extract_sse_data_multi_field_concatenation() {
    // Multiple data lines are joined with '\n' to form one JSON document.
    let event = "data: {\"jsonrpc\":\"2.0\",\ndata: \"id\":9,\ndata: \"result\":{\"v\":1}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.jsonrpc, "2.0");
    assert_eq!(result.id, serde_json::Value::Number(9.into()));
}

#[test]
fn test_extract_sse_data_trims_whitespace_around_data() {
    // Leading/trailing whitespace on each data line is trimmed.
    let event = "data:   {\"jsonrpc\":\"2.0\",\"id\":10,\"result\":{}}   ";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(10.into()));
}

#[test]
fn test_extract_sse_data_string_id() {
    // JSON-RPC id can be a string (MCP allows string|number|null).
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":\"req-xyz\",\"result\":{\"ok\":true}}";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::String("req-xyz".to_string()));
}

#[test]
fn test_extract_sse_data_empty_event_text_is_error() {
    assert!(extract_sse_data("").is_err());
    assert!(extract_sse_data("   ").is_err());
}

#[test]
fn test_extract_sse_data_blank_lines_between_fields() {
    // Real SSE can have blank separators; lines() drops empty lines, and the
    // data: field is still extracted.
    let event = "\nevent: message\n\ndata: {\"jsonrpc\":\"2.0\",\"id\":11,\"result\":{}}\n";
    let result = extract_sse_data(event).unwrap();
    assert_eq!(result.id, serde_json::Value::Number(11.into()));
}

#[test]
fn test_extract_sse_data_comment_only_is_error() {
    // A comment line (starting with ':') and nothing else → no data field.
    let event = ": keep-alive comment";
    let result = extract_sse_data(event);
    assert!(result.is_err());
}

#[test]
fn test_extract_sse_data_case_sensitive_prefix() {
    // The "data:" prefix is case-sensitive — "Data:" must not be treated as a
    // data field, so this event has no usable data.
    let event = "Data: {\"jsonrpc\":\"2.0\",\"id\":12,\"result\":{}}";
    let result = extract_sse_data(event);
    assert!(result.is_err());
}

#[test]
fn test_extract_sse_data_error_with_data_field() {
    // A data field carrying a JSON-RPC error should parse successfully and
    // expose the error object.
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":13,\"error\":{\"code\":-32700,\"message\":\"parse error\"}}";
    let result = extract_sse_data(event).unwrap();
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
    let body = "event: message\r\ndata: {\"jsonrpc\":\"2.0\",\"id\":9,\"result\":{\"ok\":true}}\r\n\r\n";
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
    let resp = t.send(&w4c_request("notifications/initialized", 5), 5000)
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
        .respond_with(ResponseTemplate::new(200).set_body_json(
            serde_json::json!({"jsonrpc":"2.0","id":2,"result":{"echo":true}}),
        ))
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
    assert!(err.message.contains("ended without data"), "got: {}", err.message);
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
