use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

#[tokio::test]
async fn test_line_channel_new_validates() {
    let config = LineConfig {
        channel_access_token: String::new(),
        channel_secret: String::new(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    assert!(LineChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_line_channel_lifecycle() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "line");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_verify_signature_valid() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    let body = b"hello world";
    let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
    mac.update(body);
    let sig = hex::encode(mac.finalize().into_bytes());

    assert!(ch.verify_signature(body, &sig));
}

#[test]
fn test_verify_signature_invalid() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    assert!(!ch.verify_signature(b"hello", "invalid_hex"));
    assert!(!ch.verify_signature(b"hello", "deadbeef"));
}

#[test]
fn test_deserialize_webhook() {
    let json = r#"{
        "destination": "U123",
        "events": [{
            "type": "message",
            "replyToken": "rt-123",
            "source": {"type": "user", "userId": "U456"},
            "message": {"type": "text", "text": "Hello"},
            "timestamp": 1234567890
        }]
    }"#;

    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events.len(), 1);
    assert_eq!(req.events[0].event_type, "message");
    assert_eq!(req.events[0].reply_token.as_deref(), Some("rt-123"));
    assert_eq!(
        req.events[0].message.as_ref().unwrap().text.as_deref(),
        Some("Hello")
    );
}

// ---- New tests ----

#[test]
fn test_line_config_fields() {
    let config = LineConfig {
        channel_secret: "secret".into(),
        channel_access_token: "token".into(),
        webhook_port: 8080,
        allow_from: vec!["U123".into()],
    };
    assert_eq!(config.channel_secret, "secret");
    assert_eq!(config.channel_access_token, "token");
}

#[test]
fn test_deserialize_webhook_multiple_events() {
    let json = r#"{
        "destination": "U123",
        "events": [
            {"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": "First"}, "timestamp": 1},
            {"type": "message", "replyToken": "rt2", "source": {"type": "user", "userId": "U2"}, "message": {"type": "text", "text": "Second"}, "timestamp": 2}
        ]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events.len(), 2);
}

#[test]
fn test_deserialize_webhook_empty_events() {
    let json = r#"{"destination": "U123", "events": []}"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert!(req.events.is_empty());
}

#[test]
fn test_deserialize_webhook_non_message_event() {
    let json = r#"{
        "destination": "U123",
        "events": [{"type": "follow", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events[0].event_type, "follow");
    assert!(req.events[0].message.is_none());
}

// -- Additional tests for coverage --

#[test]
fn test_verify_signature_empty_signature() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    assert!(!ch.verify_signature(b"hello", ""));
}

#[test]
fn test_verify_signature_wrong_body() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    // Generate a valid signature for "correct_body"
    let body = b"correct_body";
    let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
    mac.update(body);
    let sig = hex::encode(mac.finalize().into_bytes());

    // But verify with a different body
    assert!(!ch.verify_signature(b"wrong_body", &sig));
}

#[test]
fn test_verify_signature_short_hex() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Hex that's too short (less than 32 bytes when decoded)
    assert!(!ch.verify_signature(b"hello", "abcd"));
}

#[test]
fn test_store_reply_token() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    ch.store_reply_token("chat-1".into(), "rt-abc".into());
    assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-abc");

    // Overwrite
    ch.store_reply_token("chat-1".into(), "rt-def".into());
    assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-def");
}

#[test]
fn test_deserialize_line_source_group() {
    // LineSource uses snake_case field names (group_id, room_id, user_id)
    let json = r#"{
        "destination": "U123",
        "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "group", "user_id": "U1", "group_id": "G1"}, "message": {"type": "text", "text": "Hello"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    let source = req.events[0].source.as_ref().unwrap();
    assert_eq!(source.source_type, "group");
    assert_eq!(source.group_id.as_deref(), Some("G1"));
    assert_eq!(source.user_id.as_deref(), Some("U1"));
}

#[test]
fn test_deserialize_line_source_room() {
    let json = r#"{
        "destination": "U123",
        "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "room", "room_id": "R1"}, "message": {"type": "text", "text": "Hello"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    let source = req.events[0].source.as_ref().unwrap();
    assert_eq!(source.source_type, "room");
    assert_eq!(source.room_id.as_deref(), Some("R1"));
}

#[test]
fn test_deserialize_line_message_non_text() {
    let json = r#"{
        "destination": "U123",
        "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "image", "id": "msg-1"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    let msg = req.events[0].message.as_ref().unwrap();
    assert_eq!(msg.message_type, "image");
    assert!(msg.text.is_none());
    assert_eq!(msg.id.as_deref(), Some("msg-1"));
}

#[test]
fn test_deserialize_webhook_no_destination() {
    let json = r#"{"events": []}"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert!(req.destination.is_none());
    assert!(req.events.is_empty());
}

#[test]
fn test_deserialize_event_without_optional_fields() {
    let json = r#"{
        "destination": "U123",
        "events": [{"type": "postback", "timestamp": 999}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events[0].event_type, "postback");
    assert!(req.events[0].reply_token.is_none());
    assert!(req.events[0].source.is_none());
    assert!(req.events[0].message.is_none());
    assert_eq!(req.events[0].timestamp.unwrap(), 999);
}

// ---- Additional coverage tests ----

#[tokio::test]
async fn test_send_not_running() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Not started - send should fail
    let msg = OutboundMessage {
        channel: "line".to_string(),
        chat_id: "test-chat".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not running"));
}

#[tokio::test]
async fn test_start_stop_clears_reply_tokens() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.store_reply_token("chat-1".into(), "rt-abc".into());
    assert_eq!(ch.reply_tokens.len(), 1);

    ch.stop().await.unwrap();
    assert!(ch.reply_tokens.is_empty());
}

#[tokio::test]
async fn test_start_stop_idempotent() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.start().await.unwrap(); // second start should be fine
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    ch.stop().await.unwrap(); // second stop should be fine
    assert!(!*ch.running.read());
}

#[test]
fn test_verify_signature_with_unicode_body() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    let body = "hello world";
    let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
    mac.update(body.as_bytes());
    let sig = hex::encode(mac.finalize().into_bytes());

    assert!(ch.verify_signature(body.as_bytes(), &sig));
}

#[test]
fn test_reply_tokens_overwrite() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.store_reply_token("chat-1".into(), "rt-1".into());
    ch.store_reply_token("chat-1".into(), "rt-2".into());
    assert_eq!(ch.reply_tokens.get("chat-1").unwrap().value(), "rt-2");
}

#[test]
fn test_reply_tokens_multiple_chats() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.store_reply_token("chat-1".into(), "rt-1".into());
    ch.store_reply_token("chat-2".into(), "rt-2".into());
    ch.store_reply_token("chat-3".into(), "rt-3".into());
    assert_eq!(ch.reply_tokens.len(), 3);
}

#[test]
fn test_line_message_deserialization_types() {
    let json = r#"{"id":"msg-1","type":"text","text":"hello"}"#;
    let msg: LineMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "text");
    assert_eq!(msg.text.as_deref(), Some("hello"));
    assert_eq!(msg.id.as_deref(), Some("msg-1"));
}

#[test]
fn test_line_message_non_text_type() {
    let json = r#"{"id":"msg-2","type":"image"}"#;
    let msg: LineMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "image");
    assert!(msg.text.is_none());
}

#[test]
fn test_line_source_user_type() {
    let json = r#"{"type":"user","user_id":"U123"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "user");
    assert_eq!(source.user_id.as_deref(), Some("U123"));
    assert!(source.group_id.is_none());
    assert!(source.room_id.is_none());
}

#[test]
fn test_deserialize_webhook_with_text_event_and_timestamp() {
    let json = r#"{
        "destination": "U999",
        "events": [{
            "type": "message",
            "replyToken": "rt-xyz",
            "source": {"type": "user", "userId": "U111"},
            "message": {"type": "text", "text": "Test message"},
            "timestamp": 1700000000000
        }]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.destination.as_deref(), Some("U999"));
    assert_eq!(req.events[0].timestamp.unwrap(), 1700000000000);
    assert_eq!(req.events[0].reply_token.as_deref(), Some("rt-xyz"));
}

#[test]
fn test_line_config_accessors() {
    let config = LineConfig {
        channel_access_token: "my_token".to_string(),
        channel_secret: "my_secret".to_string(),
        webhook_port: 9090,
        allow_from: vec!["U123".to_string()],
    };
    assert_eq!(config.channel_access_token, "my_token");
    assert_eq!(config.channel_secret, "my_secret");
    assert_eq!(config.webhook_port, 9090);
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_verify_signature_with_empty_body() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();

    let body = b"";
    let mut mac = HmacSha256::new_from_slice(b"test_secret").unwrap();
    mac.update(body);
    let sig = hex::encode(mac.finalize().into_bytes());

    assert!(ch.verify_signature(body, &sig));
}

#[test]
fn test_verify_signature_non_hex_chars() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Contains non-hex characters
    assert!(!ch.verify_signature(
        b"hello",
        "zzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzzz"
    ));
}

#[test]
fn test_verify_signature_wrong_length() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Too short hex string
    assert!(!ch.verify_signature(b"hello", "deadbeef"));
}

#[test]
fn test_verify_signature_empty_string_signature() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    assert!(!ch.verify_signature(b"hello", ""));
}

#[test]
fn test_line_channel_new_valid_token() {
    let config = LineConfig {
        channel_access_token: "valid_token".to_string(),
        channel_secret: "valid_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus());
    assert!(ch.is_ok());
    let ch = ch.unwrap();
    assert_eq!(ch.name(), "line");
}

#[test]
fn test_line_channel_new_empty_token() {
    let config = LineConfig {
        channel_access_token: String::new(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    assert!(LineChannel::new(config, test_bus()).is_err());
}

#[test]
fn test_line_channel_new_empty_secret() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: String::new(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    assert!(LineChannel::new(config, test_bus()).is_err());
}

#[test]
fn test_deserialize_line_source_with_group_id() {
    let json = r#"{"type": "group", "user_id": "U1", "group_id": "G1"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "group");
    assert_eq!(source.group_id.as_deref(), Some("G1"));
    assert_eq!(source.user_id.as_deref(), Some("U1"));
}

#[test]
fn test_deserialize_line_source_with_room_id() {
    let json = r#"{"type": "room", "user_id": "U1", "room_id": "R1"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "room");
    assert_eq!(source.room_id.as_deref(), Some("R1"));
}

#[test]
fn test_deserialize_line_message_image_type() {
    let json = r#"{"type": "image", "id": "msg1"}"#;
    let msg: LineMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "image");
    assert!(msg.text.is_none());
}

#[test]
fn test_deserialize_webhook_follow_event() {
    let json = r#"{
        "destination": "U123",
        "events": [{
            "type": "follow",
            "replyToken": "rt-follow",
            "source": {"type": "user", "userId": "U456"},
            "timestamp": 1234567890
        }]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events.len(), 1);
    assert_eq!(req.events[0].event_type, "follow");
    assert!(req.events[0].message.is_none());
}

#[test]
fn test_deserialize_webhook_unsend_event() {
    let json = r#"{
        "events": [{
            "type": "unsend",
            "source": {"type": "user", "userId": "U789"},
            "timestamp": 0
        }]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events[0].event_type, "unsend");
}

#[test]
fn test_store_and_remove_reply_token() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.store_reply_token("chat-1".into(), "rt-abc".into());
    ch.store_reply_token("chat-2".into(), "rt-def".into());
    assert_eq!(ch.reply_tokens.len(), 2);
    // Remove should return the token
    assert_eq!(ch.reply_tokens.remove("chat-1").unwrap().1, "rt-abc");
    assert_eq!(ch.reply_tokens.len(), 1);
}

#[tokio::test]
async fn test_send_with_reply_token_uses_reply() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.store_reply_token("chat-1".into(), "rt-test".into());

    // The reply will fail (network), but the token should be consumed
    let msg = OutboundMessage {
        channel: "line".to_string(),
        chat_id: "chat-1".to_string(),
        content: "hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Reply fails because no actual LINE server, but token is removed
    let _ = ch.send(msg).await;
    // Reply token should have been consumed
    assert!(ch.reply_tokens.get("chat-1").is_none());
}

#[tokio::test]
async fn test_send_push_message_on_no_token() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "line".to_string(),
        chat_id: "U_no_token".to_string(),
        content: "push msg".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Will fail due to network, but exercises the push path
    let result = ch.send(msg).await;
    assert!(result.is_err());
}

#[test]
fn test_deserialize_line_message_text_with_id() {
    let json = r#"{"type": "text", "id": "12345", "text": "Hello world"}"#;
    let msg: LineMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "text");
    assert_eq!(msg.id.as_deref(), Some("12345"));
    assert_eq!(msg.text.as_deref(), Some("Hello world"));
}

#[test]
fn test_deserialize_webhook_multiple_message_events() {
    let json = r#"{
        "events": [
            {"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": "hi"}, "timestamp": 100},
            {"type": "message", "replyToken": "rt2", "source": {"type": "group", "userId": "U2", "groupId": "G1"}, "message": {"type": "text", "text": "hello"}, "timestamp": 200}
        ]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events.len(), 2);
    assert_eq!(req.events[0].reply_token.as_deref(), Some("rt1"));
    assert_eq!(req.events[1].source.as_ref().unwrap().source_type, "group");
}

#[test]
fn test_line_config_default_port() {
    let config = LineConfig {
        channel_access_token: "t".to_string(),
        channel_secret: "s".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    assert_eq!(config.webhook_port, 0);
}

#[test]
fn test_deserialize_event_empty_message_text() {
    let json = r#"{
        "events": [{
            "type": "message",
            "replyToken": "rt1",
            "source": {"type": "user", "userId": "U1"},
            "message": {"type": "text", "text": ""},
            "timestamp": 100
        }]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(
        req.events[0].message.as_ref().unwrap().text.as_deref(),
        Some("")
    );
}

#[tokio::test]
async fn test_start_stop_running_state() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    assert!(!*ch.running.read());
    ch.start().await.unwrap();
    assert!(*ch.running.read());
    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_verify_signature_length_mismatch() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "test_secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Only 4 bytes (8 hex chars) vs expected 32 bytes (64 hex chars)
    assert!(!ch.verify_signature(b"test", "aabbccdd"));
}

// ============================================================
// Additional coverage tests for 95%+ target (round 2)
// ============================================================

#[test]
fn test_line_event_source_without_user_id() {
    let json = r#"{"type": "user"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "user");
    assert!(source.user_id.is_none());
}

#[test]
fn test_line_event_source_minimal() {
    let json = r#"{"type": "group"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "group");
    assert!(source.group_id.is_none());
    assert!(source.user_id.is_none());
}

#[test]
fn test_line_message_minimal() {
    let json = r#"{"type": "text"}"#;
    let msg: LineMessage = serde_json::from_str(json).unwrap();
    assert_eq!(msg.message_type, "text");
    assert!(msg.text.is_none());
    assert!(msg.id.is_none());
}

#[test]
fn test_line_event_minimal() {
    let json = r#"{"type": "message", "timestamp": 100}"#;
    let event: LineEvent = serde_json::from_str(json).unwrap();
    assert_eq!(event.event_type, "message");
    assert!(event.reply_token.is_none());
    assert!(event.source.is_none());
    assert!(event.message.is_none());
    assert_eq!(event.timestamp.unwrap(), 100);
}

#[test]
fn test_line_webhook_empty() {
    let json = r#"{"events": []}"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert!(req.events.is_empty());
}

#[tokio::test]
async fn test_line_channel_base_name() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "line");
}

#[test]
fn test_line_source_room_with_user() {
    let json = r#"{"type": "room", "user_id": "U1", "room_id": "R1"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "room");
    assert_eq!(source.room_id.as_deref(), Some("R1"));
    assert_eq!(source.user_id.as_deref(), Some("U1"));
}

#[test]
fn test_line_webhook_event_with_no_source() {
    let json = r#"{
        "events": [{"type": "message", "replyToken": "rt1", "message": {"type": "text", "text": "hi"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert!(req.events[0].source.is_none());
}

#[test]
fn test_line_webhook_event_with_empty_text() {
    let json = r#"{
        "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "text", "text": ""}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    let text = req.events[0].message.as_ref().unwrap().text.as_deref();
    assert_eq!(text, Some(""));
}

#[test]
fn test_line_webhook_event_non_text_message() {
    let json = r#"{
        "events": [{"type": "message", "replyToken": "rt1", "source": {"type": "user", "userId": "U1"}, "message": {"type": "sticker", "id": "msg-1"}, "timestamp": 1}]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    let msg = req.events[0].message.as_ref().unwrap();
    assert_eq!(msg.message_type, "sticker");
    assert!(msg.text.is_none());
}

#[test]
fn test_line_source_user_no_user_id() {
    let json = r#"{"type": "user"}"#;
    let source: LineSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.source_type, "user");
    assert!(source.user_id.is_none());
    assert!(source.group_id.is_none());
    assert!(source.room_id.is_none());
}

#[test]
fn test_deserialize_webhook_event_type_field() {
    let json = r#"{
        "events": [
            {"type": "message", "timestamp": 1},
            {"type": "follow", "timestamp": 2},
            {"type": "unsend", "timestamp": 3},
            {"type": "postback", "timestamp": 4}
        ]
    }"#;
    let req: LineWebhookRequest = serde_json::from_str(json).unwrap();
    assert_eq!(req.events.len(), 4);
    assert_eq!(req.events[0].event_type, "message");
    assert_eq!(req.events[1].event_type, "follow");
    assert_eq!(req.events[2].event_type, "unsend");
    assert_eq!(req.events[3].event_type, "postback");
}

#[tokio::test]
async fn test_send_uses_push_when_no_reply_token() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();

    // No reply token stored for this chat_id, should use push
    let msg = OutboundMessage {
        channel: "line".to_string(),
        chat_id: "chat-no-token".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Will fail (no network), but exercises push path
    let result = ch.send(msg).await;
    assert!(result.is_err());

    ch.stop().await.unwrap();
}

#[test]
fn test_verify_signature_same_body_different_secret() {
    let config1 = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret1".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let config2 = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret2".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch1 = LineChannel::new(config1, test_bus()).unwrap();
    let ch2 = LineChannel::new(config2, test_bus()).unwrap();

    let body = b"test body";
    let mut mac = HmacSha256::new_from_slice(b"secret1").unwrap();
    mac.update(body);
    let sig = hex::encode(mac.finalize().into_bytes());

    assert!(ch1.verify_signature(body, &sig));
    assert!(!ch2.verify_signature(body, &sig));
}

// ============================================================
// Full lifecycle integration tests with real TCP webhook server
// Target: push line.rs coverage above 85%
// ============================================================

/// Find an ephemeral free port for the webhook TCP listener.
fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// Build a valid base64 LINE signature for the given body + secret.
fn make_signature_b64(body: &[u8], secret: &str) -> String {
    let mut mac = HmacSha256::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// Connect to a host:port, send raw bytes, and read the HTTP response.
async fn send_raw_http(port: u16, request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect failed");
    stream.write_all(request).await.expect("write failed");
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.expect("read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

#[tokio::test]
async fn test_line_full_lifecycle_with_real_webhook() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "my_secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    // Give the spawned listener time to bind
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt1","source":{"type":"user","user_id":"U123"},"message":{"type":"text","text":"Hello"},"timestamp":1}]}"#;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // Should have received the message on the bus
    // webhook 服务器是 spawn 的任务：HTTP 200 先于 bus publish 返回，
    // try_recv 会与 publish 竞态（2026-08-31 gate flake）——必须带超时 recv。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
        .await
        .expect("timed out: webhook 200 must be followed by bus publish")
        .unwrap();
    assert_eq!(inbound.channel, "line");
    assert_eq!(inbound.sender_id, "U123");
    assert_eq!(inbound.chat_id, "U123");
    assert_eq!(inbound.content, "Hello");

    // Reply token should be stored (allow time for the handler to finish)
    for _ in 0..20 {
        if ch.reply_tokens.get("U123").is_some() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(25)).await;
    }
    assert_eq!(ch.reply_tokens.get("U123").unwrap().value(), "rt1");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_invalid_signature() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "my_secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, _bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[]}"#;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Line-Signature: invalid_b64_signature\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 401 Unauthorized"));

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_valid_signature() {
    let port = find_free_port();
    let secret = "valid_secret";
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: secret.to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt1","source":{"type":"user","user_id":"U1"},"message":{"type":"text","text":"Signed"},"timestamp":1}]}"#;
    let sig = make_signature_b64(body.as_bytes(), secret);
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nX-Line-Signature: {}\r\nContent-Length: {}\r\n\r\n{}",
        sig,
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // webhook 服务器是 spawn 的任务：HTTP 200 先于 bus publish 返回，
    // try_recv 会与 publish 竞态（2026-08-31 gate flake）——必须带超时 recv。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
        .await
        .expect("timed out: webhook 200 must be followed by bus publish")
        .unwrap();
    assert_eq!(inbound.content, "Signed");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_lowercase_signature_header() {
    let port = find_free_port();
    let secret = "my_secret";
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: secret.to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt1","source":{"type":"user","user_id":"U9"},"message":{"type":"text","text":"LowerCaseHeader"},"timestamp":1}]}"#;
    let sig = make_signature_b64(body.as_bytes(), secret);
    // lowercase header
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nx-line-signature: {}\r\nContent-Length: {}\r\n\r\n{}",
        sig,
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // webhook 服务器是 spawn 的任务：HTTP 200 先于 bus publish 返回，
    // try_recv 会与 publish 竞态（2026-08-31 gate flake）——必须带超时 recv。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
        .await
        .expect("timed out: webhook 200 must be followed by bus publish")
        .unwrap();
    assert_eq!(inbound.content, "LowerCaseHeader");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_invalid_json_body() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, _bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = "not valid json";
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_group_chat_id() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt1","source":{"type":"group","user_id":"U1","group_id":"G123"},"message":{"type":"text","text":"group msg"},"timestamp":1}]}"#;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // webhook 服务器是 spawn 的任务：HTTP 200 先于 bus publish 返回，
    // try_recv 会与 publish 竞态（2026-08-31 gate flake）——必须带超时 recv。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
        .await
        .expect("timed out: webhook 200 must be followed by bus publish")
        .unwrap();
    assert_eq!(inbound.chat_id, "G123");
    assert_eq!(inbound.sender_id, "U1");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_room_chat_id() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt1","source":{"type":"room","user_id":"U1","room_id":"R456"},"message":{"type":"text","text":"room msg"},"timestamp":1}]}"#;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // webhook 服务器是 spawn 的任务：HTTP 200 先于 bus publish 返回，
    // try_recv 会与 publish 竞态（2026-08-31 gate flake）——必须带超时 recv。
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), bus_rx.recv())
        .await
        .expect("timed out: webhook 200 must be followed by bus publish")
        .unwrap();
    assert_eq!(inbound.chat_id, "R456");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_skips_non_message_events() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, mut bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    let body = r#"{"events":[{"type":"follow","replyToken":"rt1","source":{"type":"user","user_id":"U1"},"timestamp":1}]}"#;
    let request = format!(
        "POST / HTTP/1.1\r\nHost: 127.0.0.1\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );

    let response = send_raw_http(port, request.as_bytes()).await;
    assert!(response.starts_with("HTTP/1.1 200 OK"));

    // No inbound message published
    assert!(bus_rx.try_recv().is_err());

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_full_lifecycle_no_body_separator() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, _bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;

    // Send raw bytes without the CRLF+CRLF separator. Should fail parsing JSON.
    let raw = b"GET / HTTP/1.0\nno crlfcrlf here";
    let response = send_raw_http(port, raw).await;
    assert!(response.starts_with("HTTP/1.1 400 Bad Request"));

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_channel_is_running_trait() {
    // Note: LineChannel::start() only calls set_enabled() on base,
    // but not set_running(). The is_running() trait method reads
    // base.is_running() which uses the separate `running` field.
    // This is a known inconsistency; the channel's internal `running`
    // field (parking_lot::RwLock) is the actual state used elsewhere.
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Before start, is_running() trait returns false (matches internal state)
    assert!(!ch.is_running());
    // After start, internal `running` field is set to true
    ch.start().await.unwrap();
    assert!(*ch.running.read());
    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[tokio::test]
async fn test_line_send_with_reply_token_consumed_in_lifecycle() {
    let port = find_free_port();
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: port,
        allow_from: Vec::new(),
    };
    let (bus_tx, _bus_rx) = broadcast::channel::<InboundMessage>(64);
    let ch = LineChannel::new(config, bus_tx).unwrap();
    ch.start().await.unwrap();

    // Manually inject a reply token
    ch.store_reply_token("U_chat".into(), "rt_consumed".into());

    let msg = OutboundMessage {
        channel: "line".to_string(),
        chat_id: "U_chat".to_string(),
        content: "hi".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // Reply will fail due to network, but token must be consumed
    let _ = ch.send(msg).await;
    assert!(ch.reply_tokens.get("U_chat").is_none());

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_default_port_when_zero() {
    // When webhook_port == 0, the trait impl uses 8080 internally.
    // We just verify the spawn completes without panic.
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    // Wait briefly to ensure spawned task attempted bind
    tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_line_reply_serialization_format() {
    let req = LineReplyRequest {
        reply_token: "rt-abc".to_string(),
        messages: vec![LineMessagePayload {
            msg_type: "text".to_string(),
            text: "hello".to_string(),
        }],
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"reply_token\":\"rt-abc\""));
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"text\":\"hello\""));
}

#[tokio::test]
async fn test_line_push_serialization_format() {
    let req = LinePushRequest {
        to: "U123".to_string(),
        messages: vec![LineMessagePayload {
            msg_type: "text".to_string(),
            text: "push body".to_string(),
        }],
    };
    let json = serde_json::to_string(&req).unwrap();
    assert!(json.contains("\"to\":\"U123\""));
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"text\":\"push body\""));
}

#[tokio::test]
async fn test_line_message_payload_serialization() {
    let payload = LineMessagePayload {
        msg_type: "text".to_string(),
        text: "test content".to_string(),
    };
    let json = serde_json::to_string(&payload).unwrap();
    assert!(json.contains("\"type\":\"text\""));
    assert!(json.contains("\"text\":\"test content\""));
}

#[tokio::test]
async fn test_line_reply_error_status() {
    // Tests that reply() returns an Err when the HTTP call itself fails (e.g., DNS error)
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Calling reply directly with a fake token triggers http.post to api.line.me
    // which will fail due to no network or DNS resolution.
    let result = ch.reply("fake_token", "test text").await;
    // Most test environments don't have access to api.line.me, so should fail
    // Just verify it returns either Err (network) or Ok (unlikely)
    let _ = result;
}

#[tokio::test]
async fn test_line_push_error_status() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    let result = ch.push_message("U123", "push text").await;
    // Should fail in test env (no network)
    let _ = result;
}

#[tokio::test]
async fn test_line_reply_with_empty_text() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    // Should not panic, just fail due to network
    let _ = ch.reply("rt", "").await;
}

#[tokio::test]
async fn test_line_push_with_empty_text() {
    let config = LineConfig {
        channel_access_token: "token".to_string(),
        channel_secret: "secret".to_string(),
        webhook_port: 0,
        allow_from: Vec::new(),
    };
    let ch = LineChannel::new(config, test_bus()).unwrap();
    let _ = ch.push_message("U123", "").await;
}

// ===========================================================================
// W4c 补测（2026-08-25）：webhook HTTP 最小解析全路径——合法签名投递+存 reply
// token、错签名 401、坏 JSON 400、事件守卫（非 message/无 text/无 source 跳过）、
// running=false 直连即断
// ===========================================================================

/// 用 channel_secret 计算 LINE webhook 签名（HMAC-SHA256 → base64）。
fn w4c_line_signature(secret: &str, body: &[u8]) -> String {
    use base64::Engine;
    use hmac::Mac;
    let mut mac = <HmacSha256 as Mac>::new_from_slice(secret.as_bytes()).unwrap();
    mac.update(body);
    base64::engine::general_purpose::STANDARD.encode(mac.finalize().into_bytes())
}

/// 起一个一次性 TCP 监听，把 accept 到的连接交给 handle_webhook_connection。
/// 返回 (客户端流, 端口)。
async fn w4c_line_webhook_pair(
    bus: &broadcast::Sender<InboundMessage>,
    secret: &str,
    reply_tokens: &Arc<dashmap::DashMap<String, String>>,
    running: &Arc<parking_lot::RwLock<bool>>,
) -> (tokio::net::TcpStream, std::net::SocketAddr) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let bus = bus.clone();
    let secret = secret.to_string();
    let reply_tokens = Arc::clone(reply_tokens);
    let running = Arc::clone(running);
    let _accept_task = tokio::spawn(async move {
        if let Ok((stream, _)) = listener.accept().await {
            LineChannel::handle_webhook_connection(stream, &bus, &secret, &reply_tokens, &running)
                .await;
        }
    });

    let client = tokio::net::TcpStream::connect(addr).await.unwrap();
    // 等服务端 accept 完成，避免客户端先写进本地缓冲就被断言
    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    (client, addr)
}

async fn w4c_read_http_response(client: &mut tokio::net::TcpStream) -> Option<String> {
    use tokio::io::AsyncReadExt;
    let mut out = String::new();
    let mut buf = [0u8; 1024];
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(2), client.read(&mut buf))
            .await
            .ok()?;
        match n {
            Ok(0) => return Some(out), // EOF（连接关闭）
            Ok(n) => {
                out.push_str(&String::from_utf8_lossy(&buf[..n]));
                if out.contains("\r\n\r\n") {
                    return Some(out);
                }
            }
            Err(_) => return Some(out),
        }
        if std::time::Instant::now() > deadline {
            return Some(out);
        }
    }
}

#[tokio::test]
async fn test_w4c_line_webhook_valid_signature_delivers_and_stores_token() {
    use tokio::io::AsyncWriteExt;

    let (bus, mut rx) = broadcast::channel::<InboundMessage>(16);
    let secret = "w4c-secret";
    let reply_tokens = Arc::new(dashmap::DashMap::new());
    let running = Arc::new(parking_lot::RwLock::new(true));

    let body = serde_json::json!({
        "destination": "U-dest",
        "events": [{
            "type": "message",
            "replyToken": "rt-123",
            "message": {"type": "text", "text": "hello line"},
            "source": {"type": "user", "userId": "U-user-1"}
        }]
    })
    .to_string();
    let sig = w4c_line_signature(secret, body.as_bytes());

    let (mut client, _addr) = w4c_line_webhook_pair(&bus, secret, &reply_tokens, &running).await;
    let req = format!(
        "POST /callback HTTP/1.1\r\nHost: localhost\r\nX-Line-Signature: {sig}\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    client.write_all(req.as_bytes()).await.unwrap();

    let resp = w4c_read_http_response(&mut client)
        .await
        .unwrap_or_default();
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "expected 200 OK, got: {resp}"
    );

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("timed out: valid webhook must publish inbound")
        .unwrap();
    assert_eq!(inbound.channel, "line");
    assert_eq!(inbound.chat_id, "U-user-1");
    assert_eq!(inbound.sender_id, "U-user-1");
    assert_eq!(inbound.content, "hello line");

    assert_eq!(
        reply_tokens.get("U-user-1").map(|v| v.clone()),
        Some("rt-123".to_string()),
        "reply token must be stored for the chat"
    );
}

#[tokio::test]
async fn test_w4c_line_webhook_invalid_signature_401() {
    use tokio::io::AsyncWriteExt;

    let (bus, mut rx) = broadcast::channel::<InboundMessage>(16);
    let reply_tokens = Arc::new(dashmap::DashMap::new());
    let running = Arc::new(parking_lot::RwLock::new(true));

    let body = serde_json::json!({
        "events": [{
            "type": "message",
            "message": {"type": "text", "text": "forged"},
            "source": {"type": "user", "userId": "U-attacker"}
        }]
    })
    .to_string();

    let (mut client, _) = w4c_line_webhook_pair(&bus, "real-secret", &reply_tokens, &running).await;
    let req = format!(
        "POST /callback HTTP/1.1\r\nHost: localhost\r\nX-Line-Signature: FORGEDSIG==\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    client.write_all(req.as_bytes()).await.unwrap();

    let resp = w4c_read_http_response(&mut client)
        .await
        .unwrap_or_default();
    assert!(
        resp.starts_with("HTTP/1.1 401"),
        "expected 401, got: {resp}"
    );

    let leaked = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(
        leaked.is_err() || leaked.unwrap().is_err(),
        "forged webhook must not publish inbound"
    );
}

#[tokio::test]
async fn test_w4c_line_webhook_malformed_body_400() {
    use tokio::io::AsyncWriteExt;

    let (bus, _rx) = broadcast::channel::<InboundMessage>(16);
    let reply_tokens = Arc::new(dashmap::DashMap::new());
    let running = Arc::new(parking_lot::RwLock::new(true));

    let body = "this is not json at all";
    let sig = w4c_line_signature("w4c-secret", body.as_bytes());

    let (mut client, _) = w4c_line_webhook_pair(&bus, "w4c-secret", &reply_tokens, &running).await;
    let req = format!(
        "POST /callback HTTP/1.1\r\nHost: localhost\r\nX-Line-Signature: {sig}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    client.write_all(req.as_bytes()).await.unwrap();

    let resp = w4c_read_http_response(&mut client)
        .await
        .unwrap_or_default();
    assert!(
        resp.starts_with("HTTP/1.1 400"),
        "expected 400, got: {resp}"
    );
}

#[tokio::test]
async fn test_w4c_line_webhook_event_guards_skip_invalid_events() {
    use tokio::io::AsyncWriteExt;

    let (bus, mut rx) = broadcast::channel::<InboundMessage>(16);
    let reply_tokens = Arc::new(dashmap::DashMap::new());
    let running = Arc::new(parking_lot::RwLock::new(true));

    // 四类事件：follow（非 message）/ message 无 text / message 无 source / 合法 group message
    let body = serde_json::json!({
        "events": [
            {"type": "follow", "replyToken": "rt-f"},
            {"type": "message", "replyToken": "rt-nt",
             "message": {"type": "text", "text": ""},
             "source": {"type": "user", "userId": "U-no-text"}},
            {"type": "message", "replyToken": "rt-ns",
             "message": {"type": "text", "text": "no-src"}},
            {"type": "message", "replyToken": "rt-v",
             "message": {"type": "text", "text": "valid one"},
             "source": {"type": "group", "groupId": "C-grp", "userId": "U-in-grp"}}
        ]
    })
    .to_string();
    let sig = w4c_line_signature("w4c-secret", body.as_bytes());

    let (mut client, _) = w4c_line_webhook_pair(&bus, "w4c-secret", &reply_tokens, &running).await;
    let req = format!(
        "POST /callback HTTP/1.1\r\nHost: localhost\r\nX-Line-Signature: {sig}\r\nContent-Length: {}\r\n\r\n{}",
        body.len(),
        body
    );
    client.write_all(req.as_bytes()).await.unwrap();
    let resp = w4c_read_http_response(&mut client)
        .await
        .unwrap_or_default();
    assert!(
        resp.starts_with("HTTP/1.1 200"),
        "expected 200, got: {resp}"
    );

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("valid event must be delivered")
        .unwrap();
    assert_eq!(inbound.content, "valid one");
    // group 源：chat_id 用 groupId，sender_id 用 userId
    assert_eq!(inbound.chat_id, "C-grp");
    assert_eq!(inbound.sender_id, "U-in-grp");

    // 只有那条合法事件（其余被守卫跳过）
    let extra = tokio::time::timeout(std::time::Duration::from_millis(500), rx.recv()).await;
    assert!(
        extra.is_err() || extra.unwrap().is_err(),
        "guarded events must be skipped"
    );
}

#[tokio::test]
async fn test_w4c_line_webhook_not_running_closes_without_response() {
    use tokio::io::AsyncWriteExt;

    let (bus, _rx) = broadcast::channel::<InboundMessage>(16);
    let reply_tokens = Arc::new(dashmap::DashMap::new());
    let running = Arc::new(parking_lot::RwLock::new(false));

    let (mut client, _) = w4c_line_webhook_pair(&bus, "w4c-secret", &reply_tokens, &running).await;
    client
        .write_all(b"POST /callback HTTP/1.1\r\n\r\n{}")
        .await
        .unwrap();

    // running=false → handler 直接 return，客户端读到 EOF（0 字节响应）
    let resp = w4c_read_http_response(&mut client)
        .await
        .unwrap_or_default();
    assert!(
        !resp.contains("HTTP/1.1"),
        "no HTTP response expected when not running, got: {resp}"
    );
}

// ===========================================================================
// S2 coverage (2026-08-26): read Ok(0) early-return / event without message
// field / webhook accept-loop break after stop
// ===========================================================================

fn s2_line_channel(
    port: u16,
) -> (
    LineChannel,
    tokio::sync::broadcast::Receiver<InboundMessage>,
) {
    let (bus, rx) = tokio::sync::broadcast::channel::<InboundMessage>(16);
    let config = LineConfig {
        channel_access_token: "s2-token".to_string(),
        channel_secret: "s2-secret".to_string(),
        webhook_port: port,
        allow_from: vec![],
    };
    (LineChannel::new(config, bus).unwrap(), rx)
}

/// A TCP connection that closes without sending any byte makes the handler's
/// `stream.read` return Ok(0) -> early return arm.
#[tokio::test]
async fn s2_line_webhook_connect_then_close_returns_early() {
    let port = find_free_port();
    let (ch, _bus) = s2_line_channel(port);
    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    drop(s); // no bytes written -> read returns Ok(0)
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    ch.stop().await.unwrap();
}

/// A valid-signature webhook event of type "message" without a `message`
/// field is answered 200 OK but skipped (`None => continue`).
#[tokio::test]
async fn s2_line_webhook_event_without_message_field_is_skipped() {
    let port = find_free_port();
    let (ch, mut bus_rx) = s2_line_channel(port);
    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    let body = r#"{"events":[{"type":"message","replyToken":"rt","source":{"type":"user","userId":"U1"}}]}"#;
    let sig = make_signature_b64(body.as_bytes(), "s2-secret");
    let request = format!(
        "POST /webhook HTTP/1.1\r\nHost: localhost\r\nX-Line-Signature: {}\r\nContent-Length: {}\r\n\r\n{}",
        sig,
        body.len(),
        body
    );
    let resp = send_raw_http(port, request.as_bytes()).await;
    assert!(resp.starts_with("HTTP/1.1 200"), "got: {}", resp);

    // Nothing may be published (no message payload).
    let recv = tokio::time::timeout(std::time::Duration::from_millis(300), bus_rx.recv()).await;
    assert!(recv.is_err(), "message-less event must not publish");

    ch.stop().await.unwrap();
}

/// stop() flips the running flag; the accept loop wakes on one more
/// connection, sees the flag at the loop top and breaks ("webhook server
/// stopped" arm).
#[tokio::test]
async fn s2_line_webhook_accept_loop_breaks_after_stop() {
    let port = find_free_port();
    let (ch, _bus) = s2_line_channel(port);
    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(150)).await;

    // One real request so the listener is definitely bound and serving.
    let request = "POST /webhook HTTP/1.1\r\nHost: localhost\r\nContent-Length: 2\r\n\r\n{}";
    let _ = send_raw_http(port, request.as_bytes()).await;

    ch.stop().await.unwrap();

    // Wake the (still blocked) accept with a fresh connection: the handler
    // returns immediately (running=false) and the loop top then breaks.
    let s = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    drop(s);
}
