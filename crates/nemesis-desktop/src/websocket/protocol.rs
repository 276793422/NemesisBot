//! JSON-RPC 2.0 protocol types.
//!
//! Defines the Message envelope, ErrorPayload, and constructors for
//! requests, notifications, responses, and error responses.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

/// JSON-RPC version string.
pub const VERSION: &str = "2.0";

/// Standard JSON-RPC error codes.
pub const ERR_PARSE_ERROR: i32 = -32700;
pub const ERR_INVALID_REQUEST: i32 = -32600;
pub const ERR_METHOD_NOT_FOUND: i32 = -32601;
pub const ERR_INVALID_PARAMS: i32 = -32602;
pub const ERR_INTERNAL: i32 = -32603;

/// Application-specific error codes.
pub const ERR_TIMEOUT: i32 = -32001;
pub const ERR_NOT_READY: i32 = -32002;
pub const ERR_WINDOW_NOT_FOUND: i32 = -32003;

/// JSON-RPC 2.0 message envelope.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    /// JSON-RPC version (always "2.0").
    pub jsonrpc: String,
    /// Message ID (present for requests and responses, absent for notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub id: Option<String>,
    /// Method name (present for requests and notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub method: Option<String>,
    /// Parameters (present for requests and notifications).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<serde_json::Value>,
    /// Result (present for success responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<serde_json::Value>,
    /// Error (present for error responses).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<ErrorPayload>,
}

/// JSON-RPC error payload.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ErrorPayload {
    pub code: i32,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
}

impl std::fmt::Display for ErrorPayload {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}", self.code, self.message)
    }
}

impl Message {
    /// Create a request message (with ID, expects response).
    pub fn new_request(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: VERSION.to_string(),
            id: Some(uuid::Uuid::new_v4().to_string()),
            method: Some(method.to_string()),
            params: if params.is_null() { None } else { Some(params) },
            result: None,
            error: None,
        }
    }

    /// Create a notification message (no ID, no response expected).
    pub fn new_notification(method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: VERSION.to_string(),
            id: None,
            method: Some(method.to_string()),
            params: if params.is_null() { None } else { Some(params) },
            result: None,
            error: None,
        }
    }

    /// Create a success response.
    pub fn new_response(id: &str, result: serde_json::Value) -> Self {
        Self {
            jsonrpc: VERSION.to_string(),
            id: Some(id.to_string()),
            method: None,
            params: None,
            result: Some(result),
            error: None,
        }
    }

    /// Create an error response.
    pub fn new_error_response(
        id: &str,
        code: i32,
        message: &str,
        data: Option<serde_json::Value>,
    ) -> Self {
        Self {
            jsonrpc: VERSION.to_string(),
            id: Some(id.to_string()),
            method: None,
            params: None,
            result: None,
            error: Some(ErrorPayload {
                code,
                message: message.to_string(),
                data,
            }),
        }
    }

    /// Check if this is a request (has ID and method).
    pub fn is_request(&self) -> bool {
        self.id.is_some() && self.method.is_some()
    }

    /// Check if this is a notification (no ID but has method).
    pub fn is_notification(&self) -> bool {
        self.id.is_none() && self.method.is_some()
    }

    /// Check if this is a response (has ID, no method).
    pub fn is_response(&self) -> bool {
        self.id.is_some() && self.method.is_none()
    }

    /// Check if this is a success response.
    pub fn is_success_response(&self) -> bool {
        self.is_response() && self.error.is_none()
    }

    /// Check if this is an error response.
    pub fn is_error_response(&self) -> bool {
        self.is_response() && self.error.is_some()
    }

    /// Deserialize params into a typed struct.
    ///
    /// Returns an error if params are absent or deserialization fails.
    pub fn decode_params<T: DeserializeOwned>(&self) -> Result<T, String> {
        self.params
            .as_ref()
            .ok_or_else(|| "message has no params".to_string())
            .and_then(|v| {
                serde_json::from_value::<T>(v.clone())
                    .map_err(|e| format!("params decode error: {}", e))
            })
    }

    /// Deserialize result into a typed struct.
    ///
    /// Returns an error if result is absent or deserialization fails.
    pub fn decode_result<T: DeserializeOwned>(&self) -> Result<T, String> {
        self.result
            .as_ref()
            .ok_or_else(|| "message has no result".to_string())
            .and_then(|v| {
                serde_json::from_value::<T>(v.clone())
                    .map_err(|e| format!("result decode error: {}", e))
            })
    }

    /// Deserialize error.data into a typed struct.
    ///
    /// Returns an error if error or error.data are absent, or if
    /// deserialization fails.
    pub fn decode_error_data<T: DeserializeOwned>(&self) -> Result<T, String> {
        self.error
            .as_ref()
            .ok_or_else(|| "message has no error".to_string())
            .and_then(|err| {
                err.data
                    .as_ref()
                    .ok_or_else(|| "error has no data field".to_string())
            })
            .and_then(|v| {
                serde_json::from_value::<T>(v.clone())
                    .map_err(|e| format!("error.data decode error: {}", e))
            })
    }

    /// Create a request message with a specific ID.
    ///
    /// Unlike `new_request` which generates a random UUID, this allows
    /// callers to set a deterministic ID for request-response correlation.
    pub fn new_request_with_id(id: &str, method: &str, params: serde_json::Value) -> Self {
        Self {
            jsonrpc: VERSION.to_string(),
            id: Some(id.to_string()),
            method: Some(method.to_string()),
            params: if params.is_null() { None } else { Some(params) },
            result: None,
            error: None,
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
