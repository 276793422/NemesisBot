use super::*;

fn make_config() -> FeishuConfig {
    FeishuConfig {
        app_id: "cli_test".to_string(),
        app_secret: "secret".to_string(),
        verification_token: String::new(),
        encrypt_key: String::new(),
        allow_from: Vec::new(),
        api_base: String::new(),
    }
}

#[tokio::test]
async fn test_feishu_channel_new_validates() {
    let config = FeishuConfig {
        app_id: String::new(),
        app_secret: String::new(),
        verification_token: String::new(),
        encrypt_key: String::new(),
        allow_from: Vec::new(),
        api_base: String::new(),
    };
    let (tx, _rx) = broadcast::channel(256);
    assert!(FeishuChannel::new(config, tx).is_err());
}

#[tokio::test]
async fn test_feishu_channel_lifecycle() {
    let config = make_config();
    let (tx, _rx) = broadcast::channel(256);
    let ch = FeishuChannel::new(config, tx).unwrap();
    assert_eq!(ch.name(), "feishu");

    ch.start().await.unwrap();
    assert!(*ch.running.read());

    ch.stop().await.unwrap();
    assert!(!*ch.running.read());
}

#[test]
fn test_extract_sender_id_user_id() {
    let sender = FeishuEventSender {
        sender_id: Some(FeishuSenderId {
            user_id: Some("u123".to_string()),
            open_id: Some("ou456".to_string()),
            union_id: None,
        }),
        tenant_key: None,
    };
    assert_eq!(FeishuChannel::extract_sender_id(&sender), "u123");
}

#[test]
fn test_extract_sender_id_fallback() {
    let sender = FeishuEventSender {
        sender_id: Some(FeishuSenderId {
            user_id: Some(String::new()),
            open_id: Some("ou456".to_string()),
            union_id: None,
        }),
        tenant_key: None,
    };
    assert_eq!(FeishuChannel::extract_sender_id(&sender), "ou456");
}

#[test]
fn test_extract_message_content_text() {
    let msg = FeishuEventMessage {
        chat_id: Some("oc_xxx".to_string()),
        message_id: Some("om_xxx".to_string()),
        message_type: Some("text".to_string()),
        content: Some(r#"{"text":"hello"}"#.to_string()),
        chat_type: Some("group".to_string()),
    };
    assert_eq!(FeishuChannel::extract_message_content(&msg), "hello");
}

#[test]
fn test_parse_and_publish_event() {
    let (tx, mut rx) = broadcast::channel(256);

    let event = serde_json::json!({
        "header": {
            "event_id": "evt_123",
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_id": "om_test",
                "message_type": "text",
                "content": "{\"text\":\"hello world\"}",
                "chat_type": "group"
            },
            "sender": {
                "sender_id": {
                    "user_id": "u123",
                    "open_id": "ou456"
                }
            }
        }
    });

    FeishuChannel::parse_and_publish_event(&event, &tx, &[], &EventDedup::new());

    let inbound = rx.try_recv().unwrap();
    assert_eq!(inbound.channel, "feishu");
    assert_eq!(inbound.sender_id, "u123");
    assert_eq!(inbound.chat_id, "oc_test");
    assert_eq!(inbound.content, "hello world");
    assert_eq!(inbound.metadata.get("message_id").unwrap(), "om_test");
}

#[test]
fn test_parse_and_publish_event_filtered() {
    let (tx, mut rx) = broadcast::channel(256);

    let event = serde_json::json!({
        "header": {
            "event_id": "evt_456",
            "event_type": "im.message.receive_v1"
        },
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_id": "om_test",
                "message_type": "text",
                "content": "{\"text\":\"hello\"}",
                "chat_type": "p2p"
            },
            "sender": {
                "sender_id": {
                    "user_id": "u_blocked"
                }
            }
        }
    });

    FeishuChannel::parse_and_publish_event(
        &event,
        &tx,
        &["u_allowed".to_string()],
        &EventDedup::new(),
    );

    assert!(rx.try_recv().is_err());
}

#[test]
fn test_extract_json_from_protobuf() {
    // Test with embedded JSON in binary data
    let json_str = r#"{"header":{"event_id":"evt_1"},"event":{"message":{"chat_id":"oc_1","content":"{\"text\":\"hi\"}"}}}"#;
    let mut data = vec![0u8; 10]; // prefix garbage bytes (simulating protobuf framing)
    data.extend_from_slice(json_str.as_bytes());

    let result = extract_json_from_protobuf(&data);
    assert!(result.is_some());
    assert!(result.unwrap().contains("evt_1"));
}

// 回归锁：循环尾退避等待必须真的睡满时长，且可被取消
// 打断。旧形态 `select! { sleep => {}, async {} => {} }` 空 async 臂立即
// Ready，sleep 被完全短路（断连后零间隔重连锤击远端 API）。

#[tokio::test]
async fn b384_backoff_wait_sleeps_full_duration_without_cancel() {
    let (_tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    let slept = FeishuChannel::backoff_wait(std::time::Duration::from_millis(50), &mut rx).await;
    assert!(slept, "无取消时必须返回 true（睡满）");
    assert!(
        start.elapsed() >= std::time::Duration::from_millis(45),
        "退避等待被短路，实际只等了 {:?}",
        start.elapsed()
    );
}

#[tokio::test]
async fn b384_backoff_wait_returns_false_on_cancel() {
    let (tx, mut rx) = tokio::sync::oneshot::channel::<()>();
    let start = std::time::Instant::now();
    drop(tx); // sender 侧 drop → oneshot 立即完成（取消语义含 drop）
    let slept = FeishuChannel::backoff_wait(std::time::Duration::from_secs(3600), &mut rx).await;
    assert!(!slept, "取消触发时必须返回 false（打断退避）");
    assert!(
        start.elapsed() < std::time::Duration::from_secs(5),
        "取消未能打断退避等待，实际耗时 {:?}",
        start.elapsed()
    );
}

// ---------------------------------------------------------------------------
// 回归锁（BUG-20261108-019）：出站 token 自愈骨架的被拒判定——只有
// HTTP 非 2xx（token 过期 wire 形态）触发刷新重试；传输层错误与空槽
// 错误前缀不同，不得触发（防无限刷新循环）。

#[test]
fn b019_is_send_rejected_matches_http_rejection_only() {
    assert!(FeishuChannel::is_send_rejected(&NemesisError::Channel(
        "feishu send error: {\"code\":99991663,\"msg\":\"invalid access token for over 2 hours\"}"
            .to_string()
    )));
    assert!(
        !FeishuChannel::is_send_rejected(&NemesisError::Channel(
            "feishu send failed: connection refused".to_string()
        )),
        "传输层错误不触发 token 刷新重试"
    );
    assert!(
        !FeishuChannel::is_send_rejected(&NemesisError::Channel(
            "no access token available".to_string()
        )),
        "空槽错误不触发重试（空槽由发送前惰性补种兜底）"
    );
    assert!(!FeishuChannel::is_send_rejected(&NemesisError::Timeout(
        "x".to_string()
    )));
}

// ---------------------------------------------------------------------------
// BUG-20261121-045/046/047/048 回归锁
// ---------------------------------------------------------------------------

/// Bind 127.0.0.1:0，对每个进来的请求回同一份固定 HTTP 响应，直到 listener
/// 被 drop（discord spawn_gateway_mock 同族，无新 dev-dep）。
fn spawn_http_mock(status_line: &'static str, body: &'static str) -> String {
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
    format!("http://127.0.0.1:{port}")
}

/// BUG-20261121-045 回归锁：api_base 单一漏斗——空回落官方默认、尾斜杠
/// 构造期归一（discord BUG-20261109-012 同款：双斜杠路径让全部 API 断链）。
#[test]
fn b045_api_base_funnel_default_and_trailing_slash() {
    let (tx, _rx) = broadcast::channel(16);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // 空 api_base 回落官方默认（serde default 空串形态）。
    let ch = FeishuChannel::new_with_client(make_config(), tx.clone(), http.clone()).unwrap();
    assert_eq!(ch.config.api_base, super::DEFAULT_API_BASE);

    // 尾斜杠归一（Lark 国际版配置形态）。
    let mut cfg = make_config();
    cfg.api_base = "https://open.larksuite.com/".to_string();
    let ch = FeishuChannel::new_with_client(cfg, tx.clone(), http.clone()).unwrap();
    assert_eq!(ch.config.api_base, "https://open.larksuite.com");

    // Default impl 自带官方默认（config 缺键经 Default 通路）。
    assert_eq!(FeishuConfig::default().api_base, super::DEFAULT_API_BASE);
}

/// BUG-20261121-046 回归锁：start/stop 必须维护 base running 旗标（trait
/// is_running 契约面——manager 的运行中守卫读它，恒 false 时守卫对本通道
/// 失效）；api_base 指 loopback 死端口（连接拒绝，零外网），auth 失败
/// warn-only 不阻断，receive loop 随 stop 退出。二次 start 走幂等守卫
/// 诚实跳过（不 spawn 第二个 WS loop）。
#[tokio::test]
async fn b046_start_stop_maintain_base_running_flag_and_guard_duplicate_start() {
    let mut cfg = make_config();
    cfg.api_base = "http://127.0.0.1:1".to_string();
    let (tx, _rx) = broadcast::channel(16);
    let ch = FeishuChannel::new_with_client(
        cfg,
        tx,
        reqwest::Client::builder().no_proxy().build().unwrap(),
    )
    .unwrap();
    assert!(!ch.base.is_running());

    ch.start().await.unwrap();
    assert!(
        ch.base.is_running(),
        "start 成功后 trait is_running 必须为 true（BUG-20261121-046）"
    );
    assert!(*ch.running.read());

    // 幂等守卫：已运行时二次 start 诚实跳过（不 spawn 第二个 receive loop）。
    ch.start().await.unwrap();
    assert!(ch.base.is_running());

    ch.stop().await.unwrap();
    assert!(
        !ch.base.is_running(),
        "stop 后 trait is_running 必须为 false"
    );
    assert!(!*ch.running.read());
}

/// BUG-20261121-047 回归锁：send 必须校验响应体业务 code——HTTP 2xx +
/// code!=0（机器人不在群/限频/缺权限等，官方文档「code 非 0 表示失败」）
/// 此前静默当成功（agent 假报已回复）。code 判定优先于 status；400+code
/// 错误形态与 200+code=0 成功形态区分。
#[tokio::test]
async fn b047_send_rejects_business_code_on_http_success() {
    let (tx, _rx) = broadcast::channel(16);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // 200 + 业务 code != 0 → Err（修复前静默 Ok）。
    let base = spawn_http_mock(
        "HTTP/1.1 200 OK",
        r#"{"code":230002,"msg":"The bot can not be outside the group."}"#,
    );
    let mut cfg = make_config();
    cfg.api_base = base;
    let ch = FeishuChannel::new_with_client(cfg, tx.clone(), http.clone()).unwrap();
    *ch.access_token.write() = "test-token".to_string();
    let err = ch
        .send_text_once("oc_x", "hello")
        .await
        .unwrap_err()
        .to_string();
    assert!(err.contains("230002"), "须透传业务 code: {err}");
    assert!(
        err.contains("feishu send error"),
        "保持 is_send_rejected 前缀契约（token 类 code 触发刷新重试）: {err}"
    );

    // 200 + code=0 → Ok（正常成功形态）。
    let base = spawn_http_mock(
        "HTTP/1.1 200 OK",
        r#"{"code":0,"msg":"success","data":{"message_id":"om_1"}}"#,
    );
    let mut cfg = make_config();
    cfg.api_base = base;
    let ch = FeishuChannel::new_with_client(cfg, tx.clone(), http.clone()).unwrap();
    *ch.access_token.write() = "test-token".to_string();
    ch.send_text_once("oc_x", "hello").await.unwrap();

    // 400 + code != 0（token 过期 wire 形态）→ Err 且前缀契约保持。
    let base = spawn_http_mock(
        "HTTP/1.1 400 Bad Request",
        r#"{"code":99991663,"msg":"invalid access token"}"#,
    );
    let mut cfg = make_config();
    cfg.api_base = base;
    let ch = FeishuChannel::new_with_client(cfg, tx.clone(), http.clone()).unwrap();
    *ch.access_token.write() = "stale-token".to_string();
    let err = ch
        .send_text_once("oc_x", "hello")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("feishu send error") && err.contains("99991663"),
        "400+code 形态保持前缀契约: {err}"
    );

    // 200 + 非 JSON 体（HTML 错误页等）→ Err 诚实报（修复前静默 Ok）。
    let base = spawn_http_mock("HTTP/1.1 200 OK", "<html>gateway error</html>");
    let mut cfg = make_config();
    cfg.api_base = base;
    let ch = FeishuChannel::new_with_client(cfg, tx.clone(), http.clone()).unwrap();
    *ch.access_token.write() = "test-token".to_string();
    let err = ch
        .send_text_once("oc_x", "hello")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("feishu send error"),
        "非 JSON 成功形态不得当成功: {err}"
    );
}

/// BUG-20261121-048 回归锁：event_id 有界去重——at-least-once 重推
/// （平台对未确认/断线窗口事件重发）此前重复喂 agent。
#[test]
fn b048_event_dedup_bounded_fifo() {
    let dedup = EventDedup::with_cap(3);
    // 首次 = 放行，重复 = 拒绝。
    assert!(dedup.check_and_record("e1"));
    assert!(!dedup.check_and_record("e1"));
    assert!(dedup.check_and_record("e2"));
    assert!(dedup.check_and_record("e3"));
    assert!(!dedup.check_and_record("e3"));
    // cap=3：e4 进场淘汰最老 e1 → e1 重新放行（FIFO 淘汰）；e1 重入
    // 不触发再淘汰（队列恰好满）；在册条目（e4）持续拒绝。
    assert!(dedup.check_and_record("e4"));
    assert!(dedup.check_and_record("e1"), "最老条目淘汰后重新放行");
    assert!(!dedup.check_and_record("e4"), "未淘汰条目仍拒绝");
}

#[test]
fn b048_duplicate_redelivery_dropped() {
    let (tx, mut rx) = broadcast::channel(256);
    let dedup = EventDedup::new();

    let event = serde_json::json!({
        "header": {"event_id": "evt_dup", "event_type": "im.message.receive_v1"},
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_id": "om_test",
                "message_type": "text",
                "content": "{\"text\":\"hello\"}"
            },
            "sender": {"sender_id": {"user_id": "u1"}}
        }
    });

    // 首次投递放行。
    FeishuChannel::parse_and_publish_event(&event, &tx, &[], &dedup);
    assert!(rx.try_recv().is_ok());

    // 同 event_id 重推 → 丢弃。
    FeishuChannel::parse_and_publish_event(&event, &tx, &[], &dedup);
    assert!(rx.try_recv().is_err(), "重推必须被去重闸丢弃");

    // 不同 event_id → 放行。
    let mut event2 = event.clone();
    event2["header"]["event_id"] = serde_json::json!("evt_other");
    FeishuChannel::parse_and_publish_event(&event2, &tx, &[], &dedup);
    assert!(rx.try_recv().is_ok(), "不同事件必须放行");
}

#[test]
fn b048_missing_event_id_passes_through() {
    let (tx, mut rx) = broadcast::channel(256);
    let dedup = EventDedup::new();

    // header/event_id 缺失（schema 1.0 老形态）→ 保守放行，宁重复不丢。
    let event = serde_json::json!({
        "event": {
            "message": {
                "chat_id": "oc_test",
                "message_type": "text",
                "content": "{\"text\":\"legacy\"}"
            },
            "sender": {"sender_id": {"user_id": "u1"}}
        }
    });

    FeishuChannel::parse_and_publish_event(&event, &tx, &[], &dedup);
    FeishuChannel::parse_and_publish_event(&event, &tx, &[], &dedup);
    assert!(rx.try_recv().is_ok());
    assert!(rx.try_recv().is_ok(), "event_id 缺失时不得去重（保守放行）");
}

/// BUG-20261121-056 回归锁：send 成功体有帽读取（BUG-20261115-024 挂账
/// 收尾）——4MB+1 的 200 响应体在读入中途断开（exceeds limit），不再
/// 全量灌内存后才在 JSON 解析臂报错。前缀契约保持（is_send_rejected 只
/// 认 token 类子串，超帽不触发刷新重试）。
#[tokio::test]
async fn b056_send_body_over_cap_aborts_mid_read() {
    let (tx, _rx) = broadcast::channel(16);
    let http = reqwest::Client::builder().no_proxy().build().unwrap();

    // 4MB+1 字节（> SEND_BODY_CAP = 4MB）非 JSON 大体；Box::leak 造
    // &'static str 供 spawn_http_mock 复用（body 参数是静态生命周期）。
    let huge: &'static str =
        Box::leak(format!("x{}", "x".repeat(4 * 1024 * 1024)).into_boxed_str());
    let base = spawn_http_mock("HTTP/1.1 200 OK", huge);
    let mut cfg = make_config();
    cfg.api_base = base;
    let ch = FeishuChannel::new_with_client(cfg, tx, http).unwrap();
    *ch.access_token.write() = "test-token".to_string();
    let err = ch
        .send_text_once("oc_x", "hello")
        .await
        .unwrap_err()
        .to_string();
    assert!(
        err.contains("exceeds limit"),
        "4MB+1 成功体须在读入中途断开（修复前全量读入后 JSON parse 报错）: {err}"
    );
    assert!(err.contains("feishu send error"), "前缀契约保持: {err}");
}
