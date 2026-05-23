use super::*;

fn make_config() -> FeishuConfig {
    FeishuConfig {
        app_id: "cli_test".to_string(),
        app_secret: "secret".to_string(),
        verification_token: String::new(),
        encrypt_key: String::new(),
        allow_from: Vec::new(),
    }
}

#[tokio::test]
async fn test_feishu_channel_new_validates() {
    let config = FeishuConfig {
        app_id: String::new(),
        app_secret: String::new(),
        verification_token: String::new(),
        encrypt_key: String::new(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(FeishuChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_feishu_channel_lifecycle() {
    let config = make_config();
    let (tx, _rx) = broadcast::channel(256);
    let ch = FeishuChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "feishu");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_extract_sender_id_user_id() {
    let sender = FeishuEventSender {
        sender_id: Some(FeishuSenderId {
            user_id: Some("u123".to_string()),
            open_id: Some("ou456".to_string()),
            union_id: None,
        }),
        tenant_key: None,
    };
    assert_eq!(FeishuChannel::extract_sender_id(&sender), "u123");
}

#[test]
fn test_extract_sender_id_fallback() {
    let sender = FeishuEventSender {
        sender_id: Some(FeishuSenderId {
            user_id: Some(String::new()),
            open_id: Some("ou456".to_string()),
            union_id: None,
        }),
        tenant_key: None,
    };
    assert_eq!(FeishuChannel::extract_sender_id(&sender), "ou456");
}

#[test]
fn test_extract_message_content_text() {
    let msg = FeishuEventMessage {
        chat_id: Some("oc_xxx".to_string()),
        message_id: Some("om_xxx".to_string()),
        message_type: Some("text".to_string()),
        content: Some(r#"{"text":"hello"}"#.to_string()),
        chat_type: Some("group".to_string()),
    };
    assert_eq!(FeishuChannel::extract_message_content(&msg), "hello");
}

#[test]
fn test_parse_and_publish_event() {
    let (tx, mut rx) = broadcast::channel(256);

    let event = serde_json::json!({
        "header": {
            "event_id": "evt_123",
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_id": "om_test",
                "message_type": "text",
                "content": "{\"text\":\"hello world\"}",
                "chat_type": "group"
            },
            "sender": {
                "sender_id": {
                    "user_id": "u123",
                    "open_id": "ou456"
                }
            }
        }
    });

    FeishuChannel::parse_and_publish_event(&event, &tx, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "feishu");
    assert_eq!(inbound.sender_id, "u123");
    assert_eq!(inbound.chat_id, "oc_test");
    assert_eq!(inbound.content, "hello world");
    assert_eq!(inbound.metadata.get("message_id").unwrap(), "om_test");
}

#[test]
fn test_parse_and_publish_event_filtered() {
    let (tx, mut rx) = broadcast::channel(256);

    let event = serde_json::json!({
        "header": {
            "event_id": "evt_456",
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_id": "om_test",
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "p2p"
            },
            "sender": {
                "sender_id": {
                    "user_id": "u_blocked"
                }
            }
        }
    });

    FeishuChannel::parse_and_publish_event(&event, &tx, &["u_allowed".to_string()]);

    assert!(rx.try_recv().is_err());
}

#[test]
fn test_extract_json_from_protobuf() {
    // Test with embedded JSON in binary data
    let json_str = r#"{"header":{"event_id":"evt_1"},"event":{"message":{"chat_id":"oc_1","content":"{\"text\":\"hi\"}"}}}"#;
    let mut data = vec![0u8; 10]; // prefix garbage bytes (simulating protobuf framing)
    data.extend_from_slice(json_str.as_bytes());

    let result = extract_json_from_protobuf(&data);
    assert!(result.is_some());
    assert!(result.unwrap().contains("evt_1"));
}
