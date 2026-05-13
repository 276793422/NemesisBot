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
mod tests {
    use super::*;

    #[test]
    fn test_pipe_message_handshake() {
        let msg = PipeMessage::handshake();
        assert!(msg.is_handshake());
        assert_eq!(msg.version, PROTOCOL_VERSION);
    }

    #[test]
    fn test_pipe_message_ack() {
        let msg = PipeMessage::ack();
        assert!(msg.is_ack());
    }

    #[test]
    fn test_pipe_message_ws_key() {
        let msg = PipeMessage::ws_key("test-key", 8080, "/ws");
        assert!(msg.is_ws_key());
        assert_eq!(msg.data["key"], serde_json::json!("test-key"));
        assert_eq!(msg.data["port"], serde_json::json!(8080));
    }

    #[test]
    fn test_pipe_message_window_data() {
        let data = serde_json::json!({"title": "Test"});
        let msg = PipeMessage::window_data(&data);
        assert!(msg.is_window_data());
        assert_eq!(msg.data["data"]["title"], serde_json::json!("Test"));
    }

    #[test]
    fn test_pipe_message_serialization() {
        let msg = PipeMessage::handshake();
        let json = serde_json::to_string(&msg).unwrap();
        assert!(json.contains("\"type\":\"handshake\""));
        assert!(json.contains("\"version\":\"1.0\""));
    }

    #[test]
    fn test_pipe_message_deserialization() {
        let json = r#"{"type":"ack","version":"1.0","data":{"status":"ok"}}"#;
        let msg: PipeMessage = serde_json::from_str(json).unwrap();
        assert!(msg.is_ack());
        assert_eq!(msg.data["status"], serde_json::json!("ok"));
    }

    #[test]
    fn test_handshake_result() {
        let result = HandshakeResult {
            success: true,
            window_id: Some("window-1".to_string()),
            error: None,
        };
        assert!(result.success);
        assert_eq!(result.window_id.as_deref(), Some("window-1"));
    }

    // ============================================================
    // Additional tests for coverage improvement
    // ============================================================

    #[test]
    fn test_pipe_message_new() {
        let msg = PipeMessage::new("custom_type");
        assert_eq!(msg.msg_type, "custom_type");
        assert_eq!(msg.version, PROTOCOL_VERSION);
        assert!(msg.data.is_empty());
    }

    #[test]
    fn test_pipe_message_with_data() {
        let msg = PipeMessage::new("test")
            .with_data("key1", serde_json::json!("value1"))
            .with_data("key2", serde_json::json!(42));
        assert_eq!(msg.data["key1"], serde_json::json!("value1"));
        assert_eq!(msg.data["key2"], serde_json::json!(42));
    }

    #[test]
    fn test_pipe_message_type_checks() {
        let msg = PipeMessage::handshake();
        assert!(msg.is_handshake());
        assert!(!msg.is_ack());
        assert!(!msg.is_ws_key());
        assert!(!msg.is_window_data());

        let msg = PipeMessage::ack();
        assert!(!msg.is_handshake());
        assert!(msg.is_ack());

        let msg = PipeMessage::ws_key("k", 8080, "/ws");
        assert!(msg.is_ws_key());

        let msg = PipeMessage::window_data(&serde_json::json!({}));
        assert!(msg.is_window_data());
    }

    #[test]
    fn test_pipe_message_deserialization_defaults() {
        let json = r#"{"type":"custom"}"#;
        let msg: PipeMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.msg_type, "custom");
        assert!(msg.version.is_empty());
        assert!(msg.data.is_empty());
    }

    #[test]
    fn test_constants() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
        assert_eq!(PROTOCOL_NAME, "anon-pipe-v1");
        assert_eq!(HANDSHAKE_TIMEOUT, Duration::from_secs(3));
        assert_eq!(ACK_TIMEOUT, Duration::from_secs(5));
    }

    #[test]
    fn test_handshake_result_failed() {
        let result = HandshakeResult {
            success: false,
            window_id: None,
            error: Some("timeout".to_string()),
        };
        assert!(!result.success);
        assert!(result.error.unwrap().contains("timeout"));
    }

    #[test]
    fn test_pipe_message_ws_key_fields() {
        let msg = PipeMessage::ws_key("secret-key", 9090, "/api/ws");
        assert_eq!(msg.data["key"], serde_json::json!("secret-key"));
        assert_eq!(msg.data["port"], serde_json::json!(9090));
        assert_eq!(msg.data["path"], serde_json::json!("/api/ws"));
    }

    #[test]
    fn test_pipe_message_window_data_nested() {
        let data = serde_json::json!({
            "title": "Test",
            "nested": {"a": 1, "b": [1, 2, 3]}
        });
        let msg = PipeMessage::window_data(&data);
        assert_eq!(msg.data["data"]["title"], serde_json::json!("Test"));
        assert_eq!(msg.data["data"]["nested"]["a"], serde_json::json!(1));
    }
}
