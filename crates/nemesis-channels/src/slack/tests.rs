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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new()).unwrap();
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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new());
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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new());
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
    let msg =
        SlackChannel::parse_slack_event(&event, &bot_id, &["U111".to_string()], &EventDedup::new());
    assert!(msg.is_none());

    // Allowed
    let msg =
        SlackChannel::parse_slack_event(&event, &bot_id, &["U456".to_string()], &EventDedup::new());
    assert!(msg.is_some());

    // Empty = allow all
    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new());
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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new());
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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new()).unwrap();
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

    let msg = SlackChannel::parse_slack_event(&event, &bot_id, &[], &EventDedup::new()).unwrap();
    assert_eq!(msg.chat_id, "C789/1700000000.000100");
    assert_eq!(msg.metadata.get("thread_ts").unwrap(), "1700000000.000100");
}

/// BUG-20261121-051 回归锁：非空 subtype 整类丢弃——message_changed（编辑
/// / 链接 unfurl 更新）重灌 = 同一条消息二次喂 agent（discord
/// BUG-20261121-042 同族裁决，R2160 同日定全局立场）。旧测试把编辑重灌
/// 编码为期望行为（BUG-042 之前的旧立场），本测试随裁决翻转：改为断言
/// None。message_deleted / channel_join / message_replied 等其余 subtype
/// 同闸。
#[test]
fn b051_subtypes_dropped_not_refed() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));

    // 编辑（message_changed）
    let edited = serde_json::json!({
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
    assert!(
        SlackChannel::parse_slack_event(&edited, &bot_id, &[], &EventDedup::new()).is_none(),
        "编辑重灌必须整类丢弃（BUG-20261121-051，discord BUG-042 家族一致）"
    );

    // 其余 subtype（message_deleted / channel_join / message_replied）同样丢弃
    for subtype in ["message_deleted", "channel_join", "message_replied"] {
        let ev = serde_json::json!({
            "type": "message",
            "subtype": subtype,
            "user": "U456",
            "channel": "C789",
            "text": "whatever",
            "ts": "1700000000.000200"
        });
        assert!(
            SlackChannel::parse_slack_event(&ev, &bot_id, &[], &EventDedup::new()).is_none(),
            "subtype {subtype} 必须丢弃"
        );
    }
}

/// BUG-20261121-050 回归锁：stop 必须对称清 base running 旗标（trait
/// is_running 契约面——manager 运行中守卫读它，stop 后残留 true 会让
/// 通道再也无法被 start_all 启动）；手动置位模拟 start 家族模板的置位
/// 效果（修复前 stop 不清 base，本测试红）。start 幂等守卫与
/// set_running(true) 接线无法零外网锁住（start 全路径含 Slack API 调用、
/// SLACK_API_BASE 为 crate 常量不可注入）——诚实边界：代码与 feishu
/// b046 完整锁定的家族模板逐字同构；附带顺序修复（spawn 先于 running
/// 置位的自杀窗口）随同构模板一并消除。
#[tokio::test]
async fn b050_stop_clears_base_running_flag() {
    let config = SlackConfig {
        bot_token: "xoxb-test".to_string(),
        app_token: "xapp-test".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(16);
    let ch = SlackChannel::new(config, tx).unwrap();

    ch.base.set_running(true);
    *ch.running.write() = true;
    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 必须清 base 旗标（BUG-20261121-050）"
    );
    assert!(!*ch.running.read());
}

/// BUG-20261121-051 回归锁：同 ts 双投递收敛——Slack 双订阅（message.* +
/// app_mention）下同一条频道 mention 消息结构性双事件（同 channel+ts，
/// 两个 event_id 不同所以 event_id 去重不适用，必须 ts 级），旧代码两个
/// 都放行 = agent 双喂（红证实测：log/r2162_red_b051.log，旧签名下
/// is_some && is_some 断言通过 = 双放行实证）。修复后第二个必须 None。
#[test]
fn b051_duplicate_ts_second_delivery_dropped() {
    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let dedup = EventDedup::new();
    let ev = |typ: &str| {
        serde_json::json!({
            "type": typ,
            "user": "U456",
            "channel": "C789",
            "text": "<@B123> deploy please",
            "ts": "1700000000.000100"
        })
    };
    let first = SlackChannel::parse_slack_event(&ev("message"), &bot_id, &[], &dedup);
    assert!(first.is_some(), "首个事件必须放行");
    let second = SlackChannel::parse_slack_event(&ev("app_mention"), &bot_id, &[], &dedup);
    assert!(
        second.is_none(),
        "同 channel+ts 的 app_mention 必须被幂等闸丢弃"
    );
}

/// BUG-20261121-051 回归锁：不同 ts 互不影响；ts 缺失（守恒默认 "0"）
/// 保守放行宁重复不丢（feishu BUG-048 同款取舍）。
#[test]
fn b051_different_ts_and_missing_ts_pass() {
    let bot_id = Arc::new(parking_lot::RwLock::new(String::new()));
    let dedup = EventDedup::new();
    let ev = |ts: &str| {
        serde_json::json!({
            "type": "message",
            "user": "U456",
            "channel": "C789",
            "text": "hello",
            "ts": ts
        })
    };
    assert!(
        SlackChannel::parse_slack_event(&ev("1700000000.000001"), &bot_id, &[], &dedup).is_some()
    );
    assert!(
        SlackChannel::parse_slack_event(&ev("1700000000.000002"), &bot_id, &[], &dedup).is_some(),
        "不同 ts 互不影响"
    );
    // 同 ts 重复（平台重推形态）被丢
    assert!(
        SlackChannel::parse_slack_event(&ev("1700000000.000001"), &bot_id, &[], &dedup).is_none()
    );
    // ts 缺失保守放行
    let no_ts = serde_json::json!({
        "type": "message",
        "user": "U456",
        "channel": "C789",
        "text": "no ts"
    });
    assert!(
        SlackChannel::parse_slack_event(&no_ts, &bot_id, &[], &dedup).is_some(),
        "ts 缺失必须保守放行（宁重复不丢）"
    );
}

/// BUG-20261121-052 回归锁：统一信封 ack 契约——官方 Socket Mode 协议
/// 要求「除 hello/disconnect 外，每个携带 envelope_id 的信封必须 3 秒内
/// 回 ack」，未 ack 信封 Slack 必重推（重推风暴）。旧实现只在 events_api
/// 臂 ack——slash_commands/interactive 等信封从不 ack。本锁起真 WS
/// 对（loopback，client 直连 server）驱动 handle_envelope 可测单元：
/// events_api 与 slash_commands 两信封都必须收到 ack 帧，且只有
/// events_api 产生 bus 入站。
#[tokio::test]
async fn b052_envelope_ack_universal() {
    use futures::StreamExt;
    use tokio_tungstenite::tungstenite::Message;

    // WS server：发两个信封，收两个 ack
    let listener = tokio::net::TcpListener::bind(("127.0.0.1", 0))
        .await
        .unwrap();
    let port = listener.local_addr().unwrap().port();
    let server = tokio::spawn(async move {
        let (sock, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(sock).await.unwrap();

        // 信封 1：events_api（应 ack + 发布 inbound）
        let env1 = serde_json::json!({
            "type": "events_api",
            "envelope_id": "env-ev-1",
            "payload": { "event": {
                "type": "message",
                "user": "U456",
                "channel": "C789",
                "text": "hello from mock",
                "ts": "1700000000.000100"
            }}
        });
        ws.send(Message::Text(env1.to_string().into()))
            .await
            .unwrap();
        let ack1 = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("events_api ack 超时")
            .expect("ws closed")
            .unwrap();
        let v1: serde_json::Value = serde_json::from_str(&ack1.into_text().unwrap()).unwrap();
        assert_eq!(v1["envelope_id"], "env-ev-1", "events_api 必须回 ack");

        // 信封 2：slash_commands（旧代码从不 ack → 重推风暴；修复后必须 ack）
        let env2 = serde_json::json!({
            "type": "slash_commands",
            "envelope_id": "env-cmd-2",
            "payload": { "command": "/deploy" }
        });
        ws.send(Message::Text(env2.to_string().into()))
            .await
            .unwrap();
        let ack2 = tokio::time::timeout(std::time::Duration::from_secs(3), ws.next())
            .await
            .expect("slash_commands ack 超时（旧代码从不 ack 非 events_api 信封）")
            .expect("ws closed")
            .unwrap();
        let v2: serde_json::Value = serde_json::from_str(&ack2.into_text().unwrap()).unwrap();
        assert_eq!(v2["envelope_id"], "env-cmd-2", "slash_commands 必须回 ack");
    });

    // client：直连 mock server，split 出同型 sink
    let (stream, _) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}"))
        .await
        .expect("loopback ws connect");
    let (mut ws_tx, mut ws_rx) = stream.split();

    let bot_id = Arc::new(parking_lot::RwLock::new("B123".to_string()));
    let dedup = EventDedup::new();
    let (bus_tx, mut bus_rx) = tokio::sync::broadcast::channel(16);

    // 信封 1：events_api → ack + 入站发布
    let text1 = {
        // server 先发 env1；先读一条再交给 handle_envelope
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws_rx.next())
            .await
            .expect("等待 events_api 信封超时")
            .expect("ws closed");
        msg.unwrap().into_text().unwrap()
    };
    let reconnect =
        SlackChannel::handle_envelope(&text1, &mut ws_tx, &bot_id, &[], &dedup, &bus_tx).await;
    assert!(!reconnect, "events_api 信封不应触发重连");

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(3), bus_rx.recv())
        .await
        .expect("events_api 必须发布 inbound")
        .unwrap();
    assert_eq!(inbound.content, "hello from mock");

    // 信封 2：slash_commands → ack，无入站
    let text2 = {
        let msg = tokio::time::timeout(std::time::Duration::from_secs(3), ws_rx.next())
            .await
            .expect("等待 slash_commands 信封超时")
            .expect("ws closed");
        msg.unwrap().into_text().unwrap()
    };
    let reconnect2 =
        SlackChannel::handle_envelope(&text2, &mut ws_tx, &bot_id, &[], &dedup, &bus_tx).await;
    assert!(!reconnect2, "slash_commands 信封不应触发重连");

    // bus 无新增消息（slash_commands 不产生入站）
    let extra = tokio::time::timeout(std::time::Duration::from_millis(200), bus_rx.recv());
    assert!(extra.await.is_err(), "slash_commands 不得产生入站消息");

    server.await.unwrap();
}
