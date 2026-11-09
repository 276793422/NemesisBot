use super::*;

#[tokio::test]
async fn test_qq_channel_new_validates() {
    let config = QQConfig {
        app_id: String::new(),
        app_secret: String::new(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(QQChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_qq_channel_lifecycle() {
    let config = QQConfig {
        app_id: "app-123".to_string(),
        app_secret: "secret".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = QQChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "qq");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_is_duplicate() {
    let config = QQConfig {
        app_id: "app-123".to_string(),
        app_secret: "secret".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = QQChannel::new(config, tx).unwrap();

    assert!(!ch.is_duplicate("msg-1"));
    assert!(ch.is_duplicate("msg-1")); // second time is duplicate
    assert!(!ch.is_duplicate("msg-2"));
}

#[test]
fn test_default_config() {
    let config = QQConfig::default();
    assert_eq!(config.api_base, "https://api.sgroup.qq.com");
    assert!(config.app_id.is_empty());
}

#[test]
fn test_handle_c2c_message() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Hello from C2C",
        "author": {
            "user_openid": "user_open_123"
        },
        "id": "msg-c2c-1"
    });

    QQChannel::handle_c2c_message(&data, &tx, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "qq");
    assert_eq!(inbound.sender_id, "user_open_123");
    assert_eq!(inbound.chat_id, "c2c:user_open_123");
    assert_eq!(inbound.content, "Hello from C2C");
    assert_eq!(inbound.metadata.get("message_id").unwrap(), "msg-c2c-1");
    assert_eq!(inbound.metadata.get("chat_type").unwrap(), "c2c");
}

#[test]
fn test_handle_group_message() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Hello from group",
        "author": {
            "member_openid": "member_456"
        },
        "group_openid": "group_789",
        "id": "msg-group-1"
    });

    QQChannel::handle_group_message(&data, &tx, &[]);

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "qq");
    assert_eq!(inbound.sender_id, "member_456");
    assert_eq!(inbound.chat_id, "group:group_789");
    assert_eq!(inbound.content, "Hello from group");
    assert_eq!(inbound.metadata.get("group_openid").unwrap(), "group_789");
    assert_eq!(inbound.metadata.get("chat_type").unwrap(), "group");
}

#[test]
fn test_handle_c2c_message_filtered() {
    let (tx, mut rx) = broadcast::channel(256);

    let data = serde_json::json!({
        "content": "Blocked message",
        "author": {
            "user_openid": "blocked_user"
        },
        "id": "msg-blocked"
    });

    QQChannel::handle_c2c_message(&data, &tx, &["allowed_user".to_string()]);
    assert!(rx.try_recv().is_err());
}

// ---------------------------------------------------------------------------
// 回归锁（BUG-20261108-018）：循环尾退避等待必须真的睡满时长，且可被取消
// 打断。旧形态 `select! { sleep => {}, async {} => {} }` 空 async 臂立即
// Ready，sleep 被完全短路（断连后零间隔重连锤击 QQ 网关）。

#[tokio::test]
async fn b018_backoff_wait_sleeps_full_duration_without_cancel() {
    let (_tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    let slept = QQChannel::backoff_wait(std::time::Duration::from_millis(50), &mut rx).await;
    assert!(slept, "无取消时必须返回 true（睡满）");
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(45),
        "退避等待被短路，实际只等了 {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn b018_backoff_wait_returns_false_on_cancel() {
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    drop(tx); // sender 侧 drop → oneshot 立即完成（取消语义含 drop）
    let slept = QQChannel::backoff_wait(std::time::Duration::from_secs(3600), &mut rx).await;
    assert!(!slept, "取消触发时必须返回 false（打断退避）");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "取消未能打断退避等待，实际耗时 {:?}",
        start.elapsed()
    );
}

// 回归锁（BUG-20261108-018）：出站 token 自愈骨架的被拒判定——只有
// HTTP 非 2xx（token 过期 wire 形态）触发刷新重试；传输层错误与空槽
// 错误前缀不同，不得触发（防无限刷新循环）。

#[test]
fn b018_is_send_rejected_matches_http_rejection_only() {
    assert!(QQChannel::is_send_rejected(&NemesisError::Channel(
        "QQ C2C send error: {\"code\":11253,\"message\":\"access_token has expired\"}".to_string()
    )));
    assert!(QQChannel::is_send_rejected(&NemesisError::Channel(
        "QQ group send error: unauthorized".to_string()
    )));
    assert!(
        !QQChannel::is_send_rejected(&NemesisError::Channel(
            "QQ C2C send failed: connection refused".to_string()
        )),
        "传输层错误不触发 token 刷新重试"
    );
    assert!(
        !QQChannel::is_send_rejected(&NemesisError::Channel(
            "QQ access token not available".to_string()
        )),
        "空槽错误不触发重试（空槽由发送前惰性补种兜底）"
    );
    assert!(!QQChannel::is_send_rejected(&NemesisError::Timeout(
        "x".to_string()
    )));
}

// BUG-20261109-012 回归锁：api_base 尾斜杠归一（构造期写回 = 5 个消费点
// 含 spawn 闭包捕获的单一漏斗）——双斜杠路径会让 accessToken/消息/gateway
// 全部 404 断链。
#[tokio::test]
async fn qq_api_base_trailing_slash_normalized() {
    let config = QQConfig {
        app_id: "app".to_string(),
        app_secret: "sec".to_string(),
        api_base: "https://api.sgroup.qq.com/".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = QQChannel::new(config, tx).unwrap();
    assert_eq!(
        ch.config.api_base, "https://api.sgroup.qq.com",
        "api_base 尾斜杠必须在构造期归一"
    );
}
