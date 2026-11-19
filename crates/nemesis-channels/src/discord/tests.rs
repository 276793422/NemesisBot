use super::*;

#[tokio::test]
async fn test_discord_channel_new_validates_token() {
    let config = DiscordConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let result = DiscordChannel::new(config, tx);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discord_channel_lifecycle() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DiscordChannel::new(config, tx.clone()).unwrap();
    assert_eq!(ch.name(), "discord");

    // Note: start() will try to connect to Discord, so we don't test the
    // full lifecycle here. Just test that it initializes correctly.
    assert!(!*ch.running.read());
}

#[test]
fn test_split_message_short() {
    let chunks = DiscordChannel::split_message("hello", 2000);
    assert_eq!(chunks, vec!["hello"]);
}

#[test]
fn test_split_message_long() {
    let long = "a ".repeat(1500); // 3000 chars
    let chunks = DiscordChannel::split_message(&long, 2000);
    assert!(chunks.len() > 1);
    for chunk in &chunks {
        assert!(chunk.len() <= 2000);
    }
    // Reconstructed content should match (minus trailing spaces from split)
    let reconstructed: String = chunks.join(" ");
    assert_eq!(reconstructed.trim(), long.trim());
}

#[test]
fn test_split_message_at_newline() {
    let msg = "line1\nline2\n";
    let chunks = DiscordChannel::split_message(msg, 8);
    assert_eq!(chunks.len(), 2);
}

#[tokio::test]
async fn test_parse_gateway_message_basic() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Hello agent!",
        "author": {
            "id": "user456",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
        .await
        .unwrap();
    assert_eq!(msg.channel, "discord");
    assert_eq!(msg.sender_id, "user456");
    assert_eq!(msg.chat_id, "ch1");
    assert_eq!(msg.content, "Hello agent!");
}

#[tokio::test]
async fn test_parse_gateway_message_filters_bot() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "My own message",
        "author": {
            "id": "bot123",
            "username": "nemesisbot",
            "discriminator": "0"
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_filters_other_bots() {
    let bot_id = Arc::new(TokioRwLock::new(Some("bot123".to_string())));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Bot message",
        "author": {
            "id": "other_bot",
            "username": "somebot",
            "discriminator": "0",
            "bot": true
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_allowed_users() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "Hello",
        "author": {
            "id": "user999",
            "username": "bob",
            "discriminator": "0",
            "bot": false
        }
    });

    // Not allowed
    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &["user111".to_string()]).await;
    assert!(msg.is_none());

    // Allowed
    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &["user999".to_string()]).await;
    assert!(msg.is_some());
}

#[tokio::test]
async fn test_parse_gateway_message_empty_content() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "content": "",
        "author": {
            "id": "user1",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[]).await;
    assert!(msg.is_none());
}

#[tokio::test]
async fn test_parse_gateway_message_guild() {
    let bot_id = Arc::new(TokioRwLock::new(None));
    let d = serde_json::json!({
        "id": "msg1",
        "channel_id": "ch1",
        "guild_id": "guild1",
        "content": "Hello guild!",
        "author": {
            "id": "user1",
            "username": "alice",
            "discriminator": "0",
            "bot": false
        }
    });

    let msg = DiscordChannel::parse_gateway_message(&d, &bot_id, &[])
        .await
        .unwrap();
    assert_eq!(msg.metadata.get("guild_id").unwrap(), "guild1");
    assert_eq!(msg.metadata.get("is_group").unwrap(), "true");
}

#[test]
fn test_build_heartbeat_payload_with_sequence() {
    let payload = build_heartbeat_payload(Some(42));
    assert_eq!(payload["op"], 1);
    assert_eq!(payload["d"], 42);
}

#[test]
fn test_build_heartbeat_payload_without_sequence() {
    let payload = build_heartbeat_payload(None);
    assert_eq!(payload["op"], 1);
    assert!(payload["d"].is_null());
}

#[tokio::test]
async fn test_discord_new_with_client_validates_token() {
    let config = DiscordConfig::default();
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let result = DiscordChannel::new_with_client(config, tx, http);
    assert!(result.is_err());
}

#[tokio::test]
async fn test_discord_new_with_client_success() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let http = reqwest::Client::new();
    let ch = DiscordChannel::new_with_client(config, tx, http).unwrap();
    assert_eq!(ch.name(), "discord");
    assert!(!*ch.running.read());
}

// BUG-20261109-012 回归锁：api_base 尾斜杠归一（构造期写回 = 4 个消费点
// 单一漏斗）——双斜杠路径会让 gateway/bot 与消息 REST 全部 404 断链。
#[tokio::test]
async fn discord_api_base_trailing_slash_normalized() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        api_base: "https://discord.com/api/v10/".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(256);
    let ch = DiscordChannel::new(config, tx.clone()).unwrap();
    assert_eq!(
        ch.config.api_base, "https://discord.com/api/v10",
        "api_base 尾斜杠必须在构造期归一"
    );
    // new_with_client 同漏斗
    let config2 = DiscordConfig {
        token: "t".to_string(),
        api_base: "https://discord.example.com/api/".to_string(),
        ..Default::default()
    };
    let http = crate::base::outbound_http_client().unwrap();
    let ch2 = DiscordChannel::new_with_client(config2, tx.clone(), http).unwrap();
    assert_eq!(ch2.config.api_base, "https://discord.example.com/api");
}

/// 回归锁（BUG-20261109-023）：HELLO heartbeat_interval 钳位——缺失回落
/// 45s、0/极小值钳 1s 下限（tokio interval period=0 panic → 心跳任务死
/// → 永久断连循环）。
#[test]
fn test_sanitize_heartbeat_interval_ms() {
    assert_eq!(super::sanitize_heartbeat_interval_ms(None), 45_000);
    assert_eq!(super::sanitize_heartbeat_interval_ms(Some(0)), 1_000);
    assert_eq!(super::sanitize_heartbeat_interval_ms(Some(1)), 1_000);
    assert_eq!(super::sanitize_heartbeat_interval_ms(Some(999)), 1_000);
    assert_eq!(super::sanitize_heartbeat_interval_ms(Some(41_250)), 41_250);
}

/// tokio `interval` zero-period panic 的行为前提守卫：修复依赖「period 0
/// 必 panic」这一 tokio 契约（interval.rs 文档）——若未来 tokio 版本改
/// 变该行为，此测试先红，提示重新评估钳位下限的必要性。
#[tokio::test]
#[should_panic(expected = "`period` must be non-zero")]
async fn test_interval_zero_period_panics() {
    let _ = tokio::time::interval(std::time::Duration::from_millis(0));
}

/// 回归锁（BUG-20261117-079）：断线重连 RESUME 资格判定——session_id 与
/// seq 双全才可 RESUME；任一缺失/空串回 IDENTIFY。修复前 HELLO 后无条件
/// IDENTIFY：断线窗口内消息全丢 + 反复 IDENTIFY 撞 identify rate limit。
#[test]
fn test_can_resume_session() {
    // 双全 = RESUME
    assert!(super::can_resume_session(Some("sess-abc"), Some(42)));
    // session 缺失/空串 = IDENTIFY
    assert!(!super::can_resume_session(None, Some(42)));
    assert!(!super::can_resume_session(Some(""), Some(42)));
    // seq 缺失 = IDENTIFY（服务器无法对齐重放窗口）
    assert!(!super::can_resume_session(Some("sess-abc"), None));
    // 双缺 = IDENTIFY（首次连接）
    assert!(!super::can_resume_session(None, None));
}

/// RESUME payload 形态钉死（BUG-20261117-079）：op 6 + d 三字段。
#[test]
fn test_build_resume_payload_shape() {
    let p = super::build_resume_payload("tok", "sess-abc", 42);
    assert_eq!(p["op"], super::opcode::RESUME);
    assert_eq!(p["d"]["token"], "tok");
    assert_eq!(p["d"]["session_id"], "sess-abc");
    assert_eq!(p["d"]["seq"], 42);
}

/// op 常量与 Discord 协议文档对齐守卫（BUG-20261117-079）：RESUME 路径
/// 依赖 op 6 正确——常量写错时 RESUME 会被网关当未知 op 忽略，静默退回
/// 重连超时循环。
#[test]
fn test_resume_opcode_constants_align_with_protocol() {
    assert_eq!(super::opcode::IDENTIFY, 2);
    assert_eq!(super::opcode::RESUME, 6);
    assert_eq!(super::opcode::INVALID_SESSION, 9);
    assert_eq!(super::opcode::HELLO, 10);
}
