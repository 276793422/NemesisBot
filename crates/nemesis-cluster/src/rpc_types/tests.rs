use super::*;

#[test]
fn test_request_response_serialization() {
    let req = RPCRequest {
        id: "req-1".into(),
        action: ActionType::Known(KnownAction::PeerChat),
        payload: serde_json::json!({"message": "hello"}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: RPCRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.id, "req-1");
    assert_eq!(back.action, ActionType::Known(KnownAction::PeerChat));
}

#[test]
fn test_frame_encode_decode_roundtrip() {
    let payload = b"hello cluster world".to_vec();
    let frame = Frame::new(payload);
    let encoded = frame.encode();

    let (decoded, consumed) = Frame::decode(&encoded).unwrap();
    assert_eq!(decoded.data, b"hello cluster world".to_vec());
    assert_eq!(consumed, encoded.len());
}

#[test]
fn test_frame_decode_partial_buffer() {
    let payload = b"some data".to_vec();
    let frame = Frame::new(payload);
    let encoded = frame.encode();

    // Only provide half the buffer
    let half = &encoded[..encoded.len() / 2];
    assert!(Frame::decode(half).is_none());

    // Empty buffer
    assert!(Frame::decode(&[]).is_none());
}

#[test]
fn test_encode_decode_rpc_request() {
    let req = RPCRequest {
        id: "req-42".into(),
        action: ActionType::Known(KnownAction::ForgeShare),
        payload: serde_json::json!({"artifact": "skill-1"}),
        source: "node-x".into(),
        target: None,
    };

    let encoded = Frame::encode_request(&req).unwrap();
    let (frame, _) = Frame::decode(&encoded).unwrap();

    // encode_request produces WireMessage format; decode_response handles it
    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.id, "req-42");
    // WireMessage wraps the payload as result
    assert_eq!(decoded.result.unwrap()["artifact"], "skill-1");
    assert!(decoded.error.is_none());
}

// -- Additional tests: RPC types edge cases --

#[test]
fn test_action_type_display() {
    assert_eq!(ActionType::Known(KnownAction::PeerChat).to_string(), "PeerChat");
    assert_eq!(ActionType::Known(KnownAction::Ping).to_string(), "Ping");
    assert_eq!(ActionType::Known(KnownAction::Status).to_string(), "Status");
    assert_eq!(ActionType::Known(KnownAction::PeerChatCallback).to_string(), "PeerChatCallback");
    assert_eq!(ActionType::Custom("my_action".into()).to_string(), "my_action");
}

#[test]
fn test_action_type_custom_action_deserialization() {
    let json = r#""some_custom_action""#;
    let action: ActionType = serde_json::from_str(json).unwrap();
    assert_eq!(action, ActionType::Custom("some_custom_action".into()));
    assert_eq!(action.as_str(), "some_custom_action");
}

#[test]
fn test_rpc_response_with_error() {
    let resp = RPCResponse {
        id: "resp-1".into(),
        result: None,
        error: Some("connection refused".into()),
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: RPCResponse = serde_json::from_str(&json).unwrap();
    assert!(back.result.is_none());
    assert_eq!(back.error.as_deref(), Some("connection refused"));
}

#[test]
fn test_rpc_response_with_result() {
    let resp = RPCResponse {
        id: "resp-2".into(),
        result: Some(serde_json::json!({"status": "ok"})),
        error: None,
    };
    let json = serde_json::to_string(&resp).unwrap();
    let back: RPCResponse = serde_json::from_str(&json).unwrap();
    assert!(back.error.is_none());
    assert_eq!(back.result.unwrap()["status"], "ok");
}

#[test]
fn test_frame_encode_decode_response_roundtrip() {
    let resp = RPCResponse {
        id: "resp-99".into(),
        result: Some(serde_json::json!("hello")),
        error: None,
    };
    let encoded = Frame::encode_response(&resp).unwrap();
    let (frame, consumed) = Frame::decode(&encoded).unwrap();
    assert_eq!(consumed, encoded.len());

    let decoded = Frame::decode_response(&frame.data).unwrap();
    assert_eq!(decoded.id, "resp-99");
}

#[test]
fn test_frame_decode_header_only_buffer() {
    // Only 4 bytes (header) but no payload
    let buf = [0u8; 4]; // length = 0
    let result = Frame::decode(&buf);
    assert!(result.is_some());
    let (frame, consumed) = result.unwrap();
    assert_eq!(consumed, 4);
    assert!(frame.data.is_empty());
}

#[test]
fn test_frame_decode_too_short_header() {
    // Less than 4 bytes
    assert!(Frame::decode(&[0, 1, 2]).is_none());
    assert!(Frame::decode(&[0]).is_none());
}

#[test]
fn test_rpc_request_broadcast_target() {
    // Broadcast: target is None
    let req = RPCRequest {
        id: "req-bc".into(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({}),
        source: "node-a".into(),
        target: None,
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: RPCRequest = serde_json::from_str(&json).unwrap();
    assert!(back.target.is_none());
}

#[test]
fn test_rpc_request_targeted() {
    let req = RPCRequest {
        id: "req-targeted".into(),
        action: ActionType::Known(KnownAction::PeerChat),
        payload: serde_json::json!({"message": "hello"}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };
    let json = serde_json::to_string(&req).unwrap();
    let back: RPCRequest = serde_json::from_str(&json).unwrap();
    assert_eq!(back.target.unwrap(), "node-b");
}

// -- Additional tests: decode_request direct, decode_response fallback, WireMessage error --

#[test]
fn test_decode_request_direct() {
    let req = RPCRequest {
        id: "req-direct".into(),
        action: ActionType::Known(KnownAction::Ping),
        payload: serde_json::json!({"ping": "pong"}),
        source: "node-a".into(),
        target: Some("node-b".into()),
    };

    // Serialize to bytes, then decode directly
    let bytes = serde_json::to_vec(&req).unwrap();
    let decoded = Frame::decode_request(&bytes).unwrap();

    assert_eq!(decoded.id, "req-direct");
    assert_eq!(decoded.action, ActionType::Known(KnownAction::Ping));
    assert_eq!(decoded.source, "node-a");
    assert_eq!(decoded.target.unwrap(), "node-b");
    assert_eq!(decoded.payload["ping"], "pong");
}

#[test]
fn test_decode_response_fallback_both_fail() {
    // Completely invalid bytes should fail both WireMessage and direct parsing
    let invalid_bytes = b"this is not valid json at all".to_vec();
    let result = Frame::decode_response(&invalid_bytes);
    assert!(result.is_err(), "expected error for invalid bytes, got {:?}", result);
}

#[test]
fn test_decode_response_wire_message_error_response() {
    // Create a WireMessage with msg_type "error" and an error string
    let wire = crate::transport::conn::WireMessage {
        version: "1.0".into(),
        id: "err-1".into(),
        msg_type: "error".into(),
        from: "node-b".into(),
        to: "node-a".into(),
        action: "peer_chat".into(),
        payload: serde_json::Value::Null,
        timestamp: chrono::Local::now().timestamp(),
        error: "remote node crashed".into(),
    };
    let bytes = serde_json::to_vec(&wire).unwrap();

    let decoded = Frame::decode_response(&bytes).unwrap();
    assert_eq!(decoded.id, "err-1");
    assert!(decoded.result.is_some());
    assert_eq!(decoded.error.as_deref(), Some("remote node crashed"));
}

#[test]
fn test_decode_response_wire_message_success() {
    // Create a WireMessage with no error (empty string)
    let wire = crate::transport::conn::WireMessage {
        version: "1.0".into(),
        id: "wire-ok".into(),
        msg_type: "response".into(),
        from: "node-b".into(),
        to: "node-a".into(),
        action: "ping".into(),
        payload: serde_json::json!({"status": "healthy"}),
        timestamp: chrono::Local::now().timestamp(),
        error: String::new(),
    };
    let bytes = serde_json::to_vec(&wire).unwrap();

    let decoded = Frame::decode_response(&bytes).unwrap();
    assert_eq!(decoded.id, "wire-ok");
    assert_eq!(decoded.result.unwrap()["status"], "healthy");
    assert!(decoded.error.is_none());
}

#[test]
fn test_decode_request_invalid_json() {
    let invalid_bytes = b"not json".to_vec();
    let result = Frame::decode_request(&invalid_bytes);
    assert!(result.is_err());
}
