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
        "agent",
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
        "agent",
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
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "n1",
        "",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    msg.name = String::new();
    let err = msg.validate().unwrap_err();
    assert_eq!(err, MessageValidationError::MissingName);
}

#[test]
fn test_validate_announce_missing_addresses() {
    let mut msg = DiscoveryMessage::new_announce(
        "n1",
        "Name",
        vec![],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    msg.addresses = Vec::new();
    let err = msg.validate().unwrap_err();
    assert_eq!(err, MessageValidationError::MissingAddresses);
}

#[test]
fn test_validate_announce_zero_rpc_port() {
    let mut msg = DiscoveryMessage::new_announce(
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        0,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    assert!(!msg.is_expired());
}

#[test]
fn test_old_message_is_expired() {
    let mut msg = DiscoveryMessage::new_announce(
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    // Set timestamp to 200 seconds ago — beyond the 120s threshold.
    msg.timestamp = now_unix() - 200;
    assert!(msg.is_expired());
}

#[test]
fn test_boundary_message_not_expired() {
    let mut msg = DiscoveryMessage::new_announce(
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
    );
    // Exactly 120 seconds old — NOT expired (Go uses strict >).
    msg.timestamp = now_unix() - 120;
    assert!(!msg.is_expired());
}

#[test]
fn test_just_past_boundary_is_expired() {
    let mut msg = DiscoveryMessage::new_announce(
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "agent",
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
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "n1",
        "Name",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
        "n1",
        "MyNode",
        vec!["10.0.0.1".into()],
        9000,
        "worker",
        "dev",
        vec![],
        vec![],
        "agent",
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
