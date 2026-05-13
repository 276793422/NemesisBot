//! Window base - Common window functionality.
//!
//! Provides the WindowData trait and WindowBase struct that all window
//! types inherit from.

use std::sync::Arc;


use crate::websocket::client::WebSocketClient;

/// Trait for window-specific data.
pub trait WindowData: Send + Sync {
    /// Validate the data.
    fn validate(&self) -> Result<(), String>;
    /// Get the window type identifier.
    fn get_type(&self) -> String;
}

/// Base window functionality shared by all window types.
pub struct WindowBase {
    /// Window ID.
    pub id: String,
    /// Window type.
    pub window_type: String,
    /// Window data.
    pub data: Arc<dyn WindowData>,
    /// WebSocket client for communication with parent.
    pub ws_client: Option<Arc<WebSocketClient>>,
}

impl WindowBase {
    /// Create a new WindowBase.
    pub fn new(
        id: String,
        window_type: String,
        data: Arc<dyn WindowData>,
        ws_client: Option<Arc<WebSocketClient>>,
    ) -> Self {
        Self {
            id,
            window_type,
            data,
            ws_client,
        }
    }

    /// Startup the window.
    pub fn startup(&self) -> Result<(), String> {
        // Validate data
        self.data.validate()?;
        Ok(())
    }

    /// Shutdown the window.
    pub fn shutdown(&self) {
        if let Some(ref client) = self.ws_client {
            client.close();
        }
    }

    /// Get the window ID.
    pub fn get_id(&self) -> &str {
        &self.id
    }

    /// Get the window type.
    pub fn get_type(&self) -> &str {
        &self.window_type
    }

    /// Send a result back to the parent process.
    pub fn send_result(&self, result: serde_json::Value) -> Result<(), String> {
        if let Some(ref client) = self.ws_client {
            client.notify("approval.submit", result)
        } else {
            Err("no WebSocket client".to_string())
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    struct TestWindowData {
        valid: bool,
    }

    impl WindowData for TestWindowData {
        fn validate(&self) -> Result<(), String> {
            if self.valid {
                Ok(())
            } else {
                Err("invalid data".to_string())
            }
        }

        fn get_type(&self) -> String {
            "test".to_string()
        }
    }

    #[test]
    fn test_window_base_new() {
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, None);
        assert_eq!(base.get_id(), "w1");
        assert_eq!(base.get_type(), "test");
    }

    #[test]
    fn test_window_base_startup_valid() {
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, None);
        assert!(base.startup().is_ok());
    }

    #[test]
    fn test_window_base_startup_invalid() {
        let data = Arc::new(TestWindowData { valid: false });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, None);
        assert!(base.startup().is_err());
    }

    #[test]
    fn test_window_base_send_result_no_client() {
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, None);
        let result = base.send_result(serde_json::json!({}));
        assert!(result.is_err());
    }

    // ============================================================
    // Additional tests for ~92% coverage
    // ============================================================

    #[test]
    fn test_window_base_shutdown_no_client() {
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, None);
        // Should not panic
        base.shutdown();
    }

    #[test]
    fn test_window_base_shutdown_with_client() {
        use crate::websocket::client::WebSocketKey;
        let ws_key = WebSocketKey {
            key: "test-key".to_string(),
            port: 8080,
            path: "/ws".to_string(),
        };
        let client = Arc::new(WebSocketClient::new(&ws_key));
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, Some(client));
        // Should close the client
        base.shutdown();
    }

    #[test]
    fn test_window_base_new_with_client() {
        use crate::websocket::client::WebSocketKey;
        let ws_key = WebSocketKey {
            key: "test-key".to_string(),
            port: 8080,
            path: "/ws".to_string(),
        };
        let client = Arc::new(WebSocketClient::new(&ws_key));
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, Some(client.clone()));
        assert_eq!(base.get_id(), "w1");
        assert_eq!(base.get_type(), "test");
        assert!(base.ws_client.is_some());
    }

    #[test]
    fn test_window_base_send_result_with_disconnected_client() {
        use crate::websocket::client::WebSocketKey;
        let ws_key = WebSocketKey {
            key: "test-key".to_string(),
            port: 8080,
            path: "/ws".to_string(),
        };
        let client = Arc::new(WebSocketClient::new(&ws_key));
        let data = Arc::new(TestWindowData { valid: true });
        let base = WindowBase::new("w1".to_string(), "test".to_string(), data, Some(client));
        // Client is not connected, so notify should fail
        let result = base.send_result(serde_json::json!({"approved": true}));
        assert!(result.is_err());
    }

    #[test]
    fn test_window_data_trait() {
        let data = Arc::new(TestWindowData { valid: true });
        assert_eq!(data.get_type(), "test");
        assert!(data.validate().is_ok());

        let invalid_data = Arc::new(TestWindowData { valid: false });
        assert!(invalid_data.validate().is_err());
    }
}
