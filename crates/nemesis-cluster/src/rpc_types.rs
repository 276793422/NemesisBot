//! RPC protocol types and message framing.
//!
//! Defines the wire format for cluster RPC communication: request/response
//! types, action enums, and length-prefixed binary framing.

use serde::{Deserialize, Serialize};

/// RPC action types that can be performed.
///
/// Serialized as a flat string (e.g. `"Ping"`, `"query_task_result"`) to
/// match Go's `string` action field.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ActionType {
    /// Well-known action types.
    Known(KnownAction),
    /// Custom action type (e.g. "query_task_result", "confirm_task_delivery").
    Custom(String),
}

/// Well-known RPC action types.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KnownAction {
    PeerChat,
    PeerChatCallback,
    ForgeShare,
    Ping,
    Status,
}

impl ActionType {
    /// Get the string representation of this action.
    pub fn as_str(&self) -> &str {
        match self {
            ActionType::Known(KnownAction::PeerChat) => "PeerChat",
            ActionType::Known(KnownAction::PeerChatCallback) => "PeerChatCallback",
            ActionType::Known(KnownAction::ForgeShare) => "ForgeShare",
            ActionType::Known(KnownAction::Ping) => "Ping",
            ActionType::Known(KnownAction::Status) => "Status",
            ActionType::Custom(s) => s.as_str(),
        }
    }
}

impl std::fmt::Display for ActionType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

impl Serialize for ActionType {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ActionType {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let s = String::deserialize(deserializer)?;
        Ok(match s.as_str() {
            "PeerChat" => ActionType::Known(KnownAction::PeerChat),
            "PeerChatCallback" => ActionType::Known(KnownAction::PeerChatCallback),
            "ForgeShare" => ActionType::Known(KnownAction::ForgeShare),
            "Ping" => ActionType::Known(KnownAction::Ping),
            "Status" => ActionType::Known(KnownAction::Status),
            other => ActionType::Custom(other.to_string()),
        })
    }
}

/// An RPC request sent from one node to another.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RPCRequest {
    /// Unique request ID for correlating responses.
    pub id: String,
    /// The action to perform.
    pub action: ActionType,
    /// Request payload (JSON).
    pub payload: serde_json::Value,
    /// Source node ID.
    pub source: String,
    /// Target node ID (None = broadcast).
    pub target: Option<String>,
}

/// An RPC response sent back to the requester.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RPCResponse {
    /// Matches the request ID.
    pub id: String,
    /// Result payload if successful.
    pub result: Option<serde_json::Value>,
    /// Error message if failed.
    pub error: Option<String>,
}

/// Wire frame for length-prefixed binary messages.
#[derive(Debug, Clone)]
pub struct Frame {
    /// Raw payload bytes.
    pub data: Vec<u8>,
}

impl Frame {
    /// The frame header size: 4 bytes (u32 big-endian) for length.
    pub const HEADER_SIZE: usize = 4;

    /// Create a frame from raw bytes.
    pub fn new(data: Vec<u8>) -> Self {
        Self { data }
    }

    /// Encode the frame as length-prefixed bytes.
    ///
    /// Format: `[4-byte big-endian length][payload]`
    pub fn encode(&self) -> Vec<u8> {
        let len = self.data.len() as u32;
        let mut buf = Vec::with_capacity(Self::HEADER_SIZE + self.data.len());
        buf.extend_from_slice(&len.to_be_bytes());
        buf.extend_from_slice(&self.data);
        buf
    }

    /// Decode a frame from length-prefixed bytes.
    ///
    /// Returns the frame and the number of bytes consumed, or None if
    /// the buffer does not contain a complete frame.
    pub fn decode(buf: &[u8]) -> Option<(Frame, usize)> {
        if buf.len() < Self::HEADER_SIZE {
            return None;
        }
        let len = u32::from_be_bytes([buf[0], buf[1], buf[2], buf[3]]) as usize;
        let total = Self::HEADER_SIZE + len;
        if buf.len() < total {
            return None;
        }
        let data = buf[Self::HEADER_SIZE..total].to_vec();
        Some((Frame { data }, total))
    }

    /// Encode an RPC request as a framed binary message (WireMessage format).
    pub fn encode_request(req: &RPCRequest) -> std::io::Result<Vec<u8>> {
        // Convert RPCRequest to WireMessage format expected by the server
        let action_str = match &req.action {
            ActionType::Known(k) => match k {
                KnownAction::PeerChat => "peer_chat",
                KnownAction::PeerChatCallback => "peer_chat_callback",
                KnownAction::ForgeShare => "forge_share",
                KnownAction::Ping => "ping",
                KnownAction::Status => "status",
            },
            ActionType::Custom(s) => s.as_str(),
        };
        let wire = crate::transport::conn::WireMessage {
            version: "1.0".into(),
            id: req.id.clone(),
            msg_type: "request".into(),
            from: req.source.clone(),
            to: req.target.clone().unwrap_or_default(),
            action: action_str.into(),
            payload: req.payload.clone(),
            timestamp: chrono::Utc::now().timestamp(),
            error: String::new(),
        };
        let payload = serde_json::to_vec(&wire).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        Ok(Frame::new(payload).encode())
    }

    /// Encode an RPC response as a framed binary message.
    pub fn encode_response(resp: &RPCResponse) -> std::io::Result<Vec<u8>> {
        let payload = serde_json::to_vec(resp).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        Ok(Frame::new(payload).encode())
    }

    /// Decode an RPC request from a framed binary message.
    pub fn decode_request(data: &[u8]) -> std::io::Result<RPCRequest> {
        serde_json::from_slice(data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }

    /// Decode an RPC response from a framed binary message (WireMessage format).
    pub fn decode_response(data: &[u8]) -> std::io::Result<RPCResponse> {
        // Try WireMessage format first (from TcpConn server)
        if let Ok(wire) = serde_json::from_slice::<crate::transport::conn::WireMessage>(data) {
            let err = if wire.error.is_empty() { None } else { Some(wire.error) };
            return Ok(RPCResponse {
                id: wire.id,
                result: Some(wire.payload),
                error: err,
            });
        }
        // Fallback: try direct RPCResponse format
        serde_json::from_slice(data).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }
}

#[cfg(test)]
mod tests;
