//! Three-level dispatch protocol: type -> module -> cmd.

use chrono::Local;
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
            timestamp: Some(Local::now().to_rfc3339()),
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
            timestamp: Some(Local::now().to_rfc3339()),
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
            timestamp: Some(Local::now().to_rfc3339()),
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
            timestamp: Some(Local::now().to_rfc3339()),
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
            timestamp: Some(Local::now().to_rfc3339()),
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
mod tests;
