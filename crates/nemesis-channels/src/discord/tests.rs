use super::*;

#[tokio::test]
async fn test_discord_channel_new_validates_token() {
    let config = DiscordConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let result = DiscordChannel::new(config, tx);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discord_channel_lifecycle() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DiscordChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "discord");

    // Note: start() will try to connect to Discord, so we don't test the
    // full lifecycle here. Just test that it initializes correctly.
    assert!(!*ch.running.read());
}

#[test]
fn test_split_message_short() {
    let chunks = DiscordChannel::split_message("hello", 2000);
    assert_eq!(chunks, vec!["hello"]);
}

#[test]
fn test_split_message_long() {
    let long = "a ".repeat(1500); // 3000 chars
    let chunks = DiscordChannel::split_message(&long, 2000);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.len() <= 2000);
    }
    // Reconstructed content should match (minus trailing spaces from split)
    let reconstructed: String = chunks.join(" ");
    assert_eq!(reconstructed.trim(), long.trim());
}

#[test]
fn test_split_message_at_newline() {
    let msg = "line1\nline2\n";
    let chunks = DiscordChannel::split_message(msg, 8);
    assert_eq!(chunks.len(), 2);
}

#[tokio::test]
async fn test_parse_gateway_message_basic() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Hello agent!",
        "author": {
            "id": "user456",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
        .await
        .unwrap();
    assert_eq!(msg.channel, "discord");
    assert_eq!(msg.sender_id, "user456");
    assert_eq!(msg.chat_id, "ch1");
    assert_eq!(msg.content, "Hello agent!");
}

#[tokio::test]
async fn test_parse_gateway_message_filters_bot() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "My own message",
        "author": {
            "id": "bot123",
            "username": "nemesisbot",
            "discriminator": "0"
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_filters_other_bots() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Bot message",
        "author": {
            "id": "other_bot",
            "username": "somebot",
            "discriminator": "0",
            "bot": true
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_allowed_users() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Hello",
        "author": {
            "id": "user999",
            "username": "bob",
            "discriminator": "0",
            "bot": false
        }
    });

    // Not allowed
    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &["user111".to_string()]).await;
    assert!(msg.is_none());

    // Allowed
    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &["user999".to_string()]).await;
    assert!(msg.is_some());
}

#[tokio::test]
async fn test_parse_gateway_message_empty_content() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "",
        "author": {
            "id": "user1",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_guild() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "guild_id": "guild1",
        "content": "Hello guild!",
        "author": {
            "id": "user1",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
        .await
        .unwrap();
    assert_eq!(msg.metadata.get("guild_id").unwrap(), "guild1");
    assert_eq!(msg.metadata.get("is_group").unwrap(), "true");
}

#[test]
fn test_build_heartbeat_payload_with_sequence() {
    let payload = build_heartbeat_payload(Some(42));
    assert_eq!(payload["op"], 1);
    assert_eq!(payload["d"], 42);
}

#[test]
fn test_build_heartbeat_payload_without_sequence() {
    let payload = build_heartbeat_payload(None);
    assert_eq!(payload["op"], 1);
    assert!(payload["d"].is_null());
}

#[tokio::test]
async fn test_discord_new_with_client_validates_token() {
    let config = DiscordConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let result = DiscordChannel::new_with_client(config, tx, http);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discord_new_with_client_success() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let ch = DiscordChannel::new_with_client(config, tx, http).unwrap();
    assert_eq!(ch.name(), "discord");
    assert!(!*ch.running.read());
}
