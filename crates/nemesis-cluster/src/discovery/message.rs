//! Discovery message types.
//!
//! Defines the Announce/Bye message format used for UDP multicast discovery.
//! Matches the Go discovery.MessageType / DiscoveryMessage wire format exactly.

use serde::{Deserialize, Serialize};
use std::fmt;
use std::time::{SystemTime, UNIX_EPOCH};

/// Discovery protocol version. Must match the Go constant `ProtocolVersion`.
pub const PROTOCOL_VERSION: &str = "1.0";

/// Expiry threshold in seconds. Messages older than this are considered stale.
///
/// NOTE: The 120-second threshold is intentionally hardcoded for LAN scenarios
/// (RTT <1ms, clock skew <1s). This is NOT a bug — do not change to configurable
/// unless deploying in cross-subnet/high-latency networks.
const EXPIRY_THRESHOLD_SECS: i64 = 120;

// ---------------------------------------------------------------------------
// MessageType
// ---------------------------------------------------------------------------

/// Discovery message type, mirroring Go's `MessageType` string enum.
///
/// Serialized as lowercase strings (`"announce"`, `"bye"`) to remain
/// wire-compatible with the Go implementation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub enum DiscoveryMessageType {
    /// A node announcing its presence on the network.
    #[serde(rename = "announce")]
    Announce,
    /// A node announcing its departure from the network.
    #[serde(rename = "bye")]
    Bye,
}

impl DiscoveryMessageType {
    /// Returns the wire string for this variant.
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Announce => "announce",
            Self::Bye => "bye",
        }
    }
}

impl fmt::Display for DiscoveryMessageType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

// ---------------------------------------------------------------------------
// DiscoveryMessage
// ---------------------------------------------------------------------------

/// A discovery message broadcast over UDP multicast.
///
/// Wire format (JSON) matches Go's `DiscoveryMessage` struct exactly so that
/// Rust and Go nodes can coexist on the same multicast group.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiscoveryMessage {
    /// Protocol version (must be `"1.0"`).
    pub version: String,
    /// Message type.
    #[serde(rename = "type")]
    pub msg_type: DiscoveryMessageType,
    /// Unique node identifier.
    pub node_id: String,
    /// Human-readable node name.
    #[serde(default)]
    pub name: String,
    /// List of IP addresses (multiple NICs support).
    #[serde(default)]
    pub addresses: Vec<String>,
    /// RPC port number.
    #[serde(default)]
    pub rpc_port: u16,
    /// Cluster role: manager, coordinator, worker, observer, standby.
    #[serde(default)]
    pub role: String,
    /// Business category: design, development, testing, etc.
    #[serde(default)]
    pub category: String,
    /// Custom tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// List of capabilities (e.g. "llm", "tools").
    #[serde(default)]
    pub capabilities: Vec<String>,
    /// Node type: "agent" (full with LLM) or "node" (lightweight, no LLM).
    #[serde(default)]
    pub node_type: String,
    /// Unix timestamp (seconds since epoch).
    pub timestamp: i64,
}

// ---------------------------------------------------------------------------
// Constructors
// ---------------------------------------------------------------------------

impl DiscoveryMessage {
    /// Create a new announce message, mirroring Go's `NewAnnounceMessage`.
    pub fn new_announce(
        node_id: impl Into<String>,
        name: impl Into<String>,
        addresses: Vec<String>,
        rpc_port: u16,
        role: impl Into<String>,
        category: impl Into<String>,
        tags: Vec<String>,
        capabilities: Vec<String>,
        node_type: impl Into<String>,
    ) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            msg_type: DiscoveryMessageType::Announce,
            node_id: node_id.into(),
            name: name.into(),
            addresses,
            rpc_port,
            role: role.into(),
            category: category.into(),
            tags,
            capabilities,
            node_type: node_type.into(),
            timestamp: now_unix(),
        }
    }

    /// Create a new bye message, mirroring Go's `NewByeMessage`.
    pub fn new_bye(node_id: impl Into<String>) -> Self {
        Self {
            version: PROTOCOL_VERSION.to_string(),
            msg_type: DiscoveryMessageType::Bye,
            node_id: node_id.into(),
            name: String::new(),
            addresses: Vec::new(),
            rpc_port: 0,
            role: String::new(),
            category: String::new(),
            tags: Vec::new(),
            capabilities: Vec::new(),
            node_type: String::new(),
            timestamp: now_unix(),
        }
    }
}

// ---------------------------------------------------------------------------
// Validation / expiry / serialization
// ---------------------------------------------------------------------------

impl DiscoveryMessage {
    /// Validate the message, mirroring Go's `DiscoveryMessage.Validate()`.
    ///
    /// Checks:
    /// - `version` must be `"1.0"`
    /// - `node_id` must not be empty
    /// - For `Announce` messages: `name`, `addresses` (non-empty), and `rpc_port` (>0)
    ///   are required.
    pub fn validate(&self) -> Result<(), MessageValidationError> {
        if self.version != PROTOCOL_VERSION {
            return Err(MessageValidationError::UnsupportedVersion {
                version: self.version.clone(),
            });
        }

        if self.node_id.is_empty() {
            return Err(MessageValidationError::MissingNodeId);
        }

        if self.msg_type == DiscoveryMessageType::Announce {
            if self.name.is_empty() {
                return Err(MessageValidationError::MissingName);
            }
            if self.addresses.is_empty() {
                return Err(MessageValidationError::MissingAddresses);
            }
            if self.rpc_port == 0 {
                return Err(MessageValidationError::MissingRpcPort);
            }
        }

        Ok(())
    }

    /// Check whether the message has expired (timestamp older than 120 seconds).
    ///
    /// Mirrors Go's `DiscoveryMessage.IsExpired()`.
    pub fn is_expired(&self) -> bool {
        now_unix() - self.timestamp > EXPIRY_THRESHOLD_SECS
    }

    /// Serialize the message to JSON bytes, mirroring Go's `DiscoveryMessage.Bytes()`.
    pub fn to_bytes(&self) -> Result<Vec<u8>, serde_json::Error> {
        serde_json::to_vec(self)
    }

    /// Deserialize a discovery message from JSON bytes.
    pub fn from_bytes(data: &[u8]) -> Result<Self, serde_json::Error> {
        serde_json::from_slice(data)
    }
}

// ---------------------------------------------------------------------------
// Display
// ---------------------------------------------------------------------------

impl fmt::Display for DiscoveryMessage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "DiscoveryMessage{{type={}, node_id={}, name={}, addresses={:?}, rpc_port={}, \
             role={}, category={}, tags={:?}, caps={:?}}}",
            self.msg_type,
            self.node_id,
            self.name,
            self.addresses,
            self.rpc_port,
            self.role,
            self.category,
            self.tags,
            self.capabilities,
        )
    }
}

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by [`DiscoveryMessage::validate()`].
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MessageValidationError {
    #[error("unsupported protocol version: {version}")]
    UnsupportedVersion { version: String },
    #[error("node_id is required")]
    MissingNodeId,
    #[error("name is required for announce")]
    MissingName,
    #[error("addresses is required for announce")]
    MissingAddresses,
    #[error("rpc_port is required for announce")]
    MissingRpcPort,
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Return the current Unix timestamp in seconds.
fn now_unix() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests;
