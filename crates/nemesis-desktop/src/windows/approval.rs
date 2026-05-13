//! Approval window - Security approval popup.
//!
//! Manages approval window data and submission logic for security
//! action approvals.

use std::collections::HashMap;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::websocket::client::WebSocketClient;
use super::window_base::{WindowBase, WindowData};

/// Data for an approval window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ApprovalWindowData {
    pub request_id: String,
    pub operation: String,
    #[serde(default)]
    pub operation_name: String,
    pub target: String,
    pub risk_level: String,
    #[serde(default)]
    pub reason: String,
    #[serde(default)]
    pub timeout_seconds: i32,
    #[serde(default)]
    pub context: HashMap<String, String>,
    #[serde(default)]
    pub timestamp: i64,
}

impl WindowData for ApprovalWindowData {
    fn validate(&self) -> Result<(), String> {
        if self.request_id.is_empty() {
            return Err("request_id is required".to_string());
        }
        if self.operation.is_empty() {
            return Err("operation is required".to_string());
        }
        Ok(())
    }

    fn get_type(&self) -> String {
        "approval".to_string()
    }
}

impl ApprovalWindowData {
    /// Get the configured timeout as u64 seconds.
    ///
    /// Returns the timeout_seconds value cast to u64. If timeout_seconds
    /// is 0 or negative, returns a default of 60 seconds.
    pub fn get_timeout(&self) -> u64 {
        if self.timeout_seconds > 0 {
            self.timeout_seconds as u64
        } else {
            60
        }
    }
}

/// An approval window instance.
pub struct ApprovalWindow {
    base: WindowBase,
    data: Arc<ApprovalWindowData>,
}

impl ApprovalWindow {
    /// Create a new approval window.
    pub fn new(
        window_id: String,
        data: ApprovalWindowData,
        ws_client: Option<Arc<WebSocketClient>>,
    ) -> Self {
        let data_arc = Arc::new(data);
        Self {
            base: WindowBase::new(
                window_id,
                "approval".to_string(),
                data_arc.clone(),
                ws_client,
            ),
            data: data_arc,
        }
    }

    /// Startup the approval window.
    pub fn startup(&self) -> Result<(), String> {
        self.base.startup()
    }

    /// Shutdown the approval window.
    pub fn shutdown(&self) {
        self.base.shutdown();
    }

    /// Get the window ID.
    pub fn get_id(&self) -> &str {
        self.base.get_id()
    }

    /// Submit an approval decision.
    pub fn submit_approval(&self, approved: bool, reason: &str) -> Result<(), String> {
        let result = serde_json::json!({
            "approved": approved,
            "reason": reason,
            "request_id": self.data.request_id,
            "timestamp": chrono::Utc::now().timestamp(),
        });

        self.base.send_result(result)
    }

    /// Get the approval data.
    pub fn get_data(&self) -> &ApprovalWindowData {
        &self.data
    }

    /// Get the request ID.
    pub fn get_request_id(&self) -> &str {
        &self.data.request_id
    }

    /// Get the operation.
    pub fn get_operation(&self) -> &str {
        &self.data.operation
    }

    /// Get the risk level.
    pub fn get_risk_level(&self) -> &str {
        &self.data.risk_level
    }

    /// Get the target.
    pub fn get_target(&self) -> &str {
        &self.data.target
    }

    /// Get the reason.
    pub fn get_reason(&self) -> &str {
        &self.data.reason
    }

    /// Get the timeout.
    pub fn get_timeout(&self) -> i32 {
        self.data.timeout_seconds
    }

    /// Get the context.
    pub fn get_context(&self) -> &HashMap<String, String> {
        &self.data.context
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
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
            timestamp: chrono::Utc::now().timestamp(),
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
        let json = r#"{"request_id":"r1","operation":"file_write","target":"test.txt","risk_level":"HIGH"}"#;
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
}
