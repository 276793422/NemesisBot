//! MessageBus 发送失败分支补充测试（订阅者全部先行 drop → send Err →
//! dropped 计数器递增）。

use super::*;
use nemesis_types::channel::OutboundMessage;

fn inbound() -> InboundMessage {
    InboundMessage {
        channel: "test".to_string(),
        sender_id: "u".to_string(),
        chat_id: "c".to_string(),
        content: "m".to_string(),
        media: vec![],
        session_key: "test:c".to_string(),
        correlation_id: String::new(),
        metadata: std::collections::HashMap::new(),
        voice_playback: None,
    }
}

fn outbound() -> OutboundMessage {
    OutboundMessage {
        channel: "test".to_string(),
        chat_id: "c".to_string(),
        content: "m".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    }
}

/// inbound：接收者存在但已全部 drop → broadcast send Err → dropped 计数。
#[tokio::test]
async fn inbound_send_error_increments_dropped() {
    let bus = MessageBus::new();
    {
        let _rx = bus.subscribe_inbound();
        // drop 前 receiver_count = 1
    } // _rx 在此 drop → receiver_count 归 0
    bus.publish_inbound(inbound());
    assert_eq!(bus.dropped_inbound(), 1);
}

/// outbound：同型路径（send Err → warn + dropped_outbound 递增）。
#[tokio::test]
async fn outbound_send_error_increments_dropped() {
    let bus = MessageBus::new();
    {
        let _rx = bus.subscribe_outbound();
    }
    bus.publish_outbound(outbound());
    assert_eq!(bus.dropped_outbound(), 1);
}

/// 无接收者直接发布 → no-receiver 分支（不进 channel）。
#[test]
fn publish_without_receivers_counts_dropped() {
    let bus = MessageBus::new();
    bus.publish_inbound(inbound());
    bus.publish_outbound(outbound());
    assert_eq!(bus.dropped_inbound(), 1);
    assert_eq!(bus.dropped_outbound(), 1);
}

/// 关闭后发布被拒（closed 分支）。
#[test]
fn closed_bus_rejects_publish() {
    let bus = MessageBus::new();
    let _rx = bus.subscribe_inbound();
    let _rx2 = bus.subscribe_outbound();
    assert!(!bus.is_closed());
    bus.close();
    assert!(bus.is_closed());
    bus.publish_inbound(inbound());
    bus.publish_outbound(outbound());
    assert_eq!(bus.dropped_inbound(), 0);
    assert_eq!(bus.dropped_outbound(), 0);
}

/// 订阅者计数与 fan-out warn-once 旗标（第二次订阅触发 warned）。
#[test]
fn subscriber_count_and_sender_helpers() {
    let bus = MessageBus::new();
    assert_eq!(bus.inbound_subscriber_count(), 0);
    assert_eq!(bus.outbound_subscriber_count(), 0);
    let _rx = bus.subscribe_inbound();
    let _rx2 = bus.subscribe_inbound(); // 第二个订阅者 → fan-out warn 一次
    let _ox = bus.subscribe_outbound();
    let _ox2 = bus.subscribe_outbound();
    assert_eq!(bus.inbound_subscriber_count(), 2);
    assert_eq!(bus.outbound_subscriber_count(), 2);
    // sender 克隆通路：绕过 bus 直接 send 也能送达
    let tx = bus.inbound_sender();
    let mut rx = bus.subscribe_inbound();
    tx.send(inbound()).expect("direct send");
    assert_eq!(rx.try_recv().expect("recv").channel, "test");
    let _ = bus.outbound_sender();
}
