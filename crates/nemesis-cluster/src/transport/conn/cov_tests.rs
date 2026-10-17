// transport/conn.rs 覆盖率补充测试（读循环缓冲淹没丢弃 / 读侧心跳帧跳过 /
// 坏帧 read error warn / 心跳任务拍 / 写错误 warn / close 拒发 + 幂等）。
// 空闲超时断链臂由 tests.rs::test_tcp_conn_idle_timeout_closes_connection
// 专门钉死（T1 后空闲超时在读循环强制执行，本套件 idle_timeout 拉长避免
// 中途断链干扰后续步骤）。
//
// 豁免：写循环 encrypt_frame 失败臂——AES-256-GCM 加密对任意
// 输入数学上不可能失败，死防御。

use super::*;
use socket2::SockRef;
use std::time::Duration;

fn req() -> WireMessage {
    WireMessage::new_request("cov-a", "cov-b", "ping", serde_json::json!({"n": 1}))
}

/// server 端原始写一帧（4B 长度前缀 + JSON body）。
async fn send_frame(server: &mut TokioTcpStream, msg: &WireMessage) {
    let body = msg.to_bytes().unwrap();
    server
        .write_all(&(body.len() as u32).to_be_bytes())
        .await
        .unwrap();
    server.write_all(&body).await.unwrap();
    server.flush().await.unwrap();
}

/// 全链路一把梭：缓冲淹没 → 读侧心跳帧跳过（刷新活跃时间不进队列）→
/// 心跳任务拍 + 写循环 → 超长帧 read error → 对端 RST 写错误 →
/// close 拒发 + 幂等（closed 检查随读循环迭代与 close 收尾覆盖）。
#[tokio::test]
async fn tcpconn_loop_arms_full_sweep() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let (mut server, _) = listener.accept().await.unwrap();

    let config = TcpConnConfig {
        node_id: "cov-node".into(),
        address: addr.to_string(),
        read_buffer_size: 1, // recv chan cap 1 → 淹没必溢出
        send_buffer_size: 16,
        send_timeout: Duration::from_secs(2),
        idle_timeout: Duration::from_secs(60), // 拉长：断链臂归专项测试
        heartbeat_interval: Some(Duration::from_millis(50)),
        auth_token: None,
    };
    let mut conn = TcpConn::new(client_stream, config);
    conn.start().await.unwrap();
    assert!(conn.is_active());
    assert_eq!(conn.node_id(), "cov-node");

    // ① server 连发 3 帧淹没 recv 缓冲（client 不 receive，cap 1 →
    // 第 2 条起 try_send 必败 → dropped 计数增长）。
    let msg = req();
    send_frame(&mut server, &msg).await;
    send_frame(&mut server, &msg).await;
    send_frame(&mut server, &msg).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert!(
        conn.dropped_count() >= 1,
        "缓冲淹没必须产生丢弃计数：{}",
        conn.dropped_count()
    );

    // 取走 chan 里唯一的消息（验证链路确实送达）。
    let got = tokio::time::timeout(Duration::from_secs(2), conn.receive())
        .await
        .unwrap();
    assert_eq!(got.expect("首帧必达").action, "ping");

    // ② 读侧心跳帧（T2）：server 发合法心跳 → read loop parse 成功刷新
    // 活跃时间后必须跳过——不计丢弃、不进 recv channel。
    let dropped_before = conn.dropped_count();
    let hb = WireMessage::new_heartbeat("cov-server");
    send_frame(&mut server, &hb).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    assert_eq!(conn.dropped_count(), dropped_before, "心跳帧不得计为丢弃");
    let nothing = tokio::time::timeout(Duration::from_millis(150), conn.receive()).await;
    assert!(nothing.is_err(), "心跳帧不得进 recv channel");

    // ③ 心跳任务若干拍 + 写循环写出臂（client → server 方向 50ms × 400ms）。
    tokio::time::sleep(Duration::from_millis(400)).await;

    // ④ 超长帧：server 发 0xFFFFFFFF 长度前缀 → read_frame InvalidData
    // （非 UnexpectedEof）→ read error warn + 断链。
    server
        .write_all(&0xFFFF_FFFFu32.to_be_bytes())
        .await
        .unwrap();
    server.flush().await.unwrap();
    tokio::time::sleep(Duration::from_millis(150)).await;

    // ⑤ 写错误：server 带 linger(0) 关闭 → RST → 客户端下一次 write
    // ConnectionReset → write error warn + 断链。
    {
        let std_stream = server.into_std().unwrap();
        SockRef::from(&std_stream)
            .set_linger(Some(Duration::from_secs(0)))
            .unwrap();
        drop(std_stream); // RST
    }
    let _ = conn.send(&msg).await;
    tokio::time::sleep(Duration::from_millis(150)).await;
    let _ = conn.send(&msg).await;
    tokio::time::sleep(Duration::from_millis(150)).await;

    // ⑥ close：拒发 + 幂等（收尾 debug 随 close 覆盖）。
    conn.close();
    assert!(conn.is_closed());
    assert!(!conn.is_active());
    assert!(conn.send(&msg).await.is_err(), "close 后 send 必须被拒");
    conn.close(); // 第二次 no-op
}

/// start() 防重入：already started 臂（361-363 区域）+ 收尾 shutdown。
#[tokio::test]
async fn tcpconn_start_twice_rejected() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let client_stream = TokioTcpStream::connect(addr).await.unwrap();
    let (server_stream, _) = listener.accept().await.unwrap();

    let mut conn = TcpConn::new(
        client_stream,
        TcpConnConfig {
            node_id: "cov-twice".into(),
            address: addr.to_string(),
            ..TcpConnConfig::default()
        },
    );
    conn.start().await.unwrap();
    let err = conn.start().await.unwrap_err();
    assert!(err.contains("already started"), "{err}");
    conn.close();
    let _ = server_stream;
}
