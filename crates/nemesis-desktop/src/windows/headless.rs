//! Headless window - Windowless mode for testing.
//!
//! Runs without a UI, auto-approving after a delay. Used for testing
//! the communication flow without requiring a real window.

use std::sync::Arc;
use std::time::Duration;

use tracing::info;

use crate::websocket::client::WebSocketClient;
use super::approval::ApprovalWindowData;

/// Run a headless window that auto-approves after a delay.
///
/// This is used for testing the parent-child communication flow
/// without requiring a real window.
pub async fn run_headless_window(
    window_id: &str,
    data: &ApprovalWindowData,
    ws_client: Option<Arc<WebSocketClient>>,
) -> Result<(), String> {
    info!("HeadlessWindow({}): Starting", window_id);

    // Wait 1 second before auto-approving
    tokio::time::sleep(Duration::from_secs(1)).await;

    let result = serde_json::json!({
        "approved": true,
        "reason": "auto-approve (test mode)",
        "request_id": data.request_id,
        "timestamp": chrono::Utc::now().timestamp(),
    });

    info!("HeadlessWindow({}): Sending auto-approve result", window_id);

    // Send result via WebSocket
    if let Some(ref client) = ws_client {
        client.notify("approval.submit", result)?;
    }

    // Keep alive for a bit to ensure result is sent
    tokio::time::sleep(Duration::from_secs(2)).await;

    info!("HeadlessWindow({}): Completed", window_id);
    Ok(())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::window_base::WindowData;
    use std::collections::HashMap;

    fn make_approval_data() -> ApprovalWindowData {
        ApprovalWindowData {
            request_id: "req-test".to_string(),
            operation: "file_write".to_string(),
            operation_name: String::new(),
            target: "/tmp/test.txt".to_string(),
            risk_level: "low".to_string(),
            reason: String::new(),
            timeout_seconds: 60,
            context: HashMap::new(),
            timestamp: chrono::Utc::now().timestamp(),
        }
    }

    #[tokio::test]
    async fn test_run_headless_no_client() {
        let data = make_approval_data();
        let result = run_headless_window("test-window", &data, None).await;
        assert!(result.is_ok());
    }

    // ============================================================
    // Additional tests for ~92% coverage
    // ============================================================

    #[test]
    fn test_approval_window_data_for_headless() {
        let data = make_approval_data();
        assert_eq!(data.request_id, "req-test");
        assert_eq!(data.operation, "file_write");
        assert_eq!(data.risk_level, "low");
        assert_eq!(data.target, "/tmp/test.txt");
    }

    #[test]
    fn test_headless_approval_data_validate() {
        let data = make_approval_data();
        assert!(data.validate().is_ok());
    }

    #[test]
    fn test_headless_approval_data_invalid() {
        let mut data = make_approval_data();
        data.request_id = String::new();
        assert!(data.validate().is_err());
    }

    #[tokio::test]
    async fn test_run_headless_with_disconnected_client() {
        use crate::websocket::client::WebSocketKey;
        let ws_key = WebSocketKey {
            key: "test-key".to_string(),
            port: 8080,
            path: "/ws".to_string(),
        };
        let client = Arc::new(WebSocketClient::new(&ws_key));
        // Client is not connected, so notify will fail
        let data = make_approval_data();
        let result = run_headless_window("test-window", &data, Some(client)).await;
        assert!(result.is_err());
    }
}
