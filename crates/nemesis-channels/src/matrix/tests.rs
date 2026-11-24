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

// ---- BUG-20261125-020 / BUG-20261125-021 回归锁 ----

/// loopback HTTP 记录器：对第 k 个请求调 `script(k)` 拿 (状态码, body)，
/// 回响应（Connection: close 强制 reqwest 每请求新连接，accept 循环即
/// 请求序列），并记录 (请求行, 到达时刻)。返回 (地址, 命中列表)。
async fn spawn_http_recorder<F>(
    script: F,
) -> (
    std::net::SocketAddr,
    Arc<parking_lot::Mutex<Vec<(String, std::time::Instant)>>>,
)
where
    F: Fn(usize) -> (u16, String) + Send + Sync + 'static,
{
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let hits = Arc::new(parking_lot::Mutex::new(
        Vec::<(String, std::time::Instant)>::new(),
    ));
    let hits_srv = Arc::clone(&hits);
    let script = Arc::new(script);
    let counter = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    tokio::spawn(async move {
        loop {
            let Ok((mut stream, _)) = listener.accept().await else {
                return;
            };
            let hits_srv = Arc::clone(&hits_srv);
            let script = Arc::clone(&script);
            let counter = Arc::clone(&counter);
            tokio::spawn(async move {
                // 读请求头直到 \r\n\r\n（简化解析：只需请求行）。
                let mut buf = vec![0u8; 4096];
                let mut got = 0usize;
                loop {
                    match stream.read(&mut buf[got..]).await {
                        Ok(0) | Err(_) => return,
                        Ok(n) => {
                            got += n;
                            if buf[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                                break;
                            }
                            if got == buf.len() {
                                buf.extend(std::iter::repeat_n(0u8, 4096));
                            }
                            if got > 64 * 1024 {
                                return;
                            }
                        }
                    }
                }
                let head = String::from_utf8_lossy(&buf[..got]).into_owned();
                let request_line = head.lines().next().unwrap_or("").to_string();
                let k = counter.fetch_add(1, std::sync::atomic::Ordering::Relaxed) + 1;
                hits_srv
                    .lock()
                    .push((request_line, std::time::Instant::now()));
                let (status, body) = script(k);
                let reason = if status == 200 { "OK" } else { "Err" };
                let resp = format!(
                    "HTTP/1.1 {status} {reason}\r\nContent-Type: application/json\r\nConnection: close\r\nContent-Length: {}\r\n\r\n{body}",
                    body.len()
                );
                let _ = stream.write_all(resp.as_bytes()).await;
                let _ = stream.shutdown().await;
            });
        }
    });
    (addr, hits)
}

fn loopback_matrix_config(addr: std::net::SocketAddr) -> MatrixConfig {
    MatrixConfig {
        homeserver: format!("http://{addr}"),
        user_id: "@bot:matrix.org".to_string(),
        access_token: "token".to_string(),
        room_id: None,
        allow_from: Vec::new(),
    }
}

/// BUG-20261125-020 回归锁：200-非JSON 响应（反向代理故障页/响应截断形态）
/// 下 sync loop 必须带退避重发，不得紧密循环洪水打 homeserver。脚本：第 1
/// 个请求回合法 initial sync token（让预取段成功），其余回 200 HTML
/// （json parse 必失败）。3s 观测窗 + 0.8s 收尾缓冲内请求数 ∈ [2, 6]：
/// 修复后主循环退避阶梯 1s/2s/4s 至多 4 次；修复前裸 continue 毫秒级循环
/// 打出数百次（上限断言对慢机安全——调度延迟只会减少请求数）。
#[tokio::test]
async fn b020_parse_failure_backs_off_no_tight_loop_flood() {
    let (addr, hits) = spawn_http_recorder(|k| {
        if k == 1 {
            (200, "{\"next_batch\":\"t1\"}".to_string())
        } else {
            (200, "<html>gateway error page</html>".to_string())
        }
    })
    .await;
    let ch = MatrixChannel::new(loopback_matrix_config(addr), test_bus()).unwrap();

    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    ch.stop().await.unwrap();
    // 收尾缓冲：让边缘在途请求落地（stop 时循环可能正睡在 backoff 中）。
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let n = hits.lock().len();
    assert!(
        (2..=6).contains(&n),
        "200-非JSON 响应下观测窗请求数应为 [2,6]（退避阶梯），实际 {n}——\
         紧密循环洪水形态（BUG-20261125-020 修复前）会打出数百次"
    );
}

/// BUG-20261125-021 回归锁：initial sync 失败时 loop 必须在预取段带退避
/// 重试拿 token，不得静默直进主循环发无 since 的全量 /sync（无 since =
/// 全量初始同步语义，历史事件全部当新消息投递）。listener 恒回 500：
/// 观测窗内所有请求必须都是 initial sync 形态（timeout=0 + limit:0）；
/// 修复前 start 静默吞 initial_sync 失败，loop 第一发就是 timeout=30000
/// 的全量形态。
#[tokio::test]
async fn b021_initial_sync_failure_retries_without_full_sync_fallback() {
    let (addr, hits) =
        spawn_http_recorder(|_| (500, "{\"errcode\":\"M_UNKNOWN\"}".to_string())).await;
    let ch = MatrixChannel::new(loopback_matrix_config(addr), test_bus()).unwrap();

    ch.start().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_secs(3)).await;
    ch.stop().await.unwrap();
    tokio::time::sleep(std::time::Duration::from_millis(800)).await;

    let paths: Vec<String> = hits.lock().iter().map(|(line, _)| line.clone()).collect();
    assert!(
        !paths.is_empty(),
        "预取重试应持续发起 initial sync 请求（500 下退避重试）"
    );
    for p in &paths {
        assert!(
            p.contains("timeout=0") && p.contains("limit"),
            "initial sync 失败期间只允许 initial sync 形态请求（timeout=0 limit:0），\
             实际出现 {p:?} = 无 token 直进主循环全量拉取（BUG-20261125-021 修复前形态）"
        );
        assert!(
            !p.contains("timeout=30000"),
            "观测窗内出现主循环全量 sync 请求（timeout=30000）：{p:?}"
        );
    }
}
