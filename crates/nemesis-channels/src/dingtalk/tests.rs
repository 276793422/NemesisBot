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

    DingTalkChannel::parse_and_dispatch_event(
        &event,
        &tx,
        &session_webhooks,
        &[],
        &EventDedup::new(),
    );

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
        &EventDedup::new(),
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

    DingTalkChannel::parse_and_dispatch_event(
        &event,
        &tx,
        &session_webhooks,
        &[],
        &EventDedup::new(),
    );
    assert!(rx.try_recv().is_err());
}

// start() 把 session_webhooks 交给流任务的必须是共享
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
    DingTalkChannel::parse_and_dispatch_event(
        &event,
        &event_tx,
        &task_side_map,
        &[],
        &EventDedup::new(),
    );

    // 结构体侧读路径（send() 同源表达式）：必须看到任务侧写入的 webhook。
    assert_eq!(
        ch.session_webhooks
            .get("conv-share")
            .map(|w| w.value().clone()),
        Some("https://example.com/webhook".to_string()),
        "webhook written via the task-side clone must be visible to the send() read path (BUG-20261029-143)"
    );
}

// ---------------------------------------------------------------------------
// 回归锁（BUG-20261108-017）：循环尾退避等待必须真的睡满时长，且可被取消
// 打断。旧形态 `select! { sleep => {}, async {} => {} }` 空 async 臂立即
// Ready，sleep 被完全短路（断连后零间隔重连锤击钉钉网关）；feishu 同族
// 已修，此处为横向扫描漏网位点。

#[tokio::test]
async fn b017_backoff_wait_sleeps_full_duration_without_cancel() {
    let (_tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    let slept = DingTalkChannel::backoff_wait(std::time::Duration::from_millis(50), &mut rx).await;
    assert!(slept, "无取消时必须返回 true（睡满）");
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(45),
        "退避等待被短路，实际只等了 {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn b017_backoff_wait_returns_false_on_cancel() {
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    drop(tx); // sender 侧 drop → oneshot 立即完成（取消语义含 drop）
    let slept = DingTalkChannel::backoff_wait(std::time::Duration::from_secs(3600), &mut rx).await;
    assert!(!slept, "取消触发时必须返回 false（打断退避）");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "取消未能打断退避等待，实际耗时 {:?}",
        start.elapsed()
    );
}

// 回归锁（BUG-20261108-017）：poll 空载判定——204/空 body 是网关无事件
// 正常态，必须安静跳过而非落进解析臂打双重解析失败警告洪水。

#[test]
fn b017_poll_response_is_empty_semantics() {
    assert!(
        DingTalkChannel::poll_response_is_empty(204, ""),
        "204 必须判空载"
    );
    assert!(
        DingTalkChannel::poll_response_is_empty(200, ""),
        "200 空 body 必须判空载"
    );
    assert!(
        DingTalkChannel::poll_response_is_empty(200, "  \n\t"),
        "纯空白 body 必须判空载"
    );
    assert!(
        !DingTalkChannel::poll_response_is_empty(200, "[]"),
        "有内容的 200（空事件数组）不是空载，照常解析"
    );
    assert!(
        !DingTalkChannel::poll_response_is_empty(500, "server error"),
        "非 2xx 不在此判定内（走 break 重连臂）"
    );
}

// ---------------------------------------------------------------------------
// 错误串凭据脱敏（BUG-20261109-006 同族横向修复）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_direct_reply_transport_error_redacts_access_token() {
    // sessionWebhook URL 官方形态内嵌 ?access_token=；传输失败错误串不得带出。
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    let err = ch
        .send_direct_reply(
            "http://127.0.0.1:9/robot/send?access_token=SECRET-AT-77",
            "hello",
        )
        .await
        .unwrap_err()
        .to_string();
    assert!(
        !err.contains("SECRET-AT-77"),
        "access_token 泄漏进错误串: {err}"
    );
    assert!(
        err.contains("access_token=<redacted>"),
        "脱敏占位缺失: {err}"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261119-181：webhook 200+errcode!=0 业务错误信封必须 loud（镜像
// wecom 同族协议检查——钉钉关键词过滤/过期 sessionWebhook 均回 HTTP 200）
// ---------------------------------------------------------------------------

/// 拉起恒回指定 errcode 信封的 webhook 替身（HTTP 恒 200）。
// axum 是 wecom feature 的 optional 依赖（BUG-20261119-181 测试镜像 wecom
// 同族协议检查时借用）——单独 `--features dingtalk` 编译红，挂 wecom 守卫。
#[cfg(feature = "wecom")]
async fn spawn_dingtalk_errcode_server(errcode: i64) -> std::net::SocketAddr {
    let app = axum::Router::new().route(
        "/robot/send",
        axum::routing::post(move || async move {
            axum::Json(serde_json::json!({"errcode": errcode, "errmsg": "invalid webhook url"}))
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    addr
}

#[cfg(feature = "wecom")]
#[tokio::test]
async fn test_direct_reply_errcode_nonzero_is_loud_error() {
    let addr = spawn_dingtalk_errcode_server(310_000).await;
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    let url = format!("http://{addr}/robot/send?access_token=x");
    let err = ch
        .send_direct_reply(&url, "hello")
        .await
        .expect_err("200+errcode!=0 必须报错而非静默成功");
    let msg = err.to_string();
    assert!(msg.contains("errcode=310000"), "错误串应带业务码: {msg}");
    assert!(
        msg.contains("invalid webhook url"),
        "错误串应带 errmsg: {msg}"
    );
}

#[cfg(feature = "wecom")]
#[tokio::test]
async fn test_text_reply_errcode_nonzero_is_loud_error() {
    let addr = spawn_dingtalk_errcode_server(310_000).await;
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    let url = format!("http://{addr}/robot/send?access_token=x");
    let err = ch
        .send_text_reply(&url, "hello")
        .await
        .expect_err("text reply 200+errcode!=0 必须报错");
    assert!(err.to_string().contains("errcode=310000"));
}

#[cfg(feature = "wecom")]
#[tokio::test]
async fn test_direct_reply_errcode_zero_ok() {
    let addr = spawn_dingtalk_errcode_server(0).await;
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    let url = format!("http://{addr}/robot/send?access_token=x");
    ch.send_direct_reply(&url, "hello")
        .await
        .expect("errcode=0 应成功");
}

/// BUG-20261121-050 回归锁：stop 必须对称清 base running 旗标（trait
/// is_running 契约面——manager 运行中守卫读它，stop 后残留 true 会让
/// 通道再也无法被 start_all 启动）；手动置位模拟 start 家族模板的置位
/// 效果（修复前 stop 不清 base，本测试红）。start 幂等守卫与
/// set_running(true) 接线无法零外网锁住（start 全路径含钉钉 API 调用、
/// 无 api 注入能力）——诚实边界：代码与 feishu b046 完整锁定的家族
/// 模板逐字同构。
#[tokio::test]
async fn b050_stop_clears_base_running_flag() {
    let config = DingTalkConfig {
        client_id: "test-id".to_string(),
        client_secret: "test-secret".to_string(),
        allow_from: Vec::new(),
    };
    let (tx, _rx) = broadcast::channel(16);
    let ch = DingTalkChannel::new(config, tx).unwrap();

    ch.base.set_running(true);
    *ch.running.write() = true;
    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 必须清 base 旗标（BUG-20261121-050）"
    );
    assert!(!*ch.running.read());
}

/// BUG-20261121-053 回归锁：同 eventId 重推收敛——钉钉 Stream EVENT 在客
/// 户端未及时消费时网关侧重试重推，headers.event_id 旧代码解析但零消费
/// （半成品形态），同事件双喂 agent（红证实测 log/r2163_red_b053.log：旧
/// 代码下两次 dispatch 双双入站）。修复后第二条必须被幂等闸丢弃。
#[test]
fn b053_duplicate_event_id_second_delivery_dropped() {
    let (tx, mut rx) = broadcast::channel(16);
    let session_webhooks = dashmap::DashMap::new();
    let dedup = EventDedup::new();
    let mk = || StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.conversation.message.receive".to_string()),
            event_id: Some("evt-dup-1".to_string()),
            message_id: None,
        }),
        data: Some(
            serde_json::json!({
                "senderStaffId": "staff-123",
                "conversationId": "conv-456",
                "text": { "content": "Hello DingTalk" }
            })
            .to_string(),
        ),
        event_type: None,
    };
    DingTalkChannel::parse_and_dispatch_event(&mk(), &tx, &session_webhooks, &[], &dedup);
    assert!(rx.try_recv().is_ok(), "首个事件必须放行");
    DingTalkChannel::parse_and_dispatch_event(&mk(), &tx, &session_webhooks, &[], &dedup);
    assert!(
        rx.try_recv().is_err(),
        "同 eventId 重推必须被幂等闸丢弃（BUG-20261121-053）"
    );
}

/// BUG-20261121-053 回归锁：不同 eventId 互不影响；eventId 缺失 fallback
/// messageId；两者皆缺失保守放行（宁重复不丢，feishu BUG-048 同款取舍）。
#[test]
fn b053_distinct_ids_and_missing_id_pass() {
    let (tx, mut rx) = broadcast::channel(16);
    let session_webhooks = dashmap::DashMap::new();
    let dedup = EventDedup::new();
    let mk = |event_id: Option<&str>, message_id: Option<&str>| StreamEvent {
        headers: Some(StreamEventHeaders {
            event_type: Some("dingtalk.oapi.capi.conversation.message.receive".to_string()),
            event_id: event_id.map(|s| s.to_string()),
            message_id: message_id.map(|s| s.to_string()),
        }),
        data: Some(
            serde_json::json!({
                "senderStaffId": "staff-123",
                "conversationId": "conv-456",
                "text": { "content": "Hello DingTalk" }
            })
            .to_string(),
        ),
        event_type: None,
    };
    // 不同 eventId 互不影响
    DingTalkChannel::parse_and_dispatch_event(
        &mk(Some("evt-a"), None),
        &tx,
        &session_webhooks,
        &[],
        &dedup,
    );
    DingTalkChannel::parse_and_dispatch_event(
        &mk(Some("evt-b"), None),
        &tx,
        &session_webhooks,
        &[],
        &dedup,
    );
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok(), "不同 eventId 必须互不影响");
    // eventId 缺失 → fallback messageId（同 messageId 重推被丢）
    DingTalkChannel::parse_and_dispatch_event(
        &mk(None, Some("msg-1")),
        &tx,
        &session_webhooks,
        &[],
        &dedup,
    );
    assert!(rx.try_recv().is_ok(), "messageId fallback 必须生效");
    DingTalkChannel::parse_and_dispatch_event(
        &mk(None, Some("msg-1")),
        &tx,
        &session_webhooks,
        &[],
        &dedup,
    );
    assert!(rx.try_recv().is_err(), "同 messageId 重推必须被丢");
    // 两者皆缺失 → 保守放行
    DingTalkChannel::parse_and_dispatch_event(&mk(None, None), &tx, &session_webhooks, &[], &dedup);
    DingTalkChannel::parse_and_dispatch_event(&mk(None, None), &tx, &session_webhooks, &[], &dedup);
    assert!(rx.try_recv().is_ok(), "ID 全缺失必须保守放行");
    assert!(rx.try_recv().is_ok(), "ID 全缺失保守放行（宁重复不丢）");
}

/// BUG-20261125-027 回归锁：钉钉 v1.0 /v1.0/oauth2/accessToken 响应 wire
/// 形态为 camelCase `accessToken`/`expireIn`（权威真相源 = 官方 Go SDK
/// alibabacloud-go/dingtalk oauth2_1_0/client.go GetAccessTokenResponseBody
/// json tag）。原 snake_case 无 rename → serde 精确匹配字段名，线上键静默
/// 忽略 → `access_token` 恒 None → refresh_token() 报
/// "no access_token in response"、流 loop Step 1 取 unwrap_or_default()
/// 空串 → "auth returned empty token, backing off" 死循环，通道永远无法
/// 上线。修复后：线上 camelCase 形态必须解出 token；旧 snake_case 形态经
/// alias 兼容；缺 token 响应诚实 None（上游 filter 语义不变）。
#[test]
fn b027_wire_camelcase_token_response_deserializes() {
    // 线上 camelCase 形态（官方 SDK json tag 权威源）
    let body: AccessTokenResponse =
        serde_json::from_str(r#"{"accessToken": "at-wire-123", "expireIn": 7200}"#)
            .expect("线上 camelCase wire 形态必须可反序列化");
    assert_eq!(
        body.access_token.as_deref(),
        Some("at-wire-123"),
        "accessToken 必须落 access_token 字段（修复前恒 None）"
    );
    assert_eq!(body.expire_in, Some(7200), "expireIn 必须落 expire_in 字段");

    // 两个消费点语义：refresh_token() 的 filter 非 empty（229 行）
    let token = body
        .access_token
        .clone()
        .filter(|t| !t.is_empty())
        .expect("线上 token 必须通过非空 filter");
    assert_eq!(token, "at-wire-123");
    // 流 loop Step 1 的 unwrap_or_default（523 行）不再得到空串
    assert!(!body.access_token.clone().unwrap_or_default().is_empty());

    // 旧 snake_case 形态经 alias 兼容（双形态惯例，同文件身份字段同款）
    let legacy: AccessTokenResponse =
        serde_json::from_str(r#"{"access_token": "at-legacy", "expire_in": 3600}"#)
            .expect("旧 snake_case 形态必须经 alias 兼容");
    assert_eq!(legacy.access_token.as_deref(), Some("at-legacy"));
    assert_eq!(legacy.expire_in, Some(3600));

    // 缺 token 响应（错误信封等）诚实 None
    let missing: AccessTokenResponse = serde_json::from_str(r#"{"expireIn": 7200}"#).unwrap();
    assert!(missing.access_token.is_none(), "缺 token 必须诚实 None");
}
