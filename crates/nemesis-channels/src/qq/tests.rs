use super::*;

#[tokio::test]
async fn test_qq_channel_new_validates() {
    let config = QQConfig {
        app_id: String::new(),
        app_secret: String::new(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(QQChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_qq_channel_lifecycle() {
    let config = QQConfig {
        app_id: "app-123".to_string(),
        app_secret: "secret".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = QQChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "qq");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_is_duplicate() {
    let config = QQConfig {
        app_id: "app-123".to_string(),
        app_secret: "secret".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = QQChannel::new(config, tx).unwrap();

    assert!(!ch.is_duplicate("msg-1"));
    assert!(ch.is_duplicate("msg-1")); // second time is duplicate
    assert!(!ch.is_duplicate("msg-2"));
}

#[test]
fn test_default_config() {
    let config = QQConfig::default();
    assert_eq!(config.api_base, "https://api.sgroup.qq.com");
    assert!(config.app_id.is_empty());
}

#[test]
fn test_handle_c2c_message() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Hello from C2C",
        "author": {
            "user_openid": "user_open_123"
        },
        "id": "msg-c2c-1"
    });

    QQChannel::handle_c2c_message(&data, &tx, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "qq");
    assert_eq!(inbound.sender_id, "user_open_123");
    assert_eq!(inbound.chat_id, "c2c:user_open_123");
    assert_eq!(inbound.content, "Hello from C2C");
    assert_eq!(inbound.metadata.get("message_id").unwrap(), "msg-c2c-1");
    assert_eq!(inbound.metadata.get("chat_type").unwrap(), "c2c");
}

#[test]
fn test_handle_group_message() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Hello from group",
        "author": {
            "member_openid": "member_456"
        },
        "group_openid": "group_789",
        "id": "msg-group-1"
    });

    QQChannel::handle_group_message(&data, &tx, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "qq");
    assert_eq!(inbound.sender_id, "member_456");
    assert_eq!(inbound.chat_id, "group:group_789");
    assert_eq!(inbound.content, "Hello from group");
    assert_eq!(inbound.metadata.get("group_openid").unwrap(), "group_789");
    assert_eq!(inbound.metadata.get("chat_type").unwrap(), "group");
}

#[test]
fn test_handle_c2c_message_filtered() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Blocked message",
        "author": {
            "user_openid": "blocked_user"
        },
        "id": "msg-blocked"
    });

    QQChannel::handle_c2c_message(&data, &tx, &["allowed_user".to_string()]);
    assert!(rx.try_recv().is_err());
}
