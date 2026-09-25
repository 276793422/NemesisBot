//! 反向桥客户端测试（goal：节点显示名 + 反向桥与多设备汇聚，一期批次二）。
//!
//! 独立测试文件（项目纪律：生产文件禁内联测试）。两类：
//! - 纯函数：bridge_endpoint 归一 / check_access 比对 / next_backoff 步进 /
//!   hostname_node_id 形态
//! - 集成（本地回环 mock 中继）：hello→welcome 握手、conn 泵字节往返
//!   （TCP echo）、dial 失败回执、access_check 通过/拒绝、welcome 拒止
//!   不 panic 且退避重连、假死判定（服务端沉默→客户端主动断）、心跳续命
//!   （服务端回 Pong→连接不误杀）
//!
//! 集成测试的时序参数全部走 [`LoopTiming`] 小值（毫秒级），生产默认值
//! 30s/90s 不受影响。

use std::time::Duration;

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener, TcpStream};
use tokio_tungstenite::{WebSocketStream, accept_async, tungstenite::Message};

use nemesis_web::relay::protocol::{BridgeFrame, decode_frame, encode_frame};

use crate::bridge_client::{
    BridgeClientParams, LoopTiming, bridge_endpoint, check_access, constant_time_eq,
    hostname_node_id, next_backoff, run_loop,
};

// ---------------------------------------------------------------------------
// 工具
// ---------------------------------------------------------------------------

/// 毫秒级时序（退避/握手超时快速走完）。**dead_after 给 5s**：常规用例
/// 永不触发假死——并行测试下 mock 服务端（测试任务本身）可能被调度饿
/// 数百 ms，不回帧 ≠ 假死；假死语义由 death_watch_timing 专属覆盖。
fn fast_timing() -> LoopTiming {
    LoopTiming {
        heartbeat: Duration::from_millis(20),
        dead_after: Duration::from_secs(5),
        welcome_timeout: Duration::from_secs(2),
        backoff_min: Duration::from_millis(10),
        backoff_max: Duration::from_millis(50),
    }
}

/// 假死判定专属（短 dead_after；仅「服务端沉默 → 客户端主动断开重连」用例）。
fn death_watch_timing() -> LoopTiming {
    LoopTiming {
        dead_after: Duration::from_millis(150),
        ..fast_timing()
    }
}

/// 心跳续命专属（dead_after=400ms：既要在测试窗口内验证「Pong 续命不断」，
/// 又给并行调度饿留足余量——服务端每个心跳都即时回 Pong）。
fn pong_timing() -> LoopTiming {
    LoopTiming {
        dead_after: Duration::from_millis(400),
        ..fast_timing()
    }
}

/// 测试参数（node_id/version 固定，token/端口用例自定）。
fn test_params(token: &str, web_port: u16) -> BridgeClientParams {
    BridgeClientParams {
        relay_url: String::new(), // 各用例填
        token: token.to_string(),
        node_id: "bridge-testnode".to_string(),
        name: "TestNode".to_string(),
        version: "0.0.0-test".to_string(),
        web_port,
        access_token: "secret".to_string(),
        cluster_identity: None,
        bridge_rpc: None,
    }
}

/// 起 mock 中继：bind 端口 + spawn accept 任务（**不阻塞等连接**——
/// current_thread runtime 下若在测试任务里直接 await accept，此刻桥客户端
/// 任务尚未 spawn，无人发起连接 = 永久死锁；必须先让 accept 任务入队）。
/// 返回 (端口, accept 任务句柄)——用例先 spawn 客户端再 await 句柄拿 ws。
async fn spawn_mock_relay() -> (u16, tokio::task::JoinHandle<WebSocketStream<TcpStream>>) {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let handle = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        accept_async(stream).await.expect("ws handshake")
    });
    (port, handle)
}

/// 等待 mock 中继 accept 完成，拿 ws stream。
async fn relay_ws_of(
    handle: tokio::task::JoinHandle<WebSocketStream<TcpStream>>,
) -> WebSocketStream<TcpStream> {
    tokio::time::timeout(Duration::from_secs(5), handle)
        .await
        .expect("mock relay accept 超时")
        .expect("accept task panic")
}

/// 读一帧 Text 并解码为 BridgeFrame（超时 = 用例失败，防卡死）。
async fn recv_frame(ws: &mut WebSocketStream<TcpStream>) -> BridgeFrame {
    let deadline = Duration::from_secs(5);
    match tokio::time::timeout(deadline, ws.next()).await {
        Ok(Some(Ok(Message::Text(t)))) => decode_frame(&t).expect("解码 BridgeFrame"),
        Ok(other) => panic!("期望 Text 帧，实际：{other:?}"),
        Err(_) => panic!("等帧超时（{deadline:?}）"),
    }
}

/// 发一帧。
async fn send_frame(ws: &mut WebSocketStream<TcpStream>, frame: BridgeFrame) {
    let text = encode_frame(&frame).expect("编码");
    ws.send(Message::Text(text.into())).await.expect("发送");
}

/// 读到首个匹配谓词的帧（跳过中间的 Heartbeat/Pong 噪音；超时失败）。
async fn recv_frame_matching(
    ws: &mut WebSocketStream<TcpStream>,
    pred: impl Fn(&BridgeFrame) -> bool,
) -> BridgeFrame {
    for _ in 0..100 {
        let f = tokio::time::timeout(Duration::from_secs(5), recv_frame(ws))
            .await
            .expect("等匹配帧超时");
        if pred(&f) {
            return f;
        }
    }
    panic!("100 帧内未等到匹配帧");
}

/// spawn 桥客户端 run_loop（默认快速时序）。
fn spawn_client(mut params: BridgeClientParams, relay_port: u16) -> tokio::task::JoinHandle<()> {
    params.relay_url = format!("ws://127.0.0.1:{relay_port}");
    tokio::spawn(run_loop(params, fast_timing()))
}

/// spawn 桥客户端 run_loop（自定义时序——假死/续命用例）。
fn spawn_client_with(
    mut params: BridgeClientParams,
    relay_port: u16,
    timing: LoopTiming,
) -> tokio::task::JoinHandle<()> {
    params.relay_url = format!("ws://127.0.0.1:{relay_port}");
    tokio::spawn(run_loop(params, timing))
}

// ---------------------------------------------------------------------------
// 纯函数
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_bridge_endpoint_normalization() {
    // 无路径 → 自动拼 /bridge
    assert_eq!(
        bridge_endpoint("ws://vps.example.com:60600").unwrap(),
        "ws://vps.example.com:60600/bridge"
    );
    // 尾斜杠 → 归一
    assert_eq!(
        bridge_endpoint("ws://vps.example.com:60600/").unwrap(),
        "ws://vps.example.com:60600/bridge"
    );
    // 误带路径 → 截到 authority（强制 /bridge）
    assert_eq!(
        bridge_endpoint("ws://vps.example.com:60600/some/path").unwrap(),
        "ws://vps.example.com:60600/bridge"
    );
    // wss 保留
    assert_eq!(
        bridge_endpoint("wss://vps.example.com").unwrap(),
        "wss://vps.example.com/bridge"
    );
    // http/https 拒绝（协议只有 ws/wss）
    assert!(bridge_endpoint("http://example.com").is_err());
    // 缺 scheme / 缺 host 拒绝
    assert!(bridge_endpoint("example.com:60600").is_err());
    assert!(bridge_endpoint("ws://").is_err());
    // 空白宽容
    assert_eq!(bridge_endpoint("  ws://h:1  ").unwrap(), "ws://h:1/bridge");
}

#[test]
fn test_check_access() {
    // 正确哈希（secret 的 SHA-256，大小写不敏感）
    let secret_hash: String = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b"secret"))
    };
    assert!(check_access("secret", &secret_hash));
    assert!(check_access("secret", &secret_hash.to_uppercase()));
    // 错误哈希
    assert!(!check_access("secret", &format!("{:0>64}", "0")));
    // 空 access_token = fail-closed（连空串的哈希都拒）
    let empty_hash: String = {
        use sha2::{Digest, Sha256};
        format!("{:x}", Sha256::digest(b""))
    };
    assert!(!check_access("", &empty_hash));
}

#[test]
fn test_constant_time_eq() {
    assert!(constant_time_eq(b"abc", b"abc"));
    assert!(!constant_time_eq(b"abc", b"abd"));
    assert!(!constant_time_eq(b"abc", b"ab"));
    assert!(!constant_time_eq(b"", b"a"));
    assert!(constant_time_eq(b"", b""));
}

#[test]
fn test_next_backoff_ladder() {
    let max = Duration::from_secs(60);
    // 5→10→20→40→60 封顶（goal 钉死的阶梯）
    let mut cur = Duration::from_secs(5);
    let mut seen = vec![cur];
    for _ in 0..6 {
        cur = next_backoff(cur, max);
        seen.push(cur);
    }
    assert_eq!(
        seen,
        vec![
            Duration::from_secs(5),
            Duration::from_secs(10),
            Duration::from_secs(20),
            Duration::from_secs(40),
            Duration::from_secs(60),
            Duration::from_secs(60),
            Duration::from_secs(60),
        ]
    );
    // 零值防死循环（×2 恒零 → 落 max）
    assert_eq!(next_backoff(Duration::ZERO, max), max);
}

#[test]
fn test_hostname_node_id_shape() {
    let id = hostname_node_id();
    // 形态断言（不依赖具体主机名）：bridge- 前缀 + 全小写
    assert!(id.starts_with("bridge-"), "node_id={id}");
    assert_eq!(id, id.to_lowercase(), "node_id 必须全小写（URL 路径友好）");
}

// ---------------------------------------------------------------------------
// 集成：mock 中继 + conn 泵
// ---------------------------------------------------------------------------

/// 本机 TCP echo 服务：模拟真实 web server 的请求完整性语义——按
/// content-length 读满请求（**不依赖**写半 EOF；2026-09-19 修复后设备
/// 侧不再 shutdown 写半）→ 回 echo 响应 → 主动关连接（客户端读泵读到
/// EOF → 空 fin 帧，覆盖 EOF 约定路径）。
async fn spawn_echo_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                // 读到头结束（\r\n\r\n）。
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return; // 连接关闭且无完整请求
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]);
                let clen: usize = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                // 读满 body（content-length 界定，等价 hyper 服务端语义）。
                while buf.len() < head_end + clen {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                // 回 echo 响应（body = 请求 body）。
                let body = buf[head_end..head_end + clen].to_vec();
                let resp = format!("HTTP/1.1 200 OK\r\ncontent-length: {}\r\n\r\n", clen);
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&body).await;
                // drop = 关连接 → 客户端读到响应后 EOF → 空 fin 帧
            });
        }
    });
    port
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_conn_pump_roundtrip() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let web_port = spawn_echo_server().await;
    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok1", web_port), relay_port);
    let mut relay_ws = relay_ws_of(relay_accept).await;

    // hello → welcome
    let hello = recv_frame(&mut relay_ws).await;
    match hello {
        BridgeFrame::BridgeHello {
            token,
            node_id,
            name,
            version,
            ..
        } => {
            assert_eq!(token, "tok1");
            assert_eq!(node_id, "bridge-testnode");
            assert_eq!(name, "TestNode");
            assert_eq!(version, "0.0.0-test");
        }
        other => panic!("首帧应为 BridgeHello，实际 {other:?}"),
    }
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;

    // 等客户端进入主循环（首个 Heartbeat 到达即证明）
    let _ = recv_frame_matching(&mut relay_ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // 服务端下发一条短 conn：ConnOpen + 整段请求 fin=true。载荷 = 完整
    // HTTP 报文（等价 serialize_http_request 产物——本机 server 按
    // content-length 界定请求完整性）。
    use base64::Engine as _;
    let req_bytes = b"POST /echo HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: 8\r\n\r\nfoo-bar\n";
    send_frame(
        &mut relay_ws,
        BridgeFrame::ConnOpen {
            conn_id: 7,
            target: "local".into(),
        },
    )
    .await;
    send_frame(
        &mut relay_ws,
        BridgeFrame::ConnData {
            conn_id: 7,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(req_bytes),
            fin: true,
        },
    )
    .await;

    // 读回传：ConnData(响应字节, fin=false) 后跟空载荷 fin=true（EOF 约定
    // ——echo server 回完响应主动关连接）
    let mut payload = Vec::new();
    loop {
        use base64::Engine as _;
        match recv_frame_matching(&mut relay_ws, |f| {
            matches!(
                f,
                BridgeFrame::ConnData { .. } | BridgeFrame::ConnClose { .. }
            )
        })
        .await
        {
            BridgeFrame::ConnData {
                conn_id,
                data_b64,
                fin,
                ..
            } => {
                assert_eq!(conn_id, 7);
                payload.extend_from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(&data_b64)
                        .unwrap(),
                );
                if fin {
                    assert!(data_b64.is_empty(), "EOF 约定：fin 帧必须空载荷");
                    break;
                }
            }
            BridgeFrame::ConnClose { conn_id, reason } => {
                panic!("conn#{conn_id} 意外被关：{reason}");
            }
            _ => unreachable!(),
        }
    }
    // echo server 回完整 HTTP 响应（头 + body=请求 body 回显）。
    let text = String::from_utf8_lossy(&payload).to_string();
    assert!(
        text.starts_with("HTTP/1.1 200 OK\r\n"),
        "响应必须带 HTTP 头（content-length 界定语义）：{text}"
    );
    assert!(
        payload.ends_with(b"foo-bar\n"),
        "echo body 必须原样回传：{text}"
    );

    // 服务端关连接 → 客户端应退避重连（第二次 hello 证明 run_loop 存活）
    let _ = relay_ws.close(None).await;
    drop(relay_ws);
    let relay_ws2 = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let mut relay_ws2 = relay_ws2;
    let hello2 = recv_frame(&mut relay_ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "断开后应重连并重发 hello，实际 {hello2:?}"
    );

    client.abort();
}

/// keep-alive 形态本机服务：读满 content-length 后**先探测写半未 shutdown**
/// （修复点回归：shutdown 会让 hyper 在处理前断连——2026-09-19 真机彩排）
/// → 回响应 → **保持连接不关**（模拟真实 web server keep-alive）。
async fn spawn_keepalive_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((mut sock, _)) = listener.accept().await else {
                break;
            };
            tokio::spawn(async move {
                let mut buf: Vec<u8> = Vec::new();
                let mut tmp = [0u8; 4096];
                let head_end = loop {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                    if let Some(p) = buf.windows(4).position(|w| w == b"\r\n\r\n") {
                        break p + 4;
                    }
                };
                let head = String::from_utf8_lossy(&buf[..head_end]);
                let clen: usize = head
                    .lines()
                    .find_map(|l| {
                        l.strip_prefix("content-length:")
                            .and_then(|v| v.trim().parse::<usize>().ok())
                    })
                    .unwrap_or(0);
                while buf.len() < head_end + clen {
                    let n = sock.read(&mut tmp).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    buf.extend_from_slice(&tmp[..n]);
                }
                // 探测写半：请求读满后短暂再读——EOF（Ok(0)）= 客户端
                // shutdown 了（回归失败）；超时无数据 = 写半仍开（通过）。
                let probe = tokio::time::timeout(
                    std::time::Duration::from_millis(150),
                    sock.read(&mut tmp),
                )
                .await;
                match probe {
                    Err(_elapsed) => {} // 期待：超时（写半未关）
                    Ok(Ok(0)) => panic!("写半被 shutdown（修复回归）：hyper 将在处理前断连"),
                    Ok(Ok(_)) => {} // 多余字节：容忍
                    Ok(Err(_)) => {}
                }
                // 固定响应（与请求 body 解耦——验证的是「请求按
                // content-length 被处理 + 写半未被 shutdown」）。
                let resp = b"HTTP/1.1 200 OK\r\ncontent-length: 5\r\n\r\nhello";
                let _ = sock.write_all(resp).await;
                // keep-alive：不关连接，持有 sock 直至任务被 abort。
                tokio::time::sleep(std::time::Duration::from_secs(30)).await;
            });
        }
    });
    port
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_fin_true_does_not_shutdown_write_half() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    // 2026-09-19 修复回归：fin=true 请求收口**不得**半关闭写半——真实
    // hyper 服务端在写半 EOF 时于处理请求前断连（零字节响应）。keep-alive
    // 形态：本机回响应后不关连接 → 客户端不得误发 EOF（空 fin）帧。
    let web_port = spawn_keepalive_server().await;
    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok1", web_port), relay_port);
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut relay_ws).await; // hello
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut relay_ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    send_frame(
        &mut relay_ws,
        BridgeFrame::ConnOpen {
            conn_id: 9,
            target: "local".into(),
        },
    )
    .await;
    // 载荷 = 完整 HTTP 报文（content-length: 0 的 GET，等价真实短 conn）。
    let req_bytes = b"GET /x HTTP/1.1\r\nhost: 127.0.0.1\r\ncontent-length: 0\r\n\r\n";
    send_frame(
        &mut relay_ws,
        BridgeFrame::ConnData {
            conn_id: 9,
            seq: 0,
            data_b64: base64::engine::general_purpose::STANDARD.encode(req_bytes),
            fin: true,
        },
    )
    .await;

    // 收到响应 ConnData（说明本机处理了请求——若 shutdown 已在探测点 panic）。
    use base64::Engine as _;
    let mut payload = Vec::new();
    loop {
        match recv_frame_matching(&mut relay_ws, |f| {
            matches!(
                f,
                BridgeFrame::ConnData { .. } | BridgeFrame::ConnClose { .. }
            )
        })
        .await
        {
            BridgeFrame::ConnData {
                conn_id,
                data_b64,
                fin,
                ..
            } => {
                assert_eq!(conn_id, 9);
                payload.extend_from_slice(
                    &base64::engine::general_purpose::STANDARD
                        .decode(&data_b64)
                        .unwrap(),
                );
                if fin {
                    panic!("keep-alive 连接未关闭，客户端不得发 EOF（空 fin）帧");
                }
            }
            BridgeFrame::ConnClose { conn_id, reason } => {
                panic!("conn#{conn_id} 意外被关：{reason}");
            }
            _ => unreachable!(),
        }
        let text = String::from_utf8_lossy(&payload).to_string();
        if text.ends_with("hello") {
            break; // 响应 body 已完整收到
        }
    }

    // 响应后短窗口内不得出现空 fin 帧（连接保持，读泵不 EOF）。
    let extra = tokio::time::timeout(
        std::time::Duration::from_millis(300),
        recv_frame_matching(&mut relay_ws, |f| {
            matches!(
                f,
                BridgeFrame::ConnData { fin: true, .. } | BridgeFrame::ConnClose { .. }
            )
        }),
    )
    .await;
    assert!(
        extra.is_err(),
        "keep-alive 场景响应后不应有 EOF/ConnClose 上行：{extra:?}"
    );

    client.abort();
}

/// 在同一端口再 accept 一次（重连场景；同 spawn_mock_relay 的死锁规避）。
async fn spawn_mock_relay_on_same_port(
    port: u16,
) -> tokio::task::JoinHandle<WebSocketStream<TcpStream>> {
    let listener = TcpListener::bind(("127.0.0.1", port))
        .await
        .expect("rebind");
    tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept#2");
        accept_async(stream).await.expect("ws handshake#2")
    })
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_dial_failure_sends_conn_close() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    // 拿一个端口立刻释放——确保无人监听
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = l.local_addr().unwrap().port();
    drop(l);

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok2", dead_port), relay_port);
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut relay_ws).await; // hello
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut relay_ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // 下发 conn → dial 死端口 → ConnClose 回执（reason 含 dial failed）
    send_frame(
        &mut relay_ws,
        BridgeFrame::ConnOpen {
            conn_id: 9,
            target: "local".into(),
        },
    )
    .await;
    let close = recv_frame_matching(&mut relay_ws, |f| {
        matches!(f, BridgeFrame::ConnClose { conn_id: 9, .. })
    })
    .await;
    match close {
        BridgeFrame::ConnClose { reason, .. } => {
            assert!(reason.contains("dial failed"), "reason={reason}");
        }
        _ => unreachable!(),
    }
    client.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_access_check_roundtrip() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    use sha2::{Digest, Sha256};
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = l.local_addr().unwrap().port();
    drop(l); // 本用例不碰 conn，端口给死端口即可

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok3", dead_port), relay_port);
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut relay_ws).await; // hello
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut relay_ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // 正确哈希（客户端 access_token="secret"）→ ok=true
    let good = format!("{:x}", Sha256::digest(b"secret"));
    send_frame(
        &mut relay_ws,
        BridgeFrame::AccessCheck {
            request_id: "req-ok".into(),
            node_id: "bridge-testnode".into(),
            hash_hex: good,
        },
    )
    .await;
    match recv_frame_matching(&mut relay_ws, |f| {
        matches!(f, BridgeFrame::AccessResult { .. })
    })
    .await
    {
        BridgeFrame::AccessResult { request_id, ok } => {
            assert_eq!(request_id, "req-ok");
            assert!(ok);
        }
        _ => unreachable!(),
    }

    // 错误哈希 → ok=false
    send_frame(
        &mut relay_ws,
        BridgeFrame::AccessCheck {
            request_id: "req-bad".into(),
            node_id: "bridge-testnode".into(),
            hash_hex: format!("{:x}", Sha256::digest(b"wrong")),
        },
    )
    .await;
    match recv_frame_matching(&mut relay_ws, |f| {
        matches!(f, BridgeFrame::AccessResult { .. })
    })
    .await
    {
        BridgeFrame::AccessResult { request_id, ok } => {
            assert_eq!(request_id, "req-bad");
            assert!(!ok);
        }
        _ => unreachable!(),
    }
    client.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_welcome_rejected_then_retry() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = l.local_addr().unwrap().port();
    drop(l);

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("bad-token", dead_port), relay_port);
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let hello = recv_frame(&mut relay_ws).await;
    assert!(matches!(hello, BridgeFrame::BridgeHello { .. }));
    // 拒绝接入（token 不对）
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: false,
            reason: "token mismatch".into(),
            hub_node_id: String::new(),
        },
    )
    .await;
    // 服务端随后关连接 → 客户端收到被拒 → 诚实 ERROR + 退避重连。
    // 第二次 hello 到来 = 未 panic、循环健在（配对失败与网络断都走重连）。
    let _ = relay_ws.close(None).await;
    drop(relay_ws);
    let mut relay_ws2 = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let hello2 = recv_frame(&mut relay_ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "{hello2:?}"
    );
    client.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_dead_server_detection_and_reconnect() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = l.local_addr().unwrap().port();
    drop(l);

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client_with(
        test_params("tok5", dead_port),
        relay_port,
        death_watch_timing(),
    );
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut relay_ws).await; // hello
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;

    // 服务端从此**沉默**（不回 Pong）——dead_after=150ms 后客户端应判定
    // 假死主动断开并重连。第二次 hello 到来即证明「主动断开重连」生效。
    // 期间客户端会发若干 Heartbeat，全部跳过。
    let mut relay_ws2 = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let hello2 = recv_frame(&mut relay_ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "{hello2:?}"
    );
    client.abort();
}

#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_heartbeat_pong_keeps_session_alive() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let dead_port = l.local_addr().unwrap().port();
    drop(l);

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client_with(test_params("tok6", dead_port), relay_port, pong_timing());
    let mut relay_ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut relay_ws).await; // hello
    send_frame(
        &mut relay_ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;

    // 认真回 Pong：1.5s >> dead_after(400ms)——连接必须仍然健在
    // （假死判定被每个下行帧刷新）。期间会收到多个 Heartbeat。
    let deadline = tokio::time::Instant::now() + Duration::from_millis(1500);
    let mut heartbeats = 0;
    while tokio::time::Instant::now() < deadline {
        match tokio::time::timeout(Duration::from_millis(300), relay_ws.next()).await {
            Ok(Some(Ok(Message::Text(t)))) => match decode_frame(&t) {
                Ok(BridgeFrame::Heartbeat) => {
                    heartbeats += 1;
                    send_frame(&mut relay_ws, BridgeFrame::Pong).await;
                }
                Ok(_) => {}
                Err(e) => panic!("坏帧：{e}"),
            },
            Ok(Some(Ok(_))) => {}
            Ok(Some(Err(e))) => panic!("ws 错误（连接不应断）：{e}"),
            Ok(None) => panic!("客户端不应断开（Pong 在续命）"),
            Err(_) => {} // 300ms 内无帧，继续
        }
    }
    assert!(heartbeats >= 2, "2s 内应发多个心跳，实际 {heartbeats}");
    client.abort();
}

/// 最小实验:tokio-tungstenite 客户端 ↔ accept_async 服务端互发一帧
/// (隔离「测试基建 vs 桥客户端逻辑」——本测试挂 = 基建问题)。
#[tokio::test]
async fn test_minimal_ws_exchange() {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    let accept_task = tokio::spawn(async move {
        let (stream, _) = listener.accept().await.expect("accept");
        accept_async(stream).await.expect("ws handshake")
    });
    let client_task = tokio::spawn(async move {
        let (ws, _resp) = tokio_tungstenite::connect_async(format!("ws://127.0.0.1:{port}/bridge"))
            .await
            .expect("connect");
        let (mut w, mut r) = ws.split();
        w.send(Message::Text("ping".into())).await.expect("send");
        tokio::time::timeout(Duration::from_secs(5), r.next())
            .await
            .expect("等 pong 超时")
            .expect("stream ended")
            .expect("ws err")
    });
    let mut server_ws = tokio::time::timeout(Duration::from_secs(5), accept_task)
        .await
        .expect("accept 超时")
        .expect("accept err");
    let hello = tokio::time::timeout(Duration::from_secs(5), server_ws.next())
        .await
        .expect("等 ping 超时")
        .expect("stream ended")
        .expect("ws err");
    assert_eq!(hello, Message::Text("ping".into()));
    server_ws
        .send(Message::Text("pong".into()))
        .await
        .expect("send pong");
    let got = tokio::time::timeout(Duration::from_secs(5), client_task)
        .await
        .expect("client 超时")
        .expect("client panic");
    assert_eq!(got, Message::Text("pong".into()));
}

// ===========================================================================
// wave4 追加（coverage）：bridge_client.rs 残余臂。
// 既有用例已钉纯函数 + happy path（hello/welcome/conn 泵/access_check/
// 假死/退避）；本批补：hostname 回退链、cluster_identity 进 hello、
// welcome 超时、握手期畸形首帧（Binary/先行关闭/非 welcome 文本）、
// 下行噪音帧免疫（坏 JSON/Binary/反向帧/未装配 cluster_rpc+member_sync）、
// 下行坏 base64 / 未登记 conn / 服务端 ConnClose、下行背压实测收口、
// kick_reconnect 两臂（会话内踢断 + 退避等待跳过）、连失败退避重试、
// bridge_rpc 装配形态（cluster feature：attach/downstream/member_sync/
// detach 全链 + 非空 hub_node_id welcome 臂）。
// 全部走 127.0.0.1:0 临时端口；web_port 用死端口（ dial 失败即诚实回执）。
// ===========================================================================

/// 拿一个已释放的端口（dial 必失败 / relay 占位用）。
fn dead_web_port() -> u16 {
    let l = std::net::TcpListener::bind("127.0.0.1:0").expect("bind dead port");
    let p = l.local_addr().unwrap().port();
    drop(l);
    p
}

/// 集成用例全域串行（reconnect_notify 是**进程级** Notify：kick permit 会
/// 命中任何在跑的桥客户端——并行下其他用例的客户端会被误踢断连。全部
/// spawn 客户端的用例持同一把锁 + 起手清残留 permit，互不干扰）。
static BRIDGE_IT_SER: parking_lot::Mutex<()> = parking_lot::Mutex::new(());

/// 吃掉可能残留的 kick permit（notify_one 无 waiter 时存一个 permit；
/// 下一个客户端的主循环 select 会立即消费 → 误踢。测试起手排干）。
async fn drain_kick_permit() {
    let _ = tokio::time::timeout(
        Duration::from_millis(2),
        nemesis_web::relay::reconnect_notify().notified(),
    )
    .await;
}

/// hostname 链回退臂：摘除 COMPUTERNAME → 落 HOSTNAME（都缺 = unknown）。
/// GLOBAL_STATE_LOCK 串行化环境变量操作（与 EnvHomeGuard 同纪律）。
#[test]
fn test_hostname_fallback_chain() {
    let _guard = crate::GLOBAL_STATE_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let orig = std::env::var_os("COMPUTERNAME");
    let expected = std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".to_string());
    unsafe {
        std::env::remove_var("COMPUTERNAME");
    }
    let got = crate::bridge_client::hostname();
    // 先恢复再断言（断言失败也不留污染）。
    unsafe {
        match orig {
            Some(v) => std::env::set_var("COMPUTERNAME", v),
            None => std::env::remove_var("COMPUTERNAME"),
        }
    }
    assert_eq!(
        got, expected,
        "COMPUTERNAME 缺席时必须落 HOSTNAME/unknown 回退链"
    );
}

/// 二期身份快照：cluster_identity Some → hello 帧携带全部集群字段
/// （服务端据此注册进集群 registry 同权组网）。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_cluster_identity_carried_in_hello() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let mut params = test_params("tok-ci", dead_web_port());
    params.cluster_identity = Some(nemesis_web::relay::BridgeClusterIdentity {
        node_id: "node-cov-1".to_string(),
        name: "CovNode".to_string(),
        role: "worker".to_string(),
        category: "development".to_string(),
        tags: vec!["t1".to_string()],
        capabilities: vec!["exec".to_string()],
        node_type: "agent".to_string(),
        rpc_port: 12345,
        addresses: vec!["192.168.1.10".to_string()],
    });
    let client = spawn_client(params, relay_port);
    let mut ws = relay_ws_of(relay_accept).await;
    match recv_frame(&mut ws).await {
        BridgeFrame::BridgeHello {
            cluster_node_id,
            cluster_name,
            role,
            category,
            tags,
            rpc_port,
            addresses,
            ..
        } => {
            assert_eq!(cluster_node_id.as_deref(), Some("node-cov-1"));
            assert_eq!(cluster_name.as_deref(), Some("CovNode"));
            assert_eq!(role.as_deref(), Some("worker"));
            assert_eq!(category.as_deref(), Some("development"));
            assert_eq!(tags, Some(vec!["t1".to_string()]));
            assert_eq!(rpc_port, Some(12345));
            assert_eq!(addresses, Some(vec!["192.168.1.10".to_string()]));
        }
        other => panic!("首帧应为 BridgeHello，实际 {other:?}"),
    }
    client.abort();
}

/// welcome 超时：hello 发出后服务端沉默 → welcome_timeout 内无回执 →
/// 诚实断开（welcomed=false）→ 退避重连重发 hello。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_welcome_timeout_reconnects() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = l.local_addr().unwrap().port();
    drop(l);
    let timing = LoopTiming {
        welcome_timeout: Duration::from_millis(150),
        ..fast_timing()
    };
    let client = spawn_client_with(test_params("tok-wt", dead_web_port()), relay_port, timing);

    // 第一次连接：读 hello 后装聋（不回 welcome）。
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await;
    drop(ws); // 顺手关——客户端此刻多半已超时

    // 超时断开后必须重连并重发 hello。
    let mut ws2 = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let hello2 = recv_frame(&mut ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "welcome 超时后必须重连重发 hello，实际 {hello2:?}"
    );
    client.abort();
}

/// 握手期畸形首帧三连：Binary（非 Text）/ 先行关闭（流尽）/ 合法 JSON 但
/// 非 welcome 帧——各自诚实断开并重连，循环健在。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_handshake_malformed_first_frames_reconnect() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = l.local_addr().unwrap().port();
    drop(l);
    let client = spawn_client(test_params("tok-mf", dead_web_port()), relay_port);

    // A：首帧 Binary → 协议错乱断开
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await;
    ws.send(Message::Binary(vec![0u8, 1].into())).await.unwrap();
    drop(ws);

    // B：welcome 未到先关流（ws_read None）
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await;
    let _ = ws.close(None).await;
    drop(ws);

    // C：首帧是合法桥帧但不是 welcome（Heartbeat）
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await;
    send_frame(&mut ws, BridgeFrame::Heartbeat).await;
    drop(ws);

    // D：三连错后循环仍健在——第四次 hello 到来即证明。
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let hello = recv_frame(&mut ws).await;
    assert!(
        matches!(hello, BridgeFrame::BridgeHello { .. }),
        "畸形首帧三连后必须仍重连，实际 {hello:?}"
    );
    client.abort();
}

/// 下行噪音免疫：坏 JSON 文本 / Binary / 反向帧（设备→服务端方向的
/// AccessResult 被服务端发来）/ 未装配枢纽的 cluster_rpc / member_sync
/// ——全部忽略不崩，会话照常心跳 + access_check 照常应答。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_downstream_noise_frames_ignored_and_session_survives() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    use sha2::{Digest, Sha256};
    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok-nz", dead_web_port()), relay_port);
    let mut ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut ws).await; // hello
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // 五种噪音各一发。
    ws.send(Message::Text("!!not-json!!".into())).await.unwrap();
    ws.send(Message::Binary(vec![1, 2, 3].into()))
        .await
        .unwrap();
    send_frame(
        &mut ws,
        BridgeFrame::AccessResult {
            request_id: "wrong-direction".to_string(),
            ok: true,
        },
    )
    .await;
    send_frame(
        &mut ws,
        BridgeFrame::ClusterRpc {
            payload: serde_json::json!({}),
        },
    )
    .await;
    send_frame(
        &mut ws,
        BridgeFrame::MemberSync {
            payload: serde_json::json!({}),
        },
    )
    .await;

    // 会话必须仍健在：后续心跳照常。
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;
    // access_check 照常应答（功能未受损）。
    let good = format!("{:x}", Sha256::digest(b"secret"));
    send_frame(
        &mut ws,
        BridgeFrame::AccessCheck {
            request_id: "nz".to_string(),
            node_id: "bridge-testnode".to_string(),
            hash_hex: good,
        },
    )
    .await;
    match recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::AccessResult { .. })).await {
        BridgeFrame::AccessResult { request_id, ok } => {
            assert_eq!(request_id, "nz");
            assert!(ok);
        }
        _ => unreachable!(),
    }
    client.abort();
}

/// 下行 ConnData 三防御臂：坏 base64（收口 + 回执）/ 合法 base64 但 conn
/// 未登记（停发通知）/ 服务端主动 ConnClose（静默清理）——会话不受损。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_conn_data_bad_base64_unknown_conn_and_server_close() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    use base64::Engine as _;
    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok-b64", dead_web_port()), relay_port);
    let mut ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut ws).await; // hello
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // ① 坏 base64 → ConnClose("bad base64")
    send_frame(
        &mut ws,
        BridgeFrame::ConnData {
            conn_id: 11,
            seq: 0,
            data_b64: "%%%not-base64%%%".to_string(),
            fin: false,
        },
    )
    .await;
    match recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::ConnClose { .. })).await {
        BridgeFrame::ConnClose { conn_id, reason } => {
            assert_eq!(conn_id, 11);
            assert!(reason.contains("bad base64"), "reason={reason}");
        }
        _ => unreachable!(),
    }

    // ② 合法 base64 但 conn 未登记 → ConnClose("conn not found")
    send_frame(
        &mut ws,
        BridgeFrame::ConnData {
            conn_id: 11,
            seq: 1,
            data_b64: base64::engine::general_purpose::STANDARD.encode(b"x"),
            fin: false,
        },
    )
    .await;
    match recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::ConnClose { .. })).await {
        BridgeFrame::ConnClose { conn_id, reason } => {
            assert_eq!(conn_id, 11);
            assert!(reason.contains("conn not found"), "reason={reason}");
        }
        _ => unreachable!(),
    }

    // ③ 服务端主动 ConnClose → 静默清理（无回帧、不崩）。
    send_frame(
        &mut ws,
        BridgeFrame::ConnClose {
            conn_id: 11,
            reason: "browser gone".to_string(),
        },
    )
    .await;

    // 存活证明：心跳照常。
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;
    client.abort();
}

/// 黑洞本机服务：accept 后只持不读（写泵 write_all 卡死 → 下行写队列
/// 16 深度灌满 → try_send 失败 → 背压实测收口）。
async fn spawn_blackhole_server() -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").await.expect("bind");
    let port = listener.local_addr().unwrap().port();
    tokio::spawn(async move {
        loop {
            let Ok((sock, _)) = listener.accept().await else {
                break;
            };
            // 持有 socket 不读不写（30s 后随任务回收）。
            tokio::spawn(async move {
                tokio::time::sleep(Duration::from_secs(30)).await;
                drop(sock);
            });
        }
    });
    port
}

/// 下行背压：32 帧 × 64KB 灌黑洞 conn（内核缓冲 + 16 深度队列必然溢出）
/// → 诚实 ConnClose("backpressure") 收口整条 conn。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_downstream_backpressure_closes_conn() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    use base64::Engine as _;
    let web_port = spawn_blackhole_server().await;
    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let client = spawn_client(test_params("tok-bp", web_port), relay_port);
    let mut ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut ws).await; // hello
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    send_frame(
        &mut ws,
        BridgeFrame::ConnOpen {
            conn_id: 21,
            target: "local".to_string(),
        },
    )
    .await;
    let chunk = base64::engine::general_purpose::STANDARD.encode(vec![0u8; 64 * 1024]);
    for i in 0..32 {
        send_frame(
            &mut ws,
            BridgeFrame::ConnData {
                conn_id: 21,
                seq: i,
                data_b64: chunk.clone(),
                fin: false,
            },
        )
        .await;
    }
    match recv_frame_matching(&mut ws, |f| {
        matches!(f, BridgeFrame::ConnClose { conn_id: 21, .. })
    })
    .await
    {
        BridgeFrame::ConnClose { reason, .. } => {
            assert!(reason.contains("backpressure"), "reason={reason}");
        }
        _ => unreachable!(),
    }
    client.abort();
}

/// 手动重连（会话内）：主循环里被 kick → Kicked 臂 → 立即重连（跳过退避）。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_kick_reconnect_during_session() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = l.local_addr().unwrap().port();
    drop(l);
    // 大退避：若 kick 未生效，重连要等 30s——测试窗口内必超时。
    let timing = LoopTiming {
        backoff_min: Duration::from_secs(30),
        backoff_max: Duration::from_secs(30),
        ..fast_timing()
    };
    let client = spawn_client_with(test_params("tok-k1", dead_web_port()), relay_port, timing);

    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await; // hello
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    // 等首个心跳 = 已进主循环（session select 的 notified() 臂在岗）。
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;
    drop(ws);

    // 重绑端口 + 后台反复 kick（permit 合并；任何一次被消费即生效）。
    let listener2 = TcpListener::bind(("127.0.0.1", relay_port))
        .await
        .expect("rebind");
    let kicker = tokio::spawn(async {
        for _ in 0..20 {
            nemesis_web::relay::kick_reconnect();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let (stream, _) = tokio::time::timeout(Duration::from_secs(6), listener2.accept())
        .await
        .expect("kick 后必须立即重连（而非等 30s 退避）")
        .expect("accept#2");
    let mut ws2 = tokio::time::timeout(Duration::from_secs(5), accept_async(stream))
        .await
        .expect("hs 超时")
        .expect("hs err");
    let hello2 = recv_frame(&mut ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "kick 后应立即重连并重发 hello，实际 {hello2:?}"
    );
    kicker.abort();
    client.abort();
}

/// 手动重连（退避等待中）：会话断开后落在 30s 退避 select 上 → kick 跳过
/// 剩余等待立即重连。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_kick_reconnect_skips_backoff_wait() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;
    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = l.local_addr().unwrap().port();
    drop(l);
    let timing = LoopTiming {
        backoff_min: Duration::from_secs(30),
        backoff_max: Duration::from_secs(30),
        ..fast_timing()
    };
    let client = spawn_client_with(test_params("tok-k2", dead_web_port()), relay_port, timing);

    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let _ = recv_frame(&mut ws).await; // hello
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: String::new(),
        },
    )
    .await;
    // welcome 后立刻关 → session Disconnected{welcomed:true} → 30s 退避。
    let _ = ws.close(None).await;
    drop(ws);

    // 退避 select 的 notified() 臂被 kick 命中 → 立即重连。
    let listener2 = TcpListener::bind(("127.0.0.1", relay_port))
        .await
        .expect("rebind");
    let kicker = tokio::spawn(async {
        for _ in 0..20 {
            nemesis_web::relay::kick_reconnect();
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    });
    let (stream, _) = tokio::time::timeout(Duration::from_secs(6), listener2.accept())
        .await
        .expect("kick 必须跳过 30s 退避立即重连")
        .expect("accept#2");
    let mut ws2 = tokio::time::timeout(Duration::from_secs(5), accept_async(stream))
        .await
        .expect("hs 超时")
        .expect("hs err");
    let hello2 = recv_frame(&mut ws2).await;
    assert!(
        matches!(hello2, BridgeFrame::BridgeHello { .. }),
        "{hello2:?}"
    );
    kicker.abort();
    client.abort();
}

/// 连失败退避重试：中继端口先空置（connect Err 反复上报）→ 迟绑 listener
/// → 客户端必须爬出失败循环连上并重发 hello。
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_connect_failure_then_late_listener_reconnects() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    let l = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let relay_port = l.local_addr().unwrap().port();
    drop(l);
    let client = spawn_client(test_params("tok-cf", dead_web_port()), relay_port);

    // 这 300ms 里客户端在连失败 + 10-50ms 退避重试。
    tokio::time::sleep(Duration::from_millis(300)).await;
    let mut ws = relay_ws_of(spawn_mock_relay_on_same_port(relay_port).await).await;
    let hello = recv_frame(&mut ws).await;
    assert!(
        matches!(hello, BridgeFrame::BridgeHello { .. }),
        "连失败退避后必须重试成功，实际 {hello:?}"
    );
    client.abort();
}

/// bridge_rpc 装配形态（cluster 构建）：attach / 下行 member_sync 合并 /
/// 下行 cluster_rpc 请求喂本地 RPC 链回上行 / 非本机目标诚实 error /
/// 会话收尾 detach 清成员表；顺带钉非空 hub_node_id welcome 臂。
#[cfg(feature = "cluster")]
#[tokio::test]
#[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
async fn test_bridge_rpc_downstream_and_member_sync_via_hub() {
    let _ser = BRIDGE_IT_SER.lock();
    drain_kick_permit().await;

    use nemesis_cluster::rpc::client::BridgeSend;
    use nemesis_cluster::rpc::server::{RpcServer, RpcServerConfig};

    let rpc = std::sync::Arc::new(RpcServer::new(RpcServerConfig::default()));
    let bridge = std::sync::Arc::new(crate::bridge_rpc::DeviceBridgeRpc::new(
        rpc,
        "cov-node-self".to_string(),
        None,
    ));

    let (relay_port, relay_accept) = spawn_mock_relay().await;
    let mut params = test_params("tok-br", dead_web_port());
    params.bridge_rpc = Some(bridge.clone());
    let client = spawn_client(params, relay_port);
    let mut ws = relay_ws_of(relay_accept).await;

    let _ = recv_frame(&mut ws).await; // hello
    // 非空 hub_node_id welcome（hub 为集群节点的形态）。
    send_frame(
        &mut ws,
        BridgeFrame::BridgeWelcome {
            ok: true,
            reason: String::new(),
            hub_node_id: "cov-hub-node".to_string(),
        },
    )
    .await;
    let _ = recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::Heartbeat)).await;

    // member_sync 前：桥成员表为空 = 全员不可达。
    assert!(!bridge.bridge_online("other-node"));
    // 下行 member_sync：online 成员入表（自身跳过、离线不洗白）。
    send_frame(
        &mut ws,
        BridgeFrame::MemberSync {
            payload: serde_json::json!({
                "members": [
                    {"node_id": "cov-node-self", "online": true},
                    {"node_id": "other-node", "online": true},
                    {"node_id": "offline-node", "online": false}
                ]
            }),
        },
    )
    .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert!(bridge.bridge_online("other-node"), "online 成员必须入表");
    assert!(!bridge.bridge_online("offline-node"), "离线成员不洗白");
    assert!(!bridge.bridge_online("cov-node-self"), "自身条目跳过");

    // 下行 cluster_rpc 请求（目标=本机）→ 喂本地 RPC 链 → 上行回帧。
    send_frame(
        &mut ws,
        BridgeFrame::ClusterRpc {
            payload: serde_json::json!({
                "version": "1.0",
                "id": "cov-req-1",
                "type": "request",
                "from": "cov-hub-node",
                "to": "cov-node-self",
                "action": "ping",
                "payload": {},
                "timestamp": 1
            }),
        },
    )
    .await;
    match recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::ClusterRpc { .. })).await {
        BridgeFrame::ClusterRpc { payload } => {
            assert_eq!(payload["id"], "cov-req-1", "回帧必须保留关联 id");
            let t = payload["type"].as_str().unwrap_or_default();
            assert!(
                t == "response" || t == "error",
                "回帧必须是 response/error，实际 {t}"
            );
        }
        _ => unreachable!(),
    }

    // 非本机目标 → 诚实 error（不发往本地链）。
    send_frame(
        &mut ws,
        BridgeFrame::ClusterRpc {
            payload: serde_json::json!({
                "version": "1.0",
                "id": "cov-req-2",
                "type": "request",
                "from": "cov-hub-node",
                "to": "someone-else",
                "action": "ping",
                "payload": {},
                "timestamp": 1
            }),
        },
    )
    .await;
    match recv_frame_matching(&mut ws, |f| matches!(f, BridgeFrame::ClusterRpc { .. })).await {
        BridgeFrame::ClusterRpc { payload } => {
            assert_eq!(payload["id"], "cov-req-2");
            assert!(
                payload["error"]
                    .as_str()
                    .unwrap_or_default()
                    .contains("not on this device"),
                "payload={payload}"
            );
        }
        _ => unreachable!(),
    }

    // 会话收尾：服务端关 → detach（成员表清空 = 全员回落不可达）。
    let _ = ws.close(None).await;
    drop(ws);
    tokio::time::sleep(Duration::from_millis(300)).await;
    assert!(
        !bridge.bridge_online("other-node"),
        "detach 后成员表必须清空"
    );
    client.abort();
}

// ---------------------------------------------------------------------------
// wave5 round2（2026-09-25）：生产入口三臂——LoopTiming::default() 常量表、
// spawn() 组装句柄、run_loop 对不可解析 relay_url 的启动即返（桥客户端未
// 启动 error 臂）与连接失败首臂（Disconnected 状态上报 + 退避）。
// ---------------------------------------------------------------------------

mod w5r2 {
    use super::*;
    use crate::bridge_client::spawn;

    /// default() 时序常量表执行 + spawn() 生产入口真被调用（死端口 →
    /// 客户端在退避循环里转，abort 收尸，无窗口无残留）。
    #[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
    #[tokio::test]
    async fn w5_loop_timing_default_and_spawn_entry() {
        // spawn 类用例全域串行（reconnect_notify 进程级 Notify 纪律）。
        let _ser = BRIDGE_IT_SER.lock();
        drain_kick_permit().await;

        let t = LoopTiming::default();
        assert!(!t.heartbeat.is_zero());
        assert!(!t.dead_after.is_zero());
        assert!(!t.welcome_timeout.is_zero());
        assert!(!t.backoff_min.is_zero());
        assert!(!t.backoff_max.is_zero());
        assert!(t.backoff_min < t.backoff_max, "退避下限必须小于上限");

        let port = dead_web_port();
        let handle = spawn(test_params("w5b-spawn", port));
        tokio::time::sleep(Duration::from_millis(250)).await;
        handle.abort();
        let _ = handle.await; // aborted 收尸
    }

    /// relay_url 不可解析 → run_loop 启动即返（error + return，不进循环）。
    #[tokio::test]
    async fn w5_run_loop_unparseable_relay_url_returns_without_loop() {
        let mut p = test_params("w5b-badurl", dead_web_port());
        p.relay_url = ":: not a url ::".to_string();
        // 有界等待：正常应立即返回；挂死即测试超时暴露。
        tokio::time::timeout(Duration::from_secs(5), run_loop(p, fast_timing()))
            .await
            .expect("不可解析 relay_url 必须启动即返，不得进入重试循环");
    }

    /// 中继连接失败（无人监听的死端口）→ Disconnected 状态上报 + 退避重试。
    #[allow(clippy::await_holding_lock)] // BRIDGE_IT_SER 序列化闸有意跨 await 持有
    #[tokio::test]
    async fn w5_run_loop_dial_failure_reports_disconnected_then_backoff() {
        let _ser = BRIDGE_IT_SER.lock();
        drain_kick_permit().await;

        let port = dead_web_port();
        let mut p = test_params("w5b-dead", port);
        p.relay_url = format!("ws://127.0.0.1:{port}");
        let handle = tokio::spawn(run_loop(p, fast_timing()));
        // 留足时间走完「连接失败 → 上报 → 退避 → 再失败」若干轮。
        tokio::time::sleep(Duration::from_millis(300)).await;
        handle.abort();
        let _ = handle.await;
    }
}
