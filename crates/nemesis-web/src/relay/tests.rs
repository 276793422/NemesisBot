//! 反向桥批次一测试：帧协议 + 中继状态机（goal：反向桥与多设备汇聚）。
//!
//! 端到端 HTTP/WS 隧道测试（真 server + mock 桥设备）见 `http_tests.rs`。

use super::protocol::{
    BridgeFrame, ChunkedDecoder, decode_frame, encode_frame, is_chunked, parse_http_response_head,
    serialize_http_request,
};
use super::server::{RelayServer, handle_control_frame};
use base64::Engine as _;
use tokio::sync::mpsc;

// ---------------------------------------------------------------------------
// A1. 帧编解码 round-trip（全部变体）
// ---------------------------------------------------------------------------

#[test]
fn frame_round_trip_all_variants() {
    let frames = vec![
        // 老客户端形态：不带集群身份字段（serde default 兼容）。
        BridgeFrame::BridgeHello {
            token: "tok-🔐".to_string(),
            node_id: "node-abcd1234".to_string(),
            name: "我的工作站".to_string(),
            version: "0.1.0".to_string(),
            cluster_node_id: None,
            cluster_name: None,
            role: None,
            category: None,
            tags: None,
            capabilities: None,
            node_type: None,
            rpc_port: None,
            addresses: None,
        },
        // 二期形态：集群身份齐备。
        BridgeFrame::BridgeHello {
            token: "tok".to_string(),
            node_id: "bridge-yangjian".to_string(),
            name: "YANGJIAN".to_string(),
            version: "0.1.0".to_string(),
            cluster_node_id: Some("node-yangjian-85292cf8".to_string()),
            cluster_name: Some("DevOne-Renamed".to_string()),
            role: Some("worker".to_string()),
            category: Some("general".to_string()),
            tags: Some(vec!["via-bridge".to_string()]),
            capabilities: Some(vec!["chat".to_string(), "file.write".to_string()]),
            node_type: Some("agent".to_string()),
            rpc_port: Some(21949),
            addresses: Some(vec!["192.168.1.10".to_string()]),
        },
        BridgeFrame::Heartbeat,
        BridgeFrame::BridgeClose {
            reason: "shutdown".to_string(),
        },
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: "welcome".to_string(),
            hub_node_id: "node-hub-1".to_string(),
        },
        BridgeFrame::BridgeWelcome {
            ok: false,
            reason: "接入门 token 不匹配（配对失败）".to_string(),
            hub_node_id: String::new(),
        },
        BridgeFrame::Pong,
        BridgeFrame::AccessCheck {
            request_id: "req-1".to_string(),
            node_id: "node-abcd1234".to_string(),
            hash_hex: "ab".repeat(32),
        },
        BridgeFrame::AccessResult {
            request_id: "req-1".to_string(),
            ok: true,
        },
        BridgeFrame::ConnOpen {
            conn_id: 42,
            target: "local".to_string(),
        },
        BridgeFrame::ConnData {
            conn_id: 42,
            seq: 7,
            data_b64: base64::engine::general_purpose::STANDARD.encode(b"GET / HTTP/1.1\r\n\r\n"),
            fin: true,
        },
        BridgeFrame::ConnClose {
            conn_id: 42,
            reason: "eof".to_string(),
        },
        BridgeFrame::ClusterRpc {
            payload: serde_json::json!({"action": "ping", "n": 1}),
        },
        BridgeFrame::MemberSync {
            payload: serde_json::json!({"members": []}),
        },
    ];
    for f in frames {
        let text = encode_frame(&f).expect("encode");
        let back = decode_frame(&text).expect("decode");
        assert_eq!(f, back, "round-trip 应无损：{text}");
    }
}

#[test]
fn frame_tag_is_snake_case_type_field() {
    // 协议契约：内部 tag 字段名为 "type"，值为 snake_case（跨端实现依据）。
    let text = encode_frame(&BridgeFrame::Heartbeat).unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "heartbeat");
    let text = encode_frame(&BridgeFrame::BridgeHello {
        token: "t".into(),
        node_id: "n".into(),
        name: "n".into(),
        version: "v".into(),
        cluster_node_id: None,
        cluster_name: None,
        role: None,
        category: None,
        tags: None,
        capabilities: None,
        node_type: None,
        rpc_port: None,
        addresses: None,
    })
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "bridge_hello");
    let text = encode_frame(&BridgeFrame::AccessCheck {
        request_id: "r".into(),
        node_id: "n".into(),
        hash_hex: "h".into(),
    })
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&text).unwrap();
    assert_eq!(v["type"], "access_check");
}

#[test]
fn frame_decode_garbage_is_err() {
    assert!(decode_frame("not json").is_err());
    assert!(decode_frame("{\"type\":\"unknown_kind\"}").is_err());
}

// ---------------------------------------------------------------------------
// A2. 请求重序列化（serialize_http_request）
// ---------------------------------------------------------------------------

#[test]
fn serialize_get_request_no_body() {
    let req: http::Request<()> = http::Request::builder()
        .method("GET")
        .uri("http://example.com/api/status?x=1")
        .header("accept", "*/*")
        .body(())
        .unwrap();
    let (parts, _body) = req.into_parts();
    let bytes = serialize_http_request(&parts.method, &parts.uri, &parts.headers, b"");
    let text = String::from_utf8(bytes).unwrap();
    assert!(
        text.starts_with("GET /api/status?x=1 HTTP/1.1\r\n"),
        "{text}"
    );
    assert!(text.contains("accept: */*\r\n"));
    assert!(text.contains("host: example.com\r\n"));
    assert!(text.ends_with("content-length: 0\r\n\r\n"));
}

#[test]
fn serialize_post_request_rewrites_content_length() {
    let body = b"hello=world";
    let req: http::Request<Vec<u8>> = http::Request::builder()
        .method("POST")
        .uri("http://example.com/api/upload")
        .header("content-length", "999") // 谎报——重序列化必须重算
        .body(body.to_vec())
        .unwrap();
    let (parts, body) = req.into_parts();
    let bytes = serialize_http_request(&parts.method, &parts.uri, &parts.headers, &body);
    let text = String::from_utf8(bytes.clone()).unwrap();
    assert!(text.contains("content-length: 11\r\n"), "{text}");
    assert!(!text.contains("999"));
    assert!(bytes.ends_with(&body));
}

#[test]
fn serialize_strips_hop_by_hop_but_keeps_upgrade() {
    let req: http::Request<()> = http::Request::builder()
        .method("GET")
        .uri("http://example.com/ws")
        .header("connection", "Upgrade")
        .header("upgrade", "websocket")
        .header("keep-alive", "timeout=5")
        .header("sec-websocket-key", "dGhlIHNhbXBsZSBub25jZQ==")
        .body(())
        .unwrap();
    let (parts, _body) = req.into_parts();
    let text = String::from_utf8(serialize_http_request(
        &parts.method,
        &parts.uri,
        &parts.headers,
        b"",
    ))
    .unwrap();
    // WS 升级语义头必须保留。
    assert!(text.contains("connection: Upgrade\r\n"), "{text}");
    assert!(text.contains("upgrade: websocket\r\n"));
    assert!(text.contains("sec-websocket-key: dGhlIHNhbXBsZSBub25jZQ=="));
    // 其余 hop-by-hop 头剥离。
    assert!(!text.contains("keep-alive"));
}

// ---------------------------------------------------------------------------
// A3. 响应头解析 + chunked 解码
// ---------------------------------------------------------------------------

#[test]
fn parse_response_head_ok() {
    let raw =
        b"HTTP/1.1 200 OK\r\ncontent-type: text/html\r\nset-cookie: a=1; Path=/\r\n\r\n<html>";
    let (status, headers, head_len) = parse_http_response_head(raw).unwrap();
    assert_eq!(status, http::StatusCode::OK);
    assert!(headers.contains(&("content-type".to_string(), "text/html".to_string())));
    assert_eq!(head_len, raw.len() - 6); // 头部截止在 \r\n\r\n 之后（"<html>" 6 字节）
    assert!(&raw[head_len..] == b"<html>");
}

#[test]
fn parse_response_head_incomplete_returns_none() {
    let raw = b"HTTP/1.1 200 OK\r\ncontent-type: text/html";
    assert!(parse_http_response_head(raw).is_none());
    assert!(parse_http_response_head(b"").is_none());
}

#[test]
fn chunked_detection() {
    assert!(is_chunked(&[(
        "Transfer-Encoding".into(),
        "chunked".into()
    )]));
    assert!(!is_chunked(&[("Content-Length".into(), "5".into())]));
}

#[test]
fn chunked_decoder_whole_feed() {
    let mut d = ChunkedDecoder::new();
    let out = d.feed(b"5\r\nhello\r\n3\r\n wo\r\n0\r\n\r\n");
    assert_eq!(out, b"hello wo");
    assert!(d.is_finished());
}

#[test]
fn chunked_decoder_split_feed_across_boundaries() {
    let mut d = ChunkedDecoder::new();
    let o1 = d.feed(b"5\r\nhel"); // 块头 + 半个块
    assert_eq!(o1, b"hel");
    let o2 = d.feed(b"lo\r\n"); // 块尾
    assert_eq!(o2, b"lo");
    let o3 = d.feed(b"0\r\n\r\n"); // 尾块
    assert!(o3.is_empty());
    assert!(d.is_finished());
}

#[test]
fn chunked_decoder_extension_ignored() {
    // 块大小行允许 `;` 后的扩展（RFC 9112 §7.1）。
    let mut d = ChunkedDecoder::new();
    let out = d.feed(b"5;name=val\r\nabcde\r\n0\r\n\r\n");
    assert_eq!(out, b"abcde");
    assert!(d.is_finished());
}

// ---------------------------------------------------------------------------
// A4. WS 帧编解码（ws_codec）
// ---------------------------------------------------------------------------

mod ws_codec_tests {
    use super::super::ws_codec::{encode_client_frame, parse_server_frame};
    use axum::extract::ws::Message;

    #[test]
    fn client_frame_text_masked_round_trip() {
        let frame = encode_client_frame(&Message::Text("你好 bridge".into()));
        // client 帧必须带 mask 位（RFC 6455 §5.3——否则设备侧 tungstenite 拒收）。
        assert_eq!(frame[1] & 0x80, 0x80);
        let (msg, consumed) = parse_server_frame(&frame).unwrap();
        assert_eq!(consumed, frame.len());
        match msg {
            Message::Text(t) => assert_eq!(t.as_str(), "你好 bridge"),
            other => panic!("期望 Text，得到 {other:?}"),
        }
    }

    #[test]
    fn client_frame_binary_large_uses_extended_length() {
        let payload = vec![7u8; 70_000]; // > u16 → 64 位长度
        let frame = encode_client_frame(&Message::Binary(payload.clone().into()));
        assert!(frame.len() > payload.len() + 10);
        let (msg, consumed) = parse_server_frame(&frame).unwrap();
        assert_eq!(consumed, frame.len());
        match msg {
            Message::Binary(b) => assert_eq!(b.to_vec(), payload),
            other => panic!("期望 Binary，得到 {other:?}"),
        }
    }

    #[test]
    fn server_frame_unmasked_round_trip() {
        // 设备侧 web server（hyper）发出的是 unmasked server 帧。
        let mut frame = vec![0x81u8]; // FIN + text
        frame.push(5);
        frame.extend_from_slice(b"hello");
        let (msg, consumed) = parse_server_frame(&frame).unwrap();
        assert_eq!(consumed, frame.len());
        match msg {
            Message::Text(t) => assert_eq!(t.as_str(), "hello"),
            other => panic!("期望 Text，得到 {other:?}"),
        }
    }

    #[test]
    fn control_frames_round_trip() {
        for original in [Message::Ping(vec![1, 2].into()), Message::Close(None)] {
            let frame = encode_client_frame(&original);
            let (msg, _) = parse_server_frame(&frame).unwrap();
            match (original, msg) {
                (Message::Ping(a), Message::Ping(b)) => assert_eq!(a.to_vec(), b.to_vec()),
                (Message::Close(_), Message::Close(_)) => {}
                (a, b) => panic!("类型不匹配：{a:?} vs {b:?}"),
            }
        }
    }

    #[test]
    fn partial_frame_returns_none() {
        let frame = encode_client_frame(&Message::Text("abcdef".into()));
        // 缺尾部字节 → 不完整帧。
        for cut in [1usize, 3, frame.len() - 1] {
            assert!(parse_server_frame(&frame[..cut]).is_none(), "cut={cut}");
        }
        assert!(parse_server_frame(&[]).is_none());
    }

    #[test]
    fn two_frames_back_to_back_parse_sequentially() {
        let f1 = encode_client_frame(&Message::Text("a".into()));
        let f2 = encode_client_frame(&Message::Text("b".into()));
        let mut buf = f1.clone();
        buf.extend_from_slice(&f2);
        let (m1, c1) = parse_server_frame(&buf).unwrap();
        let (m2, c2) = parse_server_frame(&buf[c1..]).unwrap();
        assert_eq!(c1 + c2, buf.len());
        match (m1, m2) {
            (Message::Text(a), Message::Text(b)) => {
                assert_eq!(a.as_str(), "a");
                assert_eq!(b.as_str(), "b");
            }
            _ => panic!("期望两个 Text"),
        }
    }
}

// ---------------------------------------------------------------------------
// B. 中继状态机（RelayServer）
// ---------------------------------------------------------------------------

/// 测试脚手架：注册一台假设备并持有其下行帧接收端。
struct FakeDevice {
    generation: u64,
    outbound_rx: mpsc::Receiver<BridgeFrame>,
}

/// 注册假设备（构造下行 channel，接收端留在测试内消费/防队列满）。
fn register_fake(relay: &RelayServer, node_id: &str, token: &str, name: &str) -> FakeDevice {
    register_fake_with_identity(relay, node_id, token, name, None)
}

/// 注册假设备（带集群身份变体——二期身份事件测试用）。
fn register_fake_with_identity(
    relay: &RelayServer,
    node_id: &str,
    token: &str,
    name: &str,
    identity: Option<super::identity::BridgeClusterIdentity>,
) -> FakeDevice {
    let (tx, rx) = mpsc::channel::<BridgeFrame>(256);
    let generation = relay
        .authenticate_device(token, node_id, name, "test", identity, tx)
        .expect("注册应成功");
    FakeDevice {
        generation,
        outbound_rx: rx,
    }
}

#[test]
fn gate_open_semantics() {
    // 空 token = 门不开放（fail-closed）。
    let relay = RelayServer::new(String::new(), true);
    assert!(!relay.is_gate_open());

    let relay = RelayServer::new("tok".to_string(), true);
    assert!(relay.is_gate_open());
    assert!(relay.is_full_mode());

    let relay_pure = RelayServer::new("tok".to_string(), false);
    assert!(!relay_pure.is_full_mode());

    // 运行时开关。
    let d = register_fake(&relay, "n1", "tok", "设备一");
    relay.set_enabled(false);
    assert!(!relay.is_gate_open());
    assert!(relay.list_devices().is_empty(), "关开关应踢光设备");
    drop(d);
}

#[test]
fn authenticate_token_mismatch_rejected_with_honest_reason() {
    let relay = RelayServer::new("right-token".to_string(), true);
    let (tx, _rx) = mpsc::channel(8);
    let err = relay
        .authenticate_device("wrong-token", "n1", "名", "v", None, tx)
        .expect_err("错 token 必须拒");
    assert!(err.contains("token 不匹配"), "{err}");
    assert!(relay.list_devices().is_empty());

    // 空 node_id 拒绝。
    let (tx, _rx) = mpsc::channel(8);
    assert!(
        relay
            .authenticate_device("right-token", "", "名", "v", None, tx)
            .is_err()
    );
}

#[test]
fn device_replacement_newest_connection_wins() {
    let relay = RelayServer::new("tok".to_string(), true);
    let old = register_fake(&relay, "n1", "tok", "旧名");
    let new = register_fake(&relay, "n1", "tok", "新名");

    // 旧连接代际失效（读循环据此退出）；表里是新连接。
    assert!(!relay.generation_valid("n1", old.generation));
    assert!(relay.generation_valid("n1", new.generation));
    let devices = relay.list_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].name, "新名");

    // 旧读循环退出时不能误删新表项（代际不匹配 → no-op）。
    relay.mark_device_gone("n1", old.generation);
    assert_eq!(relay.list_devices().len(), 1);
    // 新读循环退出才真正移除。
    relay.mark_device_gone("n1", new.generation);
    assert!(relay.list_devices().is_empty());
}

#[test]
fn multi_device_concurrent_and_directed_send() {
    let relay = RelayServer::new("tok".to_string(), true);
    let mut a = register_fake(&relay, "node-aaa", "tok", "A 机");
    let mut b = register_fake(&relay, "node-bbb", "tok", "B 机");
    let mut c = register_fake(&relay, "node-ccc", "tok", "C 机");
    // 三期批次八：注册即广播 member_sync——排空注册触发的帧再断言业务。
    for d in [&mut a, &mut b, &mut c] {
        while d.outbound_rx.try_recv().is_ok() {}
    }

    let devices = relay.list_devices();
    assert_eq!(devices.len(), 3);
    assert!(devices.iter().all(|d| d.online));

    // 定向下发只到达目标设备。
    assert!(relay.send_to_device(
        "node-bbb",
        BridgeFrame::ConnOpen {
            conn_id: 1,
            target: "local".into()
        },
        0
    ));
    let f = b.outbound_rx.try_recv().expect("B 应收到帧");
    assert!(matches!(f, BridgeFrame::ConnOpen { conn_id: 1, .. }));
    assert!(a.outbound_rx.try_recv().is_err());
    assert!(c.outbound_rx.try_recv().is_err());

    // 不在线设备发送失败。
    assert!(!relay.send_to_device("node-zzz", BridgeFrame::Pong, 0));
}

#[test]
fn conn_routing_and_eof_semantics() {
    let relay = RelayServer::new("tok".to_string(), true);
    let conn_id = relay.alloc_conn_id();
    let mut rx = relay.register_conn(conn_id, "node-aaa", 0);

    assert!(relay.route_conn_data(conn_id, b"HTTP/1.1 200 OK\r\n\r\n".to_vec()));
    assert_eq!(rx.try_recv().unwrap(), b"HTTP/1.1 200 OK\r\n\r\n".to_vec());

    // close 后路由失败（通道已关）。
    relay.close_conn(conn_id, "eof");
    assert!(!relay.route_conn_data(conn_id, b"x".to_vec()));
}

#[test]
fn auth_session_is_per_device() {
    let relay = RelayServer::new("tok".to_string(), true);
    let cookie_a = relay.create_auth_session("node-aaa");
    // cookie_a 只对 node-aaa 有效。
    assert!(relay.session_valid(&cookie_a, "node-aaa"));
    assert!(!relay.session_valid(&cookie_a, "node-bbb"));
    assert!(!relay.session_valid("nonexistent", "node-aaa"));
}

#[tokio::test]
async fn access_check_three_states() {
    let relay = RelayServer::new("tok".to_string(), true);

    // ① 设备离线 → Err。
    let err = relay.access_check("node-ghost", &"ab".repeat(32)).await;
    assert!(err.is_err());

    let mut d = register_fake(&relay, "node-aaa", "tok", "A 机");

    // ② 设备回 ok=false → Ok(false)。
    let relay_clone = std::sync::Arc::new(relay);
    let hash_fail = "cd".repeat(32);
    let hash_ok = "ef".repeat(32);
    let r1 = {
        let relay = relay_clone.clone();
        let responder = async {
            // 消费下行帧直到拿到 AccessCheck，回 fail。
            loop {
                match d.outbound_rx.recv().await {
                    Some(BridgeFrame::AccessCheck { request_id, .. }) => {
                        relay_clone.deliver_access_result(&request_id, false);
                        break;
                    }
                    Some(_) => continue,
                    None => panic!("下行通道意外关闭"),
                }
            }
        };
        tokio::join!(relay.access_check("node-aaa", &hash_fail), responder).0
    };
    assert_eq!(r1, Ok(false));

    // ③ 设备回 ok=true → Ok(true)。
    let r2 = {
        let responder = async {
            loop {
                match d.outbound_rx.recv().await {
                    Some(BridgeFrame::AccessCheck { request_id, .. }) => {
                        relay_clone.deliver_access_result(&request_id, true);
                        break;
                    }
                    Some(_) => continue,
                    None => panic!("下行通道意外关闭"),
                }
            }
        };
        tokio::join!(relay_clone.access_check("node-aaa", &hash_ok), responder).0
    };
    assert_eq!(r2, Ok(true));
}

#[test]
fn heartbeat_timeout_kicks_device() {
    let relay = std::sync::Arc::new(RelayServer::new("tok".to_string(), true));
    let d = register_fake(&relay, "node-sleepy", "tok", "打盹机");

    // 拨旧 last_seen（越过 90s 心跳判定线）后跑一次维护扫描。
    relay.backdate_device_last_seen_for_test("node-sleepy", 91);
    relay.maintenance_tick();
    assert!(relay.list_devices().is_empty(), "90s 无心跳应判离线并踢出");
    drop(d);

    // 心跳活跃设备不受影响。
    let fresh = register_fake(&relay, "node-fresh", "tok", "新鲜机");
    relay.touch_device("node-fresh", 0);
    relay.maintenance_tick();
    assert_eq!(relay.list_devices().len(), 1);
    drop(fresh);
}

#[test]
fn reserved_frames_go_through_control_dispatch_without_effect() {
    let relay = std::sync::Arc::new(RelayServer::new("tok".to_string(), true));
    let mut d = register_fake(&relay, "node-aaa", "tok", "A 机");
    // 三期批次八：注册即广播——先排空，再验证预留帧不产生新下行。
    while d.outbound_rx.try_recv().is_ok() {}
    // 二/三期预留帧：一期收到即忽略（不 panic、不投递、不路由）。
    handle_control_frame(
        BridgeFrame::ClusterRpc {
            payload: serde_json::json!({"x": 1}),
        },
        &relay,
        "node-aaa",
    );
    handle_control_frame(
        BridgeFrame::MemberSync {
            payload: serde_json::json!({}),
        },
        &relay,
        "node-aaa",
    );
    assert!(d.outbound_rx.try_recv().is_err(), "预留帧不应产生任何下行");
}

#[test]
fn heartbeat_control_frame_replies_pong() {
    let relay = std::sync::Arc::new(RelayServer::new("tok".to_string(), true));
    let mut d = register_fake(&relay, "node-aaa", "tok", "A 机");
    // 三期批次八：注册即广播——排空后心跳的 Pong 才是队列首帧。
    while d.outbound_rx.try_recv().is_ok() {}
    handle_control_frame(BridgeFrame::Heartbeat, &relay, "node-aaa");
    match d.outbound_rx.try_recv().expect("心跳应回 pong") {
        BridgeFrame::Pong => {}
        other => panic!("期望 Pong，得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// C. 二期身份交换（goal 批次五）：身份事件 + 老格式兼容
// ---------------------------------------------------------------------------

use super::identity::{BridgeClusterIdentity, BridgeIdentityEvent, BridgeIdentitySink};

/// mock sink：收集全部身份事件供断言。
#[derive(Clone, Default)]
struct EventLog(std::sync::Arc<std::sync::Mutex<Vec<BridgeIdentityEvent>>>);

impl EventLog {
    fn snapshot(&self) -> Vec<(String, bool, Option<String>)> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .map(|e| {
                (
                    e.bridge_node_id.clone(),
                    e.online,
                    e.cluster.as_ref().map(|c| c.node_id.clone()),
                )
            })
            .collect()
    }
}

impl BridgeIdentitySink for EventLog {
    fn on_bridge_identity(&self, event: BridgeIdentityEvent) {
        self.0.lock().unwrap().push(event);
    }
}

fn sample_identity(node_id: &str) -> BridgeClusterIdentity {
    BridgeClusterIdentity {
        node_id: node_id.to_string(),
        name: "远端机".to_string(),
        role: "worker".to_string(),
        category: "general".to_string(),
        tags: vec![],
        capabilities: vec!["chat".to_string()],
        node_type: "agent".to_string(),
        rpc_port: 21949,
        addresses: vec!["10.0.0.5".to_string()],
    }
}

#[test]
fn legacy_hello_without_cluster_fields_still_decodes() {
    // 老版本 hello（无集群身份字段）必须可解码（serde default 兼容），
    // 且全部身份字段为 None = 纯隧道设备。
    let legacy =
        r#"{"type":"bridge_hello","token":"t","node_id":"bridge-x","name":"X","version":"0.1.0"}"#;
    let frame = decode_frame(legacy).expect("老格式必须兼容");
    let BridgeFrame::BridgeHello {
        cluster_node_id,
        rpc_port,
        ..
    } = frame
    else {
        panic!("期望 BridgeHello");
    };
    assert_eq!(cluster_node_id, None);
    assert_eq!(rpc_port, None);
}

#[test]
fn identity_events_online_offline_and_replacement_no_false_offline() {
    let relay = std::sync::Arc::new(RelayServer::new("tok".to_string(), true));
    let log = EventLog::default();
    relay.set_identity_sink(std::sync::Arc::new(log.clone()));

    // 上线：online 事件携带集群身份。
    let old = register_fake_with_identity(
        &relay,
        "bridge-x",
        "tok",
        "X 机",
        Some(sample_identity("node-x-1")),
    );
    // 顶替重连：新连接再报 online（宿主侧注册幂等）。
    let new = register_fake_with_identity(
        &relay,
        "bridge-x",
        "tok",
        "X 机",
        Some(sample_identity("node-x-1")),
    );
    // 旧循环退出（代际失配）：不误报 offline。
    relay.mark_device_gone("bridge-x", old.generation);
    // 新循环退出：报 offline。
    relay.mark_device_gone("bridge-x", new.generation);

    let events = log.snapshot();
    assert_eq!(
        events,
        vec![
            ("bridge-x".into(), true, Some("node-x-1".into())),
            ("bridge-x".into(), true, Some("node-x-1".into())),
            ("bridge-x".into(), false, Some("node-x-1".into())),
        ],
        "顶替场景：online×2 + 新连接断开 offline，旧循环不误报"
    );
}

#[test]
fn identity_events_on_heartbeat_kick_and_switch_off() {
    let relay = std::sync::Arc::new(RelayServer::new("tok".to_string(), true));
    let log = EventLog::default();
    relay.set_identity_sink(std::sync::Arc::new(log.clone()));

    // 心跳踢 → offline。
    let d = register_fake_with_identity(
        &relay,
        "bridge-sleepy",
        "tok",
        "打盹机",
        Some(sample_identity("node-sleepy-1")),
    );
    relay.backdate_device_last_seen_for_test("bridge-sleepy", 91);
    relay.maintenance_tick();
    assert!(relay.list_devices().is_empty());
    drop(d);

    // 开关踢（两台，一台纯隧道 None 身份）→ 逐设备 offline。
    let _a = register_fake_with_identity(
        &relay,
        "bridge-a",
        "tok",
        "A 机",
        Some(sample_identity("node-a-1")),
    );
    let _b = register_fake(&relay, "bridge-b", "tok", "B 机");
    relay.set_enabled(false);

    let events = log.snapshot();
    assert!(
        events.contains(&("bridge-sleepy".into(), false, Some("node-sleepy-1".into()))),
        "心跳踢应报 offline：{events:?}"
    );
    assert!(
        events.contains(&("bridge-a".into(), false, Some("node-a-1".into())))
            && events.contains(&("bridge-b".into(), false, None)),
        "开关踢应逐设备报 offline（纯隧道设备 cluster=None）：{events:?}"
    );
}

#[test]
fn hub_node_id_setter_round_trip() {
    let relay = RelayServer::new("tok".to_string(), true);
    assert_eq!(relay.hub_node_id(), "", "缺省空 = --relay 纯中继语义");
    relay.set_hub_node_id("node-hub-1".to_string());
    assert_eq!(relay.hub_node_id(), "node-hub-1");
}

// ---------------------------------------------------------------------------
// 三期批次八：member_sync 广播（快照闭包 / --relay 设备表摘要 / gate 关跳过）
// ---------------------------------------------------------------------------

#[test]
fn member_sync_broadcast_three_sources() {
    // 形态一：注入快照闭包（正常模式 hub）→ 广播 registry 摘要。
    // 快照先于注册注入——注册本身触发的广播已用该快照。
    let relay = RelayServer::new("tok".to_string(), true);
    relay.set_member_snapshot(std::sync::Arc::new(|| {
        serde_json::json!({
            "members": [
                {"node_id": "node-hub", "name": "Hub", "online": true,
                 "via_bridge": false, "addresses": [], "rpc_port": 21949,
                 "role": "coordinator", "category": "general",
                 "capabilities": [], "node_type": "agent"},
                {"node_id": "node-x", "name": "X", "online": true,
                 "via_bridge": true, "addresses": ["10.0.0.9"], "rpc_port": 21959,
                 "role": "worker", "category": "general",
                 "capabilities": [], "node_type": "agent"},
            ]
        })
    }));
    let mut dev = register_fake(&relay, "bridge-a", "tok", "A 机");
    relay.broadcast_member_sync();
    match dev.outbound_rx.try_recv().expect("应收到帧") {
        BridgeFrame::MemberSync { payload } => {
            let members = payload["members"].as_array().expect("members 数组");
            assert_eq!(members.len(), 2);
            assert_eq!(members[0]["node_id"], "node-hub");
            assert_eq!(members[1]["via_bridge"], true);
        }
        other => panic!("应收到 MemberSync，得到 {other:?}"),
    }

    // 形态二：未注入快照（--relay）→ 广播自身桥设备表摘要。
    let relay = RelayServer::new("tok".to_string(), true);
    let mut dev = register_fake(&relay, "bridge-b", "tok", "B 机");
    relay.broadcast_member_sync();
    match dev.outbound_rx.try_recv().expect("应收到帧") {
        BridgeFrame::MemberSync { payload } => {
            let members = payload["members"].as_array().expect("members 数组");
            assert_eq!(members.len(), 1, "--relay 摘要 = 桥设备表");
            assert_eq!(members[0]["node_id"], "bridge-b");
            assert_eq!(members[0]["role"], "device");
            assert_eq!(members[0]["online"], true);
        }
        other => panic!("应收到 MemberSync，得到 {other:?}"),
    }

    // 形态三：接入门关闭 → 广播静默跳过（无帧、不 panic）。
    let relay = RelayServer::new("tok".to_string(), true);
    let mut dev = register_fake(&relay, "bridge-c", "tok", "C 机");
    // 排空注册触发的那帧（注册时门还开着）。
    let _ = dev.outbound_rx.try_recv();
    relay.set_enabled(false);
    relay.broadcast_member_sync();
    assert!(dev.outbound_rx.try_recv().is_err(), "门关闭时不应广播");
}

#[test]
fn member_sync_triggered_by_lifecycle_changes() {
    let relay = RelayServer::new("tok".to_string(), true);
    let mut dev = register_fake(&relay, "bridge-a", "tok", "A 机");

    // 注册即触发（authenticate_device 变化点）：rx 里已有注册广播。
    match dev.outbound_rx.try_recv().expect("注册应触发广播") {
        BridgeFrame::MemberSync { payload } => {
            let members = payload["members"].as_array().expect("members 数组");
            assert_eq!(members[0]["node_id"], "bridge-a");
        }
        other => panic!("注册触发应发 MemberSync，得到 {other:?}"),
    }

    // 断开（代际匹配移除）→ 离线广播经 notify_identity_offline 触发——
    // 设备已不在表，帧发不到自己（尽力而为语义：表空无收件人）。
    relay.mark_device_gone("bridge-a", dev.generation);
    assert!(dev.outbound_rx.try_recv().is_err(), "移除后无新帧");
}
