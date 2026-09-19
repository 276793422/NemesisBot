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
async fn test_conn_pump_roundtrip() {
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
async fn test_fin_true_does_not_shutdown_write_half() {
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
async fn test_dial_failure_sends_conn_close() {
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
async fn test_access_check_roundtrip() {
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
async fn test_welcome_rejected_then_retry() {
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
async fn test_dead_server_detection_and_reconnect() {
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
async fn test_heartbeat_pong_keeps_session_alive() {
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
