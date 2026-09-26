//! mqtt 通道测试（独立测试文件——生产文件零内联 `#[cfg(test)]` 纪律）。
//!
//! 契约测试策略：rumqttc 无内建 mock/loopback 测试模式（其 `AsyncClient::
//! from_senders` 需要引入 flume 依赖且只覆盖出站半边），故用 **tokio TCP mock
//! 最小 broker**：手解 MQTT 3.1.1 最小帧集（CONNECT/CONNACK/SUBSCRIBE/SUBACK/
//! PUBLISH/PUBACK/PINGREQ/PINGRESP），驱动**真实 rumqttc 客户端**走完整双向
//! 链路：
//! - 入站契约：broker 下发 QoS1 PUBLISH → 通道解析信封 → bus 收到 InboundMessage；
//! - 出站契约：`send()` → 学习到的 reply_topic → broker 收到 QoS1 PUBLISH；
//! - 重连契约：连接被 broker 断开 → 自动重连 → ConnAck 后重新订阅（IoT 叙事核心）。
//!
//! 真实 broker（mosquitto/EMQX 等）端到端手测挂账（无凭证）。

use std::time::Duration;

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tokio::sync::broadcast;

use super::*;
use crate::base::Channel;
use nemesis_types::channel::OutboundMessage;

// ---------------------------------------------------------------------------
// MQTT 3.1.1 最小帧编解码（mock broker 专用）
// ---------------------------------------------------------------------------

/// MQTT 字符串：2 字节大端长度 + UTF-8 字节。
fn mqtt_string(s: &str) -> Vec<u8> {
    let b = s.as_bytes();
    let mut v = (b.len() as u16).to_be_bytes().to_vec();
    v.extend_from_slice(b);
    v
}

/// 剩余长度 varint 编码（MQTT 3.1.1 §2.2.3）。
fn encode_remaining_len(mut len: usize) -> Vec<u8> {
    let mut out = Vec::new();
    loop {
        let mut byte = (len % 128) as u8;
        len /= 128;
        if len > 0 {
            byte |= 0x80;
        }
        out.push(byte);
        if len == 0 {
            break;
        }
    }
    out
}

/// 从流上读一个完整 MQTT 包，返回 (fixed header, body)。
async fn read_packet<R>(reader: &mut R) -> std::io::Result<(u8, Vec<u8>)>
where
    R: AsyncRead + Unpin,
{
    let mut header = [0u8; 1];
    reader.read_exact(&mut header).await?;
    let mut mult = 1u32;
    let mut len = 0u32;
    loop {
        let mut byte = [0u8; 1];
        reader.read_exact(&mut byte).await?;
        len += ((byte[0] & 0x7F) as u32) * mult;
        if byte[0] & 0x80 == 0 {
            break;
        }
        mult *= 128;
    }
    let mut body = vec![0u8; len as usize];
    reader.read_exact(&mut body).await?;
    Ok((header[0], body))
}

/// CONNACK（accepted）。
async fn write_connack<W>(writer: &mut W)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let _ = writer.write_all(&[0x20, 0x02, 0x00, 0x00]).await;
}

/// SUBACK：给定 pkid + 每个.filter 一个 QoS1 成功码。
async fn write_suback<W>(writer: &mut W, pkid: u16, granted: usize)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let mut body = pkid.to_be_bytes().to_vec();
    body.extend(std::iter::repeat_n(0x01u8, granted));
    let mut frame = vec![0x90];
    frame.extend(encode_remaining_len(body.len()));
    frame.extend(body);
    let _ = writer.write_all(&frame).await;
}

/// PUBACK。
async fn write_puback<W>(writer: &mut W, pkid: u16)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let _ = writer
        .write_all(&[0x40, 0x02, (pkid >> 8) as u8, (pkid & 0xFF) as u8])
        .await;
}

/// PINGRESP。
async fn write_pingresp<W>(writer: &mut W)
where
    W: tokio::io::AsyncWrite + Unpin,
{
    let _ = writer.write_all(&[0xD0, 0x00]).await;
}

/// broker → client 的 QoS1 PUBLISH 帧。
fn broker_publish_frame(topic: &str, pkid: u16, payload: &[u8]) -> Vec<u8> {
    let mut body = mqtt_string(topic);
    body.extend(pkid.to_be_bytes());
    body.extend_from_slice(payload);
    let mut frame = vec![0x32]; // PUBLISH，DUP=0，QoS=1，RETAIN=0
    frame.extend(encode_remaining_len(body.len()));
    frame.extend(body);
    frame
}

/// 解析 CONNECT body 里的 client id（v4 布局：协议名(2+len) + level + flags +
/// keepalive(2) + payload 首字符串）。
fn parse_connect_client_id(body: &[u8]) -> String {
    if body.len() < 12 {
        return String::new();
    }
    let id_len = u16::from_be_bytes([body[10], body[11]]) as usize;
    let end = (12 + id_len).min(body.len());
    String::from_utf8_lossy(&body[12..end]).to_string()
}

/// 解析 SUBSCRIBE body → (pkid, filter 列表)。v4 无属性段。
fn parse_subscribe(body: &[u8]) -> (u16, Vec<String>) {
    if body.len() < 2 {
        return (0, Vec::new());
    }
    let pkid = u16::from_be_bytes([body[0], body[1]]);
    let mut filters = Vec::new();
    let mut i = 2;
    while i + 2 <= body.len() {
        let len = u16::from_be_bytes([body[i], body[i + 1]]) as usize;
        i += 2;
        if i + len > body.len() {
            break;
        }
        filters.push(String::from_utf8_lossy(&body[i..i + len]).to_string());
        i += len + 1; // 跳过 qos 字节
    }
    (pkid, filters)
}

/// 解析 client → broker 的 PUBLISH body → (topic, payload, pkid)。
fn parse_client_publish(body: &[u8], qos: u8) -> (String, Vec<u8>, u16) {
    if body.len() < 2 {
        return (String::new(), Vec::new(), 0);
    }
    let tlen = u16::from_be_bytes([body[0], body[1]]) as usize;
    let topic = String::from_utf8_lossy(&body[2..(2 + tlen).min(body.len())]).to_string();
    let mut pos = 2 + tlen;
    let mut pkid = 0u16;
    if qos > 0 && pos + 2 <= body.len() {
        pkid = u16::from_be_bytes([body[pos], body[pos + 1]]);
        pos += 2;
    }
    (topic, body[pos..].to_vec(), pkid)
}

// ---------------------------------------------------------------------------
// Mock broker
// ---------------------------------------------------------------------------

/// mock broker 上报给测试的事件。
#[derive(Debug)]
enum MockEvent {
    /// 收到 CONNECT（带 client id）。
    Connect(String),
    /// 收到 SUBSCRIBE（每个 filter 一条）。
    Subscribe(String),
    /// 收到 client 的 PUBLISH。
    Publish {
        topic: String,
        payload: Vec<u8>,
        qos: u8,
    },
}

/// mock broker 主循环：循环 accept，每个连接做最小握手/包交换。
///
/// `scripted`：首个连接在 SUBACK 后主动下发的 (topic, payload)（None = 纯回声模式）。
/// `close_after_scripted_ack`：true = 观察到 client 对 scripted 包的 PUBACK 后
/// 主动断开连接（触发客户端重连路径，供重连契约测试用）。
async fn run_mock_broker(
    listener: TcpListener,
    scripted: Option<(String, Vec<u8>)>,
    close_after_scripted_ack: bool,
    tx: tokio::sync::mpsc::UnboundedSender<MockEvent>,
) {
    let mut scripted_sent = false;
    let mut scripted_acked = false;
    // 主动断开只做一次（重连契约：第一次连接被关后，后续连接保持打开）
    let mut dropped_once = false;
    loop {
        let (stream, _) = match listener.accept().await {
            Ok(x) => x,
            Err(_) => return,
        };
        let (mut reader, mut writer) = stream.into_split();
        loop {
            let (header, body) = match read_packet(&mut reader).await {
                Ok(x) => x,
                Err(_) => break, // 连接关闭/异常 → 回到 accept 等重连
            };
            match header & 0xF0 {
                0x10 => {
                    // CONNECT → CONNACK
                    let _ = tx.send(MockEvent::Connect(parse_connect_client_id(&body)));
                    write_connack(&mut writer).await;
                }
                0x80 => {
                    // SUBSCRIBE → SUBACK（每个 filter 一个 QoS1 成功码）
                    let (pkid, filters) = parse_subscribe(&body);
                    for f in &filters {
                        let _ = tx.send(MockEvent::Subscribe(f.clone()));
                    }
                    write_suback(&mut writer, pkid, filters.len()).await;
                    if !scripted_sent && let Some((ref topic, ref payload)) = scripted {
                        scripted_sent = true;
                        let _ = writer
                            .write_all(&broker_publish_frame(topic, 7, payload))
                            .await;
                    }
                }
                0x30 => {
                    // client → broker 的 PUBLISH：记录 + PUBACK
                    let qos = (header & 0x06) >> 1;
                    let (topic, payload, pkid) = parse_client_publish(&body, qos);
                    let _ = tx.send(MockEvent::Publish {
                        topic,
                        payload,
                        qos,
                    });
                    if qos > 0 {
                        write_puback(&mut writer, pkid).await;
                    }
                }
                0x40 => {
                    // client 对 broker 下发 QoS1 包的 PUBACK
                    if scripted_sent {
                        scripted_acked = true;
                    }
                }
                0xC0 => write_pingresp(&mut writer).await, // PINGREQ → PINGRESP
                _ => {}
            }
            if close_after_scripted_ack && scripted_acked && scripted_sent && !dropped_once {
                dropped_once = true;
                break; // 主动断开一次：触发客户端重连；后续连接保持打开
            }
        }
    }
}

/// 拉起 mock broker，返回 (端口, 事件接收端, 任务句柄)。
async fn spawn_mock(
    scripted: Option<(String, Vec<u8>)>,
    close_after_scripted_ack: bool,
) -> (
    u16,
    tokio::sync::mpsc::UnboundedReceiver<MockEvent>,
    tokio::task::JoinHandle<()>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let task = tokio::spawn(run_mock_broker(
        listener,
        scripted,
        close_after_scripted_ack,
        tx,
    ));
    (port, rx, task)
}

/// 构造连接本机 mock 的通道配置（QoS 1、1s 重连退避、给定的订阅映射表）。
fn mock_channel_config(port: u16, topics: Vec<MqttTopicMapping>) -> MqttChannelConfig {
    MqttChannelConfig {
        broker_host: "127.0.0.1".to_string(),
        broker_port: port,
        reconnect_delay_secs: 1,
        qos: 1,
        topics,
        ..Default::default()
    }
}

/// 在超时内收下一个 mock 事件。
async fn next_event(rx: &mut tokio::sync::mpsc::UnboundedReceiver<MockEvent>) -> MockEvent {
    tokio::time::timeout(Duration::from_secs(20), rx.recv())
        .await
        .expect("等待 mock 事件超时")
        .expect("mock 事件通道关闭")
}

// ---------------------------------------------------------------------------
// 契约测试：入站 + 出站 + 重连（真实 rumqttc 客户端 × TCP mock broker）
// ---------------------------------------------------------------------------

/// 全链路契约：CONNECT/SUBSCRIBE 握手 → broker 下发 JSON 信封 → bus 收到
/// InboundMessage（topic=chat_id）→ send() 学习到的 reply_topic → broker 收到
/// QoS1 出站 PUBLISH。
#[tokio::test]
async fn contract_mock_broker_roundtrip() {
    let scripted_payload =
        br#"{"content":"hello from mock","sender_id":"sensor-1","reply_topic":"home/cmd"}"#
            .to_vec();
    let (port, mut mock_rx, mock_task) =
        spawn_mock(Some(("home/notify".to_string(), scripted_payload)), false).await;

    let (bus_tx, mut bus_rx) = broadcast::channel(16);
    let ch = MqttChannel::new(
        mock_channel_config(
            port,
            vec![MqttTopicMapping {
                topic: "home/notify".to_string(),
                reply_topic: String::new(),
            }],
        ),
        bus_tx.clone(),
    )
    .unwrap();
    ch.start().await.unwrap();

    // ── 入站：broker 下发 → bus ──
    let inbound = tokio::time::timeout(Duration::from_secs(20), bus_rx.recv())
        .await
        .expect("等待入站消息超时")
        .unwrap();
    assert_eq!(inbound.channel, "mqtt");
    assert_eq!(inbound.content, "hello from mock");
    assert_eq!(inbound.chat_id, "home/notify");
    assert_eq!(inbound.session_key, "home/notify");
    assert_eq!(inbound.sender_id, "sensor-1");
    assert_eq!(
        inbound.metadata.get("mqtt_topic").map(String::as_str),
        Some("home/notify")
    );
    assert_eq!(
        inbound.metadata.get("mqtt_qos").map(String::as_str),
        Some("1")
    );

    // mock 侧：CONNECT + SUBSCRIBE home/notify
    let mut saw_subscribe = false;
    for _ in 0..4 {
        match next_event(&mut mock_rx).await {
            MockEvent::Connect(id) => {
                assert!(id.starts_with("nemesisbot"), "默认 client id 形态: {id}");
            }
            MockEvent::Subscribe(f) => {
                if f == "home/notify" {
                    saw_subscribe = true;
                    break;
                }
            }
            MockEvent::Publish { .. } => panic!("此阶段不应有 client PUBLISH"),
        }
    }
    assert!(saw_subscribe, "mock 必须看到 SUBSCRIBE home/notify");

    // ── 出站：send（chat_id=入站 topic）→ 学习到的 home/cmd ──
    ch.send(OutboundMessage::new(
        "mqtt",
        "home/notify",
        "bot reply content",
    ))
    .await
    .unwrap();

    match next_event(&mut mock_rx).await {
        MockEvent::Publish {
            topic,
            payload,
            qos,
        } => {
            assert_eq!(qos, 1, "出站应为 QoS 1");
            assert_eq!(topic, "home/cmd", "应发到入站信封学习到的 reply_topic");
            let text = String::from_utf8_lossy(&payload).to_string();
            // 出站恒带防回环信封（from="nemesisbot"），入站 Skip 兜底才真正生效
            let envelope: serde_json::Value =
                serde_json::from_str(&text).expect("出站 payload 必须是 JSON 信封");
            assert_eq!(envelope["from"], json!("nemesisbot"), "防回环标记缺失: {text}");
            assert_eq!(envelope["content"], json!("bot reply content"));
        }
        other => panic!("期望出站 PUBLISH，收到 {other:?}"),
    }

    ch.stop().await.unwrap();
    // mock 在客户端断开后自然退出
    let _ = tokio::time::timeout(Duration::from_secs(5), mock_task).await;
}

/// 重连契约：broker 主动断开（scripted 包被 ACK 后关连接）→ 客户端自动重连 →
/// ConnAck 后重新订阅（clean_session 下订阅关系丢失，重订阅是 IoT 韧性核心）。
#[tokio::test]
async fn contract_reconnect_resubscribes_after_broker_drop() {
    let scripted_payload = b"reconnect probe".to_vec();
    let (port, mut mock_rx, mock_task) =
        spawn_mock(Some(("home/probe".to_string(), scripted_payload)), true).await;

    let (bus_tx, mut bus_rx) = broadcast::channel(16);
    let ch = MqttChannel::new(
        mock_channel_config(
            port,
            vec![MqttTopicMapping {
                topic: "home/probe".to_string(),
                reply_topic: String::new(),
            }],
        ),
        bus_tx.clone(),
    )
    .unwrap();
    ch.start().await.unwrap();

    // 第一条连接：入站可达 + 订阅一次
    let inbound = tokio::time::timeout(Duration::from_secs(20), bus_rx.recv())
        .await
        .expect("等待入站消息超时")
        .unwrap();
    assert_eq!(inbound.content, "reconnect probe");

    // 等到 broker 断开后的第二条连接：CONNECT + 再次 SUBSCRIBE
    let mut subscribe_count = 0;
    let mut connect_count = 0;
    for _ in 0..8 {
        match next_event(&mut mock_rx).await {
            MockEvent::Connect(_) => connect_count += 1,
            MockEvent::Subscribe(f) => {
                assert_eq!(f, "home/probe");
                subscribe_count += 1;
                if connect_count >= 2 && subscribe_count >= 2 {
                    break;
                }
            }
            MockEvent::Publish { .. } => {}
        }
    }
    assert!(
        connect_count >= 2 && subscribe_count >= 2,
        "断开后必须重连并重订阅（connect={connect_count}, subscribe={subscribe_count}）"
    );

    ch.stop().await.unwrap();
    let _ = tokio::time::timeout(Duration::from_secs(5), mock_task).await;
}

// ---------------------------------------------------------------------------
// 单元测试：payload 解析
// ---------------------------------------------------------------------------

#[test]
fn parse_payload_json_envelope() {
    let raw = br#"{"content":"hi","sender_id":"dev1","reply_topic":"cmd/1"}"#;
    assert_eq!(
        parse_payload(raw),
        ParsedPayload::Message {
            content: "hi".to_string(),
            sender_id: Some("dev1".to_string()),
            reply_topic: Some("cmd/1".to_string()),
        }
    );
}

#[test]
fn parse_payload_plain_text_fallback() {
    assert_eq!(
        parse_payload(b"plain note\n"),
        ParsedPayload::Message {
            content: "plain note".to_string(),
            sender_id: None,
            reply_topic: None,
        }
    );
}

#[test]
fn parse_payload_json_without_content_falls_back_to_raw() {
    let raw = br#"{"foo":1,"bar":"x"}"#;
    assert_eq!(
        parse_payload(raw),
        ParsedPayload::Message {
            content: String::from_utf8_lossy(raw).to_string(),
            sender_id: None,
            reply_topic: None,
        }
    );
}

#[test]
fn parse_payload_anti_loop_marker_skips() {
    assert_eq!(
        parse_payload(br#"{"content":"echo","from":"nemesisbot"}"#),
        ParsedPayload::Skip
    );
}

#[test]
fn parse_payload_empty_skips() {
    assert_eq!(parse_payload(b""), ParsedPayload::Skip);
}

#[test]
fn parse_payload_invalid_utf8_lossy() {
    let parsed = parse_payload(&[0xFF, b'a', b'b']);
    match parsed {
        ParsedPayload::Message { content, .. } => {
            assert!(
                content.contains("ab"),
                "lossy 文本应保留可读部分: {content}"
            );
        }
        other => panic!("期望 Message，收到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// 单元测试：topic filter 匹配
// ---------------------------------------------------------------------------

#[test]
fn topic_match_exact() {
    assert!(mqtt_topic_matches("a/b", "a/b"));
    assert!(!mqtt_topic_matches("a/b", "a/b/c"));
    assert!(!mqtt_topic_matches("a/b", "a"));
}

#[test]
fn topic_match_plus_single_level() {
    assert!(mqtt_topic_matches("a/+/c", "a/b/c"));
    assert!(mqtt_topic_matches("a/+/c", "a//c"), "+ 匹配空层级");
    assert!(!mqtt_topic_matches("a/+/c", "a/b/c/d"), "+ 不跨层");
    assert!(!mqtt_topic_matches("+", "a/b"));
}

#[test]
fn topic_match_hash_tail() {
    assert!(mqtt_topic_matches("a/#", "a"), "父级 # 匹配父级本身");
    assert!(mqtt_topic_matches("a/#", "a/b/c"));
    assert!(!mqtt_topic_matches("a/#", "ab/c"));
}

#[test]
fn topic_match_hash_alone() {
    assert!(mqtt_topic_matches("#", "anything"));
    assert!(mqtt_topic_matches("#", "a/b/c"));
}

#[test]
fn topic_match_mismatch_levels() {
    assert!(!mqtt_topic_matches("a/b/#", "a/x/y"));
    assert!(mqtt_topic_matches("a/b/#", "a/b"));
}

// ---------------------------------------------------------------------------
// 单元测试：回包 topic 解析优先级 + config 校验 + 未启动出站
// ---------------------------------------------------------------------------

/// 构造带映射表的通道（不 start——解析逻辑只读 config/学习表）。
fn make_channel(topics: Vec<MqttTopicMapping>, default_reply: &str) -> MqttChannel {
    let (tx, _rx) = broadcast::channel(4);
    MqttChannel::new(
        MqttChannelConfig {
            broker_host: "127.0.0.1".to_string(),
            topics,
            default_reply_topic: default_reply.to_string(),
            ..Default::default()
        },
        tx,
    )
    .unwrap()
}

#[test]
fn reply_topic_precedence_learned_over_map_over_default() {
    let ch = make_channel(
        vec![MqttTopicMapping {
            topic: "sensors/+/state".to_string(),
            reply_topic: "sensors/cmd/map".to_string(),
        }],
        "default/reply",
    );
    // ② 映射命中（通配 filter 匹配具体 chat_id）
    assert_eq!(
        ch.resolve_reply_topic("sensors/kitchen/state").as_deref(),
        Some("sensors/cmd/map")
    );
    // ① 学习条目覆盖映射
    ch.reply_topics.insert(
        "sensors/kitchen/state".to_string(),
        "learned/cmd".to_string(),
    );
    assert_eq!(
        ch.resolve_reply_topic("sensors/kitchen/state").as_deref(),
        Some("learned/cmd")
    );
    // ③ 未命中映射 → 默认回包
    assert_eq!(
        ch.resolve_reply_topic("other/topic").as_deref(),
        Some("default/reply")
    );
    // ④ 三者皆缺位 → None（出站诚实报错）
    let bare = make_channel(Vec::new(), "");
    assert_eq!(bare.resolve_reply_topic("orphan/topic"), None);
}

#[test]
fn config_validation_rejects_bad_qos() {
    let (tx, _rx) = broadcast::channel(4);
    let err = MqttChannel::new(
        MqttChannelConfig {
            broker_host: "127.0.0.1".to_string(),
            qos: 3,
            ..Default::default()
        },
        tx.clone(),
    )
    .unwrap_err();
    assert!(err.to_string().contains("qos"), "报错应指明 qos: {err}");
}

#[test]
fn config_validation_rejects_empty_host_and_filter() {
    let (tx, _rx) = broadcast::channel(4);
    assert!(
        MqttChannel::new(
            MqttChannelConfig {
                broker_host: "  ".to_string(),
                ..Default::default()
            },
            tx.clone(),
        )
        .is_err()
    );
    assert!(
        MqttChannel::new(
            MqttChannelConfig {
                broker_host: "127.0.0.1".to_string(),
                topics: vec![MqttTopicMapping {
                    topic: " ".to_string(),
                    reply_topic: String::new(),
                }],
                ..Default::default()
            },
            tx,
        )
        .is_err()
    );
}

#[tokio::test]
async fn send_before_start_errors() {
    let ch = make_channel(Vec::new(), "default/reply");
    let err = ch
        .send(OutboundMessage::new("mqtt", "a/b", "hi"))
        .await
        .unwrap_err();
    assert!(
        err.to_string().contains("not running"),
        "未启动出站应诚实报错: {err}"
    );
}
