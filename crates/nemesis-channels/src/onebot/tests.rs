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

/// BUG-20261113-149：allow_from 白名单判定（家族惯例：空 = 全放行；
/// 精确匹配 QQ user_id；群/私聊一致按发送者判）。
#[test]
fn test_bug149_sender_allowed() {
    let allowlist = vec!["10001".to_string(), "10002".to_string()];

    // 空 = 全放行
    assert!(OneBotChannel::sender_allowed(&[], "99999"));
    // 白名单内
    assert!(OneBotChannel::sender_allowed(&allowlist, "10001"));
    assert!(OneBotChannel::sender_allowed(&allowlist, "10002"));
    // 白名单外（含前缀不匹配形态——判据是精确相等，不是 starts_with）
    assert!(!OneBotChannel::sender_allowed(&allowlist, "99999"));
    assert!(!OneBotChannel::sender_allowed(&allowlist, "1000"));
}

// ===========================================================================
// BUG-20261115-016 回归锁：半死 WS 连接（对端零窗口/进程挂起）上的单帧写必须
// 有界超时（历史：写锁跨无界 sink.send → 出站派发循环停摆 + 清理臂/重连臂
// 抢锁同样挂起 = 三重级联死锁）。
// 形态：ws_send_bounded 泛型 S 用 duplex(64) 传输——对端永不 poll，客户端帧
// 写满 64 字节缓冲后在 tokio 层永远 pending；start_paused 虚拟时钟在运行时
// 空闲时自动推进到超时 deadline——确定性、真实耗时毫秒级。不走真实 TCP：
// Windows loopback RLS 快路径内核缓冲可达 MB 级（SO_SNDBUF 也不被尊重），
// 真实挂起形态跨平台不可复现。
// ===========================================================================

#[tokio::test(start_paused = true)]
async fn test_bug016_bounded_send_times_out_on_wedged_transport() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;
    use tokio_tungstenite::tungstenite::protocol::Role;

    let (client_io, server_io) = tokio::io::duplex(64);
    let server =
        tokio_tungstenite::WebSocketStream::from_raw_socket(server_io, Role::Server, None).await;
    let (_never_read, _never_write) = server.split();
    let ws =
        tokio_tungstenite::WebSocketStream::from_raw_socket(client_io, Role::Client, None).await;
    let (mut sink, _stream) = ws.split();

    let started = std::time::Instant::now();
    let outcome = OneBotChannel::ws_send_bounded(&mut sink, Message::Text("x".repeat(4096).into()))
        .await
        .expect("挂死传输上的写必须返回 TimedOut 而非 Err/永久挂起");
    assert!(
        matches!(outcome, super::BoundedSend::TimedOut),
        "必须命中超时臂而非 Sent: {outcome:?}"
    );
    // 虚拟时钟推进：真实耗时必须远小于超时上限（无真实等待）
    assert!(
        started.elapsed() < std::time::Duration::from_secs(super::WS_SEND_TIMEOUT_SECS),
        "超时应由虚拟时钟触发，真实耗时 {:?} 异常",
        started.elapsed()
    );
}

/// 清槽后的快速失败语义：send 对空槽必须立刻报 not connected
/// （无界时代挂死连接的第二发 send 是第二次永久挂起）。
#[tokio::test]
async fn test_bug016_send_with_cleared_slot_fails_fast_not_connected() {
    let config = OneBotConfig {
        ws_url: "ws://127.0.0.1:1".to_string(),
        access_token: None,
        reconnect_interval: 0,
        group_trigger_prefix: Vec::new(),
        allow_from: Vec::new(),
    };
    let ch = OneBotChannel::new(config, test_bus()).unwrap();
    *ch.running.write() = true;
    // 槽位为 None（超时臂清槽后的形态）
    assert!(ch.ws_sink.read().await.is_none());

    let err = ch
        .send(OutboundMessage {
            channel: "onebot".to_string(),
            chat_id: "private:1".to_string(),
            content: "hi".to_string(),
            message_type: String::new(),
            meta: Default::default(),
        })
        .await
        .expect_err("空槽 send 必须快速失败");
    assert!(
        format!("{err}").contains("not connected"),
        "空槽应报 not connected: {err}"
    );
}
