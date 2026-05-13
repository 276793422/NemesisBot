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
mod tests {
    use super::*;

    // -----------------------------------------------------------------------
    // Constructor tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_new_announce_populates_all_fields() {
        let msg = DiscoveryMessage::new_announce(
            "node-1",
            "MyNode",
            vec!["10.0.0.1".into(), "192.168.1.1".into()],
            9000,
            "worker",
            "development",
            vec!["gpu".into()],
            vec!["llm".into(), "tools".into()],
        );

        assert_eq!(msg.version, PROTOCOL_VERSION);
        assert_eq!(msg.msg_type, DiscoveryMessageType::Announce);
        assert_eq!(msg.node_id, "node-1");
        assert_eq!(msg.name, "MyNode");
        assert_eq!(msg.addresses, vec!["10.0.0.1", "192.168.1.1"]);
        assert_eq!(msg.rpc_port, 9000);
        assert_eq!(msg.role, "worker");
        assert_eq!(msg.category, "development");
        assert_eq!(msg.tags, vec!["gpu"]);
        assert_eq!(msg.capabilities, vec!["llm", "tools"]);
        assert!(msg.timestamp > 0);
    }

    #[test]
    fn test_new_bye_minimal_fields() {
        let msg = DiscoveryMessage::new_bye("node-2");

        assert_eq!(msg.version, PROTOCOL_VERSION);
        assert_eq!(msg.msg_type, DiscoveryMessageType::Bye);
        assert_eq!(msg.node_id, "node-2");
        assert!(msg.name.is_empty());
        assert!(msg.addresses.is_empty());
        assert_eq!(msg.rpc_port, 0);
        assert!(msg.role.is_empty());
        assert!(msg.category.is_empty());
        assert!(msg.tags.is_empty());
        assert!(msg.capabilities.is_empty());
        assert!(msg.timestamp > 0);
    }

    // -----------------------------------------------------------------------
    // Validate tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_validate_valid_announce() {
        let msg = DiscoveryMessage::new_announce(
            "n1",
            "Name",
            vec!["10.0.0.1".into()],
            9000,
            "worker",
            "dev",
            vec![],
            vec![],
        );
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn test_validate_valid_bye() {
        let msg = DiscoveryMessage::new_bye("n1");
        assert!(msg.validate().is_ok());
    }

    #[test]
    fn test_validate_wrong_version() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        msg.version = "2.0".into();
        let err = msg.validate().unwrap_err();
        assert!(matches!(
            err,
            MessageValidationError::UnsupportedVersion { .. }
        ));
        assert!(err.to_string().contains("2.0"));
    }

    #[test]
    fn test_validate_empty_node_id() {
        let mut msg = DiscoveryMessage::new_bye("n1");
        msg.node_id = String::new();
        let err = msg.validate().unwrap_err();
        assert_eq!(err, MessageValidationError::MissingNodeId);
    }

    #[test]
    fn test_validate_announce_missing_name() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        msg.name = String::new();
        let err = msg.validate().unwrap_err();
        assert_eq!(err, MessageValidationError::MissingName);
    }

    #[test]
    fn test_validate_announce_missing_addresses() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec![], 9000, "worker", "dev", vec![], vec![],
        );
        msg.addresses = Vec::new();
        let err = msg.validate().unwrap_err();
        assert_eq!(err, MessageValidationError::MissingAddresses);
    }

    #[test]
    fn test_validate_announce_zero_rpc_port() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 0, "worker", "dev", vec![], vec![],
        );
        msg.rpc_port = 0;
        let err = msg.validate().unwrap_err();
        assert_eq!(err, MessageValidationError::MissingRpcPort);
    }

    #[test]
    fn test_validate_bye_does_not_require_announce_fields() {
        // A bye message with empty name/addresses/rpc_port is still valid.
        let msg = DiscoveryMessage::new_bye("node-x");
        assert!(msg.validate().is_ok());
    }

    // -----------------------------------------------------------------------
    // IsExpired tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_fresh_message_not_expired() {
        let msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        assert!(!msg.is_expired());
    }

    #[test]
    fn test_old_message_is_expired() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        // Set timestamp to 200 seconds ago — beyond the 120s threshold.
        msg.timestamp = now_unix() - 200;
        assert!(msg.is_expired());
    }

    #[test]
    fn test_boundary_message_not_expired() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        // Exactly 120 seconds old — NOT expired (Go uses strict >).
        msg.timestamp = now_unix() - 120;
        assert!(!msg.is_expired());
    }

    #[test]
    fn test_just_past_boundary_is_expired() {
        let mut msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        // 121 seconds old — expired.
        msg.timestamp = now_unix() - 121;
        assert!(msg.is_expired());
    }

    // -----------------------------------------------------------------------
    // JSON serialization / deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_json_roundtrip_announce() {
        let msg = DiscoveryMessage::new_announce(
            "node-42",
            "TestNode",
            vec!["10.0.0.1".into(), "172.16.0.1".into()],
            8080,
            "manager",
            "testing",
            vec!["tag1".into()],
            vec!["llm".into()],
        );

        let json = serde_json::to_string(&msg).unwrap();
        let back: DiscoveryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn test_json_roundtrip_bye() {
        let msg = DiscoveryMessage::new_bye("node-99");
        let json = serde_json::to_string(&msg).unwrap();
        let back: DiscoveryMessage = serde_json::from_str(&json).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn test_json_field_names_match_go() {
        // Ensure the JSON keys match exactly what Go produces.
        let msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        let json = serde_json::to_string(&msg).unwrap();

        // Verify expected JSON keys are present.
        assert!(json.contains(r#""version":"1.0""#));
        assert!(json.contains(r#""type":"announce""#));
        assert!(json.contains(r#""node_id":"n1""#));
        assert!(json.contains(r#""name":"Name""#));
        assert!(json.contains(r#""addresses":["#));
        assert!(json.contains(r#""rpc_port":9000"#));
        assert!(json.contains(r#""role":"worker""#));
        assert!(json.contains(r#""category":"dev""#));
        assert!(json.contains(r#""tags":[]"#));
        assert!(json.contains(r#""capabilities":[]"#));
        assert!(json.contains(r#""timestamp":"#));
    }

    #[test]
    fn test_to_bytes_from_bytes() {
        let msg = DiscoveryMessage::new_announce(
            "n1", "Name", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        let bytes = msg.to_bytes().unwrap();
        let back = DiscoveryMessage::from_bytes(&bytes).unwrap();
        assert_eq!(back, msg);
    }

    #[test]
    fn test_from_bytes_invalid_json() {
        let result = DiscoveryMessage::from_bytes(b"not json at all");
        assert!(result.is_err());
    }

    #[test]
    fn test_from_bytes_empty_slice() {
        let result = DiscoveryMessage::from_bytes(b"");
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Deserialize Go-compatible JSON
    // -----------------------------------------------------------------------

    #[test]
    fn test_deserialize_from_go_json() {
        // Simulate JSON produced by Go's json.Marshal(DiscoveryMessage).
        let go_json = r#"{
            "version": "1.0",
            "type": "announce",
            "node_id": "go-node-1",
            "name": "GoNode",
            "addresses": ["192.168.1.100"],
            "rpc_port": 9000,
            "role": "worker",
            "category": "development",
            "tags": ["production"],
            "capabilities": ["llm", "tools"],
            "timestamp": 1745900000
        }"#;

        let msg: DiscoveryMessage = serde_json::from_str(go_json).unwrap();
        assert_eq!(msg.version, "1.0");
        assert_eq!(msg.msg_type, DiscoveryMessageType::Announce);
        assert_eq!(msg.node_id, "go-node-1");
        assert_eq!(msg.name, "GoNode");
        assert_eq!(msg.addresses, vec!["192.168.1.100"]);
        assert_eq!(msg.rpc_port, 9000);
        assert_eq!(msg.role, "worker");
        assert_eq!(msg.category, "development");
        assert_eq!(msg.tags, vec!["production"]);
        assert_eq!(msg.capabilities, vec!["llm", "tools"]);
        assert_eq!(msg.timestamp, 1745900000);
    }

    #[test]
    fn test_deserialize_bye_from_go_json() {
        let go_json = r#"{
            "version": "1.0",
            "type": "bye",
            "node_id": "go-node-1",
            "name": "",
            "addresses": [],
            "rpc_port": 0,
            "role": "",
            "category": "",
            "tags": [],
            "capabilities": [],
            "timestamp": 1745900000
        }"#;

        let msg: DiscoveryMessage = serde_json::from_str(go_json).unwrap();
        assert_eq!(msg.msg_type, DiscoveryMessageType::Bye);
        assert_eq!(msg.node_id, "go-node-1");
    }

    // -----------------------------------------------------------------------
    // Display
    // -----------------------------------------------------------------------

    #[test]
    fn test_display_announce() {
        let msg = DiscoveryMessage::new_announce(
            "n1", "MyNode", vec!["10.0.0.1".into()], 9000, "worker", "dev", vec![], vec![],
        );
        let s = msg.to_string();
        assert!(s.contains("type=announce"));
        assert!(s.contains("node_id=n1"));
        assert!(s.contains("name=MyNode"));
        assert!(s.contains("rpc_port=9000"));
        assert!(s.contains("role=worker"));
    }

    #[test]
    fn test_display_bye() {
        let msg = DiscoveryMessage::new_bye("node-42");
        let s = msg.to_string();
        assert!(s.contains("type=bye"));
        assert!(s.contains("node_id=node-42"));
    }

    // -----------------------------------------------------------------------
    // DiscoveryMessageType
    // -----------------------------------------------------------------------

    #[test]
    fn test_message_type_as_str() {
        assert_eq!(DiscoveryMessageType::Announce.as_str(), "announce");
        assert_eq!(DiscoveryMessageType::Bye.as_str(), "bye");
    }

    #[test]
    fn test_message_type_display() {
        assert_eq!(format!("{}", DiscoveryMessageType::Announce), "announce");
        assert_eq!(format!("{}", DiscoveryMessageType::Bye), "bye");
    }

    #[test]
    fn test_message_type_serde_roundtrip() {
        // Announce
        let json = serde_json::to_string(&DiscoveryMessageType::Announce).unwrap();
        assert_eq!(json, r#""announce""#);
        let back: DiscoveryMessageType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DiscoveryMessageType::Announce);

        // Bye
        let json = serde_json::to_string(&DiscoveryMessageType::Bye).unwrap();
        assert_eq!(json, r#""bye""#);
        let back: DiscoveryMessageType = serde_json::from_str(&json).unwrap();
        assert_eq!(back, DiscoveryMessageType::Bye);
    }

    #[test]
    fn test_message_type_rejects_unknown() {
        let result = serde_json::from_str::<DiscoveryMessageType>(r#""unknown""#);
        // serde rename_all is not set; it uses explicit serde(rename) per variant.
        // "unknown" should not match either variant.
        assert!(result.is_err());
    }

    // -----------------------------------------------------------------------
    // Default / missing fields during deserialization
    // -----------------------------------------------------------------------

    #[test]
    fn test_deserialize_with_missing_optional_fields() {
        // Go may omit zero-value fields. Thanks to serde(default), they
        // should come through as empty/zero.
        let json = r#"{
            "version": "1.0",
            "type": "announce",
            "node_id": "n1",
            "timestamp": 1745900000
        }"#;
        let msg: DiscoveryMessage = serde_json::from_str(json).unwrap();
        assert_eq!(msg.version, "1.0");
        assert_eq!(msg.node_id, "n1");
        assert!(msg.name.is_empty());
        assert!(msg.addresses.is_empty());
        assert_eq!(msg.rpc_port, 0);
        assert!(msg.role.is_empty());
        assert!(msg.category.is_empty());
        assert!(msg.tags.is_empty());
        assert!(msg.capabilities.is_empty());
    }

    // -----------------------------------------------------------------------
    // MessageValidationError display
    // -----------------------------------------------------------------------

    #[test]
    fn test_error_display_messages() {
        assert_eq!(
            MessageValidationError::UnsupportedVersion {
                version: "2.0".into()
            }
            .to_string(),
            "unsupported protocol version: 2.0"
        );
        assert_eq!(
            MessageValidationError::MissingNodeId.to_string(),
            "node_id is required"
        );
        assert_eq!(
            MessageValidationError::MissingName.to_string(),
            "name is required for announce"
        );
        assert_eq!(
            MessageValidationError::MissingAddresses.to_string(),
            "addresses is required for announce"
        );
        assert_eq!(
            MessageValidationError::MissingRpcPort.to_string(),
            "rpc_port is required for announce"
        );
    }

    // -----------------------------------------------------------------------
    // Protocol version constant
    // -----------------------------------------------------------------------

    #[test]
    fn test_protocol_version_value() {
        assert_eq!(PROTOCOL_VERSION, "1.0");
    }
}
