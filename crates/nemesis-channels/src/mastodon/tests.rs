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
