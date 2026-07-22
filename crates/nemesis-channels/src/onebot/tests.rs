use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

#[tokio::test]
async fn test_onebot_channel_new_validates() {
    let config = OneBotConfig {
        ws_url: String::new(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    assert!(OneBotChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_onebot_channel_lifecycle() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 30,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "onebot");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_dedup_ring() {
    let mut ring = DedupRing::new(4);
    assert!(!ring.check_and_add("a"));
    assert!(ring.check_and_add("a"));
    assert!(!ring.check_and_add("b"));
    assert!(!ring.check_and_add("c"));
    assert!(!ring.check_and_add("d"));
    // "a" should be evicted now
    assert!(!ring.check_and_add("e"));
    assert!(ring.check_and_add("b")); // still in ring
}

#[test]
fn test_parse_json_int64() {
    assert_eq!(
        OneBotChannel::parse_json_int64(&serde_json::json!(12345)),
        Some(12345)
    );
    assert_eq!(
        OneBotChannel::parse_json_int64(&serde_json::json!("12345")),
        Some(12345)
    );
    assert_eq!(
        OneBotChannel::parse_json_int64(&serde_json::json!(null)),
        None
    );
}

#[test]
fn test_parse_message_segments_string() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();
    ch.set_self_id(12345);

    let result = ch.parse_message_segments(&serde_json::json!("hello world"));
    assert_eq!(result.text, "hello world");
    assert!(!result.is_bot_mentioned);
}

#[test]
fn test_parse_message_segments_with_at() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();
    ch.set_self_id(12345);

    let result = ch.parse_message_segments(&serde_json::json!("[CQ:at,qq=12345] hello"));
    assert!(result.is_bot_mentioned);
    assert_eq!(result.text, "hello");
}

#[test]
fn test_check_group_trigger_mentioned() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: vec!["/bot".to_string()],
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();

    let (triggered, content) = ch.check_group_trigger("hello", true);
    assert!(triggered);
    assert_eq!(content, "hello");
}

#[test]
fn test_check_group_trigger_prefix() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: vec!["/bot".to_string()],
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();

    let (triggered, content) = ch.check_group_trigger("/bot hello", false);
    assert!(triggered);
    assert_eq!(content, "hello");
}

#[test]
fn test_build_send_request() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();

    let (action, _) = ch.build_send_request("group:12345", "hello").unwrap();
    assert_eq!(action, "send_group_msg");

    let (action, _) = ch.build_send_request("private:67890", "hello").unwrap();
    assert_eq!(action, "send_private_msg");
}

#[test]
fn test_next_echo() {
    let config = OneBotConfig {
        ws_url: "ws://localhost:6700".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();

    let echo1 = ch.next_echo();
    let echo2 = ch.next_echo();
    assert_ne!(echo1, echo2);
    assert!(echo1.starts_with("api_"));
}
