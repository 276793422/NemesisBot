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
mod tests;
