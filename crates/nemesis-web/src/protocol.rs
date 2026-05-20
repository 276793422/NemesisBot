//! Three-level dispatch protocol: type -> module -> cmd.

use chrono::Utc;
use serde::{Deserialize, Serialize};

/// Protocol message with three-level dispatch.
///
/// Extended with `req_id` and `error` fields for request/response correlation.
/// Both new fields are `Option` with `skip_serializing_if` for full backward compatibility.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProtocolMessage {
    #[serde(rename = "type")]
    pub msg_type: String,
    pub module: String,
    pub cmd: String,

    // ---- Extended fields (all Option, backward compatible) ----
    #[serde(default, skip_serializing_if = "Option::is_none", rename = "reqId")]
    pub req_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,

    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
}

impl ProtocolMessage {
    /// Parse raw JSON bytes into a ProtocolMessage.
    pub fn parse(raw: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(raw)
    }

    /// Check if raw JSON uses the new three-level format.
    pub fn is_new_protocol(raw: &[u8]) -> bool {
        #[derive(Deserialize)]
        struct Probe {
            module: Option<String>,
        }
        match serde_json::from_slice::<Probe>(raw) {
            Ok(p) => p.module.as_ref().map_or(false, |m| !m.is_empty()),
            Err(_) => false,
        }
    }

    /// Create a new protocol message.
    pub fn new(msg_type: &str, module: &str, cmd: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            msg_type: msg_type.to_string(),
            module: module.to_string(),
            cmd: cmd.to_string(),
            req_id: None,
            error: None,
            data,
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Build an API request message (type="request").
    pub fn request(module: &str, cmd: &str, req_id: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            msg_type: "request".to_string(),
            module: module.to_string(),
            cmd: cmd.to_string(),
            req_id: Some(req_id.to_string()),
            error: None,
            data,
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Build a successful API response message (type="response").
    pub fn response_ok(module: &str, cmd: &str, req_id: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            msg_type: "response".to_string(),
            module: module.to_string(),
            cmd: cmd.to_string(),
            req_id: Some(req_id.to_string()),
            error: None,
            data,
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Build an error API response message (type="response" with error).
    pub fn response_err(module: &str, cmd: &str, req_id: &str, error: &str) -> Self {
        Self {
            msg_type: "response".to_string(),
            module: module.to_string(),
            cmd: cmd.to_string(),
            req_id: Some(req_id.to_string()),
            error: Some(error.to_string()),
            data: None,
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Build a server push message (type="push").
    pub fn push(module: &str, cmd: &str, data: Option<serde_json::Value>) -> Self {
        Self {
            msg_type: "push".to_string(),
            module: module.to_string(),
            cmd: cmd.to_string(),
            req_id: None,
            error: None,
            data,
            timestamp: Some(Utc::now().to_rfc3339()),
        }
    }

    /// Check if this is an API request message.
    pub fn is_request(&self) -> bool {
        self.msg_type == "request"
    }

    /// Check if this is an API response message.
    pub fn is_response(&self) -> bool {
        self.msg_type == "response"
    }

    /// Check if this is a server push message.
    pub fn is_push(&self) -> bool {
        self.msg_type == "push"
    }

    /// Serialize to JSON bytes.
    pub fn to_json(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Decode the data field into a typed value.
    pub fn decode_data<T: serde::de::DeserializeOwned>(&self) -> Result<T, String> {
        match &self.data {
            Some(v) => serde_json::from_value(v.clone()).map_err(|e| e.to_string()),
            None => Err("message has no data".to_string()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_protocol() {
        let json = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"}}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        assert_eq!(msg.msg_type, "message");
        assert_eq!(msg.module, "chat");
        assert_eq!(msg.cmd, "send");
    }

    #[test]
    fn test_is_new_protocol() {
        assert!(ProtocolMessage::is_new_protocol(br#"{"type":"message","module":"chat","cmd":"send"}"#));
        assert!(!ProtocolMessage::is_new_protocol(br#"{"type":"message","cmd":"send"}"#));
        assert!(!ProtocolMessage::is_new_protocol(br#"not json"#));
    }

    #[test]
    fn test_new_message() {
        let msg = ProtocolMessage::new("system", "heartbeat", "pong", None);
        assert_eq!(msg.msg_type, "system");
        assert!(msg.timestamp.is_some());
    }

    #[test]
    fn test_to_json_roundtrip() {
        let msg = ProtocolMessage::new("message", "chat", "send", Some(serde_json::json!({"content": "hi"})));
        let bytes = msg.to_json().unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.msg_type, "message");
        assert_eq!(parsed.module, "chat");
    }

    #[test]
    fn test_parse_invalid_json() {
        let result = ProtocolMessage::parse(b"not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_parse_missing_fields() {
        let result = ProtocolMessage::parse(br#"{"type":"message"}"#);
        // Should fail because module and cmd are required
        assert!(result.is_err());
    }

    #[test]
    fn test_is_new_protocol_various() {
        // Valid new protocol
        assert!(ProtocolMessage::is_new_protocol(br#"{"type":"msg","module":"chat","cmd":"send"}"#));
        // Empty module
        assert!(!ProtocolMessage::is_new_protocol(br#"{"module":""}"#));
        // Missing module
        assert!(!ProtocolMessage::is_new_protocol(br#"{"type":"msg","cmd":"send"}"#));
    }

    #[test]
    fn test_decode_data_success() {
        let msg = ProtocolMessage::parse(
            br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"}}"#
        ).unwrap();

        #[derive(serde::Deserialize)]
        struct ChatData { content: String }
        let data: ChatData = msg.decode_data().unwrap();
        assert_eq!(data.content, "hello");
    }

    #[test]
    fn test_decode_data_no_data() {
        let msg = ProtocolMessage::new("system", "heartbeat", "ping", None);
        let result: Result<String, _> = msg.decode_data();
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("no data"));
    }

    #[test]
    fn test_decode_data_wrong_type() {
        let msg = ProtocolMessage::new("test", "mod", "cmd", Some(serde_json::json!("not an object")));
        let result: Result<std::collections::HashMap<String, String>, _> = msg.decode_data();
        assert!(result.is_err());
    }

    #[test]
    fn test_timestamp_auto_set() {
        let msg = ProtocolMessage::new("system", "test", "cmd", None);
        assert!(msg.timestamp.is_some());
        let ts = msg.timestamp.unwrap();
        assert!(!ts.is_empty());
    }

    #[test]
    fn test_skip_serializing_none_fields() {
        let msg = ProtocolMessage::new("test", "mod", "cmd", None);
        let json = msg.to_json().unwrap();
        let json_str = String::from_utf8(json).unwrap();
        // Should not contain "data" key since it's None
        assert!(!json_str.contains("\"data\""));
    }

    #[test]
    fn test_protocol_message_with_complex_data() {
        let data = serde_json::json!({
            "nested": {"key": "value"},
            "array": [1, 2, 3],
            "bool": true,
            "null": null
        });
        let msg = ProtocolMessage::new("message", "test", "complex", Some(data.clone()));
        let bytes = msg.to_json().unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.data.unwrap(), data);
    }

    #[test]
    fn test_parse_with_extra_fields() {
        let json = br#"{"type":"message","module":"chat","cmd":"send","extra":"ignored"}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        assert_eq!(msg.msg_type, "message");
        assert_eq!(msg.module, "chat");
    }

    #[test]
    fn test_parse_preserves_data_types() {
        let json = br#"{"type":"message","module":"chat","cmd":"send","data":{"num":42,"bool":true,"null":null,"str":"hello","arr":[1,2,3]}}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        let data = msg.data.unwrap();
        assert_eq!(data["num"], 42);
        assert_eq!(data["bool"], true);
        assert_eq!(data["null"], serde_json::Value::Null);
        assert_eq!(data["str"], "hello");
        assert_eq!(data["arr"].as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_is_new_protocol_with_empty_module() {
        assert!(!ProtocolMessage::is_new_protocol(br#"{"type":"msg","module":""}"#));
    }

    #[test]
    fn test_is_new_protocol_with_only_module() {
        // Has module but no type - should still be new protocol
        assert!(ProtocolMessage::is_new_protocol(br#"{"module":"chat","cmd":"send"}"#));
    }

    #[test]
    fn test_new_message_without_data() {
        let msg = ProtocolMessage::new("system", "health", "ping", None);
        assert!(msg.data.is_none());
        assert_eq!(msg.msg_type, "system");
        assert_eq!(msg.module, "health");
        assert_eq!(msg.cmd, "ping");
    }

    #[test]
    fn test_new_message_with_null_data() {
        let msg = ProtocolMessage::new("test", "mod", "cmd", Some(serde_json::Value::Null));
        assert!(msg.data.is_some());
    }

    #[test]
    fn test_new_message_with_array_data() {
        let data = serde_json::json!([1, "two", true]);
        let msg = ProtocolMessage::new("test", "mod", "cmd", Some(data));
        let bytes = msg.to_json().unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.data.unwrap().as_array().unwrap().len(), 3);
    }

    #[test]
    fn test_decode_data_into_vec() {
        let msg = ProtocolMessage::parse(
            br#"{"type":"test","module":"m","cmd":"c","data":[1,2,3]}"#
        ).unwrap();
        let data: Vec<i32> = msg.decode_data().unwrap();
        assert_eq!(data, vec![1, 2, 3]);
    }

    #[test]
    fn test_decode_data_type_mismatch() {
        let msg = ProtocolMessage::parse(
            br#"{"type":"test","module":"m","cmd":"c","data":"not a number"}"#
        ).unwrap();
        let result: Result<i32, _> = msg.decode_data();
        assert!(result.is_err());
    }

    #[test]
    fn test_to_json_produces_valid_json() {
        let msg = ProtocolMessage::new("test", "mod", "cmd", Some(serde_json::json!({"key": "val"})));
        let bytes = msg.to_json().unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        // Should be valid JSON
        let _: serde_json::Value = serde_json::from_str(&json_str).unwrap();
    }

    #[test]
    fn test_parse_with_unicode_in_data() {
        let json = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"\u4f60\u597d\u4e16\u754c"}}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        let data = msg.data.unwrap();
        assert!(data["content"].as_str().unwrap().contains("\u{4f60}"));
    }

    #[test]
    fn test_parse_with_nested_object_data() {
        let json = br#"{"type":"test","module":"m","cmd":"c","data":{"user":{"name":"alice","age":30},"tags":["a","b"]}}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        let data = msg.data.unwrap();
        assert_eq!(data["user"]["name"], "alice");
        assert_eq!(data["user"]["age"], 30);
        assert_eq!(data["tags"].as_array().unwrap().len(), 2);
    }

    #[test]
    fn test_is_new_protocol_garbage_data() {
        assert!(!ProtocolMessage::is_new_protocol(b"\x00\x01\x02"));
        assert!(!ProtocolMessage::is_new_protocol(b""));
        assert!(!ProtocolMessage::is_new_protocol(b"null"));
        assert!(!ProtocolMessage::is_new_protocol(b"123"));
    }

    #[test]
    fn test_new_message_timestamp_is_rfc3339() {
        let msg = ProtocolMessage::new("system", "test", "cmd", None);
        let ts = msg.timestamp.unwrap();
        // Should parse as RFC 3339
        let _dt = chrono::DateTime::parse_from_rfc3339(&ts).unwrap();
    }

    #[test]
    fn test_parse_requires_module_and_cmd() {
        // Missing cmd
        let result = ProtocolMessage::parse(br#"{"type":"msg","module":"chat"}"#);
        assert!(result.is_err());
    }

    #[test]
    fn test_roundtrip_preserves_all_fields() {
        let data = serde_json::json!({"nested": {"deep": true}, "list": [1, 2]});
        let original = ProtocolMessage::new("message", "chat", "send", Some(data));
        let bytes = original.to_json().unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.msg_type, original.msg_type);
        assert_eq!(parsed.module, original.module);
        assert_eq!(parsed.cmd, original.cmd);
        assert_eq!(parsed.data, original.data);
    }

    #[test]
    fn test_skip_serializing_none_timestamp() {
        // Manually create a message with no timestamp
        let json = br#"{"type":"message","module":"chat","cmd":"send"}"#;
        let msg = ProtocolMessage::parse(json).unwrap();
        // timestamp is not in the original JSON, so it should be None
        assert!(msg.timestamp.is_none());
    }

    // ============================================================
    // Extended protocol tests: req_id, error, request/response/push
    // ============================================================

    #[test]
    fn test_request_builder() {
        let msg = ProtocolMessage::request("models", "list", "req-001", Some(serde_json::json!({})));
        assert_eq!(msg.msg_type, "request");
        assert_eq!(msg.module, "models");
        assert_eq!(msg.cmd, "list");
        assert_eq!(msg.req_id.as_deref(), Some("req-001"));
        assert!(msg.error.is_none());
        assert!(msg.timestamp.is_some());
        assert!(msg.is_request());
        assert!(!msg.is_response());
        assert!(!msg.is_push());
    }

    #[test]
    fn test_response_ok_builder() {
        let msg = ProtocolMessage::response_ok(
            "models", "list", "req-001",
            Some(serde_json::json!([{"name": "gpt-4"}])),
        );
        assert_eq!(msg.msg_type, "response");
        assert_eq!(msg.req_id.as_deref(), Some("req-001"));
        assert!(msg.error.is_none());
        assert!(msg.data.is_some());
        assert!(msg.is_response());
        assert!(!msg.is_request());
    }

    #[test]
    fn test_response_err_builder() {
        let msg = ProtocolMessage::response_err("models", "list", "req-002", "not found");
        assert_eq!(msg.msg_type, "response");
        assert_eq!(msg.req_id.as_deref(), Some("req-002"));
        assert_eq!(msg.error.as_deref(), Some("not found"));
        assert!(msg.data.is_none());
    }

    #[test]
    fn test_push_builder() {
        let msg = ProtocolMessage::push("logs", "append", Some(serde_json::json!({"line": "test"})));
        assert_eq!(msg.msg_type, "push");
        assert!(msg.req_id.is_none());
        assert!(msg.error.is_none());
        assert!(msg.is_push());
    }

    #[test]
    fn test_request_response_roundtrip() {
        let req = ProtocolMessage::request("models", "list", "req-100", Some(serde_json::json!({"detail": true})));
        let bytes = req.to_json().unwrap();
        let parsed = ProtocolMessage::parse(&bytes).unwrap();
        assert_eq!(parsed.msg_type, "request");
        assert_eq!(parsed.req_id.as_deref(), Some("req-100"));
        assert_eq!(parsed.data.unwrap()["detail"], true);
    }

    #[test]
    fn test_error_response_roundtrip() {
        let resp = ProtocolMessage::response_err("test", "cmd", "err-1", "something failed");
        let bytes = resp.to_json().unwrap();
        let json_str = String::from_utf8(bytes).unwrap();
        // Should contain "error" field
        assert!(json_str.contains("\"error\""));
        assert!(json_str.contains("something failed"));
        // Should NOT contain "data" (None is skipped)
        assert!(!json_str.contains("\"data\""));

        let parsed = ProtocolMessage::parse(json_str.as_bytes()).unwrap();
        assert_eq!(parsed.error.as_deref(), Some("something failed"));
    }

    #[test]
    fn test_backward_compat_old_messages() {
        // Old messages without req_id/error should parse fine
        let old = br#"{"type":"message","module":"chat","cmd":"send","data":{"content":"hello"}}"#;
        let msg = ProtocolMessage::parse(old).unwrap();
        assert!(msg.req_id.is_none());
        assert!(msg.error.is_none());
        assert_eq!(msg.module, "chat");
    }

    #[test]
    fn test_is_request_helpers() {
        assert!(ProtocolMessage::request("m", "c", "r", None).is_request());
        assert!(ProtocolMessage::response_ok("m", "c", "r", None).is_response());
        assert!(ProtocolMessage::push("m", "c", None).is_push());
        // Old-style messages
        assert!(!ProtocolMessage::new("message", "m", "c", None).is_request());
        assert!(!ProtocolMessage::new("system", "m", "c", None).is_response());
    }

    #[test]
    fn test_skip_serializing_none_req_id_and_error() {
        let msg = ProtocolMessage::new("message", "chat", "send", None);
        let json_str = String::from_utf8(msg.to_json().unwrap()).unwrap();
        assert!(!json_str.contains("\"reqId\""));
        assert!(!json_str.contains("\"error\""));
    }
}
