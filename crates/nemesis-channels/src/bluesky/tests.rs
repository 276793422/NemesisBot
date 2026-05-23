use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

#[tokio::test]
async fn test_bluesky_channel_new_validates() {
    let config = BlueskyConfig {
        server: String::new(),
        handle: String::new(),
        password: String::new(),
        did: None,
        poll_interval: 0,
        allow_from: Vec::new(),
    };
    assert!(BlueskyChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_bluesky_channel_lifecycle() {
    let config = BlueskyConfig {
        server: "https://bsky.social".to_string(),
        handle: "test.bsky.social".to_string(),
        password: "password".to_string(),
        did: None,
        poll_interval: 10,
        allow_from: Vec::new(),
    };
    let ch = BlueskyChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "bluesky");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_build_post_uri() {
    let uri = BlueskyChannel::build_post_uri(
        "did:plc:abc123",
        "3k2la7bfx2x2y",
    );
    assert_eq!(uri, "at://did:plc:abc123/app.bsky.feed.post/3k2la7bfx2x2y");
}

#[test]
fn test_seen_tracking() {
    let config = BlueskyConfig {
        server: "https://bsky.social".to_string(),
        handle: "test.bsky.social".to_string(),
        password: "password".to_string(),
        did: None,
        poll_interval: 10,
        allow_from: Vec::new(),
    };
    let ch = BlueskyChannel::new(config, test_bus()).unwrap();

    assert!(!ch.is_seen("notif-1"));
    ch.mark_seen("notif-1");
    assert!(ch.is_seen("notif-1"));
}

#[test]
fn test_default_poll_interval() {
    let config = BlueskyConfig {
        server: "https://bsky.social".to_string(),
        handle: "test.bsky.social".to_string(),
        password: "password".to_string(),
        did: None,
        poll_interval: 0,
        allow_from: Vec::new(),
    };
    let ch = BlueskyChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.config.poll_interval, 10);
}

// ---- New tests ----

#[test]
fn test_bluesky_config_with_did() {
    let config = BlueskyConfig {
        server: "https://bsky.social".into(),
        handle: "user.bsky.social".into(),
        password: "pass".into(),
        did: Some("did:plc:abc".into()),
        poll_interval: 30,
        allow_from: vec!["did:plc:other".into()],
    };
    assert!(config.did.is_some());
    assert_eq!(config.poll_interval, 30);
}

#[test]
fn test_seen_tracking_multiple() {
    let config = BlueskyConfig {
        server: "https://bsky.social".to_string(),
        handle: "test.bsky.social".to_string(),
        password: "password".to_string(),
        did: None,
        poll_interval: 10,
        allow_from: Vec::new(),
    };
    let ch = BlueskyChannel::new(config, test_bus()).unwrap();

    for i in 0..10 {
        assert!(!ch.is_seen(&format!("n-{}", i)));
        ch.mark_seen(&format!("n-{}", i));
        assert!(ch.is_seen(&format!("n-{}", i)));
    }
}

#[test]
fn test_build_post_uri_various() {
    let uri = BlueskyChannel::build_post_uri("did:plc:test123", "abc");
    assert!(uri.starts_with("at://"));
    assert!(uri.contains("did:plc:test123"));
    assert!(uri.ends_with("/abc"));
}

#[tokio::test]
async fn test_bluesky_double_stop() {
    let config = BlueskyConfig {
        server: "https://bsky.social".to_string(),
        handle: "test.bsky.social".to_string(),
        password: "password".to_string(),
        did: None,
        poll_interval: 10,
        allow_from: Vec::new(),
    };
    let ch = BlueskyChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.stop().await.unwrap();
    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}
