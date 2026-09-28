//! mattermost 通道测试（P26）：纯函数单测 + mock REST 出站形态断言 +
//! mock WS 网关入站全链路。真实 Mattermost 平台端到端挂账（无凭证）。

use super::*;

// ---------------------------------------------------------------------------
// 配置校验
// ---------------------------------------------------------------------------

#[test]
fn test_new_rejects_empty_base_url() {
    let (tx, _rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: "  ".to_string(),
        bot_token: "tok".to_string(),
        allow_from: vec![],
        channels: vec![],
    };
    let result = MattermostChannel::new(config, tx);
    assert!(result.is_err());
}

#[test]
fn test_new_rejects_bad_scheme() {
    let (tx, _rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: "mattermost.example.com".to_string(),
        bot_token: "tok".to_string(),
        allow_from: vec![],
        channels: vec![],
    };
    let result = MattermostChannel::new(config, tx);
    assert!(result.is_err(), "缺 http(s):// 前缀必须拒绝");
}

#[test]
fn test_new_rejects_empty_token() {
    let (tx, _rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: "https://mm.example.com".to_string(),
        bot_token: String::new(),
        allow_from: vec![],
        channels: vec![],
    };
    let result = MattermostChannel::new(config, tx);
    assert!(result.is_err());
}

#[test]
fn test_new_valid_config_and_name() {
    let (tx, _rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: "https://mm.example.com/".to_string(),
        bot_token: "tok".to_string(),
        allow_from: vec![],
        channels: vec![],
    };
    let ch = MattermostChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "mattermost");
    assert!(!ch.is_running());
    assert!(!*ch.running.read(), "start 前不得标记 running");
}

// ---------------------------------------------------------------------------
// URL / chat_id / 鉴权帧
// ---------------------------------------------------------------------------

#[test]
fn test_ws_url_from_base() {
    assert_eq!(
        MattermostChannel::ws_url_from_base("https://mm.example.com"),
        "wss://mm.example.com/api/v4/websocket"
    );
    assert_eq!(
        MattermostChannel::ws_url_from_base("http://127.0.0.1:8065"),
        "ws://127.0.0.1:8065/api/v4/websocket"
    );
    // 尾斜杠去重，不产生双斜杠
    assert_eq!(
        MattermostChannel::ws_url_from_base("https://mm.example.com/"),
        "wss://mm.example.com/api/v4/websocket"
    );
}

#[test]
fn test_parse_chat_id() {
    let (channel, root) = MattermostChannel::parse_chat_id("chan42");
    assert_eq!(channel, "chan42");
    assert!(root.is_none());

    let (channel, root) = MattermostChannel::parse_chat_id("chan42/roo1");
    assert_eq!(channel, "chan42");
    assert_eq!(root.unwrap(), "roo1");
}

#[test]
fn test_build_auth_message() {
    let msg = MattermostChannel::build_auth_message("secret-tok");
    let v: serde_json::Value = serde_json::from_str(&msg).unwrap();
    assert_eq!(v["seq"], 1);
    assert_eq!(v["action"], "authentication_challenge");
    assert_eq!(v["data"]["token"], "secret-tok");
}

// ---------------------------------------------------------------------------
// 消息格式映射
// ---------------------------------------------------------------------------

#[test]
fn test_strip_bot_mention() {
    assert_eq!(
        MattermostChannel::strip_bot_mention("@testbot 帮我看下日志", "testbot"),
        "帮我看下日志"
    );
    // 未配置 username = 原样返回
    assert_eq!(
        MattermostChannel::strip_bot_mention("@other 你好", ""),
        "@other 你好"
    );
    // 只剥自身提及，其他用户提及保留
    assert_eq!(
        MattermostChannel::strip_bot_mention("@alice @testbot hi", "testbot"),
        "@alice hi"
    );
}

#[test]
fn test_format_mapping_markdown_passthrough() {
    let (tx, _rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: "https://mm.example.com".to_string(),
        bot_token: "tok".to_string(),
        allow_from: vec![],
        channels: vec![],
    };
    let ch = MattermostChannel::new(config, tx).unwrap();
    ch.set_bot_username("testbot".to_string());

    // 入站：markdown 恒等透传（代码块/粗体双端原生），仅剥提及
    let raw = "@testbot **bold** and `code`\n```rust\nfn main() {}\n```";
    let internal = ch.to_internal_text(raw);
    assert!(internal.starts_with("**bold**"), "提及已剥离: {internal}");
    assert!(internal.contains("```rust"), "代码块保留");

    // 出站：内部 markdown → Mattermost markdown 恒等透传
    let out = MattermostChannel::to_mattermost_markdown("  # 标题\n- 列表项  ");
    assert_eq!(out, "# 标题\n- 列表项");
}

// ---------------------------------------------------------------------------
// posted 事件解析
// ---------------------------------------------------------------------------

/// 构造标准 `posted` 事件（data.post 为官方序列化字符串形态）。
fn posted_event(post: serde_json::Value, sender_name: &str) -> serde_json::Value {
    serde_json::json!({
        "event": "posted",
        "data": {
            "channel_display_name": "Town Square",
            "channel_name": "town-square",
            "channel_id": post["channel_id"].as_str().unwrap_or("chan1"),
            "sender_name": sender_name,
            "post": post.to_string(),
        },
        "seq": 6,
    })
}

fn sample_post() -> serde_json::Value {
    serde_json::json!({
        "id": "post1",
        "create_at": 1700000000000i64,
        "user_id": "user1",
        "channel_id": "chan1",
        "root_id": "",
        "message": "hello from mattermost",
        "type": "",
        "props": {},
    })
}

#[test]
fn test_parse_posted_event_basic() {
    let event = posted_event(sample_post(), "@alice");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[])
        .expect("正常事件必须解析成功");
    assert_eq!(msg.channel, "mattermost");
    assert_eq!(msg.sender_id, "user1");
    assert_eq!(msg.chat_id, "chan1");
    assert_eq!(msg.content, "hello from mattermost");
    assert_eq!(msg.metadata.get("post_id").unwrap(), "post1");
    assert_eq!(msg.metadata.get("channel_name").unwrap(), "town-square");
    assert_eq!(msg.metadata.get("sender_name").unwrap(), "alice");
    assert!(msg.metadata.get("was_mentioned").is_none());
}

#[test]
fn test_parse_posted_event_thread() {
    let mut post = sample_post();
    post["root_id"] = serde_json::json!("rootpost9");
    let event = posted_event(post, "@alice");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]).unwrap();
    assert_eq!(msg.chat_id, "chan1/rootpost9", "线程回复复合 chat_id");
    assert_eq!(msg.metadata.get("root_id").unwrap(), "rootpost9");
}

#[test]
fn test_parse_posted_event_post_as_object() {
    // 兼容非标准的对象形态（post 直接是对象而非字符串）
    let mut event = posted_event(sample_post(), "@alice");
    event["data"]["post"] = sample_post();
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]).unwrap();
    assert_eq!(msg.content, "hello from mattermost");
}

#[test]
fn test_parse_posted_event_filters_own_echo() {
    let event = posted_event(sample_post(), "@testbot");
    // user_id == bot_user_id → 自己消息回声，必须过滤（死循环防线）
    let msg = MattermostChannel::parse_posted_event(&event, "user1", "testbot", &[], &[]);
    assert!(msg.is_none(), "自己的消息必须被过滤");
}

#[test]
fn test_parse_posted_event_filters_bot_and_webhook_props() {
    let mut post = sample_post();
    post["props"]["from_bot"] = serde_json::json!("true");
    let event = posted_event(post, "@somebot");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_none(), "from_bot 消息必须过滤");

    let mut post = sample_post();
    post["props"]["from_webhook"] = serde_json::json!("true");
    let event = posted_event(post, "@hook");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_none(), "from_webhook 消息必须过滤");
}

#[test]
fn test_parse_posted_event_filters_system_post() {
    let mut post = sample_post();
    post["type"] = serde_json::json!("system_join_channel");
    let event = posted_event(post, "@alice");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_none(), "系统消息必须过滤");
}

#[test]
fn test_parse_posted_event_empty_text() {
    let mut post = sample_post();
    post["message"] = serde_json::json!("");
    let event = posted_event(post, "@alice");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_none());
}

#[test]
fn test_parse_posted_event_mention_detection() {
    let mut post = sample_post();
    post["message"] = serde_json::json!("@testbot 帮我查下状态");
    let event = posted_event(post, "@alice");
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]).unwrap();
    assert_eq!(msg.metadata.get("was_mentioned").unwrap(), "true");
    assert_eq!(msg.content, "帮我查下状态", "入站提及剥离");
}

#[test]
fn test_parse_posted_event_allow_from() {
    let event = posted_event(sample_post(), "@alice");

    // 不在白名单（按 user_id）→ 拒
    let msg = MattermostChannel::parse_posted_event(
        &event,
        "botid",
        "testbot",
        &["user9".to_string()],
        &[],
    );
    assert!(msg.is_none());

    // 白名单按用户名（含 @ 前缀形态）→ 放行
    let msg = MattermostChannel::parse_posted_event(
        &event,
        "botid",
        "testbot",
        &["@alice".to_string()],
        &[],
    );
    assert!(msg.is_some());

    // 空 = 全放行
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_some());
}

#[test]
fn test_parse_posted_event_channel_filter() {
    // 按频道 ID 过滤
    let event = posted_event(sample_post(), "@alice");
    let msg = MattermostChannel::parse_posted_event(
        &event,
        "botid",
        "testbot",
        &[],
        &["chan9".to_string()],
    );
    assert!(msg.is_none(), "未监听频道必须过滤");

    // 按频道名称过滤（channel_name 映射）
    let msg = MattermostChannel::parse_posted_event(
        &event,
        "botid",
        "testbot",
        &[],
        &["town-square".to_string()],
    );
    assert!(msg.is_some());

    // 空 = 全部频道
    let msg = MattermostChannel::parse_posted_event(&event, "botid", "testbot", &[], &[]);
    assert!(msg.is_some());
}

// ---------------------------------------------------------------------------
// mock REST API：出站请求形态断言
// ---------------------------------------------------------------------------

/// 极简 mock Mattermost REST API：阻塞线程逐连接应答，完整读一个请求
/// （按 Content-Length 补齐 body）后原样捕获，返回 (基地址, 捕获句柄)。
fn spawn_mock_api(response_body: &'static str) -> (String, Arc<std::sync::Mutex<Option<String>>>) {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind mock");
    let addr = listener.local_addr().expect("addr");
    let captured: Arc<std::sync::Mutex<Option<String>>> = Arc::new(std::sync::Mutex::new(None));
    let cap = captured.clone();
    std::thread::spawn(move || {
        let Ok((mut stream, _)) = listener.accept() else {
            return;
        };
        let mut buf = Vec::new();
        let mut chunk = [0u8; 4096];
        // 读到 header 结束（\r\n\r\n）
        loop {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
            if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        // 按 Content-Length 补齐 body
        let head_end = buf
            .windows(4)
            .position(|w| w == b"\r\n\r\n")
            .map(|p| p + 4)
            .unwrap_or(buf.len());
        let content_length = String::from_utf8_lossy(&buf[..head_end])
            .to_lowercase()
            .lines()
            .find_map(|l| {
                l.strip_prefix("content-length:")
                    .and_then(|v| v.trim().parse::<usize>().ok())
            })
            .unwrap_or(0);
        while buf.len() < head_end + content_length {
            let n = stream.read(&mut chunk).unwrap_or(0);
            if n == 0 {
                break;
            }
            buf.extend_from_slice(&chunk[..n]);
        }
        *cap.lock().unwrap() = Some(String::from_utf8_lossy(&buf).to_string());

        let resp = format!(
            "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
            response_body.len(),
            response_body
        );
        let _ = stream.write_all(resp.as_bytes());
    });
    (format!("http://{addr}"), captured)
}

/// 轮询等待 mock 服务端捕获到请求（reqwest 异步发送，需短暂等待）。
fn wait_captured(captured: &std::sync::Mutex<Option<String>>) -> String {
    for _ in 0..200 {
        if let Some(req) = captured.lock().unwrap().clone() {
            return req;
        }
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    panic!("mock server 未捕获到请求");
}

fn test_config(base_url: String) -> MattermostConfig {
    MattermostConfig {
        base_url,
        bot_token: "secret-token".to_string(),
        allow_from: vec![],
        channels: vec![],
    }
}

#[tokio::test]
async fn test_post_message_request_shape() {
    let (base, captured) = spawn_mock_api(r#"{"status":"ok"}"#);
    let (tx, _rx) = broadcast::channel(16);
    let ch = MattermostChannel::new(test_config(base), tx).unwrap();
    *ch.running.write() = true;

    // 复合 chat_id 拆解：chan42 顶层 + root_id 线程形态
    let msg = OutboundMessage::new("mattermost", "chan42/roo1", "**bold** hello");
    ch.send(msg).await.expect("出站发送必须成功");
    assert_eq!(ch.base.messages_sent(), 1);

    let req = wait_captured(&captured);
    let lower = req.to_lowercase();
    assert!(
        lower.starts_with("post /api/v4/posts "),
        "必须 POST /api/v4/posts: {}",
        &req[..req.find("\r\n").unwrap_or(req.len())]
    );
    assert!(
        lower.contains("authorization: bearer secret-token"),
        "必须带 Bearer 鉴权头"
    );

    let body_start = req.find("\r\n\r\n").expect("header/body 分隔") + 4;
    let body: serde_json::Value =
        serde_json::from_str(req[body_start..].trim()).expect("JSON body");
    assert_eq!(body["channel_id"], "chan42");
    assert_eq!(body["root_id"], "roo1");
    assert_eq!(body["message"], "**bold** hello");
}

#[tokio::test]
async fn test_send_requires_running() {
    let (tx, _rx) = broadcast::channel(16);
    let ch = MattermostChannel::new(test_config("https://mm.example.com".to_string()), tx).unwrap();
    let msg = OutboundMessage::new("mattermost", "chan1", "hi");
    let result = ch.send(msg).await;
    assert!(result.is_err(), "未 start 时 send 必须诚实拒绝");
}

#[tokio::test]
async fn test_validate_bot_token_success() {
    let (base, captured) = spawn_mock_api(r#"{"id":"botid9","username":"mybot"}"#);
    let (tx, _rx) = broadcast::channel(16);
    let ch = MattermostChannel::new(test_config(base), tx).unwrap();

    let (user_id, username) = ch.validate_bot_token().await.expect("校验必须成功");
    assert_eq!(user_id, "botid9");
    assert_eq!(username, "mybot");

    let req = wait_captured(&captured);
    let lower = req.to_lowercase();
    assert!(
        lower.starts_with("get /api/v4/users/me "),
        "必须 GET /api/v4/users/me: {}",
        &req[..req.find("\r\n").unwrap_or(req.len())]
    );
    assert!(lower.contains("authorization: bearer secret-token"));
}

// ---------------------------------------------------------------------------
// mock WS 网关：入站全链路 + 鉴权失败路径
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_ws_inbound_flow_publishes_inbound_message() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    let (tx, mut rx) = broadcast::channel(16);
    let config = MattermostConfig {
        base_url: base.clone(),
        bot_token: "tok123".to_string(),
        allow_from: vec![],
        channels: vec![],
    };
    let ch = MattermostChannel::new(config, tx).unwrap();
    ch.set_bot_user_id("botuserid1".to_string());
    ch.set_bot_username("testbot".to_string());

    let ctx = WsCtx {
        bot_token: "tok123".to_string(),
        bot_user_id: ch.bot_user_id.clone(),
        bot_username: ch.bot_username.clone(),
        allow_from: vec![],
        listen_channels: vec![],
        bus_sender: ch.bus_sender.clone(),
    };

    // 服务端：收鉴权帧 → 回 OK → 推 posted 事件 → 关连接
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream)
            .await
            .expect("ws 升级");
        // 1. 收鉴权帧并校验形态
        let auth = ws.next().await.unwrap().expect("鉴权帧");
        let auth_json: serde_json::Value =
            serde_json::from_str(auth.into_text().unwrap().as_str()).unwrap();
        assert_eq!(auth_json["action"], "authentication_challenge");
        assert_eq!(auth_json["seq"], 1);
        assert_eq!(auth_json["data"]["token"], "tok123");
        // 2. 回鉴权 OK
        ws.send(Message::text(r#"{"status":"OK","seq_reply":1}"#))
            .await
            .unwrap();
        // 3. 推 posted 事件（官方 data.post 字符串形态，含 @提及）
        let post = serde_json::json!({
            "id": "postA",
            "user_id": "user1",
            "channel_id": "chanA",
            "root_id": "",
            "message": "@testbot hello over ws",
            "type": "",
            "props": {},
        });
        let event = serde_json::json!({
            "event": "posted",
            "data": {
                "channel_name": "town-square",
                "channel_id": "chanA",
                "sender_name": "@alice",
                "post": post.to_string(),
            },
            "seq": 2,
        });
        ws.send(Message::text(event.to_string())).await.unwrap();
        // 4. 略等客户端消费完再关（关连接 → ws_session 返回 true）
        tokio::time::sleep(std::time::Duration::from_millis(150)).await;
        let _ = ws.close(None).await;
    });

    // 客户端：直连 mock 网关并跑单连接会话
    let url = MattermostChannel::ws_url_from_base(&base);
    let (stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let need_reconnect = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        MattermostChannel::ws_session(stream, ctx),
    )
    .await
    .expect("ws_session 必须在超时前结束");
    assert!(need_reconnect, "服务端主动关闭 = 需要重连");
    server.await.unwrap();

    // 入站消息经 bus 到达，字段全对
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(5), rx.recv())
        .await
        .expect("必须在超时前收到入站消息")
        .expect("bus 未关闭");
    assert_eq!(inbound.channel, "mattermost");
    assert_eq!(inbound.sender_id, "user1");
    assert_eq!(inbound.chat_id, "chanA");
    assert_eq!(inbound.content, "hello over ws", "提及已剥离");
    assert_eq!(inbound.metadata.get("was_mentioned").unwrap(), "true");
    assert_eq!(inbound.metadata.get("sender_name").unwrap(), "alice");
}

#[tokio::test]
async fn test_ws_session_auth_fail_requests_reconnect() {
    use futures::{SinkExt, StreamExt};
    use tokio_tungstenite::tungstenite::Message;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let base = format!("http://{addr}");

    let (tx, _rx) = broadcast::channel(16);
    let ctx = WsCtx {
        bot_token: "bad-token".to_string(),
        bot_user_id: Arc::new(parking_lot::RwLock::new(String::new())),
        bot_username: Arc::new(parking_lot::RwLock::new(String::new())),
        allow_from: vec![],
        listen_channels: vec![],
        bus_sender: tx,
    };

    // 服务端：收鉴权帧 → 回 FAIL → 关闭
    let server = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.unwrap();
        let mut ws = tokio_tungstenite::accept_async(stream).await.unwrap();
        let auth = ws.next().await.unwrap().unwrap();
        let auth_json: serde_json::Value =
            serde_json::from_str(auth.into_text().unwrap().as_str()).unwrap();
        assert_eq!(auth_json["data"]["token"], "bad-token");
        ws.send(Message::text(
            r#"{"status":"FAIL","seq_reply":1,"error":{"message":"Invalid or expired session"}}"#,
        ))
        .await
        .unwrap();
        let _ = ws.close(None).await;
    });

    let url = MattermostChannel::ws_url_from_base(&base);
    let (stream, _) = tokio_tungstenite::connect_async(&url).await.unwrap();
    let need_reconnect = tokio::time::timeout(
        std::time::Duration::from_secs(10),
        MattermostChannel::ws_session(stream, ctx),
    )
    .await
    .expect("ws_session 必须在超时前结束");
    assert!(
        need_reconnect,
        "鉴权 FAIL = 需要重连（交还重连循环退避处理）"
    );
    server.await.unwrap();
}
