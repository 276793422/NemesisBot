//! wecom 通道契约测试（P25）。
//!
//! 覆盖三面：
//! 1. 加解密协议面：签名（排序归一/篡改拒绝）、EncodingAESKey 派生、
//!    加解密 round-trip（含 receiveid / 32 字节填充边界）、篡改拒绝；
//! 2. 出站契约：群机器人 webhook POST 消息形态（markdown/text/默认）、
//!    chat_id 直连 URL 与 webhooks 路由表优先级、errcode != 0 失败路径；
//! 3. 入站契约：回调端点 GET 验证（明文回显/坏签名拒绝）、POST JSON/XML
//!    双形态（验签 + 解密 + bus 发布）、坏签名拒绝、allow_list 过滤。

use super::*;

use base64::Engine as _;
use sha1::Digest as _;

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

/// 生成一对（EncodingAESKey 43 字符形态, 原始 32 字节密钥）。
///
/// EncodingAESKey = base64(key 32B) 去掉末尾 '='（44 → 43 字符）。
fn make_aes_key() -> (String, [u8; 32]) {
    let key: [u8; 32] = rand::random();
    let mut b64 = base64::engine::general_purpose::STANDARD.encode(key);
    b64.pop(); // 去掉单个 '='（32 字节 → 44 字符带 1 个填充）
    (b64, key)
}

/// 查询串百分号编码（测试构造 GET 参数用，覆盖 base64 的 +/= 字符）。
fn qenc(s: &str) -> String {
    let mut out = String::new();
    for b in s.bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

/// 拉起一个捕获 POST body 的本地 HTTP 服务（群机器人 webhook 替身）。
async fn spawn_capture_server() -> (
    std::net::SocketAddr,
    tokio::sync::mpsc::UnboundedReceiver<serde_json::Value>,
) {
    let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
    let app = axum::Router::new().route(
        "/robot/send",
        axum::routing::post(move |body: String| {
            let tx = tx.clone();
            async move {
                let v: serde_json::Value =
                    serde_json::from_str(&body).expect("webhook 请求体应为合法 JSON");
                let _ = tx.send(v);
                axum::Json(serde_json::json!({"errcode": 0, "errmsg": "ok"}))
            }
        }),
    );
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        let _ = axum::serve(listener, app).await;
    });
    (addr, rx)
}

/// 拉起一个恒回业务错误的 webhook 替身（errcode != 0）。
async fn spawn_errcode_server(errcode: i64) -> std::net::SocketAddr {
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

/// 轮询等回调端口可用（start() 异步拉起 server，避免首请求竞态）。
async fn wait_http_ready(client: &reqwest::Client, base: &str) {
    for _ in 0..100 {
        if client.get(base).send().await.is_ok() {
            return;
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("wecom 回调服务未在预算时间内就绪");
}

/// 构造一个仅出站（webhook）配置。
fn outbound_only_config(webhook_url: String) -> WeComConfig {
    WeComConfig {
        webhook_url,
        ..Default::default()
    }
}

/// 构造一个完整回调配置（token + aes_key，随机端口）。
fn callback_config(token: &str, encoding_aes_key: &str, allow_from: Vec<String>) -> WeComConfig {
    WeComConfig {
        token: token.to_string(),
        encoding_aes_key: encoding_aes_key.to_string(),
        listen_addr: "127.0.0.1:0".to_string(),
        callback_path: "/wecom/callback".to_string(),
        allow_from,
        ..Default::default()
    }
}

// ---------------------------------------------------------------------------
// 加解密协议面
// ---------------------------------------------------------------------------

#[test]
fn test_signature_verify_roundtrip_and_tamper() {
    let sig = crypto::signature("tok", "1700000000", "nonce1", "ENCRYPT_B64");
    // 确定性
    assert_eq!(
        sig,
        crypto::signature("tok", "1700000000", "nonce1", "ENCRYPT_B64")
    );
    // 正向校验
    assert!(crypto::verify_signature(
        "tok",
        "1700000000",
        "nonce1",
        "ENCRYPT_B64",
        &sig
    ));
    // 参数顺序无关（协议按四串字典序排序后拼接，归一化不应影响结果）
    assert!(crypto::verify_signature(
        "tok",
        "nonce1",
        "1700000000",
        "ENCRYPT_B64",
        &sig
    ));
    // 伪造 token / 篡改密文 / 篡改签名 → 拒绝
    assert!(!crypto::verify_signature(
        "bad",
        "1700000000",
        "nonce1",
        "ENCRYPT_B64",
        &sig
    ));
    assert!(!crypto::verify_signature(
        "tok",
        "1700000000",
        "nonce1",
        "TAMPERED",
        &sig
    ));
    assert!(!crypto::verify_signature(
        "tok",
        "1700000000",
        "nonce1",
        "ENCRYPT_B64",
        "deadbeef"
    ));
}

#[test]
fn test_signature_matches_manual_sorted_concat() {
    // 与协议定义逐字对照：sort([token, ts, nonce, encrypt]) 拼接 → sha1 hex
    let (token, ts, nonce, encrypt) = ("bbb", "222", "ccc", "aaa");
    let mut parts = [token, ts, nonce, encrypt];
    parts.sort_unstable();
    let joined = parts.concat();
    // 排序归一：["222","aaa","bbb","ccc"] → "222aaabbbccc"
    assert_eq!(joined, "222aaabbbccc");
    let sig = crypto::signature(token, ts, nonce, encrypt);
    // 独立手算一遍 sha1（不经 signature 函数）
    let mut hasher = sha1::Sha1::new();
    hasher.update(joined.as_bytes());
    assert_eq!(sig, hex::encode(hasher.finalize()));
}

#[test]
fn test_aes_key_derivation() {
    let (enc, key) = make_aes_key();
    assert_eq!(enc.len(), 43, "EncodingAESKey 规范长度 43 字符");
    assert_eq!(crypto::aes_key_from_encoding(&enc).unwrap(), key);

    // 非法 base64 / 长度不足 → 诚实报错
    assert!(crypto::aes_key_from_encoding("!!!!").is_err());
    assert!(crypto::aes_key_from_encoding("short").is_err());
    // 合法 base64 但长度不对（43 个 'A' 解出 32 字节零值是巧合合法形态，
    // 这里用 42 字符破坏 4 的倍数对齐）
    assert!(crypto::aes_key_from_encoding(&"A".repeat(42)).is_err());
}

#[test]
fn test_encrypt_decrypt_roundtrip() {
    let (_, key) = make_aes_key();
    let msg = r#"{"msgtype":"text","text":{"content":"你好 wecom"}}"#;

    // 自建应用形态：receiveid = corp_id
    let enc = crypto::encrypt_message(&key, msg, "corp123").unwrap();
    let (m, r) = crypto::decrypt_message(&key, &enc).unwrap();
    assert_eq!(m, msg);
    assert_eq!(r, "corp123");

    // 智能机器人形态：receiveid 可为空
    let enc2 = crypto::encrypt_message(&key, msg, "").unwrap();
    let (m2, r2) = crypto::decrypt_message(&key, &enc2).unwrap();
    assert_eq!(m2, msg);
    assert_eq!(r2, "");
}

#[test]
fn test_pad32_boundary_roundtrip() {
    // 构造 raw_len ≡ 0 (mod 32) 的载荷 → 填充值 = 32（企业微信 32 字节块
    // 填充与标准 PKCS7-16 的分叉点），round-trip 必须无损。
    let (_, key) = make_aes_key();
    let msg = "0123456789abcdef0123456789abcdef"; // 32 字节
    let base = 20 + msg.len(); // random(16) + len(4) + msg
    let need = (32 - base % 32) % 32; // receiveid 长度使 raw_len 对齐 32
    let receiveid = "x".repeat(need);
    assert_eq!(
        (base + receiveid.len()) % 32,
        0,
        "测试前置：raw_len 应对齐 32"
    );

    let enc = crypto::encrypt_message(&key, msg, &receiveid).unwrap();
    let (m, r) = crypto::decrypt_message(&key, &enc).unwrap();
    assert_eq!(m, msg);
    assert_eq!(r, receiveid);
}

#[test]
fn test_decrypt_rejects_tampered_and_garbage() {
    let (_, key) = make_aes_key();
    let msg = r#"{"msgtype":"text","text":{"content":"hello"}}"#;
    let enc = crypto::encrypt_message(&key, msg, "corp").unwrap();

    // 翻转中段块一个 bit：明文必崩坏——无论落在长度字段还是填充，
    // 结果都不应还原出原文（确定性断言：Err 或内容不同均算拒绝成功）
    let mut bytes = base64::engine::general_purpose::STANDARD
        .decode(&enc)
        .unwrap();
    let mid = (bytes.len() / 2) & !15;
    bytes[mid] ^= 0x01;
    let tampered = base64::engine::general_purpose::STANDARD.encode(&bytes);
    if let Ok((m, _)) = crypto::decrypt_message(&key, &tampered) {
        assert_ne!(m, msg);
    }

    // 非法 base64 → Err
    assert!(crypto::decrypt_message(&key, "not-base64!!!").is_err());
    // 错误密钥 → 长度/填充解析大概率崩坏，且绝不还原原文
    let (_, other_key) = make_aes_key();
    if let Ok((m, _)) = crypto::decrypt_message(&other_key, &enc) {
        assert_ne!(m, msg);
    }
}

// ---------------------------------------------------------------------------
// 回调明文解析
// ---------------------------------------------------------------------------

#[test]
fn test_parse_callback_message_variants() {
    // 智能机器人新格式（camelCase 嵌套 from）
    let m = parse_callback_message(
        r#"{"msgtype":"text","text":{"content":"在吗"},"from":{"userId":"u1","name":"张三"},"chatId":"wr_g"}"#,
    )
    .unwrap();
    assert_eq!(m.content, "在吗");
    assert_eq!(m.sender_id, "u1");
    assert_eq!(m.sender_nick, "张三");
    assert_eq!(m.chat_id, "wr_g");
    assert_eq!(m.msg_type, "text");

    // 经典应用回调字段形态（PascalCase 扁平）
    let m2 =
        parse_callback_message(r#"{"MsgType":"text","Content":"hi","FromUserName":"u2"}"#).unwrap();
    assert_eq!(m2.content, "hi");
    assert_eq!(m2.sender_id, "u2");
    assert_eq!(m2.chat_id, "");
    assert_eq!(m2.msg_type, "text");

    // 非 JSON → None（上层走 ok + 告警路径）
    assert!(parse_callback_message("not json").is_none());
}

#[test]
fn test_extract_encrypt_json_and_xml() {
    // JSON 形态（智能机器人）
    let e = extract_encrypt_from_body(r#"{"encrypt":"ABC+/="}"#).unwrap();
    assert_eq!(e, "ABC+/=");

    // XML + CDATA 形态（经典应用回调）
    let e2 = extract_encrypt_from_body(
        "<xml><ToUserName><![CDATA[corp]]></ToUserName><Encrypt><![CDATA[XYZ+/=]]></Encrypt><AgentID>1</AgentID></xml>",
    )
    .unwrap();
    assert_eq!(e2, "XYZ+/=");

    // XML 无 CDATA
    let e3 = extract_encrypt_from_body("<xml><Encrypt>PLAINB64</Encrypt></xml>").unwrap();
    assert_eq!(e3, "PLAINB64");

    // 双形态都没有 → None
    assert!(extract_encrypt_from_body(r#"{"foo":1}"#).is_none());
}

// ---------------------------------------------------------------------------
// 通道校验与生命周期
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_wecom_channel_new_validates() {
    let (tx, _rx) = broadcast::channel(16);

    // 两侧都没配 → Err
    assert!(WeComChannel::new(WeComConfig::default(), tx.clone()).is_err());
    // EncodingAESKey 非法 → fail-fast
    let bad_key = WeComConfig {
        webhook_url: "http://example.invalid/hook".to_string(),
        encoding_aes_key: "bad!".to_string(),
        ..Default::default()
    };
    assert!(WeComChannel::new(bad_key, tx).is_err());
}

#[tokio::test]
async fn test_wecom_channel_lifecycle() {
    let (tx, _rx) = broadcast::channel(16);
    let ch = WeComChannel::new(
        outbound_only_config("http://example.invalid/hook".to_string()),
        tx,
    )
    .unwrap();

    assert_eq!(ch.name(), "wecom");
    assert!(!ch.is_running());
    assert!(ch.callback_addr().is_none());

    ch.start().await.unwrap();
    assert!(ch.is_running());
    // 仅出站模式不开回调端口
    assert!(ch.callback_addr().is_none());

    // 未 running 时 send 诚实拒绝（此处 running 已 true，用 stop 后验证）
    ch.stop().await.unwrap();
    assert!(!ch.is_running());
    assert!(ch.callback_addr().is_none());

    let msg = OutboundMessage::new("wecom", "chat", "hello");
    assert!(ch.send(msg).await.is_err());
}

#[test]
fn test_wecom_config_template_shape_deserializes() {
    // config.default.json 的 wecom 段形态（含 enabled/sync_to 等模板层字段）
    // 必须能被 WeComConfig 容忍式反序列化（未知字段忽略）
    let raw = serde_json::json!({
        "enabled": false,
        "webhook_url": "",
        "webhooks": {},
        "token": "",
        "encoding_aes_key": "",
        "corp_id": "",
        "listen_addr": "0.0.0.0:9898",
        "callback_path": "/wecom/callback",
        "allow_from": [],
        "sync_to": []
    });
    let cfg: WeComConfig = serde_json::from_value(raw).unwrap();
    assert_eq!(cfg.listen_addr, "0.0.0.0:9898");
    assert_eq!(cfg.callback_path, "/wecom/callback");
}

#[test]
fn test_build_webhook_body_mapping() {
    let mut md = OutboundMessage::new("wecom", "c", "**hi**");
    md.message_type = "markdown".to_string();
    let v = WeComChannel::build_webhook_body(&md);
    assert_eq!(v["msgtype"], "markdown");
    assert_eq!(v["markdown"]["content"], "**hi**");

    let mut tx = OutboundMessage::new("wecom", "c", "plain");
    tx.message_type = "text".to_string();
    let v = WeComChannel::build_webhook_body(&tx);
    assert_eq!(v["msgtype"], "text");
    assert_eq!(v["text"]["content"], "plain");

    // 空 message_type → 默认 markdown
    let v = WeComChannel::build_webhook_body(&OutboundMessage::new("wecom", "c", "x"));
    assert_eq!(v["msgtype"], "markdown");
}

// ---------------------------------------------------------------------------
// 出站契约（群机器人 webhook POST）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_outbound_post_shape_markdown_and_text() {
    let (addr, mut rx) = spawn_capture_server().await;
    let (tx, _rx) = broadcast::channel(16);
    let ch = WeComChannel::new(
        outbound_only_config(format!("http://{addr}/robot/send")),
        tx,
    )
    .unwrap();
    ch.start().await.unwrap();

    // markdown（默认形态）
    let mut md = OutboundMessage::new("wecom", "group-a", "**hello** wecom");
    md.message_type = "markdown".to_string();
    ch.send(md).await.unwrap();
    let body = rx.recv().await.unwrap();
    assert_eq!(body["msgtype"], "markdown");
    assert_eq!(body["markdown"]["content"], "**hello** wecom");

    // text 形态
    let mut txt = OutboundMessage::new("wecom", "group-a", "plain text");
    txt.message_type = "text".to_string();
    ch.send(txt).await.unwrap();
    let body = rx.recv().await.unwrap();
    assert_eq!(body["msgtype"], "text");
    assert_eq!(body["text"]["content"], "plain text");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_outbound_route_by_chat_id_map_and_direct_url() {
    let (addr1, mut rx1) = spawn_capture_server().await;
    let (addr2, mut rx2) = spawn_capture_server().await;
    let (tx, _rx) = broadcast::channel(16);

    // webhooks 路由表优先于默认 webhook_url
    let cfg = WeComConfig {
        webhook_url: format!("http://{addr2}/robot/send"),
        webhooks: [("group-b".to_string(), format!("http://{addr1}/robot/send"))]
            .into_iter()
            .collect(),
        ..Default::default()
    };
    let ch = WeComChannel::new(cfg, tx).unwrap();
    ch.start().await.unwrap();

    ch.send(OutboundMessage::new("wecom", "group-b", "routed"))
        .await
        .unwrap();
    let body = rx1.recv().await.unwrap();
    assert_eq!(body["markdown"]["content"], "routed");

    ch.send(OutboundMessage::new("wecom", "other-group", "default"))
        .await
        .unwrap();
    let body = rx2.recv().await.unwrap();
    assert_eq!(body["markdown"]["content"], "default");

    // chat_id 本身是完整 URL → 直连优先
    ch.send(OutboundMessage::new(
        "wecom",
        &format!("http://{addr1}/robot/send"),
        "direct",
    ))
    .await
    .unwrap();
    let body = rx1.recv().await.unwrap();
    assert_eq!(body["markdown"]["content"], "direct");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_outbound_errcode_nonzero_is_error() {
    let addr = spawn_errcode_server(95000).await;
    let (tx, _rx) = broadcast::channel(16);
    let ch = WeComChannel::new(
        outbound_only_config(format!("http://{addr}/robot/send")),
        tx,
    )
    .unwrap();
    ch.start().await.unwrap();

    let err = ch
        .send(OutboundMessage::new("wecom", "group-a", "x"))
        .await
        .unwrap_err();
    assert!(
        format!("{err}").contains("95000"),
        "错误信息应携带 errcode: {err}"
    );

    ch.stop().await.unwrap();
}

// ---------------------------------------------------------------------------
// 入站契约（智能机器人回调）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_callback_url_verify_roundtrip_and_reject() {
    let (enc_key, key) = make_aes_key();
    let token = "test-token-1";
    let (tx, _rx) = broadcast::channel(16);
    let ch = WeComChannel::new(callback_config(token, &enc_key, vec![]), tx).unwrap();
    ch.start().await.unwrap();

    let addr = ch.callback_addr().expect("入站模式 start 后应有回调地址");
    let client = reqwest::Client::new();
    wait_http_ready(&client, &format!("http://{addr}/wecom/callback")).await;

    let echo_plain = "echo-plaintext-验证";
    let echostr = crypto::encrypt_message(&key, echo_plain, "").unwrap();
    let sig = crypto::signature(token, "1700000000", "n1", &echostr);
    let url = format!(
        "http://{addr}/wecom/callback?msg_signature={}&timestamp=1700000000&nonce=n1&echostr={}",
        qenc(&sig),
        qenc(&echostr)
    );

    // 正常验证：明文原样回显（协议要求纯文本，非 JSON 包装）
    let resp = client.get(&url).send().await.unwrap();
    assert_eq!(resp.status(), 200);
    assert_eq!(resp.text().await.unwrap(), echo_plain);

    // 坏签名 → 400 拒绝
    let bad_url = format!(
        "http://{addr}/wecom/callback?msg_signature={}&timestamp=1700000000&nonce=n1&echostr={}",
        qenc(&"0".repeat(40)),
        qenc(&echostr)
    );
    let resp = client.get(&bad_url).send().await.unwrap();
    assert_eq!(resp.status(), 400);

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_callback_message_json_post_publishes_inbound() {
    let (enc_key, key) = make_aes_key();
    let token = "test-token-2";
    let (tx, mut rx_bus) = broadcast::channel(16);
    let ch = WeComChannel::new(callback_config(token, &enc_key, vec![]), tx).unwrap();
    ch.start().await.unwrap();

    let addr = ch.callback_addr().unwrap();
    let base = format!("http://{addr}/wecom/callback");
    let client = reqwest::Client::new();
    wait_http_ready(&client, &base).await;

    // JSON 形态（智能机器人）：{"encrypt": ...}
    let msg_json = serde_json::json!({
        "msgtype": "text",
        "text": {"content": "你好机器人"},
        "from": {"userId": "u1001", "name": "张三"},
        "chatId": "wr_group9"
    });
    let plaintext = msg_json.to_string();
    let encrypt = crypto::encrypt_message(&key, &plaintext, "").unwrap();
    let sig = crypto::signature(token, "1700000001", "n2", &encrypt);
    let resp = client
        .post(format!(
            "{base}?msg_signature={}&timestamp=1700000001&nonce=n2",
            qenc(&sig)
        ))
        .json(&serde_json::json!({"encrypt": encrypt}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);
    let v: serde_json::Value = resp.json().await.unwrap();
    assert_eq!(v["errcode"], 0);

    // bus 侧收到结构化入站消息
    let inbound = tokio::time::timeout(std::time::Duration::from_secs(3), rx_bus.recv())
        .await
        .expect("应在超时前收到入站消息")
        .unwrap();
    assert_eq!(inbound.channel, "wecom");
    assert_eq!(inbound.content, "你好机器人");
    assert_eq!(inbound.sender_id, "u1001");
    assert_eq!(inbound.chat_id, "wr_group9");
    assert_eq!(inbound.session_key, "wecom:wr_group9");
    assert_eq!(inbound.metadata.get("sender_nick").unwrap(), "张三");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_callback_message_xml_post_publishes_inbound() {
    let (enc_key, key) = make_aes_key();
    let token = "test-token-3";
    let (tx, mut rx_bus) = broadcast::channel(16);
    let ch = WeComChannel::new(callback_config(token, &enc_key, vec![]), tx).unwrap();
    ch.start().await.unwrap();

    let addr = ch.callback_addr().unwrap();
    let base = format!("http://{addr}/wecom/callback");
    let client = reqwest::Client::new();
    wait_http_ready(&client, &base).await;

    // XML 形态（经典应用回调）：<Encrypt><![CDATA[...]]></Encrypt>
    let plaintext = serde_json::json!({
        "MsgType": "text",
        "Content": "xml hello",
        "FromUserName": "u2002",
        "ChatId": "chat_xml"
    })
    .to_string();
    let encrypt = crypto::encrypt_message(&key, &plaintext, "").unwrap();
    let sig = crypto::signature(token, "1700000002", "n3", &encrypt);
    let body = format!(
        "<xml><ToUserName><![CDATA[corp]]></ToUserName><Encrypt><![CDATA[{encrypt}]]></Encrypt><AgentID>1</AgentID></xml>"
    );
    let resp = client
        .post(format!(
            "{base}?msg_signature={}&timestamp=1700000002&nonce=n3",
            qenc(&sig)
        ))
        .header("Content-Type", "text/xml")
        .body(body)
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    let inbound = tokio::time::timeout(std::time::Duration::from_secs(3), rx_bus.recv())
        .await
        .expect("应在超时前收到入站消息")
        .unwrap();
    assert_eq!(inbound.content, "xml hello");
    assert_eq!(inbound.sender_id, "u2002");
    assert_eq!(inbound.chat_id, "chat_xml");

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_callback_rejects_bad_signature_without_publish() {
    let (enc_key, key) = make_aes_key();
    let token = "test-token-4";
    let (tx, mut rx_bus) = broadcast::channel(16);
    let ch = WeComChannel::new(callback_config(token, &enc_key, vec![]), tx).unwrap();
    ch.start().await.unwrap();

    let addr = ch.callback_addr().unwrap();
    let base = format!("http://{addr}/wecom/callback");
    let client = reqwest::Client::new();
    wait_http_ready(&client, &base).await;

    let encrypt = crypto::encrypt_message(&key, r#"{"text":{"content":"x"}}"#, "").unwrap();

    // 坏签名 POST → 400，且 bus 无消息
    let resp = client
        .post(format!(
            "{base}?msg_signature={}&timestamp=t&nonce=n",
            qenc(&"f".repeat(40))
        ))
        .json(&serde_json::json!({"encrypt": encrypt}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    // 缺 encrypt 字段 → 400
    let sig = crypto::signature(token, "t", "n", "placeholder");
    let resp = client
        .post(format!(
            "{base}?msg_signature={}&timestamp=t&nonce=n",
            qenc(&sig)
        ))
        .json(&serde_json::json!({"foo": 1}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(
        rx_bus.try_recv().is_err(),
        "被拒绝的请求不应产生任何入站消息"
    );

    ch.stop().await.unwrap();
}

#[tokio::test]
async fn test_callback_allow_list_filter() {
    let (enc_key, key) = make_aes_key();
    let token = "test-token-5";
    let (tx, mut rx_bus) = broadcast::channel(16);
    // 白名单只放行 u_allowed
    let ch = WeComChannel::new(
        callback_config(token, &enc_key, vec!["u_allowed".to_string()]),
        tx,
    )
    .unwrap();
    ch.start().await.unwrap();

    let addr = ch.callback_addr().unwrap();
    let base = format!("http://{addr}/wecom/callback");
    let client = reqwest::Client::new();
    wait_http_ready(&client, &base).await;

    // 非白名单用户 → 200（平台侧不重试）但消息被过滤
    let plaintext = serde_json::json!({
        "msgtype": "text",
        "text": {"content": "should be filtered"},
        "from": {"userId": "u_blocked"},
        "chatId": "wr_g"
    })
    .to_string();
    let encrypt = crypto::encrypt_message(&key, &plaintext, "").unwrap();
    let sig = crypto::signature(token, "1700000003", "n4", &encrypt);
    let resp = client
        .post(format!(
            "{base}?msg_signature={}&timestamp=1700000003&nonce=n4",
            qenc(&sig)
        ))
        .json(&serde_json::json!({"encrypt": encrypt}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 200);

    tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    assert!(rx_bus.try_recv().is_err(), "allow_list 外的消息不应进 bus");

    ch.stop().await.unwrap();
}
