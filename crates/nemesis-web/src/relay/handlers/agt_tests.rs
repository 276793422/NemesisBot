//! relay/handlers.rs AGT 覆盖率批次（2026-09-24）。
//!
//! 与 http_tests（e2e 全链路）互补：本文件声明在 handlers.rs 内部，可触达
//! 私有项（relay_status_html / is_websocket_upgrade）。聚焦 e2e 未覆盖的
//! 确定性臂：
//! - `extract_cookie` 多对 cookie 中目标缺失 / 目标命中分支
//! - `relay_status_html(full_mode=true)` dashboard 入口臂
//! - `handle_relay_api_enabled` 四路：relay None / 请求体过大 / 缺 on 字段 /
//!   on=true|false 成功（运行时开关生效）
//! - `handle_relay_login` 门关 404 / 请求体过大
//! - `handle_auth_submit` 错误梯子：门关 / 过大 / 坏 JSON / 坏哈希格式 /
//!   access_check Err（设备不在表 →「设备不在线」）
//! - `handle_auth_page` 门关 404 / 无 cookie 输入页（含 HTML 转义）/
//!   已授权会话 → 面板重定向
//! - `handle_ws_tunnel` 无效 WS 升级头（缺 sec-websocket-key）→ 400
//! - `handle_http_tunnel` 设备 EOF 先于响应头 → 502「未返回任何响应」
//!   （in-process 设备：直接 authenticate_device 持下行 rx +
//!   route_conn_data 喂响应字节，无需真 socket）
//! - /bridge 上行帧梯子（真 socket）：静默掉线（握手窗口 EOF）、集群身份
//!   hello 组装、握手后重复 hello / 设备发 ConnOpen（协议错乱）/ ConnData
//!   坏 base64 / 未知 conn 的 ConnData → ConnClose 回执 / ClusterRpc 无
//!   sink WARN → 装 mock sink 后回声 / Binary 忽略 / BridgeClose 优雅退出
//! - 同 node_id 重连顶替：旧连接退出、新连接存活
//!
//! 结构性豁免（见报告）：10s hello 超时臂、30s 响应头超时臂（时序）、
//! encode_or_none 失败臂（合法帧恒可编码，防御性）、下行 send 失败臂
//! （设备在 online 检查后瞬间断开的竞态）、响应构造失败臂（需设备回
//! 非法头字节——hyper/parse 层先拒绝）、256MB body 超限臂。

use super::*;
use crate::relay::cluster_frame::ClusterFrameSink;
use crate::relay::protocol::{BridgeFrame, decode_frame, encode_frame};
use crate::relay::server::{DEVICE_OUTBOUND_CAPACITY, RelayServer};
use axum::body::Body;
use axum::extract::Request;
use axum::http::{HeaderMap, Request as HttpRequest, StatusCode, header};
use axum::response::Response;
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use std::time::Duration;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio_tungstenite::tungstenite::Message as TungMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

// -----------------------------------------------------------------------
// 直调 harness（无需真 socket）
// -----------------------------------------------------------------------

fn agt_relay(token: &str) -> Arc<RelayServer> {
    Arc::new(RelayServer::new(token.to_string(), true))
}

/// in-process 设备：直接登记，持下行 rx 扮演设备（online = 表里有 + 90s
/// 心跳窗内新登记）。
fn agt_register_device(relay: &RelayServer, node_id: &str) -> mpsc::Receiver<BridgeFrame> {
    let (tx, rx) = mpsc::channel::<BridgeFrame>(DEVICE_OUTBOUND_CAPACITY);
    relay
        .authenticate_device("agt-token", node_id, "设备<&X>", "agt-1.0", None, tx)
        .expect("登记设备");
    rx
}

fn agt_req(method: &str, uri: &str, headers: &[(&str, &str)], body: Body) -> Request {
    let mut b = HttpRequest::builder().method(method).uri(uri);
    for (k, v) in headers {
        b = b.header(*k, *v);
    }
    b.body(body).unwrap()
}

async fn agt_resp(resp: Response) -> (StatusCode, String) {
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1 << 20)
        .await
        .unwrap();
    (status, String::from_utf8_lossy(&bytes).to_string())
}

// -----------------------------------------------------------------------
// extract_cookie / relay_status_html（私有项）
// -----------------------------------------------------------------------

#[test]
fn agt_extract_cookie_matrix() {
    let mut headers = HeaderMap::new();
    // 无 Cookie 头 → None（早退臂）
    assert_eq!(extract_cookie(&headers, "nemesis_bridge_auth"), None);

    // 目标命中（多对中取值 + 空白容忍）
    headers.insert(
        header::COOKIE,
        header::HeaderValue::from_static("a=1; nemesis_bridge_auth=tok3n ; b=2"),
    );
    assert_eq!(
        extract_cookie(&headers, "nemesis_bridge_auth"),
        Some("tok3n".to_string())
    );

    // 目标缺失（其余 cookie 存在）→ 循环走完 → None
    headers.insert(header::COOKIE, header::HeaderValue::from_static("a=1; b=2"));
    assert_eq!(extract_cookie(&headers, "nemesis_bridge_auth"), None);

    // 无 '=' 的裸段不炸
    headers.insert(
        header::COOKIE,
        header::HeaderValue::from_static("junk; a=1"),
    );
    assert_eq!(extract_cookie(&headers, "nemesis_bridge_auth"), None);
}

#[test]
fn agt_relay_status_html_full_mode_toggle() {
    // full_mode=true → dashboard 入口按钮臂
    let full = relay_status_html(true);
    assert!(full.contains("打开本机 Dashboard"), "full: {full}");
    assert!(!full.contains("纯中继模式"));
    // full_mode=false → 纯中继提示臂
    let bare = relay_status_html(false);
    assert!(bare.contains("纯中继模式"));
    assert!(!bare.contains("打开本机 Dashboard"));
}

// -----------------------------------------------------------------------
// handle_relay_api_enabled / handle_relay_login 直调
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_handle_relay_api_enabled_ladder() {
    let relay = agt_relay("agt-token");

    // ① relay 未配置
    let resp = handle_relay_api_enabled(
        None,
        agt_req("POST", "/api/relay/enabled", &[], Body::empty()),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("中继服务端未配置"), "{body}");

    // ② 请求体过大（> 4KB）
    let big = vec![b'x'; 5000];
    let resp = handle_relay_api_enabled(
        Some(relay.clone()),
        agt_req("POST", "/api/relay/enabled", &[], Body::from(big)),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("请求体过大"), "{body}");

    // ③ 缺 on 字段
    let resp = handle_relay_api_enabled(
        Some(relay.clone()),
        agt_req("POST", "/api/relay/enabled", &[], Body::from("{}")),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("缺少 on"), "{body}");

    // ④ on=true → 开启生效；on=false → 关闭生效
    for (on, expect) in [(true, true), (false, false)] {
        let payload = format!("{{\"on\": {on}}}");
        let resp = handle_relay_api_enabled(
            Some(relay.clone()),
            agt_req("POST", "/api/relay/enabled", &[], Body::from(payload)),
        )
        .await;
        let (status, body) = agt_resp(resp).await;
        assert_eq!(status, StatusCode::OK);
        let v: serde_json::Value = serde_json::from_str(&body).unwrap();
        assert_eq!(v["ok"], true);
        assert_eq!(v["enabled"], expect);
        assert_eq!(relay.is_enabled(), expect);
    }
    relay.set_enabled(true);
}

#[tokio::test]
async fn agt_handle_relay_login_gate_and_oversize() {
    let relay = agt_relay("agt-token");
    let hash = relay.admin_hash_hex();

    // ① 门关 → 404
    relay.set_enabled(false);
    let resp = handle_relay_login(
        relay.clone(),
        agt_req(
            "POST",
            "/relay/login",
            &[],
            Body::from(format!("{{\"hash\":\"{hash}\"}}")),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    relay.set_enabled(true);

    // ② 请求体过大
    let big = vec![b'y'; 5000];
    let resp = handle_relay_login(
        relay.clone(),
        agt_req("POST", "/relay/login", &[], Body::from(big)),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("请求体过大"), "{body}");

    // ③ 错误哈希 → 401
    let resp = handle_relay_login(
        relay.clone(),
        agt_req(
            "POST",
            "/relay/login",
            &[],
            Body::from("{\"hash\":\"deadbeef\"}"),
        ),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    assert!(body.contains("令牌错误"), "{body}");
}

// -----------------------------------------------------------------------
// handle_auth_submit / handle_auth_page 直调
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_handle_auth_submit_error_ladder() {
    let relay = agt_relay("agt-token");
    let hex64 = "a".repeat(64);

    // ① 门关 → 404
    relay.set_enabled(false);
    let resp = handle_auth_submit(
        relay.clone(),
        "node-a".to_string(),
        agt_req(
            "POST",
            "/d/node-a/__auth",
            &[],
            Body::from(format!("{{\"hash\":\"{hex64}\"}}")),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    relay.set_enabled(true);

    // ② 请求体过大（> 4KB）
    let big = vec![b'z'; 5000];
    let resp = handle_auth_submit(
        relay.clone(),
        "node-a".to_string(),
        agt_req("POST", "/d/node-a/__auth", &[], Body::from(big)),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::OK, "auth JSON 回复恒 200，错误在 body");
    assert!(body.contains("请求体过大"), "{body}");

    // ③ 坏 JSON
    let resp = handle_auth_submit(
        relay.clone(),
        "node-a".to_string(),
        agt_req("POST", "/d/node-a/__auth", &[], Body::from("not-json")),
    )
    .await;
    let (_, body) = agt_resp(resp).await;
    assert!(body.contains("请求格式错误"), "{body}");

    // ④ 哈希格式错误（长度/字符不合法）
    for bad in ["short", &"g".repeat(64)] {
        let payload = serde_json::json!({ "hash": bad }).to_string();
        let resp = handle_auth_submit(
            relay.clone(),
            "node-a".to_string(),
            agt_req("POST", "/d/node-a/__auth", &[], Body::from(payload)),
        )
        .await;
        let (_, body) = agt_resp(resp).await;
        assert!(body.contains("令牌哈希格式错误"), "hash={bad} body={body}");
    }

    // ⑤ 合法哈希但设备不在表 → access_check Err「设备不在线」
    let payload = serde_json::json!({ "hash": hex64 }).to_string();
    let resp = handle_auth_submit(
        relay.clone(),
        "ghost-node".to_string(),
        agt_req("POST", "/d/ghost-node/__auth", &[], Body::from(payload)),
    )
    .await;
    let (_, body) = agt_resp(resp).await;
    assert!(body.contains("设备不在线"), "{body}");
}

#[tokio::test]
async fn agt_handle_auth_page_gate_and_redirect() {
    let relay = agt_relay("agt-token");

    // ① 门关 → 404
    relay.set_enabled(false);
    let resp = handle_auth_page(
        relay.clone(),
        "node-a".to_string(),
        agt_req("GET", "/d/node-a/__auth", &[], Body::empty()),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::NOT_FOUND);
    relay.set_enabled(true);

    // ② 无 cookie → 输入页（设备不在表 → 名字回退 node_id；登记设备 →
    //    显示名 HTML 转义）
    let _rx = agt_register_device(&relay, "node-a");
    let resp = handle_auth_page(
        relay.clone(),
        "node-a".to_string(),
        agt_req("GET", "/d/node-a/__auth", &[], Body::empty()),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::OK);
    assert!(
        body.contains("设备&lt;&amp;X&gt;"),
        "display name escaped: {body}"
    );
    assert!(body.contains("访问设备"));

    // ③ 已授权会话 → 面板重定向
    let cookie = relay.create_auth_session("node-a");
    let resp = handle_auth_page(
        relay.clone(),
        "node-a".to_string(),
        agt_req(
            "GET",
            "/d/node-a/__auth",
            &[(header::COOKIE.as_str(), &format!("{AUTH_COOKIE}={cookie}"))],
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(resp.status(), StatusCode::SEE_OTHER);
    let loc = resp
        .headers()
        .get(header::LOCATION)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    assert_eq!(loc, "/d/node-a/");
}

// -----------------------------------------------------------------------
// 隧道直调：in-process 设备（持下行 rx + route_conn_data 喂字节）
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_ws_tunnel_invalid_upgrade_request() {
    let relay = agt_relay("agt-token");
    let _rx = agt_register_device(&relay, "node-ws");
    let cookie = relay.create_auth_session("node-ws");
    // upgrade 形头（connection/upgrade 齐备 → is_websocket_upgrade true）
    // 但缺 sec-websocket-key → WebSocketUpgrade 提取失败 → 400
    let resp = handle_device_request(
        relay.clone(),
        "node-ws".to_string(),
        agt_req(
            "GET",
            "/d/node-ws/ws",
            &[
                (header::COOKIE.as_str(), &format!("{AUTH_COOKIE}={cookie}")),
                (header::CONNECTION.as_str(), "Upgrade"),
                (header::UPGRADE.as_str(), "websocket"),
            ],
            Body::empty(),
        ),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert!(body.contains("无效的 WS 升级请求"), "{body}");
}

#[tokio::test]
async fn agt_http_tunnel_device_eof_before_head() {
    let relay = agt_relay("agt-token");
    let mut rx = agt_register_device(&relay, "node-t");
    let cookie = relay.create_auth_session("node-t");

    let relay2 = relay.clone();
    let handle = tokio::spawn(async move {
        handle_device_request(
            relay2,
            "node-t".to_string(),
            agt_req(
                "GET",
                "/d/node-t/",
                &[(header::COOKIE.as_str(), &format!("{AUTH_COOKIE}={cookie}"))],
                Body::empty(),
            ),
        )
        .await
    });

    // 等 ConnOpen（设备视角下行）
    let conn_id = loop {
        let frame = tokio::time::timeout(Duration::from_secs(5), rx.recv())
            .await
            .expect("等 ConnOpen 不超时")
            .expect("下行通道存活");
        if let BridgeFrame::ConnOpen { conn_id, .. } = frame {
            break conn_id;
        }
    };
    // 等 conn 登记（register_conn 在 send 之后才执行），然后喂空载荷 = EOF
    for _ in 0..100 {
        if relay.route_conn_data(conn_id, Vec::new()) {
            break;
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
    let resp = tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("handler 不超时")
        .expect("handler 不 panic");
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(body.contains("未返回任何响应"), "{body}");
}

// -----------------------------------------------------------------------
// /bridge 真 socket e2e（同 http_tests harness 形态）
// -----------------------------------------------------------------------

#[allow(clippy::type_complexity)]
async fn agt_spawn_relay_server(
    token: &str,
) -> (
    std::net::SocketAddr,
    Arc<RelayServer>,
    tokio::sync::broadcast::Sender<()>,
) {
    let relay = Arc::new(RelayServer::new(token.to_string(), false));
    let config = crate::WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        version: "relay-agt-test".to_string(),
        ..Default::default()
    };
    let mut server = crate::WebServer::new(config);
    server.set_relay(relay.clone());
    let (shutdown_tx, shutdown_rx) = tokio::sync::broadcast::channel(1);
    let (bound_tx, bound_rx) = tokio::sync::oneshot::channel();
    tokio::spawn(async move {
        let _ = server
            .start_with_shutdown(shutdown_rx, Some(bound_tx))
            .await;
    });
    let addr = bound_rx.await.expect("server 应完成 bind");
    (addr, relay, shutdown_tx)
}

#[allow(clippy::too_many_arguments)]
async fn agt_connect_device(
    addr: std::net::SocketAddr,
    node_id: &str,
    expect_ok: bool,
) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
    let (mut ws, _) = connect_async(format!("ws://{addr}/bridge"))
        .await
        .expect("连接 /bridge");
    let hello = BridgeFrame::BridgeHello {
        token: "agt-token".to_string(),
        node_id: node_id.to_string(),
        name: format!("dev-{node_id}"),
        version: "agt".to_string(),
        cluster_node_id: None,
        cluster_name: None,
        role: None,
        category: None,
        tags: None,
        capabilities: None,
        node_type: None,
        rpc_port: None,
        addresses: None,
    };
    ws.send(TungMessage::Text(encode_frame(&hello).unwrap().into()))
        .await
        .expect("发 hello");
    let welcome = agt_recv_frame(&mut ws).await;
    match welcome {
        Some(BridgeFrame::BridgeWelcome { ok, .. }) => assert_eq!(ok, expect_ok, "welcome ok"),
        other => panic!("期望 welcome，得到 {other:?}"),
    }
    ws
}

/// 收一帧（跳过控制帧噪声）；流结束返回 None。
async fn agt_recv_frame(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Option<BridgeFrame> {
    loop {
        match ws.next().await {
            Some(Ok(TungMessage::Text(t))) => return decode_frame(&t).ok(),
            Some(Ok(_)) => continue,
            Some(Err(_)) | None => return None,
        }
    }
}

/// 收下一帧业务帧（跳过 MemberSync/心跳噪声），带总超时。
async fn agt_recv_business(
    ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
) -> Option<BridgeFrame> {
    for _ in 0..50 {
        let f = tokio::time::timeout(Duration::from_secs(2), agt_recv_frame(ws)).await;
        match f {
            Ok(Some(BridgeFrame::MemberSync { .. }))
            | Ok(Some(BridgeFrame::Heartbeat))
            | Ok(Some(BridgeFrame::Pong)) => continue,
            Ok(other) => return other,
            Err(_) => return None,
        }
    }
    None
}

async fn agt_send_frame(ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, frame: &BridgeFrame) {
    ws.send(TungMessage::Text(encode_frame(frame).unwrap().into()))
        .await
        .expect("设备发帧");
}

#[tokio::test]
async fn agt_bridge_handshake_silent_drop_closes() {
    let (_addr, relay, _shutdown) = agt_spawn_relay_server("agt-token").await;
    // 连上 /bridge 但不发 hello → 直接断开：服务端握手循环读到 None →
    // 关闭（不走鉴权，设备表保持空）
    let (ws, _) = connect_async(format!("ws://{_addr}/bridge"))
        .await
        .expect("连接 /bridge");
    drop(ws);
    for _ in 0..50 {
        if relay.list_devices().is_empty() {
            tokio::time::sleep(Duration::from_millis(50)).await;
            // 表从未有过条目；这里只等连接清理完成（无 panic 即可）
            break;
        }
    }
    assert!(relay.list_devices().is_empty());
}

#[tokio::test]
async fn agt_bridge_hello_cluster_identity_assembles_snapshot() {
    let (addr, relay, _shutdown) = agt_spawn_relay_server("agt-token").await;
    let (mut ws, _) = connect_async(format!("ws://{addr}/bridge"))
        .await
        .expect("连接 /bridge");
    // 集群身份字段齐备 → 组装 BridgeClusterIdentity 快照（None 臂之外的
    // 完整 Some 路径）
    let hello = BridgeFrame::BridgeHello {
        token: "agt-token".to_string(),
        node_id: "node-ci".to_string(),
        name: "身份设备".to_string(),
        version: "agt".to_string(),
        cluster_node_id: Some("cluster-1".to_string()),
        cluster_name: Some("边缘认证".to_string()),
        role: Some("worker".to_string()),
        category: Some("development".to_string()),
        tags: Some(vec!["rust".to_string()]),
        capabilities: Some(vec!["exec".to_string()]),
        node_type: Some("agent".to_string()),
        rpc_port: Some(22000),
        addresses: Some(vec!["10.0.0.9:22000".to_string()]),
    };
    ws.send(TungMessage::Text(encode_frame(&hello).unwrap().into()))
        .await
        .expect("发 hello");
    match agt_recv_frame(&mut ws).await {
        Some(BridgeFrame::BridgeWelcome {
            ok: true,
            hub_node_id,
            ..
        }) => {
            // welcome 携带 hub 身份（未设置 hub_node_id → 空串）
            assert_eq!(hub_node_id, "");
        }
        other => panic!("期望 ok welcome，得到 {other:?}"),
    }
    let devices = relay.list_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].node_id, "node-ci");
    assert_eq!(devices[0].name, "身份设备");
    assert!(devices[0].online);
}

#[tokio::test]
async fn agt_bridge_upstream_frame_ladder() {
    let (addr, relay, _shutdown) = agt_spawn_relay_server("agt-token").await;
    let mut ws = agt_connect_device(addr, "node-ladder", true).await;

    // ① 握手后重复 hello → 忽略（连接存活）
    agt_send_frame(
        &mut ws,
        &BridgeFrame::BridgeHello {
            token: "agt-token".to_string(),
            node_id: "node-ladder".to_string(),
            name: "dup".to_string(),
            version: "agt".to_string(),
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
    )
    .await;

    // ② 设备发 ConnOpen（协议错乱）→ 防御性忽略
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ConnOpen {
            conn_id: 1,
            target: "local".to_string(),
        },
    )
    .await;

    // ③ ConnData 坏 base64 → close_conn（无回执，连接仍存活）
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ConnData {
            conn_id: 2,
            seq: 0,
            data_b64: "!!not-base64!!".to_string(),
            fin: false,
        },
    )
    .await;

    // ④ 未登记 conn 的合法 ConnData → 设备收到 ConnClose{"conn not found"}
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ConnData {
            conn_id: 987_654,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(b"hi"),
            fin: false,
        },
    )
    .await;
    match agt_recv_business(&mut ws).await {
        Some(BridgeFrame::ConnClose { conn_id, reason }) => {
            assert_eq!(conn_id, 987_654);
            assert_eq!(reason, "conn not found");
        }
        other => panic!("期望 ConnClose，得到 {other:?}"),
    }

    // ⑤ ClusterRpc 无 sink（纯中继形态默认）→ WARN 忽略（连接存活）。
    //    帧按序处理：随后发一个有回执的未知 conn 标记帧，收到其 ConnClose
    //    即证明 id=1 帧已在 sink 装配前处理完毕（消除装配竞态）。
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ClusterRpc {
            payload: serde_json::json!({ "id": 1 }),
        },
    )
    .await;
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ConnData {
            conn_id: 987_655,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(b"marker"),
            fin: false,
        },
    )
    .await;
    match agt_recv_business(&mut ws).await {
        Some(BridgeFrame::ConnClose { conn_id, .. }) => assert_eq!(conn_id, 987_655),
        other => panic!("期望标记 ConnClose，得到 {other:?}"),
    }

    // ⑥ 装 mock sink → ClusterRpc 回声
    #[derive(Clone, Default)]
    struct AgtSink {
        hits: Arc<std::sync::atomic::AtomicUsize>,
    }
    impl ClusterFrameSink for AgtSink {
        fn on_cluster_frame(
            &self,
            from_device: &str,
            payload: serde_json::Value,
        ) -> std::pin::Pin<
            Box<dyn std::future::Future<Output = Option<serde_json::Value>> + Send + '_>,
        > {
            let hits = self.hits.clone();
            let id = payload
                .get("id")
                .cloned()
                .unwrap_or(serde_json::Value::Null);
            let dev = from_device.to_string();
            Box::pin(async move {
                hits.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                assert_eq!(dev, "node-ladder");
                Some(serde_json::json!({ "echo": id, "from": dev }))
            })
        }
    }
    let sink = AgtSink::default();
    relay.set_cluster_frame_sink(Arc::new(sink.clone()));
    agt_send_frame(
        &mut ws,
        &BridgeFrame::ClusterRpc {
            payload: serde_json::json!({ "id": 42 }),
        },
    )
    .await;
    match agt_recv_business(&mut ws).await {
        Some(BridgeFrame::ClusterRpc { payload }) => {
            assert_eq!(payload["echo"], 42);
            assert_eq!(payload["from"], "node-ladder");
        }
        other => panic!("期望 ClusterRpc 回声，得到 {other:?}"),
    }
    assert_eq!(sink.hits.load(std::sync::atomic::Ordering::SeqCst), 1);

    // ⑦ Binary 帧 → 底层处理忽略（连接存活）
    ws.send(TungMessage::Binary(vec![1, 2, 3].into()))
        .await
        .expect("发 Binary");

    // ⑧ BridgeClose → 设备主动优雅退出：socket 关闭 + 设备表清理
    agt_send_frame(
        &mut ws,
        &BridgeFrame::BridgeClose {
            reason: "bye".to_string(),
        },
    )
    .await;
    let mut closed = false;
    for _ in 0..50 {
        if agt_recv_frame(&mut ws).await.is_none() {
            closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(closed, "BridgeClose 后服务端应关 socket");
    assert!(
        relay
            .list_devices()
            .iter()
            .all(|d| d.node_id != "node-ladder"),
        "设备表应清理"
    );
}

#[tokio::test]
async fn agt_bridge_reconnect_replaces_old_connection() {
    let (addr, relay, _shutdown) = agt_spawn_relay_server("agt-token").await;
    let mut old = agt_connect_device(addr, "node-re", true).await;
    let mut new = agt_connect_device(addr, "node-re", true).await;

    // 新连接存活：设备表里恰好一条，online
    let devices = relay.list_devices();
    assert_eq!(devices.len(), 1, "顶替后仍一条表项");
    assert!(devices[0].online);

    // 旧 socket 终将被关闭（写循环 rx None / 代际失效双路任一）
    let mut old_closed = false;
    for _ in 0..100 {
        if agt_recv_frame(&mut old).await.is_none() {
            old_closed = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
    assert!(old_closed, "被顶替的旧连接应关闭");
    let _ = new
        .send(TungMessage::Text(
            encode_frame(&BridgeFrame::BridgeClose {
                reason: "cleanup".to_string(),
            })
            .unwrap()
            .into(),
        ))
        .await;
}

// -----------------------------------------------------------------------
// Wave5 批次：handle_ws_tunnel 的「设备下行通道已死」503 臂——设备仍在表
// （device_online = contains_key）但下行 rx 已 drop → send_to_device
// Closed → false。升级头齐备但走不到 WebSocketUpgrade 提取（发送检查在前），
// 故无需合法 sec-websocket-key。
// -----------------------------------------------------------------------

#[tokio::test]
async fn agt_ws_tunnel_offline_device_returns_503() {
    let relay = agt_relay("agt-token");
    let rx = agt_register_device(&relay, "node-dead");
    let cookie = relay.create_auth_session("node-dead");
    drop(rx); // 表项仍在（online 快路径为 contains_key），但下行通道已闭

    let resp = handle_device_request(
        relay.clone(),
        "node-dead".to_string(),
        agt_req(
            "GET",
            "/d/node-dead/ws",
            &[
                (header::COOKIE.as_str(), &format!("{AUTH_COOKIE}={cookie}")),
                (header::CONNECTION.as_str(), "Upgrade"),
                (header::UPGRADE.as_str(), "websocket"),
            ],
            Body::empty(),
        ),
    )
    .await;
    let (status, body) = agt_resp(resp).await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert!(body.contains("请求无法送达设备"), "{body}");
}
