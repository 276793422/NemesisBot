use super::*;

#[tokio::test]
async fn test_whatsapp_channel_new_validates_url() {
    let config = WhatsAppConfig {
        bridge_url: String::new(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(WhatsAppChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_whatsapp_channel_lifecycle() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".to_string(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "whatsapp");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[tokio::test]
async fn test_whatsapp_send_queues_on_bridge_failure() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:19999".to_string(), // unreachable
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "whatsapp".to_string(),
        chat_id: "12345".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
    };
    ch.send(msg).await.unwrap(); // should succeed (queued on bridge failure)

    let outbound = ch.drain_outbound();
    assert_eq!(outbound.len(), 1);
    assert_eq!(outbound[0].content, "Hello");
}

#[test]
fn test_process_inbound_message() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".to_string(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();

    let msg = WhatsAppInboundMessage {
        msg_type: Some("message".to_string()),
        from: Some("+1234567890".to_string()),
        chat: Some("+1234567890".to_string()),
        content: Some("Hello".to_string()),
        id: Some("msg-1".to_string()),
        from_name: Some("John".to_string()),
        media: None,
    };

    let (sender, chat, content) = ch.process_inbound(&msg).unwrap();
    assert_eq!(sender, "+1234567890");
    assert_eq!(content, "Hello");
}

#[test]
fn test_process_inbound_non_message() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".to_string(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();

    let msg = WhatsAppInboundMessage {
        msg_type: Some("receipt".to_string()),
        from: None,
        chat: None,
        content: None,
        id: None,
        from_name: None,
        media: None,
    };

    assert!(ch.process_inbound(&msg).is_none());
}

// ---- New tests ----

#[test]
fn test_whatsapp_config_fields() {
    let config = WhatsAppConfig {
        bridge_url: "http://bridge:8080".into(),
        api_key: Some("key123".into()),
        allow_from: vec!["+1234567890".into()],
    };
    assert_eq!(config.bridge_url, "http://bridge:8080");
    assert_eq!(config.api_key.as_deref(), Some("key123"));
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_process_inbound_with_media() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".into(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();

    let msg = WhatsAppInboundMessage {
        msg_type: Some("message".into()),
        from: Some("+111".into()),
        chat: Some("+111".into()),
        content: Some("See this image".into()),
        id: Some("msg-media".into()),
        from_name: None,
        media: Some(WhatsAppMedia {
            media_type: "image".into(),
            url: "http://bridge/media/123".into(),
            mime_type: Some("image/jpeg".into()),
        }),
    };
    let result = ch.process_inbound(&msg);
    assert!(result.is_some());
}

#[tokio::test]
async fn test_whatsapp_double_stop() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".into(),
        api_key: None,
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();
    ch.stop().await.unwrap();
    ch.stop().await.unwrap(); // double stop should be fine
    assert!(!*ch.running.read());
}

#[test]
fn test_process_inbound_allow_from_filter() {
    let config = WhatsAppConfig {
        bridge_url: "http://localhost:8080".into(),
        api_key: None,
        allow_from: vec!["+999".into()],
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = WhatsAppChannel::new(config, tx).unwrap();

    let msg = WhatsAppInboundMessage {
        msg_type: Some("message".into()),
        from: Some("+888".into()),
        chat: Some("+888".into()),
        content: Some("Hello".into()),
        id: Some("id".into()),
        from_name: None,
        media: None,
    };
    assert!(ch.process_inbound(&msg).is_none()); // not in allow_from
}
