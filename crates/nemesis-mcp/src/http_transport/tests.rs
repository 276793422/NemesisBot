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
    let event = "data: {\"jsonrpc\":\"2.0\",\"id\":5,\"error\":{\"code\":-32600,\"message\":\"bad\"}}";
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
