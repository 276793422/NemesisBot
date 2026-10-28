use super::*;

#[tokio::test]
async fn test_dingtalk_channel_new_validates() {
    let config = DingTalkConfig {
        client_id: String::new(),
        client_secret: String::new(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(DingTalkChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_dingtalk_channel_lifecycle() {
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "dingtalk");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[tokio::test]
async fn test_dingtalk_send_without_webhook_fails() {
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "dingtalk".to_string(),
        chat_id: "unknown-chat".to_string(),
        content: "Hello".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    assert!(ch.send(msg).await.is_err());
}

#[test]
fn test_extract_callback_content() {
    let data = DingTalkCallbackData {
        text: DingTalkTextContent {
            content: "hello".to_string(),
        },
        sender_staff_id: "staff-1".to_string(),
        sender_nick: "Alice".to_string(),
        conversation_id: "conv-1".to_string(),
        conversation_type: "1".to_string(),
        session_webhook: "https://example.com/webhook".to_string(),
        content: None,
    };
    assert_eq!(DingTalkChannel::extract_callback_content(&data), "hello");
}

#[test]
fn test_parse_and_dispatch_event() {
    let (tx, mut rx) = broadcast::channel(256);
    let session_webhooks = dashmap::DashMap::new();

    let event = StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.conversation.message.receive".to_string()),
            event_id: Some("evt-1".to_string()),
            message_id: None,
        }),
        data: Some(
            serde_json::json!({
                "senderStaffId": "staff-123",
                "senderNick": "Alice",
                "conversationId": "conv-456",
                "conversationType": "1",
                "sessionWebhook": "https://example.com/webhook",
                "text": {
                    "content": "Hello DingTalk"
                },
                "msgtype": "text"
            })
            .to_string(),
        ),
        event_type: None,
    };

    DingTalkChannel::parse_and_dispatch_event(&event, &tx, &session_webhooks, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "dingtalk");
    assert_eq!(inbound.sender_id, "staff-123");
    assert_eq!(inbound.chat_id, "conv-456");
    assert_eq!(inbound.content, "Hello DingTalk");
    assert_eq!(inbound.metadata.get("sender_nick").unwrap(), "Alice");

    // Verify session webhook was stored
    assert_eq!(
        session_webhooks.get("conv-456").map(|w| w.value().clone()),
        Some("https://example.com/webhook".to_string())
    );
}

#[test]
fn test_parse_and_dispatch_event_filtered() {
    let (tx, mut rx) = broadcast::channel(256);
    let session_webhooks = dashmap::DashMap::new();

    let event = StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.conversation.message.receive".to_string()),
            event_id: Some("evt-2".to_string()),
            message_id: None,
        }),
        data: Some(
            serde_json::json!({
                "senderStaffId": "blocked_staff",
                "conversationId": "conv-789",
                "text": {
                    "content": "Blocked message"
                }
            })
            .to_string(),
        ),
        event_type: None,
    };

    DingTalkChannel::parse_and_dispatch_event(
        &event,
        &tx,
        &session_webhooks,
        &["allowed_staff".to_string()],
    );

    assert!(rx.try_recv().is_err());
}

#[test]
fn test_parse_and_dispatch_event_unhandled_type() {
    let (tx, mut rx) = broadcast::channel(256);
    let session_webhooks = dashmap::DashMap::new();

    let event = StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.other.event".to_string()),
            event_id: Some("evt-3".to_string()),
            message_id: None,
        }),
        data: Some("{}".to_string()),
        event_type: None,
    };

    DingTalkChannel::parse_and_dispatch_event(&event, &tx, &session_webhooks, &[]);
    assert!(rx.try_recv().is_err());
}

// BUG-20261029-143：start() 把 session_webhooks 交给流任务的必须是共享
// 克隆——裸 DashMap::clone() 是逐 shard 深拷贝，流事件写入的会话 webhook
// 落私有副本，send() 的读路径（self.session_webhooks.get）读原表恒 miss，
// 钉钉通道每条回复必失败（"no session_webhook found for chat"）。
// 本测试复刻任务侧捕获形态 + 真实写入路径（parse_and_dispatch_event）
// 后，断言结构体侧读路径可见——旧实现（裸 DashMap）红，Arc 化后绿。
#[test]
fn test_session_webhook_shared_between_task_clone_and_read_path() {
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    // 复刻 start() 397 行的捕获形态：与字段同源克隆交给「任务侧」。
    let task_side_map = ch.session_webhooks.clone();

    let (event_tx, event_rx) = broadcast::channel(256);
    drop(event_rx); // 事件分发结果本测试不消费
    let event = StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.conversation.message.receive".to_string()),
            event_id: Some("evt-share-1".to_string()),
            message_id: None,
        }),
        data: Some(
            serde_json::json!({
                "senderStaffId": "staff-123",
                "senderNick": "Alice",
                "conversationId": "conv-share",
                "conversationType": "1",
                "sessionWebhook": "https://example.com/webhook",
                "text": {
                    "content": "Hello DingTalk"
                },
                "msgtype": "text"
            })
            .to_string(),
        ),
        event_type: None,
    };
    DingTalkChannel::parse_and_dispatch_event(&event, &event_tx, &task_side_map, &[]);

    // 结构体侧读路径（send() 同源表达式）：必须看到任务侧写入的 webhook。
    assert_eq!(
        ch.session_webhooks
            .get("conv-share")
            .map(|w| w.value().clone()),
        Some("https://example.com/webhook".to_string()),
        "webhook written via the task-side clone must be visible to the send() read path (BUG-20261029-143)"
    );
}
