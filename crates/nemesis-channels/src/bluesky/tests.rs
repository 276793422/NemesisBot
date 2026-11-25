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

fn seen_map() -> std::sync::Arc<parking_lot::RwLock<SeenTable>> {
    std::sync::Arc::new(parking_lot::RwLock::new(SeenTable::default()))
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
            .map
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
            .map
            .contains_key("at://did:plc:x/app.bsky.feed.post/10")
    );
    assert!(
        seen.read()
            .map
            .contains_key("at://did:plc:y/app.bsky.feed.post/11")
    );
    assert!(
        !seen
            .read()
            .map
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

/// BUG-20261125-019 回归锁：start/stop 必须维护 base running 旗标 + 幂等双启
/// 守卫（BUG-20261121-050 同构家族横向扫漏网补齐——manager 的 start_all 守卫 /
/// any_channel_running / status 上报三消费点都读 trait is_running）。
/// 服务端点指 loopback 死端口（后台 receive loop 连接失败零外网，start 本体
/// 不阻塞）；二次 start 走幂等守卫诚实跳过（不 spawn 第二个 receive loop）。
#[tokio::test]
async fn b019_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = BlueskyConfig {
        server: "http://127.0.0.1:1".to_string(),
        handle: "h".to_string(),
        password: "p".to_string(),
        did: None,
        poll_interval: 3600,
        allow_from: Vec::new(),
    };
    let (tx, _) = tokio::sync::broadcast::channel(16);
    let ch = BlueskyChannel::new(config, tx).unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 后 trait is_running 必须为 true（BUG-20261125-019）"
    );
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
}

/// BUG-20261125-026 回归锁：seen 表溢出 FIFO 逐出最旧而非 clear 全遗忘。
/// 原形态 >500 时 clear() 只留当前 1 条——同批溢出点之后的窗口内旧通知
/// 立即重发布、下一轮窗口其余旧通知（~24 条）再重灌 = bot 对旧帖成批
/// 重复回复（重启丢内存有 first_poll 基线守，clear 制造同态状态却不触发
/// 基线）。修复后：容量帽仍生效，但轮询窗口内（≤25 条 << 500）条目
/// 任何时刻完整保留；重复插入不重复入队（FIFO 序不腐化）。
#[test]
fn b026_seen_table_overflow_fifo_evicts_oldest_keeps_recent() {
    let seen = seen_map();

    // 灌到容量帽并溢出 1 条（原形态此处触发 clear 全遗忘）
    for i in 0..501 {
        seen.write().insert(format!("at://x/{i}"));
    }
    {
        let table = seen.read();
        assert!(
            table.map.len() <= 500,
            "容量帽必须仍生效（防长驻进程无界增长）"
        );
        assert!(
            !table.map.contains_key("at://x/0"),
            "最旧条目必须被 FIFO 逐出"
        );
        // 窗口内新近条目全部保留（原 clear 形态只留最后 1 条，本断言红）
        for i in 400..501 {
            assert!(
                table.map.contains_key(&format!("at://x/{i}")),
                "新近条目 at://x/{i} 必须保留（clear 全遗忘会让窗口内旧通知重灌重复回复）"
            );
        }
    }

    // 重复插入不改变 FIFO 序：重插已有键后继续灌新键，逐出的仍按原插入序
    seen.write().insert("at://x/460".to_string());
    for i in 501..510 {
        seen.write().insert(format!("at://x/{i}"));
    }
    let table = seen.read();
    assert!(
        table.map.contains_key("at://x/460"),
        "重插已有键不得让它被当作最新（order 队列不得重复入队）"
    );
    assert!(
        !table.map.contains_key("at://x/0"),
        "继续溢出逐出的仍是最旧条目"
    );
    assert!(table.map.contains_key("at://x/509"), "最新条目必须保留");
}

/// BUG-20261125-026 端到端语义锁：真实触发时序下溢出后窗口旧通知不重发布。
/// 时序复刻：表近满（500 填充条）→ 本轮窗口 old-1/old-2 基线标已见 →
/// 新通知 X ingest 触发插入溢出（原形态此处 clear 全遗忘，只留 X）→
/// 下一轮窗口 old-1 再进来必须去重（原形态 contains_key false → 重灌
/// 重复回复）。X 本身恰好被原形态 clear 重插保留，单看 X 不分红——
/// 分红点是窗口内其余条目。
#[test]
fn b026_overflow_does_not_republish_window_notifications() {
    let seen = seen_map();
    let bus = test_bus();
    let mut rx = bus.subscribe();

    // 表灌到恰满容量（历史轮次积累）
    for i in 0..500 {
        seen.write().insert(format!("at://filler/{i}"));
    }

    // 本轮窗口 old-1/old-2 基线标已见（最近条目，排在队列尾部）
    let old1 = bluesky_notif_json(
        "at://did:plc:w/app.bsky.feed.post/old-1",
        "mention",
        "did:plc:w",
        "walter.bsky.social",
        "old mention",
    );
    let old2 = bluesky_notif_json(
        "at://did:plc:v/app.bsky.feed.post/old-2",
        "reply",
        "did:plc:v",
        "vera.bsky.social",
        "old reply",
    );
    BlueskyChannel::mark_notifications_seen(&[old1.clone(), old2.clone()], &seen);
    assert!(rx.try_recv().is_err(), "基线不发布");

    // 新通知 X（未见过）触发插入溢出
    let fresh = bluesky_notif_json(
        "at://did:plc:n/app.bsky.feed.post/new-1",
        "mention",
        "did:plc:n",
        "newcomer.bsky.social",
        "fresh mention",
    );
    assert!(
        BlueskyChannel::ingest_notification(&fresh, OWN_DID, &seen, &[], &bus),
        "新通知必须正常发布"
    );
    let first = rx.try_recv().expect("X 发布一次");
    assert_eq!(first.sender_id, "newcomer.bsky.social");

    // 下一轮窗口 old-1 再进来：必须去重零发布
    // （原形态 clear 后 contains_key false → 重灌重复回复，本断言红）
    assert!(
        !BlueskyChannel::ingest_notification(&old1, OWN_DID, &seen, &[], &bus),
        "溢出淘汰后窗口内旧通知不得重发布（BUG-20261125-026 核心）"
    );
    assert!(rx.try_recv().is_err(), "去重命中必须零发布");

    // old-2 同理
    assert!(
        !BlueskyChannel::ingest_notification(&old2, OWN_DID, &seen, &[], &bus),
        "窗口内第二个旧通知同样不得重发布"
    );
    assert!(rx.try_recv().is_err(), "全程只允许 X 一条发布");
}
