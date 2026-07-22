use super::*;

#[tokio::test]
async fn test_webhook_channel_lifecycle() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert_eq!(ch.name(), "webhook");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_validate_api_key_no_auth() {
    let config = WebhookInboundConfig {
        api_key: String::new(),
        ..Default::default()
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert!(ch.validate_api_key("anything"));
    assert!(ch.validate_api_key(""));
}

#[test]
fn test_validate_api_key_with_auth() {
    let config = WebhookInboundConfig {
        api_key: "secret".to_string(),
        ..Default::default()
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert!(ch.validate_api_key("secret"));
    assert!(!ch.validate_api_key("wrong"));
}

#[test]
fn test_extract_routing() {
    let (channel, chat_id) = WebhookInboundChannel::extract_routing(
        "/webhook/incoming/telegram/123",
        "/webhook/incoming",
    );
    assert_eq!(channel, Some("telegram"));
    assert_eq!(chat_id, Some("123"));
}

#[test]
fn test_extract_routing_no_extra() {
    let (channel, chat_id) =
        WebhookInboundChannel::extract_routing("/webhook/incoming", "/webhook/incoming");
    assert_eq!(channel, None);
    assert_eq!(chat_id, None);
}

#[test]
fn test_process_request() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();

    let req = WebhookRequest {
        content: "hello".to_string(),
        sender_id: Some("user1".to_string()),
        chat_id: Some("chat1".to_string()),
        metadata: Some({
            let mut m = HashMap::new();
            m.insert("key".to_string(), serde_json::json!("value"));
            m
        }),
    };

    let (sender, chat, metadata) = ch.process_request(&req);
    assert_eq!(sender, "user1");
    assert_eq!(chat, "chat1");
    assert_eq!(metadata.get("platform").unwrap(), "webhook_inbound");
    assert_eq!(metadata.get("key").unwrap(), "value");
}

#[tokio::test]
async fn test_send_resolves_pending() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let rx = ch.register_pending("chat1".to_string(), Duration::from_secs(5));
    assert!(!ch.pending.is_empty());

    let msg = OutboundMessage {
        channel: "webhook".to_string(),
        chat_id: "chat1".to_string(),
        content: "response".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.unwrap();

    let response = tokio::time::timeout(Duration::from_secs(1), rx).await;
    assert!(response.is_ok());
    assert_eq!(response.unwrap().unwrap(), "response");
    assert!(ch.pending.is_empty());
}

#[tokio::test]
async fn test_send_queues_when_no_pending() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "webhook".to_string(),
        chat_id: "chat1".to_string(),
        content: "orphaned".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.unwrap();

    let queued = ch.drain_outbound();
    assert_eq!(queued.len(), 1);
    assert_eq!(queued[0].content, "orphaned");
}

// ---- Additional comprehensive webhook inbound tests ----

// === Configuration edge cases ===

#[test]
fn test_config_default_values() {
    let config = WebhookInboundConfig::default();
    assert_eq!(config.listen_addr, ":9090");
    assert_eq!(config.path, "/webhook/incoming");
    assert_eq!(config.api_key, "");
    assert!(config.allow_from.is_empty());
}

#[test]
fn test_new_with_empty_listen_addr() {
    let config = WebhookInboundConfig {
        listen_addr: String::new(),
        path: String::new(),
        api_key: String::new(),
        allow_from: Vec::new(),
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert_eq!(ch.listen_addr(), ":9090"); // defaults
    assert_eq!(ch.path(), "/webhook/incoming");
}

#[test]
fn test_new_custom_path() {
    let config = WebhookInboundConfig {
        listen_addr: ":8080".to_string(),
        path: "/custom/webhook".to_string(),
        api_key: String::new(),
        allow_from: Vec::new(),
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert_eq!(ch.path(), "/custom/webhook");
}

// === API key validation ===

#[test]
fn test_validate_api_key_case_sensitive() {
    let config = WebhookInboundConfig {
        api_key: "SecretKey".to_string(),
        ..Default::default()
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert!(ch.validate_api_key("SecretKey"));
    assert!(!ch.validate_api_key("secretkey"));
    assert!(!ch.validate_api_key("SECRETKEY"));
}

#[test]
fn test_validate_api_key_empty_vs_empty() {
    let config = WebhookInboundConfig {
        api_key: String::new(),
        ..Default::default()
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    // No auth required - any key works
    assert!(ch.validate_api_key(""));
    assert!(ch.validate_api_key("any-random-key"));
}

#[test]
fn test_validate_api_key_long_key() {
    let long_key = "k".repeat(1000);
    let config = WebhookInboundConfig {
        api_key: long_key.clone(),
        ..Default::default()
    };
    let ch = WebhookInboundChannel::new(config).unwrap();
    assert!(ch.validate_api_key(&long_key));
    assert!(!ch.validate_api_key("wrong"));
}

// === Extract routing edge cases ===

#[test]
fn test_extract_routing_with_trailing_slash() {
    let (ch, chat) = WebhookInboundChannel::extract_routing(
        "/webhook/incoming/telegram/123/",
        "/webhook/incoming",
    );
    assert_eq!(ch, Some("telegram"));
    assert_eq!(chat, Some("123/"));
}

#[test]
fn test_extract_routing_channel_only() {
    let (ch, chat) =
        WebhookInboundChannel::extract_routing("/webhook/incoming/telegram", "/webhook/incoming");
    assert_eq!(ch, Some("telegram"));
    assert_eq!(chat, None);
}

#[test]
fn test_extract_routing_wrong_base() {
    let (ch, chat) =
        WebhookInboundChannel::extract_routing("/other/path/telegram/123", "/webhook/incoming");
    assert_eq!(ch, None);
    assert_eq!(chat, None);
}

#[test]
fn test_extract_routing_empty_path() {
    let (ch, chat) =
        WebhookInboundChannel::extract_routing("/webhook/incoming", "/webhook/incoming");
    assert_eq!(ch, None);
    assert_eq!(chat, None);
}

#[test]
fn test_extract_routing_with_base_trailing_slash() {
    let (ch, chat) = WebhookInboundChannel::extract_routing(
        "/webhook/incoming/discord/456",
        "/webhook/incoming/",
    );
    assert_eq!(ch, Some("discord"));
    assert_eq!(chat, Some("456"));
}

#[test]
fn test_extract_routing_nested_path() {
    let (ch, chat) =
        WebhookInboundChannel::extract_routing("/webhook/incoming/a/b", "/webhook/incoming");
    assert_eq!(ch, Some("a"));
    assert_eq!(chat, Some("b"));
}

// === Process request edge cases ===

#[test]
fn test_process_request_no_sender() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();

    let req = WebhookRequest {
        content: "hello".to_string(),
        sender_id: None,
        chat_id: None,
        metadata: None,
    };

    let (sender, chat, metadata) = ch.process_request(&req);
    assert_eq!(sender, "webhook"); // default sender
    assert_eq!(chat, "webhook:default"); // default chat
    assert_eq!(metadata.get("platform").unwrap(), "webhook_inbound");
}

#[test]
fn test_process_request_with_metadata() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();

    let req = WebhookRequest {
        content: "hello".to_string(),
        sender_id: Some("user1".to_string()),
        chat_id: Some("chat1".to_string()),
        metadata: Some({
            let mut m = HashMap::new();
            m.insert("num".to_string(), serde_json::json!(42));
            m.insert("bool_val".to_string(), serde_json::json!(true));
            m
        }),
    };

    let (_, _, metadata) = ch.process_request(&req);
    assert_eq!(metadata.get("num").unwrap(), "42");
    assert_eq!(metadata.get("bool_val").unwrap(), "true");
}

#[test]
fn test_process_request_empty_content() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();

    let req = WebhookRequest {
        content: String::new(),
        sender_id: None,
        chat_id: None,
        metadata: None,
    };

    let (_, _, _) = ch.process_request(&req); // should not panic
}

// === Pending request handling ===

#[tokio::test]
async fn test_send_resolves_correct_pending() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let rx1 = ch.register_pending("chat-1".to_string(), Duration::from_secs(5));
    let rx2 = ch.register_pending("chat-2".to_string(), Duration::from_secs(5));

    // Send to chat-1 only
    let msg = OutboundMessage {
        channel: "webhook".to_string(),
        chat_id: "chat-1".to_string(),
        content: "response-1".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.unwrap();

    let resp1 = tokio::time::timeout(Duration::from_secs(1), rx1).await;
    assert_eq!(resp1.unwrap().unwrap(), "response-1");

    // chat-2 should still be pending
    assert!(ch.pending.contains_key("chat-2"));

    // Now send to chat-2
    let msg2 = OutboundMessage {
        channel: "webhook".to_string(),
        chat_id: "chat-2".to_string(),
        content: "response-2".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg2).await.unwrap();

    let resp2 = tokio::time::timeout(Duration::from_secs(1), rx2).await;
    assert_eq!(resp2.unwrap().unwrap(), "response-2");
}

#[tokio::test]
async fn test_send_when_not_running_fails() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    // Not started

    let msg = OutboundMessage {
        channel: "webhook".to_string(),
        chat_id: "chat-1".to_string(),
        content: "test".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[tokio::test]
async fn test_stop_clears_pending() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    ch.register_pending("chat-1".to_string(), Duration::from_secs(5));
    ch.register_pending("chat-2".to_string(), Duration::from_secs(5));
    assert_eq!(ch.pending.len(), 2);

    ch.stop().await.unwrap();
    assert!(ch.pending.is_empty());
}

#[tokio::test]
async fn test_drain_outbound_returns_all() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    for i in 0..5 {
        let msg = OutboundMessage {
            channel: "webhook".to_string(),
            chat_id: format!("orphan-{}", i),
            content: format!("msg {}", i),
            message_type: String::new(),
            meta: Default::default(),
        };
        ch.send(msg).await.unwrap();
    }

    let queued = ch.drain_outbound();
    assert_eq!(queued.len(), 5);

    // Second drain should be empty
    let queued2 = ch.drain_outbound();
    assert!(queued2.is_empty());
}

// === Cleanup expired ===

#[tokio::test]
async fn test_cleanup_expired_requests() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let _rx = ch.register_pending("expire-me".to_string(), Duration::from_millis(10));
    assert!(!ch.pending.is_empty());

    tokio::time::sleep(Duration::from_millis(50)).await;
    ch.cleanup_expired();

    assert!(ch.pending.is_empty());
}

#[tokio::test]
async fn test_cleanup_keeps_active_requests() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();
    ch.start().await.unwrap();

    let _rx = ch.register_pending("keep-me".to_string(), Duration::from_secs(60));
    ch.cleanup_expired();
    assert!(ch.pending.contains_key("keep-me"));
}

// === Lifecycle ===

#[tokio::test]
async fn test_start_stop_idempotent() {
    let config = WebhookInboundConfig::default();
    let ch = WebhookInboundChannel::new(config).unwrap();

    ch.start().await.unwrap();
    ch.start().await.unwrap(); // second start
    ch.stop().await.unwrap();
    ch.stop().await.unwrap(); // second stop
}
