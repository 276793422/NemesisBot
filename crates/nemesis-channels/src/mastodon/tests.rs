use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

#[test]
fn test_strip_html_tags() {
    assert_eq!(strip_html_tags("<p>Hello</p>"), "Hello");
    assert_eq!(strip_html_tags("<b>bold</b> text"), "bold text");
    assert_eq!(strip_html_tags("<a href=\"url\">link</a>"), "link");
}

#[test]
fn test_strip_html_entities() {
    assert_eq!(strip_html_tags("a &amp; b"), "a & b");
    assert_eq!(strip_html_tags("&lt;tag&gt;"), "<tag>");
}

#[tokio::test]
async fn test_mastodon_channel_new_validates() {
    let config = MastodonConfig {
        server: String::new(),
        access_token: String::new(),
        allow_from: Vec::new(),
    };
    assert!(MastodonChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_mastodon_channel_lifecycle() {
    let config = MastodonConfig {
        server: "https://mastodon.social".to_string(),
        access_token: "token".to_string(),
        allow_from: Vec::new(),
    };
    let ch = MastodonChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "mastodon");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_seen_notification_tracking() {
    let config = MastodonConfig {
        server: "https://mastodon.social".to_string(),
        access_token: "token".to_string(),
        allow_from: Vec::new(),
    };
    let ch = MastodonChannel::new(config, test_bus()).unwrap();

    assert!(!ch.is_seen("notif-1"));
    ch.mark_seen("notif-1");
    assert!(ch.is_seen("notif-1"));
}

#[test]
fn test_notifications_url() {
    let config = MastodonConfig {
        server: "https://mastodon.social".to_string(),
        access_token: "token".to_string(),
        allow_from: Vec::new(),
    };
    let ch = MastodonChannel::new(config, test_bus()).unwrap();
    assert_eq!(
        ch.notifications_url(),
        "https://mastodon.social/api/v1/notifications"
    );
}

// ---- New tests ----

#[test]
fn test_strip_html_complex() {
    assert_eq!(
        strip_html_tags("<div><p>para1</p><p>para2</p></div>"),
        "para1para2"
    );
    assert_eq!(strip_html_tags("no html here"), "no html here");
    assert_eq!(strip_html_tags(""), "");
    // &nbsp; is an HTML entity, not a tag; strip_html_tags leaves entities as-is
    // (it only decodes the 5 common formatting entities, and &nbsp; isn't among them).
    assert_eq!(strip_html_tags("&nbsp;"), "&nbsp;");
}

#[test]
fn test_mastodon_config_with_allow_from() {
    let config = MastodonConfig {
        server: "https://m.social".into(),
        access_token: "tok".into(),
        allow_from: vec!["@user@m.social".into()],
    };
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_seen_notification_multiple() {
    let config = MastodonConfig {
        server: "https://mastodon.social".into(),
        access_token: "token".into(),
        allow_from: Vec::new(),
    };
    let ch = MastodonChannel::new(config, test_bus()).unwrap();
    for i in 0..20 {
        assert!(!ch.is_seen(&format!("n-{}", i)));
        ch.mark_seen(&format!("n-{}", i));
        assert!(ch.is_seen(&format!("n-{}", i)));
    }
}

#[tokio::test]
async fn test_mastodon_double_stop() {
    let config = MastodonConfig {
        server: "https://mastodon.social".into(),
        access_token: "token".into(),
        allow_from: Vec::new(),
    };
    let ch = MastodonChannel::new(config, test_bus()).unwrap();
    ch.start().await.unwrap();
    ch.stop().await.unwrap();
    ch.stop().await.unwrap();
}

// ---- BUG-20261113-149（allow_from 死键） / BUG-20261113-150（首启回灌）回归 ----

fn mastodon_mention_json(id: &str, username: &str, content: &str) -> serde_json::Value {
    serde_json::json!({
        "id": id,
        "type": "mention",
        "account": {"id": "a1", "username": username, "display_name": null},
        "status": {"id": format!("s-{id}"), "content": content, "in_reply_to_id": null}
    })
}

fn seen_map() -> std::sync::Arc<parking_lot::RwLock<HashMap<String, bool>>> {
    std::sync::Arc::new(parking_lot::RwLock::new(HashMap::new()))
}

/// BUG-20261113-149：allow_from 白名单闸——白名单外 mention 不发布；
/// 白名单内照常；空 = 全放行。
#[test]
fn test_bug149_ingest_notification_allow_from_gate() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notif = mastodon_mention_json("n1", "intruder", "<p>hello</p>");
    let allowed = vec!["alice".to_string()];

    // 白名单外：不发布
    assert!(!MastodonChannel::ingest_notification(
        &notif, &seen, &allowed, &bus
    ));
    assert!(rx.try_recv().is_err());
    // 白名单外 mention 不应消耗去重表（闸在去重前）
    assert!(!seen.read().contains_key("n1"));

    // 白名单内：发布
    let notif_alice = mastodon_mention_json("n2", "alice", "<p>hi bot</p>");
    assert!(MastodonChannel::ingest_notification(
        &notif_alice,
        &seen,
        &allowed,
        &bus
    ));
    let inbound = rx.try_recv().expect("allowed sender must publish");
    assert_eq!(inbound.channel, "mastodon");
    assert_eq!(inbound.sender_id, "alice");
    assert_eq!(inbound.chat_id, "s-n2");
    assert_eq!(inbound.content, "hi bot");
}

/// BUG-20261113-149：空 allow_from = 全放行（家族惯例）。
#[test]
fn test_bug149_ingest_notification_empty_allow_from_allows_all() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notif = mastodon_mention_json("n1", "anyone", "<p>hello</p>");
    assert!(MastodonChannel::ingest_notification(
        &notif,
        &seen,
        &[],
        &bus
    ));
    assert!(rx.try_recv().is_ok());
}

/// BUG-20261113-150：mark_mentions_seen 首轮基线——窗口内 mention 全标
/// 已见不发布；后续对同 id 再 ingest 被去重（不重复回复旧帖）。
#[test]
fn test_bug150_mark_mentions_seen_baseline_then_dedup() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    let notifications = vec![
        mastodon_mention_json("old-1", "alice", "<p>old question</p>"),
        mastodon_mention_json("old-2", "bob", "<p>old other</p>"),
        serde_json::json!({
            "id": "f-1",
            "type": "follow",
            "account": {"id": "a3", "username": "carol", "display_name": null}
        }),
    ];

    // 首轮基线：全标已见（follow 类型不标），零发布
    MastodonChannel::mark_mentions_seen(&notifications, &seen);
    assert!(rx.try_recv().is_err(), "baseline must not publish");
    assert!(seen.read().contains_key("old-1"));
    assert!(seen.read().contains_key("old-2"));
    assert!(!seen.read().contains_key("f-1"));

    // 同一 mention 再进来（重启场景下 seen 表存活时的重复投递）→ 去重不发布
    assert!(!MastodonChannel::ingest_notification(
        &notifications[0],
        &seen,
        &[],
        &bus
    ));
    assert!(rx.try_recv().is_err());
}
