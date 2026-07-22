use super::*;

fn make_approval_data() -> ApprovalWindowData {
    let mut context = HashMap::new();
    context.insert("source".to_string(), "test".to_string());
    ApprovalWindowData {
        request_id: "req-123".to_string(),
        operation: "file_write".to_string(),
        operation_name: "Write File".to_string(),
        target: "/tmp/test.txt".to_string(),
        risk_level: "high".to_string(),
        reason: "writing to system directory".to_string(),
        timeout_seconds: 60,
        context,
        timestamp: chrono::Local::now().timestamp(),
    }
}

#[test]
fn test_approval_data_validate() {
    let data = make_approval_data();
    assert!(data.validate().is_ok());
}

#[test]
fn test_approval_data_validate_missing_request_id() {
    let mut data = make_approval_data();
    data.request_id = String::new();
    assert!(data.validate().is_err());
}

#[test]
fn test_approval_data_validate_missing_operation() {
    let mut data = make_approval_data();
    data.operation = String::new();
    assert!(data.validate().is_err());
}

#[test]
fn test_approval_window_new() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    assert_eq!(window.get_id(), "w1");
    assert_eq!(window.get_request_id(), "req-123");
    assert_eq!(window.get_operation(), "file_write");
    assert_eq!(window.get_risk_level(), "high");
}

#[test]
fn test_approval_window_startup() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    assert!(window.startup().is_ok());
}

#[test]
fn test_approval_window_submit_no_client() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    let result = window.submit_approval(true, "approved");
    assert!(result.is_err());
}

#[test]
fn test_approval_data_serialization() {
    let data = make_approval_data();
    let json = serde_json::to_string(&data).unwrap();
    let back: ApprovalWindowData = serde_json::from_str(&json).unwrap();
    assert_eq!(back.request_id, "req-123");
    assert_eq!(back.operation, "file_write");
}

#[test]
fn test_approval_data_get_timeout() {
    let data = make_approval_data();
    assert_eq!(data.get_timeout(), 60);
}

#[test]
fn test_approval_data_get_timeout_default() {
    let mut data = make_approval_data();
    data.timeout_seconds = 0;
    assert_eq!(data.get_timeout(), 60);

    data.timeout_seconds = -5;
    assert_eq!(data.get_timeout(), 60);
}

#[test]
fn test_approval_data_get_timeout_custom() {
    let mut data = make_approval_data();
    data.timeout_seconds = 120;
    assert_eq!(data.get_timeout(), 120);
}

// ============================================================
// Additional tests for ~92% coverage
// ============================================================

#[test]
fn test_approval_window_getters() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    assert_eq!(window.get_target(), "/tmp/test.txt");
    assert_eq!(window.get_reason(), "writing to system directory");
    assert_eq!(window.get_timeout(), 60);
    assert_eq!(window.get_context().get("source").unwrap(), "test");
}

#[test]
fn test_approval_window_get_data() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    let d = window.get_data();
    assert_eq!(d.request_id, "req-123");
    assert_eq!(d.operation, "file_write");
    assert_eq!(d.risk_level, "high");
}

#[test]
fn test_approval_window_shutdown() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    // Should not panic
    window.shutdown();
}

#[test]
fn test_approval_window_submit_approval_approved() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    let result = window.submit_approval(true, "looks good");
    // No WS client, should fail
    assert!(result.is_err());
}

#[test]
fn test_approval_window_submit_approval_denied() {
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    let result = window.submit_approval(false, "dangerous operation");
    assert!(result.is_err());
}

#[test]
fn test_approval_window_startup_invalid_data() {
    let mut data = make_approval_data();
    data.request_id = String::new();
    let window = ApprovalWindow::new("w1".to_string(), data, None);
    assert!(window.startup().is_err());
}

#[test]
fn test_approval_data_deserialization_minimal() {
    let json =
        r#"{"request_id":"r1","operation":"file_write","target":"test.txt","risk_level":"HIGH"}"#;
    let data: ApprovalWindowData = serde_json::from_str(json).unwrap();
    assert_eq!(data.request_id, "r1");
    assert_eq!(data.operation_name, "");
    assert_eq!(data.reason, "");
    assert_eq!(data.timeout_seconds, 0);
    assert!(data.context.is_empty());
    assert_eq!(data.timestamp, 0);
}

#[test]
fn test_approval_data_deserialization_full() {
    let json = r#"{
        "request_id": "r2",
        "operation": "file_delete",
        "operation_name": "Delete File",
        "target": "/etc/passwd",
        "risk_level": "CRITICAL",
        "reason": "system file deletion",
        "timeout_seconds": 120,
        "context": {"user": "admin", "channel": "cli"},
        "timestamp": 1700000000
    }"#;
    let data: ApprovalWindowData = serde_json::from_str(json).unwrap();
    assert_eq!(data.request_id, "r2");
    assert_eq!(data.operation_name, "Delete File");
    assert_eq!(data.target, "/etc/passwd");
    assert_eq!(data.risk_level, "CRITICAL");
    assert_eq!(data.reason, "system file deletion");
    assert_eq!(data.timeout_seconds, 120);
    assert_eq!(data.context.get("user").unwrap(), "admin");
    assert_eq!(data.context.get("channel").unwrap(), "cli");
    assert_eq!(data.timestamp, 1700000000);
}

#[test]
fn test_approval_data_debug() {
    let data = make_approval_data();
    let debug = format!("{:?}", data);
    assert!(debug.contains("req-123"));
    assert!(debug.contains("file_write"));
}

#[test]
fn test_approval_data_clone() {
    let data = make_approval_data();
    let cloned = data.clone();
    assert_eq!(cloned.request_id, data.request_id);
    assert_eq!(cloned.operation, data.operation);
    assert_eq!(cloned.risk_level, data.risk_level);
}

#[test]
fn test_approval_data_get_type() {
    let data = make_approval_data();
    assert_eq!(data.get_type(), "approval");
}

#[test]
fn test_approval_window_new_with_ws_client() {
    use crate::websocket::client::WebSocketKey;
    let ws_key = WebSocketKey {
        key: "test-key".to_string(),
        port: 8080,
        path: "/ws".to_string(),
    };
    let client = Arc::new(WebSocketClient::new(&ws_key));
    let data = make_approval_data();
    let window = ApprovalWindow::new("w1".to_string(), data, Some(client));
    assert_eq!(window.get_id(), "w1");
}
