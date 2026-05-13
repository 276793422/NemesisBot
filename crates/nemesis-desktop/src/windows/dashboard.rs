//! Dashboard window - Main desktop dashboard.
//!
//! Manages the persistent dashboard window that shows the bot status,
//! logs, and configuration.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::websocket::client::WebSocketClient;
use super::window_base::{WindowBase, WindowData};

/// Data for a dashboard window.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DashboardWindowData {
    /// Authentication token.
    pub token: String,
    /// Web server port.
    pub web_port: u16,
    /// Web server host.
    pub web_host: String,
}

impl WindowData for DashboardWindowData {
    fn validate(&self) -> Result<(), String> {
        if self.token.is_empty() {
            return Err("token is required".to_string());
        }
        if self.web_port == 0 {
            return Err(format!("invalid web port: {}", self.web_port));
        }
        Ok(())
    }

    fn get_type(&self) -> String {
        "dashboard".to_string()
    }
}

impl DashboardWindowData {
    /// Get the authentication token.
    ///
    /// Returns Some(token) if the token is non-empty, None otherwise.
    pub fn get_token(&self) -> Option<String> {
        if self.token.is_empty() {
            None
        } else {
            Some(self.token.clone())
        }
    }

    /// Get the web server port.
    pub fn get_web_port(&self) -> u16 {
        self.web_port
    }

    /// Get the web server host.
    pub fn get_web_host(&self) -> String {
        self.web_host.clone()
    }
}

/// A dashboard window instance.
pub struct DashboardWindow {
    base: WindowBase,
    data: Arc<DashboardWindowData>,
    ws_client: Option<Arc<WebSocketClient>>,
}

impl DashboardWindow {
    /// Create a new dashboard window.
    pub fn new(
        window_id: String,
        data: DashboardWindowData,
        ws_client: Option<Arc<WebSocketClient>>,
    ) -> Self {
        let data_arc = Arc::new(data);
        let ws = ws_client.clone();
        Self {
            base: WindowBase::new(
                window_id,
                "dashboard".to_string(),
                data_arc.clone(),
                ws_client,
            ),
            data: data_arc,
            ws_client: ws,
        }
    }

    /// Startup the dashboard window.
    pub fn startup(&self) -> Result<(), String> {
        self.base.startup()?;

        // Register notification handlers for dashboard
        if let Some(ref client) = self.ws_client {
            // window.bring_to_front
            client.register_notification_handler("window.bring_to_front", |_msg| {
                // In full implementation: show window
            });

            // window.minimize
            client.register_notification_handler("window.minimize", |_msg| {
                // In full implementation: minimize window
            });

            // state.service_status
            client.register_notification_handler("state.service_status", |_msg| {
                // In full implementation: update UI
            });

            // system.ping
            client.register_handler("system.ping", |msg| {
                Ok(crate::websocket::protocol::Message::new_response(
                    msg.id.as_deref().unwrap_or(""),
                    serde_json::json!({"status": "ok"}),
                ))
            });
        }

        Ok(())
    }

    /// Shutdown the dashboard window.
    pub fn shutdown(&self) {
        self.base.shutdown();
    }

    /// Get the window ID.
    pub fn get_id(&self) -> &str {
        self.base.get_id()
    }

    /// Get the dashboard data.
    pub fn get_data(&self) -> &DashboardWindowData {
        &self.data
    }

    /// Get the token.
    pub fn get_token(&self) -> &str {
        &self.data.token
    }

    /// Get the web port.
    pub fn get_web_port(&self) -> u16 {
        self.data.web_port
    }

    /// Get the web host.
    pub fn get_web_host(&self) -> &str {
        &self.data.web_host
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn make_dashboard_data() -> DashboardWindowData {
        DashboardWindowData {
            token: "test-token-12345".to_string(),
            web_port: 8080,
            web_host: "127.0.0.1".to_string(),
        }
    }

    #[test]
    fn test_dashboard_data_validate() {
        let data = make_dashboard_data();
        assert!(data.validate().is_ok());
    }

    #[test]
    fn test_dashboard_data_validate_no_token() {
        let mut data = make_dashboard_data();
        data.token = String::new();
        assert!(data.validate().is_err());
    }

    #[test]
    fn test_dashboard_data_validate_zero_port() {
        let mut data = make_dashboard_data();
        data.web_port = 0;
        assert!(data.validate().is_err());
    }

    #[test]
    fn test_dashboard_window_new() {
        let data = make_dashboard_data();
        let window = DashboardWindow::new("w1".to_string(), data, None);
        assert_eq!(window.get_id(), "w1");
        assert_eq!(window.get_token(), "test-token-12345");
        assert_eq!(window.get_web_port(), 8080);
        assert_eq!(window.get_web_host(), "127.0.0.1");
    }

    #[test]
    fn test_dashboard_window_startup() {
        let data = make_dashboard_data();
        let window = DashboardWindow::new("w1".to_string(), data, None);
        assert!(window.startup().is_ok());
    }

    #[test]
    fn test_dashboard_data_serialization() {
        let data = make_dashboard_data();
        let json = serde_json::to_string(&data).unwrap();
        let back: DashboardWindowData = serde_json::from_str(&json).unwrap();
        assert_eq!(back.token, "test-token-12345");
        assert_eq!(back.web_port, 8080);
    }

    #[test]
    fn test_dashboard_data_get_token() {
        let data = make_dashboard_data();
        assert_eq!(data.get_token(), Some("test-token-12345".to_string()));
    }

    #[test]
    fn test_dashboard_data_get_token_empty() {
        let mut data = make_dashboard_data();
        data.token = String::new();
        assert_eq!(data.get_token(), None);
    }

    #[test]
    fn test_dashboard_data_get_web_port() {
        let data = make_dashboard_data();
        assert_eq!(data.get_web_port(), 8080);
    }

    #[test]
    fn test_dashboard_data_get_web_host() {
        let data = make_dashboard_data();
        assert_eq!(data.get_web_host(), "127.0.0.1");
    }

    // ============================================================
    // Additional tests for ~92% coverage
    // ============================================================

    #[test]
    fn test_dashboard_window_shutdown() {
        let data = make_dashboard_data();
        let window = DashboardWindow::new("w1".to_string(), data, None);
        // Should not panic
        window.shutdown();
    }

    #[test]
    fn test_dashboard_window_startup_invalid() {
        let mut data = make_dashboard_data();
        data.token = String::new();
        let window = DashboardWindow::new("w1".to_string(), data, None);
        assert!(window.startup().is_err());
    }

    #[test]
    fn test_dashboard_window_startup_with_ws_client() {
        use crate::websocket::client::WebSocketKey;
        let ws_key = WebSocketKey {
            key: "test-key".to_string(),
            port: 8080,
            path: "/ws".to_string(),
        };
        let client = Arc::new(WebSocketClient::new(&ws_key));
        let data = make_dashboard_data();
        let window = DashboardWindow::new("w1".to_string(), data, Some(client));
        // Startup should succeed (valid data + ws client for handler registration)
        assert!(window.startup().is_ok());
    }

    #[test]
    fn test_dashboard_window_get_data() {
        let data = make_dashboard_data();
        let window = DashboardWindow::new("w1".to_string(), data, None);
        let d = window.get_data();
        assert_eq!(d.token, "test-token-12345");
        assert_eq!(d.web_port, 8080);
    }

    #[test]
    fn test_dashboard_data_debug() {
        let data = make_dashboard_data();
        let debug = format!("{:?}", data);
        assert!(debug.contains("test-token-12345"));
    }

    #[test]
    fn test_dashboard_data_clone() {
        let data = make_dashboard_data();
        let cloned = data.clone();
        assert_eq!(cloned.token, data.token);
        assert_eq!(cloned.web_port, data.web_port);
        assert_eq!(cloned.web_host, data.web_host);
    }

    #[test]
    fn test_dashboard_data_get_type() {
        let data = make_dashboard_data();
        assert_eq!(data.get_type(), "dashboard");
    }

    #[test]
    fn test_dashboard_data_serialization_roundtrip() {
        let data = make_dashboard_data();
        let json = serde_json::to_string(&data).unwrap();
        let parsed: DashboardWindowData = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.token, data.token);
        assert_eq!(parsed.web_port, data.web_port);
        assert_eq!(parsed.web_host, data.web_host);
    }

    #[test]
    fn test_dashboard_data_various_hosts() {
        let data = DashboardWindowData {
            token: "tok".to_string(),
            web_port: 443,
            web_host: "0.0.0.0".to_string(),
        };
        assert_eq!(data.get_web_host(), "0.0.0.0");
        assert_eq!(data.get_web_port(), 443);
    }
}
