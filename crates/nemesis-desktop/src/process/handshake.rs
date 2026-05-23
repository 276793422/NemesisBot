//! Pipe-based handshake protocol for parent-child process communication.
//!
//! Implements the `anon-pipe-v1` protocol with JSON-encoded messages
//! over stdin/stdout pipes for initial setup between parent and child.

use std::collections::HashMap;
use std::time::Duration;

use serde::{Deserialize, Serialize};

/// Protocol version string.
pub const PROTOCOL_VERSION: &str = "1.0";
/// Protocol name.
pub const PROTOCOL_NAME: &str = "anon-pipe-v1";
/// Handshake timeout.
pub const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(3);
/// ACK timeout.
pub const ACK_TIMEOUT: Duration = Duration::from_secs(5);

/// Result of a handshake operation.
#[derive(Debug, Clone)]
pub struct HandshakeResult {
    pub success: bool,
    pub window_id: Option<String>,
    pub error: Option<String>,
}

/// A pipe message used for parent-child communication.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PipeMessage {
    /// Message type: "handshake", "ws_key", "ack", "error", "window_data".
    #[serde(rename = "type")]
    pub msg_type: String,
    /// Protocol version.
    #[serde(default)]
    pub version: String,
    /// Additional data.
    #[serde(default)]
    pub data: HashMap<String, serde_json::Value>,
}

impl PipeMessage {
    /// Create a new pipe message.
    pub fn new(msg_type: &str) -> Self {
        Self {
            msg_type: msg_type.to_string(),
            version: PROTOCOL_VERSION.to_string(),
            data: HashMap::new(),
        }
    }

    /// Add a data field.
    pub fn with_data(mut self, key: &str, value: serde_json::Value) -> Self {
        self.data.insert(key.to_string(), value);
        self
    }

    /// Create a handshake message.
    pub fn handshake() -> Self {
        Self::new("handshake")
            .with_data("protocol", serde_json::json!(PROTOCOL_NAME))
            .with_data("version", serde_json::json!(PROTOCOL_VERSION))
    }

    /// Create an ACK message.
    pub fn ack() -> Self {
        Self::new("ack").with_data("status", serde_json::json!("ok"))
    }

    /// Create a WS key message.
    pub fn ws_key(key: &str, port: u16, path: &str) -> Self {
        Self::new("ws_key")
            .with_data("key", serde_json::json!(key))
            .with_data("port", serde_json::json!(port))
            .with_data("path", serde_json::json!(path))
    }

    /// Create a window data message.
    pub fn window_data(data: &serde_json::Value) -> Self {
        Self::new("window_data").with_data("data", data.clone())
    }

    /// Check if this is an ACK message.
    pub fn is_ack(&self) -> bool {
        self.msg_type == "ack"
    }

    /// Check if this is a handshake message.
    pub fn is_handshake(&self) -> bool {
        self.msg_type == "handshake"
    }

    /// Check if this is a WS key message.
    pub fn is_ws_key(&self) -> bool {
        self.msg_type == "ws_key"
    }

    /// Check if this is a window data message.
    pub fn is_window_data(&self) -> bool {
        self.msg_type == "window_data"
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
