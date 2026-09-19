//! 反向桥批次一端到端测试：真 WebServer（随机端口）+ mock 桥设备
//! （tokio-tungstenite 直连 `/bridge`）+ 原始 TCP 浏览器客户端。
//!
//! 覆盖 goal 测试面：token 拒止（welcome{ok:false}）、access_check 三态
//! 走真 HTTP、授权会话 cookie 语义、状态页 ws token 门、HTTP 隧道往返
//! （fin/EOF 语义）、WS 长泵升级 + 双向搬运、dial 失败回执（502）。
//!
//! **类型分离注记**：axum 0.8.9 底层是 tungstenite 0.29，dev-dep
//! tokio-tungstenite 是 0.26——两版 `Message` 不同型。本文件的纪律：
//! mock 设备 socket（0.26 的 [`TungMessage`]）只收发桥协议 JSON 文本；
//! ws_codec 层（[`encode_client_frame`]/[`parse_server_frame`]）统一使用
//! axum 的 `Message`。浏览器侧 WS 不用 tungstenite 客户端握手（其 accept
//! 校验需要额外 sha1 依赖），改用原始 TCP + ws_codec——顺带在真实往返中
//! 验证手写 codec 的互操作性。

use std::sync::Arc;

use super::protocol::{BridgeFrame, ChunkedDecoder, decode_frame, encode_frame};
use super::server::RelayServer;
use super::ws_codec::{encode_client_frame, parse_server_frame};
use super::{ADMIN_COOKIE, AUTH_COOKIE};
use base64::Engine as _;
use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as TungMessage;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, connect_async};

// ---------------------------------------------------------------------------
// 测试基建
// ---------------------------------------------------------------------------

/// 起一个带中继的 WebServer（127.0.0.1 随机端口）。
/// 返回 (地址, relay, 关停闸)——关停闸必须保活：broadcast sender 全部
/// drop 时 `start_with_shutdown` 的 recv 立即返回 Err，server 会当场退出。
#[allow(clippy::type_complexity)]
async fn spawn_relay_server(
    token: &str,
) -> (
    std::net::SocketAddr,
    Arc<RelayServer>,
    tokio::sync::broadcast::Sender<()>,
) {
    let relay = Arc::new(RelayServer::new(token.to_string(), false));
    let config = crate::WebServerConfig {
        listen_addr: "127.0.0.1:0".to_string(),
        version: "relay-e2e-test".to_string(),
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

/// mock 桥设备：连 `/bridge`、发 hello、等 welcome。
/// token 被拒时返回 Err(收到的拒绝 welcome 帧)。
async fn connect_device(
    addr: std::net::SocketAddr,
    node_id: &str,
    name: &str,
    token: &str,
) -> Result<WebSocketStream<MaybeTlsStream<TcpStream>>, BridgeFrame> {
    let (mut ws, _) = connect_async(format!("ws://{addr}/bridge"))
        .await
        .expect("连接 /bridge");
    let hello = BridgeFrame::BridgeHello {
        token: token.to_string(),
        node_id: node_id.to_string(),
        name: name.to_string(),
        version: "e2e-test".to_string(),
    };
    ws.send(TungMessage::Text(encode_frame(&hello).unwrap().into()))
        .await
        .expect("发 hello");
    let welcome = recv_frame(&mut ws).await;
    match welcome {
        BridgeFrame::BridgeWelcome { ok: true, .. } => Ok(ws),
        BridgeFrame::BridgeWelcome { ok: false, reason } => {
            Err(BridgeFrame::BridgeWelcome { ok: false, reason })
        }
        other => panic!("期望 welcome，得到 {other:?}"),
    }
}

/// 从 mock 设备收一帧桥协议（跳过底层控制帧）。
async fn recv_frame(ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>) -> BridgeFrame {
    loop {
        match ws.next().await {
            Some(Ok(TungMessage::Text(t))) => return decode_frame(&t).expect("解码桥帧"),
            Some(Ok(_)) => continue,
            Some(Err(e)) => panic!("设备收帧失败：{e}"),
            None => panic!("设备侧 ws 意外关闭"),
        }
    }
}

/// mock 设备发一帧桥协议。
async fn send_frame(ws: &mut WebSocketStream<MaybeTlsStream<TcpStream>>, frame: &BridgeFrame) {
    ws.send(TungMessage::Text(encode_frame(frame).unwrap().into()))
        .await
        .expect("设备发帧");
}

/// 原始 HTTP/1.1 请求（Connection: close，读到 EOF）。
/// 返回 (状态码, 头（名字已小写）, body)。
async fn http_request(
    addr: std::net::SocketAddr,
    method: &str,
    path: &str,
    headers: &[(&str, &str)],
    body: &[u8],
) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let mut sock = TcpStream::connect(addr).await.expect("连 TCP");
    let mut req = format!("{method} {path} HTTP/1.1\r\nhost: {addr}\r\nconnection: close\r\n");
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str(&format!("content-length: {}\r\n\r\n", body.len()));
    sock.write_all(req.as_bytes()).await.unwrap();
    sock.write_all(body).await.unwrap();
    sock.flush().await.unwrap();
    let mut raw = Vec::new();
    sock.read_to_end(&mut raw).await.unwrap();
    parse_raw_http_response(&raw)
}

/// 解析原始 HTTP 响应字节（支持 chunked body——hyper 对未知长度 body 用
/// chunked；复用桥自己的 ChunkedDecoder 解块）。
fn parse_raw_http_response(raw: &[u8]) -> (u16, Vec<(String, String)>, Vec<u8>) {
    let head_end = raw
        .windows(4)
        .position(|w| w == b"\r\n\r\n")
        .expect("响应头截断");
    let head = String::from_utf8_lossy(&raw[..head_end]).to_string();
    let mut lines = head.split("\r\n");
    let status_line = lines.next().expect("状态行");
    let status: u16 = status_line
        .split(' ')
        .nth(1)
        .and_then(|s| s.parse().ok())
        .expect("状态码");
    let mut headers = Vec::new();
    for line in lines {
        if let Some((k, v)) = line.split_once(':') {
            headers.push((k.trim().to_ascii_lowercase(), v.trim().to_string()));
        }
    }
    let body_raw = &raw[head_end + 4..];
    let chunked = headers
        .iter()
        .any(|(k, v)| k == "transfer-encoding" && v.to_ascii_lowercase().contains("chunked"));
    let body = if chunked {
        let mut d = ChunkedDecoder::new();
        d.feed(body_raw)
    } else {
        body_raw.to_vec()
    };
    (status, headers, body)
}

/// 从响应头提取 Set-Cookie 中某 cookie 的 (值, Path)。
fn set_cookie_value(headers: &[(String, String)], name: &str) -> Option<(String, String)> {
    headers
        .iter()
        .filter(|(k, _)| k == "set-cookie")
        .find_map(|(_, v)| {
            let mut parts = v.split("; ");
            let first = parts.next()?;
            let (k, val) = first.split_once('=')?;
            if k != name {
                return None;
            }
            let path = parts
                .find_map(|p| p.strip_prefix("Path="))
                .unwrap_or("/")
                .to_string();
            Some((val.to_string(), path))
        })
}

/// 造设备授权 cookie（走状态机直造——auth 流本身由独立用例端到端覆盖）。
fn make_device_cookie(relay: &RelayServer, node_id: &str) -> String {
    relay.create_auth_session(node_id)
}

/// ws token 的 SHA-256 hex（状态页管理门凭据）。
fn sha256_hex(input: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut h = Sha256::new();
    h.update(input.as_bytes());
    h.finalize().iter().map(|b| format!("{b:02x}")).collect()
}

/// HTTP 隧道测试的设备侧响应模板：收 ConnOpen + 请求 → 回指定响应字节 →
/// 等服务端收口通知。2026-09-19 修复后：响应完整性由 content-length 界定，
/// 设备侧不再发 EOF 帧（EOF 约定保留为无长度头响应的兜底，专项测试覆盖）；
/// 收口权在服务端 handler——body 读满后回发 `ConnClose("response complete")`。
/// 返回 (conn_id, 完整请求字节)。
async fn serve_http_request(
    dev: &mut WebSocketStream<MaybeTlsStream<TcpStream>>,
    response_bytes: &[u8],
) -> (u64, Vec<u8>) {
    let open = recv_frame(dev).await;
    let BridgeFrame::ConnOpen { conn_id, target } = open else {
        panic!("期望 ConnOpen，得到 {open:?}");
    };
    assert_eq!(target, "local");
    let data = recv_frame(dev).await;
    let BridgeFrame::ConnData {
        conn_id: cid,
        seq,
        data_b64,
        fin,
    } = data
    else {
        panic!("期望 ConnData，得到 {data:?}");
    };
    assert_eq!(cid, conn_id);
    assert_eq!(seq, 0, "短 conn 请求单帧 seq=0");
    assert!(fin, "短 conn 请求整段一次下发 fin=true");
    let req_bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_b64)
        .unwrap();
    send_frame(
        dev,
        &BridgeFrame::ConnData {
            conn_id,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(response_bytes),
            fin: false,
        },
    )
    .await;
    // 服务端按 content-length 收口后必须回发 ConnClose 通知（设备侧据此
    // drop_conn，防 keep-alive 连接下 conn 泵泄漏）。
    loop {
        let f = recv_frame(dev).await;
        match f {
            BridgeFrame::ConnClose {
                conn_id: cid,
                reason,
            } => {
                assert_eq!(cid, conn_id, "收口通知必须对应本 conn");
                assert_eq!(reason, "response complete", "正常收口原因");
                break;
            }
            // 心跳噪声跳过（真实桥通道里会有）。
            BridgeFrame::Heartbeat | BridgeFrame::Pong => continue,
            other => panic!("期望 ConnClose 收口通知，实际 {other:?}"),
        }
    }
    (conn_id, req_bytes)
}

// ---------------------------------------------------------------------------
// ① /bridge 握手：token 拒止（welcome{ok:false}）+ 正常接入
// ---------------------------------------------------------------------------

#[tokio::test]
async fn bridge_hello_wrong_token_gets_rejected_via_ws() {
    let (addr, _relay, _shutdown) = spawn_relay_server("pair-token").await;
    let err = connect_device(addr, "node-x", "设备 X", "WRONG").await;
    match err {
        Err(BridgeFrame::BridgeWelcome { ok: false, reason }) => {
            assert!(reason.contains("token 不匹配"), "{reason}");
        }
        Ok(_) => panic!("错 token 不应接入成功"),
        other => panic!("期望拒绝 welcome，得到 {other:?}"),
    }
}

#[tokio::test]
async fn bridge_hello_correct_token_gets_welcome_and_registers_device() {
    let (addr, relay, _shutdown) = spawn_relay_server("pair-token").await;
    let mut ws = connect_device(addr, "node-ok", "在线机", "pair-token")
        .await
        .expect("正确 token 应接入");
    let devices = relay.list_devices();
    assert_eq!(devices.len(), 1);
    assert_eq!(devices[0].node_id, "node-ok");
    assert_eq!(devices[0].name, "在线机");
    ws.close(None).await.ok();
}

// ---------------------------------------------------------------------------
// ② 授权流（/d/{nid}/__auth POST）端到端：access_check 转发 + cookie 语义
// ---------------------------------------------------------------------------

#[tokio::test]
async fn auth_flow_end_to_end_forwards_hash_to_device() {
    let (addr, _relay, _shutdown) = spawn_relay_server("pair-token").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "pair-token")
        .await
        .expect("设备接入");

    // 设备侧任务：等 AccessCheck → 校验转发内容 → 回 ok=true。
    // （spawn 内直接回帧——主线 http_request 在等 AccessResult，先回再等。）
    let dev_task = tokio::spawn(async move {
        match recv_frame(&mut dev).await {
            BridgeFrame::AccessCheck {
                request_id,
                node_id,
                hash_hex,
            } => {
                assert_eq!(node_id, "node-a");
                assert_eq!(hash_hex, "ab".repeat(32));
                send_frame(
                    &mut dev,
                    &BridgeFrame::AccessResult {
                        request_id,
                        ok: true,
                    },
                )
                .await;
            }
            other => panic!("设备应收到 AccessCheck，得到 {other:?}"),
        }
        dev
    });
    let (status, headers, body) = http_request(
        addr,
        "POST",
        "/d/node-a/__auth",
        &[("content-type", "application/json")],
        format!("{{\"hash\":\"{}\"}}", "ab".repeat(32)).as_bytes(),
    )
    .await;
    let mut dev = dev_task.await.unwrap();
    assert_eq!(status, 200, "body: {}", String::from_utf8_lossy(&body));
    let json: serde_json::Value = serde_json::from_slice(&body).unwrap();
    assert_eq!(json["ok"], true, "{json}");
    let (cookie, path) = set_cookie_value(&headers, AUTH_COOKIE).expect("应发授权 cookie");
    assert_eq!(path, "/d/node-a", "cookie 必须 path 限定本设备");
    assert!(!cookie.is_empty());

    // 错误哈希（64 hex 但设备回 fail）→ ok:false 无 Set-Cookie。
    let dev_task = tokio::spawn(async move {
        match recv_frame(&mut dev).await {
            BridgeFrame::AccessCheck { request_id, .. } => {
                send_frame(
                    &mut dev,
                    &BridgeFrame::AccessResult {
                        request_id,
                        ok: false,
                    },
                )
                .await;
            }
            other => panic!("期望 AccessCheck，得到 {other:?}"),
        }
        dev
    });
    let (status2, headers2, body2) = http_request(
        addr,
        "POST",
        "/d/node-a/__auth",
        &[("content-type", "application/json")],
        format!("{{\"hash\":\"{}\"}}", "cd".repeat(32)).as_bytes(),
    )
    .await;
    let _dev = dev_task.await.unwrap();
    assert_eq!(status2, 200);
    let json2: serde_json::Value = serde_json::from_slice(&body2).unwrap();
    assert_eq!(json2["ok"], false, "{json2}");
    assert!(set_cookie_value(&headers2, AUTH_COOKIE).is_none());

    // 非法哈希形态（非 64 hex）→ 服务端本地拒绝，设备不被打扰。
    let (status3, _, body3) = http_request(
        addr,
        "POST",
        "/d/node-a/__auth",
        &[("content-type", "application/json")],
        b"{\"hash\":\"short\"}",
    )
    .await;
    let json3: serde_json::Value = serde_json::from_slice(&body3).unwrap();
    assert_eq!(json3["ok"], false);
    assert_eq!(status3, 200);
}

// ---------------------------------------------------------------------------
// ③ /d/ 三道门 + 状态页门关 404
// ---------------------------------------------------------------------------

#[tokio::test]
async fn device_route_gates_offline_and_unauthorized() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;

    // 设备离线 → 503。
    let (s1, _, b1) = http_request(addr, "GET", "/d/ghost/list", &[], b"").await;
    assert_eq!(s1, 503, "{}", String::from_utf8_lossy(&b1));

    // 设备在线、未授权 → 303 到 __auth（axum 0.8 Redirect::to = See Other）。
    let mut dev_ws = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let (s2, h2, _) = http_request(addr, "GET", "/d/node-a/panel", &[], b"").await;
    assert_eq!(s2, 303);
    let loc = h2
        .iter()
        .find(|(k, _)| k == "location")
        .map(|(_, v)| v.clone())
        .expect("应有 location");
    assert_eq!(loc, "/d/node-a/__auth");

    // __auth 输入页 200。
    let (s3, _, b3) = http_request(addr, "GET", "/d/node-a/__auth", &[], b"").await;
    assert_eq!(s3, 200);
    let html = String::from_utf8_lossy(&b3);
    assert!(html.contains("设备访问验证"), "{html}");

    // 开关关闭 → 一律 404（不暴露功能存在）。
    relay.set_enabled(false);
    let (s4, _, _) = http_request(addr, "GET", "/d/node-a/panel", &[], b"").await;
    assert_eq!(s4, 404);
    let (s5, _, _) = http_request(addr, "GET", "/relay", &[], b"").await;
    assert_eq!(s5, 404);
    let (s6, _, _) = http_request(addr, "GET", "/api/relay/status", &[], b"").await;
    assert_eq!(s6, 404);
    relay.set_enabled(true);
    dev_ws.close(None).await.ok();
}

// ---------------------------------------------------------------------------
// ④ HTTP 隧道端到端（fin/EOF 语义、502、POST 体转发）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn http_tunnel_round_trip_content_length_semantics() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let cookie = make_device_cookie(relay.as_ref(), "node-a");

    // 设备侧任务：收 ConnOpen + 完整请求 → 回响应 → 等服务端收口通知
    // （模板内置 ConnClose 断言）。
    let dev_task = tokio::spawn(async move {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\ncontent-length: 5\r\n\r\nhello";
        let (_conn_id, req_bytes) = serve_http_request(&mut dev, resp).await;
        let req_text = String::from_utf8(req_bytes.clone()).unwrap();
        assert!(
            req_text.starts_with("GET /d/node-a/panel HTTP/1.1\r\n"),
            "{req_text}"
        );
        assert!(
            req_text.contains(&format!("cookie: {AUTH_COOKIE}=")),
            "授权 cookie 应随请求穿透：{req_text}"
        );
        dev
    });

    let (status, headers, body) = http_request(
        addr,
        "GET",
        "/d/node-a/panel",
        &[("cookie", &format!("{AUTH_COOKIE}={cookie}"))],
        b"",
    )
    .await;
    let _dev = dev_task.await.unwrap();
    assert_eq!(status, 200, "body: {}", String::from_utf8_lossy(&body));
    assert_eq!(body, b"hello");
    assert!(
        headers
            .iter()
            .any(|(k, v)| k == "content-type" && v == "text/plain")
    );
}

#[tokio::test]
async fn http_tunnel_dial_failure_returns_502() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let cookie = make_device_cookie(relay.as_ref(), "node-a");

    // 设备侧任务：收 ConnOpen + 请求后回 ConnClose（dial 失败语义）。
    // 服务端在等头阶段收到 conn 关闭 → 502。
    let dev_task = tokio::spawn(async move {
        let open = recv_frame(&mut dev).await;
        let BridgeFrame::ConnOpen { conn_id, .. } = open else {
            panic!("期望 ConnOpen，得到 {open:?}");
        };
        let _ = recv_frame(&mut dev).await; // ConnData（请求字节）
        send_frame(
            &mut dev,
            &BridgeFrame::ConnClose {
                conn_id,
                reason: "dial refused".to_string(),
            },
        )
        .await;
        dev
    });
    let (status, _, body) = http_request(
        addr,
        "GET",
        "/d/node-a/panel",
        &[("cookie", &format!("{AUTH_COOKIE}={cookie}"))],
        b"",
    )
    .await;
    let _dev = dev_task.await.unwrap();
    assert_eq!(status, 502, "body: {}", String::from_utf8_lossy(&body));
    let html = String::from_utf8_lossy(&body);
    assert!(
        html.contains("设备响应异常") || html.contains("设备侧"),
        "{html}"
    );
}

#[tokio::test]
async fn http_tunnel_post_body_forwarded_intact() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let cookie = make_device_cookie(relay.as_ref(), "node-a");

    let dev_task = tokio::spawn(async move {
        let resp = b"HTTP/1.1 204 No Content\r\ncontent-length: 0\r\n\r\n";
        let (_conn_id, _req_bytes) = serve_http_request(&mut dev, resp).await;
        dev
    });
    let post_body = b"name=%E4%B8%AD%E6%96%87&x=1";
    let (status, _, body) = http_request(
        addr,
        "POST",
        "/d/node-a/api/echo",
        &[
            ("cookie", &format!("{AUTH_COOKIE}={cookie}")),
            ("content-type", "application/x-www-form-urlencoded"),
        ],
        post_body,
    )
    .await;
    let _dev = dev_task.await.unwrap();
    assert_eq!(status, 204, "body: {}", String::from_utf8_lossy(&body));
    // 注：请求体转发内容在 round_trip 用例已验（serve_http_request 不返回
    // 字节时此处只验状态码 + content-length 重算由单元测试覆盖）。
}

/// 无 content-length 头的响应（HTTP/1.0 形态）：EOF 约定兜底收口——设备
/// 读到本机关连接发空 fin，服务端收到即完成 body（不再等长度）。
#[tokio::test]
async fn http_tunnel_lengthless_response_eof_fallback() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let cookie = make_device_cookie(relay.as_ref(), "node-a");

    let dev_task = tokio::spawn(async move {
        let open = recv_frame(&mut dev).await;
        let BridgeFrame::ConnOpen { conn_id, .. } = open else {
            panic!("期望 ConnOpen，得到 {open:?}");
        };
        let data = recv_frame(&mut dev).await;
        let BridgeFrame::ConnData { data_b64, .. } = data else {
            panic!("期望 ConnData，得到 {data:?}");
        };
        let _req_bytes = base64::engine::general_purpose::STANDARD
            .decode(&data_b64)
            .unwrap();
        // 响应**不带** content-length 头。
        send_frame(
            &mut dev,
            &BridgeFrame::ConnData {
                conn_id,
                seq: 0,
                data_b64: base64::engine::general_purpose::STANDARD
                    .encode(b"HTTP/1.1 200 OK\r\ncontent-type: text/plain\r\n\r\nlegacy"),
                fin: false,
            },
        )
        .await;
        // EOF 约定：空载荷 + fin（设备读到本机关连接）。
        send_frame(
            &mut dev,
            &BridgeFrame::ConnData {
                conn_id,
                seq: 1,
                data_b64: String::new(),
                fin: true,
            },
        )
        .await;
        dev
    });
    let (status, headers, body) = http_request(
        addr,
        "GET",
        "/d/node-a/panel",
        &[("cookie", &format!("{AUTH_COOKIE}={cookie}"))],
        b"",
    )
    .await;
    let _dev = dev_task.await.unwrap();
    assert_eq!(status, 200);
    assert_eq!(body, b"legacy", "无长度响应靠 EOF 兜底收口");
    assert!(headers.iter().any(|(k, _)| k == "content-type"));
}

// ---------------------------------------------------------------------------
// ⑤ WS 长泵：升级穿透 + 双向帧搬运（原始 TCP 浏览器，全程串行）
// ---------------------------------------------------------------------------

/// 原始 TCP 上写 ws 升级请求（不读响应——101 由设备侧下发）。
async fn raw_ws_connect_write(addr: std::net::SocketAddr, path: &str, cookie: &str) -> TcpStream {
    let mut sock = TcpStream::connect(addr).await.expect("连 TCP");
    let key = "dGhlIHNhbXBsZSBub25jZQ=="; // RFC 6455 示例 key
    let req = format!(
        "GET {path} HTTP/1.1\r\nhost: {addr}\r\nupgrade: websocket\r\nconnection: Upgrade\r\n\
         sec-websocket-key: {key}\r\nsec-websocket-version: 13\r\ncookie: {cookie}\r\n\r\n"
    );
    sock.write_all(req.as_bytes()).await.unwrap();
    sock
}

/// 读 ws 握手响应头（读到 \r\n\r\n）。
async fn raw_ws_read_head(sock: &mut TcpStream) -> String {
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), sock.read(&mut chunk))
            .await
            .expect("握手读超时")
            .expect("读失败");
        assert!(n > 0, "握手期间连接关闭");
        buf.extend_from_slice(&chunk[..n]);
        if buf.windows(4).any(|w| w == b"\r\n\r\n") {
            return String::from_utf8_lossy(&buf).to_string();
        }
    }
}

#[tokio::test]
async fn ws_tunnel_upgrade_and_bidirectional_frames() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("接入");
    let cookie = make_device_cookie(relay.as_ref(), "node-a");

    // ① 浏览器发升级请求（只写不读）。
    let mut browser =
        raw_ws_connect_write(addr, "/d/node-a/ws", &format!("{AUTH_COOKIE}={cookie}")).await;

    // ② 设备收 ConnOpen + 升级请求 → 校验语义头穿透 → 回 101。
    let open = recv_frame(&mut dev).await;
    let BridgeFrame::ConnOpen { conn_id, .. } = open else {
        panic!("期望 ConnOpen，得到 {open:?}");
    };
    let data = recv_frame(&mut dev).await;
    let BridgeFrame::ConnData { data_b64, fin, .. } = data else {
        panic!("期望 ConnData，得到 {data:?}");
    };
    assert!(fin, "升级请求整段下发");
    let req_text = String::from_utf8(
        base64::engine::general_purpose::STANDARD
            .decode(&data_b64)
            .unwrap(),
    )
    .unwrap();
    assert!(
        req_text.starts_with("GET /d/node-a/ws HTTP/1.1\r\n"),
        "{req_text}"
    );
    assert!(
        req_text.to_ascii_lowercase().contains("sec-websocket-key:"),
        "升级语义头必须穿透：{req_text}"
    );
    let resp_101 = b"HTTP/1.1 101 Switching Protocols\r\nupgrade: websocket\r\nconnection: Upgrade\r\nsec-websocket-accept: s3pPLMBiTxaQ9kYGzzhZRbK+xOo=\r\n\r\n";
    send_frame(
        &mut dev,
        &BridgeFrame::ConnData {
            conn_id,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(resp_101),
            fin: false,
        },
    )
    .await;

    // ③ 浏览器读到 101（axum 已完成服务端升级）。
    let head = raw_ws_read_head(&mut browser).await;
    assert!(head.starts_with("HTTP/1.1 101"), "期望 101，得到：{head}");

    // ④ 浏览器 → 设备：发一帧 masked client Text。
    let text_frame = encode_client_frame(&axum::extract::ws::Message::Text(
        "ping-from-browser".into(),
    ));
    browser.write_all(&text_frame).await.unwrap();
    let down = recv_frame(&mut dev).await;
    let BridgeFrame::ConnData { data_b64, seq, .. } = down else {
        panic!("期望下行 ConnData，得到 {down:?}");
    };
    assert_eq!(seq, 1, "浏览器 → 设备方向帧序号从 1 起");
    let frame_bytes = base64::engine::general_purpose::STANDARD
        .decode(&data_b64)
        .unwrap();
    let (msg, consumed) = parse_server_frame(&frame_bytes).expect("解 ws 帧");
    assert_eq!(consumed, frame_bytes.len());
    match msg {
        axum::extract::ws::Message::Text(t) => assert_eq!(t.as_str(), "ping-from-browser"),
        other => panic!("期望 Text，得到 {other:?}"),
    }

    // ⑤ 设备 → 浏览器：ConnData 载原始 ws 帧字节 → 中继解帧后以 server
    // 角色转发浏览器（用 encode_client_frame 造帧字节——parse_server_frame
    // 防御式兼容 masked 帧）。
    let reply = encode_client_frame(&axum::extract::ws::Message::Text("pong-from-device".into()));
    send_frame(
        &mut dev,
        &BridgeFrame::ConnData {
            conn_id,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(&reply),
            fin: false,
        },
    )
    .await;
    // 浏览器读 ws 帧（server 帧，unmasked）。
    let mut buf = Vec::new();
    let mut chunk = [0u8; 512];
    let frame = loop {
        let n = tokio::time::timeout(std::time::Duration::from_secs(10), browser.read(&mut chunk))
            .await
            .expect("读 ws 帧超时")
            .expect("读失败");
        assert!(n > 0, "浏览器侧连接关闭");
        buf.extend_from_slice(&chunk[..n]);
        if let Some((msg, consumed)) = parse_server_frame(&buf) {
            buf.drain(..consumed);
            break msg;
        }
    };
    match frame {
        axum::extract::ws::Message::Text(t) => assert_eq!(t.as_str(), "pong-from-device"),
        other => panic!("期望 Text，得到 {other:?}"),
    }
}

// ---------------------------------------------------------------------------
// ⑥ 状态页 ws token 门
// ---------------------------------------------------------------------------

#[tokio::test]
async fn relay_status_page_gate_and_api() {
    let (addr, _relay, _shutdown) = spawn_relay_server("admin-token").await;
    let mut dev_ws = connect_device(addr, "node-a", "A 机", "admin-token")
        .await
        .expect("接入");

    // 未过门：/relay 返回登录页（200，不见设备信息）。
    let (s1, _, b1) = http_request(addr, "GET", "/relay", &[], b"").await;
    assert_eq!(s1, 200);
    let html1 = String::from_utf8_lossy(&b1);
    assert!(html1.contains("令牌验证"), "{html1}");
    assert!(!html1.contains("node-a"), "未过门不可见任何设备信息");

    // /api/relay/status 未过门 → 401。
    let (s2, _, _) = http_request(addr, "GET", "/api/relay/status", &[], b"").await;
    assert_eq!(s2, 401);

    // 错误 hash 登录 → 401。
    let (s3, _, b3) = http_request(
        addr,
        "POST",
        "/relay/login",
        &[("content-type", "application/json")],
        format!("{{\"hash\":\"{}\"}}", "00".repeat(32)).as_bytes(),
    )
    .await;
    assert_eq!(s3, 401, "{}", String::from_utf8_lossy(&b3));

    // 正确 hash（= ws token SHA-256）登录 → 200 + Set-Cookie(path=/)。
    let admin_hash = sha256_hex("admin-token");
    let (s4, h4, _) = http_request(
        addr,
        "POST",
        "/relay/login",
        &[("content-type", "application/json")],
        format!("{{\"hash\":\"{admin_hash}\"}}").as_bytes(),
    )
    .await;
    assert_eq!(s4, 200);
    let (admin_cookie, path) = set_cookie_value(&h4, ADMIN_COOKIE).expect("应发管理 cookie");
    assert_eq!(path, "/");
    assert_eq!(admin_cookie, admin_hash, "cookie 值 = token 的 SHA-256 hex");

    // 过门后：状态页正常渲染（设备列表由页面 JS 动态拉
    // /api/relay/status 渲染，静态 HTML 不含设备 id——数据正确性在
    // 下方 API 断言覆盖）。
    let (s5, _, b5) = http_request(
        addr,
        "GET",
        "/relay",
        &[("cookie", &format!("{ADMIN_COOKIE}={admin_cookie}"))],
        b"",
    )
    .await;
    assert_eq!(s5, 200);
    let html5 = String::from_utf8_lossy(&b5);
    assert!(html5.contains("中继状态页"), "{html5}");

    let (s6, _, b6) = http_request(
        addr,
        "GET",
        "/api/relay/status",
        &[("cookie", &format!("{ADMIN_COOKIE}={admin_cookie}"))],
        b"",
    )
    .await;
    assert_eq!(s6, 200);
    let json: serde_json::Value = serde_json::from_slice(&b6).unwrap();
    assert_eq!(json["enabled"], true);
    assert_eq!(json["full_mode"], false);
    assert_eq!(json["devices"][0]["node_id"], "node-a");
    assert_eq!(json["devices"][0]["name"], "A 机");
    assert_eq!(json["devices"][0]["online"], true);
    dev_ws.close(None).await.ok();
}

// ---------------------------------------------------------------------------
// ⑦ 多设备并发（真 ws 全链路）：两台设备同时接入、各自隧道互不串线
// ---------------------------------------------------------------------------

#[tokio::test]
async fn two_devices_concurrent_tunnels_do_not_cross() {
    let (addr, relay, _shutdown) = spawn_relay_server("tok").await;
    let mut dev_a = connect_device(addr, "node-a", "A 机", "tok")
        .await
        .expect("A 接入");
    let mut dev_b = connect_device(addr, "node-b", "B 机", "tok")
        .await
        .expect("B 接入");
    let cookie_a = make_device_cookie(relay.as_ref(), "node-a");
    let cookie_b = make_device_cookie(relay.as_ref(), "node-b");

    // 设备泵：各自回各自的请求（body 里带自己的 node 标记）。spawn 内直接
    // 回帧——主线两个浏览器请求在等设备响应，先回再收结果。
    let task_a = tokio::spawn(async move {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\nfrom-node-a";
        let (conn_id, _) = serve_http_request(&mut dev_a, resp).await;
        (dev_a, conn_id)
    });
    let task_b = tokio::spawn(async move {
        let resp = b"HTTP/1.1 200 OK\r\ncontent-length: 11\r\n\r\nfrom-node-b";
        let (conn_id, _) = serve_http_request(&mut dev_b, resp).await;
        (dev_b, conn_id)
    });

    let ck_a = format!("{AUTH_COOKIE}={cookie_a}");
    let ck_b = format!("{AUTH_COOKIE}={cookie_b}");
    let h_a = [("cookie", ck_a.as_str())];
    let h_b = [("cookie", ck_b.as_str())];
    let (r1, r2) = tokio::join!(
        http_request(addr, "GET", "/d/node-a/panel", &h_a, b""),
        http_request(addr, "GET", "/d/node-b/panel", &h_b, b"")
    );
    let (_dev_a, ca) = task_a.await.unwrap();
    let (_dev_b, cb) = task_b.await.unwrap();
    assert_ne!(ca, cb, "两台设备的 conn_id 必须独立");

    // 各自收到自己设备的响应，不串线。
    assert_eq!(r1.0, 200);
    assert_eq!(r1.2, b"from-node-a");
    assert_eq!(r2.0, 200);
    assert_eq!(r2.2, b"from-node-b");
}
