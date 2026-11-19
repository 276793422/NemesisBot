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
    let uri = BlueskyChannel::build_post_uri("did:plc:abc123", "3k2la7bfx2x2y");
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

// ---- BUG-20261113-149（allow_from 死键） / BUG-20261113-150（首启回灌）回归 ----

fn bluesky_notif_json(
    uri: &str,
    reason: &str,
    did: &str,
    handle: &str,
    text: &str,
) -> serde_json::Value {
    serde_json::json!({
        "uri": uri,
        "cid": format!("cid-{uri}"),
        "reason": reason,
        "author": {"did": did, "handle": handle},
        "record": {"text": text}
    })
}

fn seen_map() -> std::sync::Arc<parking_lot::RwLock<HashMap<String, bool>>> {
    std::sync::Arc::new(parking_lot::RwLock::new(HashMap::new()))
}

const OWN_DID: &str = "did:plc:me";

/// BUG-20261113-149：allow_from 白名单闸（匹配 Bluesky handle）——白名单外
/// mention/reply 不发布；白名单内照常；空 = 全放行。
#[test]
fn test_bug149_ingest_notification_allow_from_gate() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notif = bluesky_notif_json(
        "at://did:plc:x/app.bsky.feed.post/1",
        "mention",
        "did:plc:x",
        "intruder.bsky.social",
        "hello bot",
    );
    let allowed = vec!["alice.bsky.social".to_string()];

    // 白名单外：不发布
    assert!(!BlueskyChannel::ingest_notification(
        &notif, OWN_DID, &seen, &allowed, &bus
    ));
    assert!(rx.try_recv().is_err());
    // 白名单外不消耗去重表（闸在去重前）
    assert!(
        !seen
            .read()
            .contains_key("at://did:plc:x/app.bsky.feed.post/1")
    );

    // 白名单内：发布
    let notif_alice = bluesky_notif_json(
        "at://did:plc:a/app.bsky.feed.post/2",
        "reply",
        "did:plc:a",
        "alice.bsky.social",
        "replying to you",
    );
    assert!(BlueskyChannel::ingest_notification(
        &notif_alice,
        OWN_DID,
        &seen,
        &allowed,
        &bus
    ));
    let inbound = rx.try_recv().expect("allowed sender must publish");
    assert_eq!(inbound.channel, "bluesky");
    assert_eq!(inbound.sender_id, "alice.bsky.social");
    assert_eq!(inbound.content, "replying to you");
}

/// BUG-20261113-149：空 allow_from = 全放行。
#[test]
fn test_bug149_ingest_notification_empty_allow_from_allows_all() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notif = bluesky_notif_json(
        "at://did:plc:y/app.bsky.feed.post/3",
        "mention",
        "did:plc:y",
        "anyone.bsky.social",
        "hello",
    );
    assert!(BlueskyChannel::ingest_notification(
        &notif,
        OWN_DID,
        &seen,
        &[],
        &bus
    ));
    assert!(rx.try_recv().is_ok());
}

/// 自发帖闸（既有行为钉死）：author.did == own_did 的 mention 不发布。
#[test]
fn test_bug149_ingest_notification_own_post_rejected() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notif = bluesky_notif_json(
        "at://did:plc:me/app.bsky.feed.post/4",
        "mention",
        OWN_DID,
        "me.bsky.social",
        "my own post mentioning myself",
    );
    assert!(!BlueskyChannel::ingest_notification(
        &notif,
        OWN_DID,
        &seen,
        &[],
        &bus
    ));
    assert!(rx.try_recv().is_err());
}

/// BUG-20261113-150：mark_notifications_seen 首轮基线——窗口内
/// mention/reply uri 全标已见不发布（follow 不标）；后续同 uri 再
/// ingest 被去重。
#[test]
fn test_bug150_mark_notifications_seen_baseline_then_dedup() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notifications = vec![
        bluesky_notif_json(
            "at://did:plc:x/app.bsky.feed.post/10",
            "mention",
            "did:plc:x",
            "alice.bsky.social",
            "old question",
        ),
        bluesky_notif_json(
            "at://did:plc:y/app.bsky.feed.post/11",
            "reply",
            "did:plc:y",
            "bob.bsky.social",
            "old reply",
        ),
        bluesky_notif_json(
            "at://did:plc:z/app.bsky.feed.post/12",
            "follow",
            "did:plc:z",
            "carol.bsky.social",
            "",
        ),
    ];

    // 首轮基线：mention/reply 标已见，零发布
    BlueskyChannel::mark_notifications_seen(&notifications, &seen);
    assert!(rx.try_recv().is_err(), "baseline must not publish");
    assert!(
        seen.read()
            .contains_key("at://did:plc:x/app.bsky.feed.post/10")
    );
    assert!(
        seen.read()
            .contains_key("at://did:plc:y/app.bsky.feed.post/11")
    );
    assert!(
        !seen
            .read()
            .contains_key("at://did:plc:z/app.bsky.feed.post/12")
    );

    // 同一 mention 再进来 → 去重不发布
    assert!(!BlueskyChannel::ingest_notification(
        &notifications[0],
        OWN_DID,
        &seen,
        &[],
        &bus
    ));
    assert!(rx.try_recv().is_err());
}
