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

// ---------------------------------------------------------------------------
// BUG-20261121-041/042/043/044 回归锁
// ---------------------------------------------------------------------------

/// Bind 127.0.0.1:0，对每个进来的请求回同一份固定 HTTP 响应，直到 listener
/// 被 drop（telegram spawn_status_mock 同族，无新 dev-dep）。
fn spawn_gateway_mock(status_line: &'static str, body: &'static str) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let Ok(mut stream) = stream else { break };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "{status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

/// BUG-20261121-041 回归锁：fetch_gateway_bot_url status 闸（R736 同款，
/// 单一真相源）——401 合法 JSON 体必须报「HTTP 401」+ 透传服务器 message，
/// 不得吞成 missing url；200+url 形态与 v10 查询串拼装钉死；200 缺 url
/// 与 401 报错形态区分。
#[tokio::test]
async fn fetch_gateway_bot_url_status_gate_and_url_shape() {
    // 200 + url → Ok 且带 v10/encoding=json 查询串。
    let port = spawn_gateway_mock("HTTP/1.1 200 OK", r#"{"url":"wss://gateway.discord.gg"}"#);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let url = super::fetch_gateway_bot_url(&http, &format!("http://127.0.0.1:{port}/api"), "tok")
        .await
        .unwrap();
    assert_eq!(url, "wss://gateway.discord.gg/?v=10&encoding=json");

    // 401 合法 JSON → Err 点名 HTTP status + 服务器 message（修复前活路径
    // 把它吞成 "missing 'url' in gateway response"）。
    let port = spawn_gateway_mock(
        "HTTP/1.1 401 Unauthorized",
        r#"{"message":"401: Unauthorized","code":0}"#,
    );
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let err = super::fetch_gateway_bot_url(&http, &format!("http://127.0.0.1:{port}/api"), "tok")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("HTTP 401"), "须点名 status: {err}");
    assert!(
        err.contains("401: Unauthorized"),
        "须透传服务器 message: {err}"
    );

    // 200 但缺 url → 明确报缺键（与 status 失败形态区分）。
    let port = spawn_gateway_mock("HTTP/1.1 200 OK", r#"{"message":"ok"}"#);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();
    let err = super::fetch_gateway_bot_url(&http, &format!("http://127.0.0.1:{port}/api"), "tok")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("missing 'url'"), "缺键形态: {err}");
}

/// BUG-20261121-042 回归锁：进站事件白名单只认 MESSAGE_CREATE——
/// MESSAGE_UPDATE（编辑/embed unfurl）不得进 inbound。
#[test]
fn is_inbound_event_whitelist() {
    assert!(super::is_inbound_event("MESSAGE_CREATE"));
    assert!(!super::is_inbound_event("MESSAGE_UPDATE"));
    assert!(!super::is_inbound_event("READY"));
    assert!(!super::is_inbound_event("RESUMED"));
    assert!(!super::is_inbound_event("TYPING_START"));
    assert!(!super::is_inbound_event(""));
}

/// BUG-20261121-043 回归锁：start/stop 必须维护 base running 旗标
/// （trait is_running 契约面——manager 的运行中守卫读它，恒 false 时守卫
/// 对本通道失效）；api_base 指 loopback 死端口（连接拒绝，零外网），
/// URL fetch 重试循环随 stop 退出。二次 start 走幂等守卫诚实跳过。
#[tokio::test]
async fn start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let config = DiscordConfig {
        token: "test-token".to_string(),
        api_base: "http://127.0.0.1:1".to_string(),
        ..Default::default()
    };
    let (tx, _rx) = broadcast::channel(16);
    let ch = DiscordChannel::new_with_client(config, tx, reqwest::Client::new()).unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 成功后 trait is_running 必须为 true（BUG-20261121-043）"
    );
    assert!(*ch.running.read());

    // 幂等守卫：已运行时二次 start 诚实跳过（不 spawn 第二个 gateway loop）。
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
    assert!(!*ch.running.read());
}

/// BUG-20261121-044 回归锁：分隔符位于 idx 0（前导换行/空格的超长文）
/// 不产出空 chunk——空 chunk 送 Discord 400「Cannot send an empty
/// message」，且 send() 的 `?` 短路会让整条消息投递失败。
#[test]
fn split_message_leading_separator_no_empty_chunk() {
    let long = format!("\n{}", "x".repeat(3000));
    let chunks = DiscordChannel::split_message(&long, 2000);
    assert!(!chunks.is_empty());
    assert!(
        chunks.iter().all(|c| !c.is_empty()),
        "空 chunk 会撞 Discord 400 且中断整条投递: {chunks:?}"
    );

    // 空格前导同款。
    let long2 = format!(" {}", "y".repeat(3000));
    let chunks2 = DiscordChannel::split_message(&long2, 2000);
    assert!(chunks2.iter().all(|c| !c.is_empty()));
}
