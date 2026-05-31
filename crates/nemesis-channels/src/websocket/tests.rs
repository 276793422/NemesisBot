use super::*;

// ---------------------------------------------------------------------------
// Protocol serialization tests
// ---------------------------------------------------------------------------

#[test]
fn test_client_message_deserialize() {
    let json = r#"{"type":"message","content":"hello"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type, "message");
    assert_eq!(msg.content, "hello");
}

#[test]
fn test_client_message_ping() {
    let json = r#"{"type":"ping"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type, "ping");
    assert!(msg.content.is_empty());
}

#[test]
fn test_client_message_with_timestamp() {
    let json = r#"{"type":"message","content":"hi","timestamp":"2026-01-01T00:00:00Z"}"#;
    let msg: ClientMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.msg_type, "message");
    assert_eq!(msg.content, "hi");
}

#[test]
fn test_server_message_message() {
    let msg = ServerMessage::message("assistant", "Hello".to_string());
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"message""#));
    assert!(json.contains(r#""role":"assistant""#));
    assert!(json.contains(r#""content":"Hello""#));
    assert!(!json.contains("error"));
    // Timestamp should be ISO 8601 format
    assert!(json.contains("timestamp"));
}

#[test]
fn test_server_message_pong() {
    let msg = ServerMessage::pong();
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"pong""#));
    assert!(!json.contains("role"));
    assert!(!json.contains("content"));
    assert!(!json.contains("error"));
}

#[test]
fn test_server_message_error() {
    let msg = ServerMessage::error_msg("Something went wrong");
    let json = serde_json::to_string(&msg).unwrap();
    assert!(json.contains(r#""type":"error""#));
    assert!(json.contains(r#""error":"Something went wrong""#));
    assert!(!json.contains("role"));
    assert!(!json.contains("content"));
}

#[test]
fn test_server_message_timestamp_iso8601() {
    let msg = ServerMessage::pong();
    let json = serde_json::to_string(&msg).unwrap();
    // Parse out the timestamp value and verify it's ISO 8601
    let val: serde_json::Value = serde_json::from_str(&json).unwrap();
    let ts = val["timestamp"].as_str().unwrap();
    // Should contain 'T' separator (RFC 3339 / ISO 8601)
    assert!(ts.contains('T'), "timestamp should be ISO 8601 format, got: {ts}");
}

// ---------------------------------------------------------------------------
// Config tests
// ---------------------------------------------------------------------------

#[test]
fn test_config_default() {
    let config = WebSocketChannelConfig::default();
    assert!(config.host.is_empty());
    assert_eq!(config.port, 0);
    assert!(config.path.is_empty());
    assert!(config.auth_token.is_empty());
    assert!(config.allow_from.is_empty());
    assert!(config.sync_to.is_empty());
}

#[test]
fn test_config_fields() {
    let config = WebSocketChannelConfig {
        host: "127.0.0.1".to_string(),
        port: 49001,
        path: "/ws".to_string(),
        auth_token: "secret".to_string(),
        allow_from: vec!["user1".to_string()],
        sync_to: vec!["web".to_string()],
    };
    assert_eq!(config.host, "127.0.0.1");
    assert_eq!(config.port, 49001);
    assert_eq!(config.path, "/ws");
    assert_eq!(config.auth_token, "secret");
    assert_eq!(config.allow_from, vec!["user1"]);
    assert_eq!(config.sync_to, vec!["web"]);
}

// ---------------------------------------------------------------------------
// Channel lifecycle tests (no real TCP binding)
// ---------------------------------------------------------------------------

#[test]
fn test_new_channel() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let config = WebSocketChannelConfig::default();
    let ch = WebSocketChannel::new(config, bus_tx);
    assert_eq!(ch.name(), "websocket");
    assert!(!ch.is_running());
}

#[test]
fn test_channel_name() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let config = WebSocketChannelConfig {
        host: "0.0.0.0".to_string(),
        port: 49001,
        path: "/ws".to_string(),
        ..Default::default()
    };
    let ch = WebSocketChannel::new(config, bus_tx);
    assert_eq!(ch.name(), "websocket");
}

// ---------------------------------------------------------------------------
// handle_text_message unit tests
// ---------------------------------------------------------------------------

/// Helper: empty allow list (allow all)
fn empty_allow() -> Vec<String> { vec![] }

/// Helper: specific allow list
fn specific_allow() -> Vec<String> { vec!["websocket:client_allowed".to_string()] }

#[test]
fn test_handle_text_message_ping() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        r#"{"type":"ping"}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    let response = send_rx.try_recv().unwrap();
    assert!(response.contains(r#""type":"pong""#));
}

#[test]
fn test_handle_text_message_empty_content() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        r#"{"type":"message","content":""}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    let response = send_rx.try_recv().unwrap();
    assert!(response.contains(r#""type":"error""#));
    assert!(response.contains("cannot be empty"));
}

#[test]
fn test_handle_text_message_valid() {
    let (bus_tx, mut bus_rx) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, _) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        r#"{"type":"message","content":"hello"}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    let inbound = bus_rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "websocket");
    assert_eq!(inbound.chat_id, "websocket:client_123");
    assert_eq!(inbound.content, "hello");
}

#[test]
fn test_handle_text_message_returns_content() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, _) = tokio::sync::mpsc::unbounded_channel::<String>();

    let result = handle_text_message(
        r#"{"type":"message","content":"sync me"}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    assert_eq!(result, Some("sync me".to_string()));
}

#[test]
fn test_handle_text_message_ping_returns_none() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, _) = tokio::sync::mpsc::unbounded_channel::<String>();

    let result = handle_text_message(
        r#"{"type":"ping"}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    assert_eq!(result, None);
}

#[test]
fn test_handle_text_message_invalid_json() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        "not json",
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    let response = send_rx.try_recv().unwrap();
    assert!(response.contains(r#""type":"error""#));
}

#[test]
fn test_handle_text_message_unknown_type() {
    let (bus_tx, _) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, mut send_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        r#"{"type":"unknown"}"#,
        "client_123",
        &bus_tx,
        &send_tx,
        "websocket",
        &empty_allow(),
    );

    let response = send_rx.try_recv().unwrap();
    assert!(response.contains(r#""type":"error""#));
    assert!(response.contains("Unknown message type"));
}

#[test]
fn test_handle_text_message_allow_list_pass() {
    let (bus_tx, mut bus_rx) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, _) = tokio::sync::mpsc::unbounded_channel::<String>();

    handle_text_message(
        r#"{"type":"message","content":"hello"}"#,
        "client_allowed",
        &bus_tx,
        &send_tx,
        "websocket",
        &specific_allow(),
    );

    let inbound = bus_rx.try_recv().unwrap();
    assert_eq!(inbound.content, "hello");
}

#[test]
fn test_handle_text_message_allow_list_block() {
    let (bus_tx, mut bus_rx) = tokio::sync::broadcast::channel::<InboundMessage>(64);
    let (send_tx, _) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Client not in allow list — message should be silently dropped
    handle_text_message(
        r#"{"type":"message","content":"hello"}"#,
        "client_blocked",
        &bus_tx,
        &send_tx,
        "websocket",
        &specific_allow(),
    );

    // No message published (silently dropped)
    assert!(bus_rx.try_recv().is_err());
}
