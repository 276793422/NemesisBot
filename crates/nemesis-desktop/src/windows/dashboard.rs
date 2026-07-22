//! Dashboard window - Main desktop dashboard.
//!
//! Manages the persistent dashboard window that shows the bot status,
//! logs, and configuration.

use std::sync::Arc;

use serde::{Deserialize, Serialize};

use super::window_base::{WindowBase, WindowData};
use crate::websocket::client::WebSocketClient;

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
mod tests;
