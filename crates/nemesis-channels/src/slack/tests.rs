use super::*;

#[test]
fn test_parse_slack_chat_id_simple() {
    let (channel, thread) = SlackChannel::parse_slack_chat_id("C12345");
    assert_eq!(channel, "C12345");
    assert!(thread.is_none());
}

#[test]
fn test_parse_slack_chat_id_with_thread() {
    let (channel, thread) = SlackChannel::parse_slack_chat_id("C12345/1234567890.123456");
    assert_eq!(channel, "C12345");
    assert_eq!(thread.unwrap(), "1234567890.123456");
}

#[tokio::test]
async fn test_slack_channel_new_validates_tokens() {
    let config = SlackConfig {
        bot_token: String::new(),
        app_token: String::new(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let result = SlackChannel::new(config, tx);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_slack_channel_lifecycle() {
    let config = SlackConfig {
        bot_token: "xoxb-test".to_string(),
        app_token: "xapp-test".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = SlackChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "slack");

    // Note: start() will try to connect to Slack, so we don't test the
    // full lifecycle here. Just test that it initializes correctly.
    assert!(!*ch.running.read());
}

#[test]
fn test_strip_bot_mention() {
    let config = SlackConfig {
        bot_token: "xoxb-test".to_string(),
        app_token: "xapp-test".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = SlackChannel::new(config, tx).unwrap();
    ch.set_bot_user_id("U12345".to_string());

    let text = ch.strip_bot_mention("<@U12345> hello world");
    assert_eq!(text, "hello world");
}

#[test]
fn test_parse_slack_event_basic() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "Hello agent!",
        "ts": "1700000000.000100"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
    assert_eq!(msg.channel, "slack");
    assert_eq!(msg.sender_id, "U456");
    assert_eq!(msg.chat_id, "C789");
    assert_eq!(msg.content, "Hello agent!");
}

#[test]
fn test_parse_slack_event_filters_bot() {
    let bot_id = Arc::new(parking_lot::RwLock::new("U456".to_string()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "My message",
        "ts": "1700000000.000100"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
    assert!(msg.is_none());
}

#[test]
fn test_parse_slack_event_filters_bot_id_field() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "Bot message",
        "ts": "1700000000.000100",
        "bot_id": "B999"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
    assert!(msg.is_none());
}

#[test]
fn test_parse_slack_event_allowed_users() {
    let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "Hello",
        "ts": "1700000000.000100"
    });

    // Not allowed
    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &["U111".to_string()]);
    assert!(msg.is_none());

    // Allowed
    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &["U456".to_string()]);
    assert!(msg.is_some());

    // Empty = allow all
    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
    assert!(msg.is_some());
}

#[test]
fn test_parse_slack_event_empty_text() {
    let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "",
        "ts": "1700000000.000100"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]);
    assert!(msg.is_none());
}

#[test]
fn test_parse_slack_event_app_mention() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let event = serde_json::json!({
        "type": "app_mention",
        "user": "U456",
        "channel": "C789",
        "text": "<@B123> help me",
        "ts": "1700000000.000100"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
    assert_eq!(msg.metadata.get("was_mentioned").unwrap(), "true");
}

#[test]
fn test_parse_slack_event_thread() {
    let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
    let event = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "Reply in thread",
        "ts": "1700000001.000200",
        "thread_ts": "1700000000.000100"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
    assert_eq!(msg.chat_id, "C789/1700000000.000100");
    assert_eq!(msg.metadata.get("thread_ts").unwrap(), "1700000000.000100");
}

#[test]
fn test_parse_slack_event_message_changed() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let event = serde_json::json!({
        "type": "message",
        "subtype": "message_changed",
        "channel": "C789",
        "message": {
            "user": "U456",
            "text": "Edited message text",
            "ts": "1700000000.000100"
        },
        "ts": "1700000001.000200"
    });

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[]).unwrap();
    assert_eq!(msg.content, "Edited message text");
}
