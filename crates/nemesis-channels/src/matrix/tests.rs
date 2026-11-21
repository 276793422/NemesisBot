use super::*;

fn test_bus() -> broadcast::Sender<InboundMessage> {
    let (tx, _) = broadcast::channel(256);
    tx
}

// Directly exercises the allow_from rule (single source of truth shared by
// process_sync_events and start()), without needing a live Matrix server.
#[test]
fn test_sender_allowed_rule() {
    // Empty allow_from = open (everyone allowed).
    assert!(MatrixChannel::sender_allowed(&[], "@anyone:matrix.org"));
    assert!(MatrixChannel::sender_allowed(&[], ""));
    // Non-empty = only listed senders pass.
    let list = vec!["@alice:m.org".to_string(), "@bob:m.org".to_string()];
    assert!(MatrixChannel::sender_allowed(&list, "@alice:m.org"));
    assert!(MatrixChannel::sender_allowed(&list, "@bob:m.org"));
    assert!(!MatrixChannel::sender_allowed(&list, "@eve:m.org")); // not listed
    assert!(!MatrixChannel::sender_allowed(&list, "")); // empty sender rejected
    // Exact-match semantics — no substring / prefix / case folding.
    assert!(!MatrixChannel::sender_allowed(&list, "alice:m.org")); // missing '@'
    assert!(!MatrixChannel::sender_allowed(&list, "@ALICE:M.ORG")); // case-sensitive
}

#[tokio::test]
async fn test_matrix_channel_new_validates() {
    let config = MatrixConfig {
        homeserver: String::new(),
        user_id: String::new(),
        access_token: String::new(),
        room_id: None,
        allow_from: Vec::new(),
    };
    assert!(MatrixChannel::new(config, test_bus()).is_err());
}

#[tokio::test]
async fn test_matrix_channel_lifecycle() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: Some("!room:matrix.org".to_string()),
        allow_from: Vec::new(),
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();
    assert_eq!(ch.name(), "matrix");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_process_sync_events() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: Vec::new(),
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();

    let sync = MatrixSyncResponse {
        next_batch: "batch-2".to_string(),
        rooms: Some(MatrixRooms {
            join: Some({
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "!room:matrix.org".to_string(),
                    MatrixJoinedRoom {
                        timeline: Some(MatrixTimeline {
                            events: vec![
                                MatrixEvent {
                                    event_type: "m.room.message".to_string(),
                                    content: Some(MatrixContent {
                                        msgtype: Some("m.text".to_string()),
                                        body: Some("Hello".to_string()),
                                    }),
                                    sender: Some("@user:matrix.org".to_string()),
                                    event_id: Some("$event1".to_string()),
                                    origin_server_ts: Some(1234567890),
                                },
                                MatrixEvent {
                                    event_type: "m.room.message".to_string(),
                                    content: Some(MatrixContent {
                                        msgtype: Some("m.text".to_string()),
                                        body: Some("Bot message".to_string()),
                                    }),
                                    sender: Some("@bot:matrix.org".to_string()),
                                    event_id: Some("$event2".to_string()),
                                    origin_server_ts: Some(1234567891),
                                },
                            ],
                        }),
                    },
                );
                map
            }),
        }),
    };

    let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
    assert_eq!(messages.len(), 1);
    assert_eq!(messages[0].0, "@user:matrix.org");
    assert_eq!(messages[0].2, "Hello");
}

// ---- New tests ----

#[test]
fn test_matrix_config_fields() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".into(),
        user_id: "@bot:matrix.org".into(),
        access_token: "secret".into(),
        room_id: Some("!room:matrix.org".into()),
        allow_from: vec!["@user:matrix.org".into()],
    };
    assert_eq!(config.homeserver, "https://matrix.org");
    assert_eq!(config.user_id, "@bot:matrix.org");
    assert!(config.room_id.is_some());
    assert_eq!(config.allow_from.len(), 1);
}

#[test]
fn test_matrix_sync_response_deserialize() {
    let json = r#"{"next_batch":"s1","rooms":{"join":{"!r:m.org":{"timeline":{"events":[{"type":"m.room.message","content":{"msgtype":"m.text","body":"hi"},"sender":"@u:m.org","event_id":"$e1","origin_server_ts":1}]}}}}}"#;
    let sync: MatrixSyncResponse = serde_json::from_str(json).unwrap();
    assert_eq!(sync.next_batch, "s1");
    assert!(sync.rooms.is_some());
}

#[test]
fn test_matrix_sync_empty_rooms() {
    let json = r#"{"next_batch":"b1"}"#;
    let sync: MatrixSyncResponse = serde_json::from_str(json).unwrap();
    assert!(sync.rooms.is_none());
}

#[test]
fn test_process_sync_empty() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: Vec::new(),
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();

    let sync = MatrixSyncResponse {
        next_batch: "b1".to_string(),
        rooms: None,
    };
    let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
    assert!(messages.is_empty());
}

#[test]
fn test_process_sync_non_message_events_ignored() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: Vec::new(),
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();

    let sync = MatrixSyncResponse {
        next_batch: "b2".to_string(),
        rooms: Some(MatrixRooms {
            join: Some({
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "!room:matrix.org".to_string(),
                    MatrixJoinedRoom {
                        timeline: Some(MatrixTimeline {
                            events: vec![MatrixEvent {
                                event_type: "m.room.member".to_string(),
                                content: Some(MatrixContent {
                                    msgtype: None,
                                    body: None,
                                }),
                                sender: Some("@user:matrix.org".to_string()),
                                event_id: Some("$e1".to_string()),
                                origin_server_ts: Some(1),
                            }],
                        }),
                    },
                );
                map
            }),
        }),
    };
    let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
    assert!(messages.is_empty());
}

#[test]
fn test_process_sync_allow_from_filter() {
    let config = MatrixConfig {
        homeserver: "https://matrix.org".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: vec!["@allowed:matrix.org".to_string()],
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();

    let sync = MatrixSyncResponse {
        next_batch: "b3".to_string(),
        rooms: Some(MatrixRooms {
            join: Some({
                let mut map = std::collections::HashMap::new();
                map.insert(
                    "!room:matrix.org".to_string(),
                    MatrixJoinedRoom {
                        timeline: Some(MatrixTimeline {
                            events: vec![MatrixEvent {
                                event_type: "m.room.message".to_string(),
                                content: Some(MatrixContent {
                                    msgtype: Some("m.text".to_string()),
                                    body: Some("Hello".to_string()),
                                }),
                                sender: Some("@blocked:matrix.org".to_string()),
                                event_id: Some("$e1".to_string()),
                                origin_server_ts: Some(1),
                            }],
                        }),
                    },
                );
                map
            }),
        }),
    };
    let messages = ch.process_sync_events(&sync, "@bot:matrix.org");
    assert!(messages.is_empty()); // sender not in allow_from
}

/// BUG-20261121-050 回归锁：start/stop 必须维护 base running 旗标（trait
/// is_running 契约面——manager 运行中守卫读它，恒 false 时守卫对本通道
/// 失效）；二次 start 走幂等守卫诚实跳过（不 spawn 第二个 sync loop）。
/// homeserver 指 loopback 死端口（连接拒绝，零外网；initial_sync/sync
/// 失败均为非阻断分支）。
#[tokio::test]
async fn b050_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = MatrixConfig {
        homeserver: "http://127.0.0.1:1".to_string(),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: Vec::new(),
    };
    let ch = MatrixChannel::new(config, test_bus()).unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 成功后 trait is_running 必须为 true（BUG-20261121-050）"
    );
    assert!(*ch.running.read());

    // 幂等守卫：已运行时二次 start 诚实跳过（不 spawn 第二个 sync loop）。
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
    assert!(!*ch.running.read());
}
