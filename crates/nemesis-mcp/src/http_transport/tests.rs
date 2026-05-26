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
