//! wechat 通道测试（P29）：契约测试面 = 验签三方案 / 握手与回调拒绝路径 /
//! 入站消息映射 / 出站请求形态（mock HTTP）/ 真实 TCP 回调全链路。
//!
//! ⚠️ 协议形态为实现假设（D-3 fallback），本文件钉死的是**本模块自洽契约**，
//! 真机接入时按 iLink 官方文档校准后同步更新。

use super::*;

// ---------------------------------------------------------------------------
// 辅助
// ---------------------------------------------------------------------------

/// 找一个临时空闲端口（绑定后释放，小竞态可接受——line/tests.rs 同款）。
fn find_free_port() -> u16 {
    let listener = std::net::TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

/// 连接 host:port，发送原始字节，读回 HTTP 响应文本。
async fn send_raw_http(port: u16, request: &[u8]) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let mut stream = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .expect("connect failed");
    stream.write_all(request).await.expect("write failed");
    let mut buf = vec![0u8; 65536];
    let n = stream.read(&mut buf).await.expect("read failed");
    String::from_utf8_lossy(&buf[..n]).to_string()
}

/// 测试配置（token 只进 config；监听 127.0.0.1 随机端口）。
fn test_config(port: u16) -> WeChatConfig {
    WeChatConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        token: "test-token".to_string(),
        ..WeChatConfig::default()
    }
}

fn test_channel(
    config: WeChatConfig,
) -> (
    WeChatChannel,
    tokio::sync::broadcast::Receiver<InboundMessage>,
) {
    let (tx, rx) = broadcast::channel(64);
    let ch = WeChatChannel::new(config, tx).unwrap();
    (ch, rx)
}

// ---------------------------------------------------------------------------
// Config / 装配校验
// ---------------------------------------------------------------------------

#[test]
fn test_config_defaults_and_resolutions() {
    let cfg = WeChatConfig::default();
    assert_eq!(cfg.base_url, DEFAULT_BASE_URL);
    assert_eq!(cfg.send_path, DEFAULT_SEND_PATH);
    assert_eq!(cfg.callback_path_resolved(), "/wechat/callback");
    assert_eq!(cfg.callback_listen_addr_resolved(), DEFAULT_LISTEN_ADDR);
    assert_eq!(cfg.signature_scheme().unwrap(), SignatureScheme::HmacSha256);

    // 出站 URL 归一：尾部斜杠去除 + path 补斜杠
    let cfg = WeChatConfig {
        base_url: "http://example.com/".to_string(),
        send_path: "v1/send".to_string(),
        ..WeChatConfig::default()
    };
    assert_eq!(cfg.send_url().unwrap(), "http://example.com/v1/send");

    // 空 base_url → None（出站诚实报错，不假设兜底）
    let cfg = WeChatConfig {
        base_url: String::new(),
        ..WeChatConfig::default()
    };
    assert!(cfg.send_url().is_none());
}

#[test]
fn test_new_rejects_empty_token() {
    let (tx, _rx) = broadcast::channel(64);
    let config = WeChatConfig {
        token: String::new(),
        ..WeChatConfig::default()
    };
    assert!(WeChatChannel::new(config, tx).is_err());
}

#[test]
fn test_new_rejects_unknown_signature_scheme() {
    let (tx, _rx) = broadcast::channel(64);
    let config = WeChatConfig {
        token: "t".to_string(),
        signature_scheme: "md5".to_string(),
        ..WeChatConfig::default()
    };
    assert!(WeChatChannel::new(config, tx).is_err());
}

#[test]
fn test_signature_scheme_parse() {
    assert_eq!(
        SignatureScheme::parse(""),
        Some(SignatureScheme::HmacSha256)
    );
    assert_eq!(
        SignatureScheme::parse("HMAC-SHA256"),
        Some(SignatureScheme::HmacSha256)
    );
    assert_eq!(SignatureScheme::parse("sha1"), Some(SignatureScheme::Sha1));
    assert_eq!(SignatureScheme::parse("NONE"), Some(SignatureScheme::None));
    assert_eq!(SignatureScheme::parse("md5"), None);
}

// ---------------------------------------------------------------------------
// 验签三方案
// ---------------------------------------------------------------------------

#[test]
fn test_sha1_signature_known_vector() {
    // 经典向量：sorted(["1","2","3"]).join("") = "123"
    // sha1("123") = 40bd001563085fc35165329ea1ff5c5ecbdbbeef（公开已知值）
    assert_eq!(
        compute_sha1_signature("1", "2", "3"),
        "40bd001563085fc35165329ea1ff5c5ecbdbbeef"
    );
    // 排序无关性：参数顺序置换结果一致
    assert_eq!(
        compute_sha1_signature("3", "1", "2"),
        compute_sha1_signature("1", "2", "3")
    );
    // token 不同 → 签名不同
    assert_ne!(
        compute_sha1_signature("other", "1", "2"),
        compute_sha1_signature("token", "1", "2")
    );
}

#[test]
fn test_hmac_signature_verify_roundtrip_and_tamper() {
    let body = br#"{"content":"hi"}"#;
    let sig = compute_hmac_sha256_signature("test-token", "1700", "n1", body);
    assert!(verify_callback_signature(
        SignatureScheme::HmacSha256,
        "test-token",
        "1700",
        "n1",
        body,
        &sig
    ));
    // 篡改 body → 拒
    assert!(!verify_callback_signature(
        SignatureScheme::HmacSha256,
        "test-token",
        "1700",
        "n1",
        br#"{"content":"hi2"}"#,
        &sig
    ));
    // 错误 token → 拒
    assert!(!verify_callback_signature(
        SignatureScheme::HmacSha256,
        "wrong",
        "1700",
        "n1",
        body,
        &sig
    ));
    // 大小写不敏感比较（hex 大写形态也接受）
    assert!(verify_callback_signature(
        SignatureScheme::HmacSha256,
        "test-token",
        "1700",
        "n1",
        body,
        &sig.to_uppercase()
    ));
}

#[test]
fn test_verify_none_scheme_allows_and_missing_nonce_rejected() {
    // none：显式全放行（含缺 timestamp/nonce）
    assert!(verify_callback_signature(
        SignatureScheme::None,
        "t",
        "",
        "",
        b"",
        ""
    ));
    // 非 none：缺 timestamp / nonce → 拒
    assert!(!verify_callback_signature(
        SignatureScheme::Sha1,
        "t",
        "",
        "n",
        b"",
        "x"
    ));
    assert!(!verify_callback_signature(
        SignatureScheme::HmacSha256,
        "t",
        "1700",
        "",
        b"",
        "x"
    ));
}

// ---------------------------------------------------------------------------
// 回调纯逻辑（handle_callback_with）
// ---------------------------------------------------------------------------

fn reply_of(
    ch: &WeChatChannel,
    method: &str,
    path: &str,
    query: &str,
    body: &[u8],
    sig: Option<&str>,
) -> CallbackReply {
    // 子模块直接访问私有字段（同 crate 模块树内可见）
    WeChatChannel::handle_callback_with(
        &ch.config,
        &ch.bus_sender,
        &ch.base,
        method,
        path,
        query,
        body,
        sig,
    )
}

#[test]
fn test_callback_wrong_path_is_404() {
    let (ch, _rx) = test_channel(test_config(19990));
    let reply = reply_of(
        &ch,
        "GET",
        "/other",
        "signature=x&timestamp=1&nonce=2",
        b"",
        None,
    );
    assert_eq!(reply.status_code(), 404);
}

#[test]
fn test_callback_missing_signature_is_403() {
    let (ch, _rx) = test_channel(test_config(19991));
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        "timestamp=1&nonce=2",
        b"{}",
        None,
    );
    assert_eq!(reply.status_code(), 403);
}

#[test]
fn test_get_handshake_ok_and_forbidden() {
    let (ch, _rx) = test_channel(test_config(19992));
    let (ts, nonce, echo) = ("1700000000", "n1", "echo-abc");
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, b"");

    // 合法签名 → 200 + 原样回显 echostr
    let reply = reply_of(
        &ch,
        "GET",
        "/wechat/callback",
        &format!("signature={sig}&timestamp={ts}&nonce={nonce}&echostr={echo}"),
        b"",
        None,
    );
    assert_eq!(reply.status_code(), 200);
    match reply {
        CallbackReply::Ok { body, .. } => assert_eq!(body, echo),
        other => panic!("expected Ok, got {other:?}"),
    }

    // 错误签名 → 403
    let reply = reply_of(
        &ch,
        "GET",
        "/wechat/callback",
        &format!("signature=deadbeef&timestamp={ts}&nonce={nonce}&echostr={echo}"),
        b"",
        None,
    );
    assert_eq!(reply.status_code(), 403);

    // 缺 echostr → 400
    let reply = reply_of(
        &ch,
        "GET",
        "/wechat/callback",
        &format!("signature={sig}&timestamp={ts}&nonce={nonce}"),
        b"",
        None,
    );
    assert_eq!(reply.status_code(), 400);
}

#[test]
fn test_post_rejects_bad_signature_and_publishes_good_one() {
    let (ch, mut rx) = test_channel(test_config(19993));
    let body = br#"{"msg_type":"text","from_user_id":"wxid_1","content":"hi"}"#;
    let (ts, nonce) = ("1700000001", "n2");

    // 错误签名 → 403 + 无消息
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some("bad-signature"),
    );
    assert_eq!(reply.status_code(), 403);
    assert!(rx.try_recv().is_err());

    // 正确签名（header 携带）→ 200 ack + InboundMessage
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 200);
    let inbound = rx.try_recv().expect("inbound published");
    assert_eq!(inbound.channel, "wechat");
    assert_eq!(inbound.sender_id, "wxid_1");
    assert_eq!(inbound.content, "hi");
    assert_eq!(inbound.session_key, "wechat:wxid_1");
}

#[test]
fn test_post_garbage_body_is_400() {
    let (ch, mut rx) = test_channel(test_config(19994));
    let (ts, nonce) = ("1700000002", "n3");
    let body = b"not-json";
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 400);
    assert!(rx.try_recv().is_err());
}

#[test]
fn test_unsupported_method_is_405() {
    let (ch, _rx) = test_channel(test_config(19995));
    let reply = reply_of(
        &ch,
        "DELETE",
        "/wechat/callback",
        "timestamp=1&nonce=2&signature=x",
        b"",
        None,
    );
    assert_eq!(reply.status_code(), 405);
}

// ---------------------------------------------------------------------------
// 入站消息映射（alias / contacts / allow_from / 群聊 chat_id）
// ---------------------------------------------------------------------------

#[test]
fn test_inbound_mapping_aliases_and_contacts() {
    let config = WeChatConfig {
        contacts: HashMap::from([("wxid_bob".to_string(), "老王".to_string())]),
        ..test_config(19996)
    };
    let (ch, mut rx) = test_channel(config);

    // alias 形态 wire（sender_id / msgtype / message_id）；含非 ASCII，
    // 用 json! 构造（br#""#" 字节串字面量只允许 ASCII）
    let body_str = serde_json::json!({
        "msgtype": "text",
        "sender_id": "wxid_bob",
        "from_nickname": "Bob",
        "chat_id": "room_9",
        "chat_type": "group",
        "content": "别名消息",
        "message_id": "m-9"
    })
    .to_string();
    let body = body_str.as_bytes();
    let (ts, nonce) = ("1700000003", "n4");
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 200);

    let inbound = rx.try_recv().expect("published");
    assert_eq!(inbound.sender_id, "wxid_bob");
    assert_eq!(inbound.chat_id, "room_9");
    assert_eq!(inbound.session_key, "wechat:room_9");
    assert_eq!(inbound.content, "别名消息");
    assert_eq!(
        inbound.metadata.get("contact_alias").map(String::as_str),
        Some("老王")
    );
    assert_eq!(
        inbound.metadata.get("chat_type").map(String::as_str),
        Some("group")
    );
    assert_eq!(
        inbound.metadata.get("msg_id").map(String::as_str),
        Some("m-9")
    );
    assert_eq!(
        inbound.metadata.get("from_nickname").map(String::as_str),
        Some("Bob")
    );
}

#[test]
fn test_inbound_chat_id_defaults_to_sender_and_allow_from_filter() {
    // allow_from 白名单外仍 ack（200）但不发布
    let config = WeChatConfig {
        allow_from: vec!["wxid_allowed".to_string()],
        ..test_config(19997)
    };
    let (ch, mut rx) = test_channel(config);
    let (ts, nonce) = ("1700000004", "n5");

    // 白名单外：合法签名 → 200，无消息
    let body = br#"{"from_user_id":"wxid_stranger","content":"spam"}"#;
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 200);
    assert!(rx.try_recv().is_err());

    // 白名单内 + chat_id 缺省回落 sender_id（单聊）
    let body = br#"{"from_user_id":"wxid_allowed","content":"hello"}"#;
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 200);
    let inbound = rx.try_recv().expect("published");
    assert_eq!(inbound.chat_id, "wxid_allowed");
    assert_eq!(inbound.session_key, "wechat:wxid_allowed");
}

#[test]
fn test_inbound_empty_content_is_400() {
    let (ch, mut rx) = test_channel(test_config(19998));
    let (ts, nonce) = ("1700000005", "n6");
    let body = br#"{"from_user_id":"wxid_x","content":""}"#;
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let reply = reply_of(
        &ch,
        "POST",
        "/wechat/callback",
        &format!("timestamp={ts}&nonce={nonce}"),
        body,
        Some(&sig),
    );
    assert_eq!(reply.status_code(), 400);
    assert!(rx.try_recv().is_err());
}

// ---------------------------------------------------------------------------
// query 解析
// ---------------------------------------------------------------------------

#[test]
fn test_parse_query_params_percent_decoding() {
    let params = parse_query_params("a=1&b=%E4%BD%A0%E5%A5%BD&flag&c=x%2By");
    assert_eq!(query_get_pub(&params, "a"), Some("1"));
    assert_eq!(query_get_pub(&params, "b"), Some("你好"));
    assert_eq!(query_get_pub(&params, "flag"), Some(""));
    assert_eq!(query_get_pub(&params, "c"), Some("x+y"));
    assert_eq!(query_get_pub(&params, "missing"), None);
}

fn query_get_pub<'a>(params: &'a [(String, String)], key: &str) -> Option<&'a str> {
    params
        .iter()
        .find(|(k, _)| k == key)
        .map(|(_, v)| v.as_str())
}

// ---------------------------------------------------------------------------
// 真实 TCP 回调全链路（IT 契约：握手 / 验签拒绝 / 消息发布）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_inbound_callback_contract_via_real_tcp() {
    let port = find_free_port();
    let config = WeChatConfig {
        callback_listen_addr: format!("127.0.0.1:{port}"),
        ..test_config(port)
    };
    let (ch, mut rx) = test_channel(config);
    ch.start().await.unwrap();
    // 等待 spawn 的 listener 完成绑定
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let (ts, nonce) = ("1700000100", "tcp1");

    // ① GET URL 验证握手（query 携带合法签名 → 200 回显 echostr）
    let echo = "echo-tcp-1";
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, b"");
    let resp = send_raw_http(
        port,
        format!(
            "GET {}?signature={sig}&timestamp={ts}&nonce={nonce}&echostr={echo} HTTP/1.1\r\nHost: t\r\n\r\n",
            DEFAULT_CALLBACK_PATH
        )
        .as_bytes(),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "got: {resp}");
    assert!(resp.contains(echo), "echostr 必须原样回显: {resp}");

    // ② GET 错误签名 → 403
    let resp = send_raw_http(
        port,
        format!(
            "GET {}?signature=deadbeef&timestamp={ts}&nonce={nonce}&echostr={echo} HTTP/1.1\r\nHost: t\r\n\r\n",
            DEFAULT_CALLBACK_PATH
        )
        .as_bytes(),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");

    // ③ POST 合法签名消息（header 携带签名）→ 200 ack + InboundMessage
    // （含非 ASCII，用 json! 构造——字节串字面量只允许 ASCII）
    let body_str = serde_json::json!({
        "msg_type": "text",
        "from_user_id": "wxid_tcp",
        "content": "tcp 你好",
        "msg_id": "m-tcp-1"
    })
    .to_string();
    let body = body_str.as_bytes();
    let sig = compute_hmac_sha256_signature("test-token", ts, nonce, body);
    let mut request = format!(
        "POST {}?timestamp={ts}&nonce={nonce} HTTP/1.1\r\nHost: t\r\nX-Wechat-Signature: {sig}\r\nContent-Length: {}\r\n\r\n",
        DEFAULT_CALLBACK_PATH,
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    let resp = send_raw_http(port, &request).await;
    assert!(resp.starts_with("HTTP/1.1 200 OK"), "got: {resp}");
    let inbound = rx.try_recv().expect("inbound published");
    assert_eq!(inbound.sender_id, "wxid_tcp");
    assert_eq!(inbound.content, "tcp 你好");
    assert_eq!(inbound.session_key, "wechat:wxid_tcp");

    // ④ POST 错误签名（正确 body）→ 403 + 无消息
    let mut request = format!(
        "POST {}?timestamp={ts}&nonce={nonce} HTTP/1.1\r\nHost: t\r\nX-Wechat-Signature: 0000\r\nContent-Length: {}\r\n\r\n",
        DEFAULT_CALLBACK_PATH,
        body.len()
    )
    .into_bytes();
    request.extend_from_slice(body);
    let resp = send_raw_http(port, &request).await;
    assert!(resp.starts_with("HTTP/1.1 403"), "got: {resp}");
    assert!(rx.try_recv().is_err());

    // ⑤ 错误路径 → 404
    let resp = send_raw_http(
        port,
        format!(
            "POST /other?timestamp={ts}&nonce={nonce} HTTP/1.1\r\nHost: t\r\nContent-Length: 2\r\n\r\n{{}}"
        )
        .as_bytes(),
    )
    .await;
    assert!(resp.starts_with("HTTP/1.1 404"), "got: {resp}");

    ch.stop().await.unwrap();
    assert!(!ch.is_running());
}

// ---------------------------------------------------------------------------
// 出站契约（mock REST server：请求形态 + 非 2xx 报错）
// ---------------------------------------------------------------------------

/// mock 出站 REST：accept 一连接，捕获请求字节，回指定状态码。
fn spawn_mock_outbound(port: u16, status_line: &'static str) -> Arc<std::sync::Mutex<String>> {
    let captured = Arc::new(std::sync::Mutex::new(String::new()));
    let captured_clone = captured.clone();
    let listener = std::net::TcpListener::bind(("127.0.0.1", port)).expect("bind mock");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            use std::io::{Read, Write};
            let mut buf = [0u8; 65536];
            let n = stream.read(&mut buf).unwrap_or(0);
            *captured_clone.lock().unwrap() = String::from_utf8_lossy(&buf[..n]).to_string();
            let body = br#"{"code":0}"#;
            let resp = format!(
                "HTTP/1.1 {status_line}\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                body.len()
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.write_all(body);
        }
    });
    captured
}

#[tokio::test]
async fn test_outbound_request_shape_contract() {
    let port = find_free_port();
    let captured = spawn_mock_outbound(port, "200 OK");

    let config = WeChatConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        ..test_config(port)
    };
    let (ch, _rx) = test_channel(config);
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "wechat".to_string(),
        chat_id: "wxid_out".to_string(),
        content: "出站回复".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    ch.send(msg).await.expect("send ok");

    // 给 mock 线程一点时间落捕获
    for _ in 0..50 {
        if !captured.lock().unwrap().is_empty() {
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    let req = captured.lock().unwrap().clone();
    assert!(!req.is_empty(), "mock 必须收到请求");

    // 请求行：POST {send_path}
    assert!(
        req.starts_with(&format!("POST {} ", DEFAULT_SEND_PATH)),
        "请求行必须是 POST {DEFAULT_SEND_PATH}: {req}"
    );
    // 鉴权：Bearer token（来自 config，非硬编码）；reqwest/hyper 上线小写头名，
    // 断言对整段做小写归一
    assert!(
        req.to_ascii_lowercase()
            .contains("authorization: bearer test-token"),
        "必须携带 Bearer token: {req}"
    );
    // JSON 体：to_user_id / msg_type=text / content
    assert!(req.contains(r#""to_user_id":"wxid_out""#), "body: {req}");
    assert!(req.contains(r#""msg_type":"text""#), "body: {req}");
    assert!(req.contains(r#""content":"出站回复""#), "body: {req}");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_outbound_non_2xx_is_error() {
    let port = find_free_port();
    let _captured = spawn_mock_outbound(port, "500 Internal Server Error");

    let config = WeChatConfig {
        base_url: format!("http://127.0.0.1:{port}"),
        ..test_config(port)
    };
    let (ch, _rx) = test_channel(config);
    ch.start().await.unwrap();

    let msg = OutboundMessage {
        channel: "wechat".to_string(),
        chat_id: "wxid_out2".to_string(),
        content: "will fail".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    let result = ch.send(msg).await;
    assert!(result.is_err(), "非 2xx 必须报错");
    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_send_fails_when_not_running_or_empty_chat_id() {
    let (ch, _rx) = test_channel(test_config(19999));
    let msg = OutboundMessage {
        channel: "wechat".to_string(),
        chat_id: "wxid_x".to_string(),
        content: "hi".to_string(),
        message_type: String::new(),
        meta: Default::default(),
    };
    // 未启动 → 拒
    assert!(ch.send(msg.clone()).await.is_err());

    ch.start().await.unwrap();
    // 空 chat_id → 拒
    let empty = OutboundMessage {
        chat_id: String::new(),
        ..msg.clone()
    };
    assert!(ch.send(empty).await.is_err());
    ch.stop().await.unwrap();
}
