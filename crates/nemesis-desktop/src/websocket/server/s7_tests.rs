//! S7 冲刺覆盖测试：websocket/server.rs 的升级失败分支、连接 RST 后的
//! 读/写错误分支、call_child 的发送失败 / pending 被 stop 清掉 / 超时
//! 三种错误出口。
//!
//! rogue client 模式：用 tungstenite 直连本地服务器并发 auth，之后故意
//! 制造错误条件。全部走 127.0.0.1 随机端口。

use std::sync::Arc;
use std::time::Duration;

use futures_util::SinkExt;
use tokio::net::TcpStream;
use tokio_tungstenite::tungstenite::Message as WsMessage;
use tokio_tungstenite::{connect_async, MaybeTlsStream, WebSocketStream};

use super::*;

/// 连上服务器并发送 auth，返回活跃的 rogue WebSocket。
async fn rogue_connect(port: u16, key: &str) -> WebSocketStream<MaybeTlsStream<TcpStream>> {
    let (mut ws, _) = connect_async(format!("ws://127.0.0.1:{}/{}", port, key))
        .await
        .unwrap();
    ws.send(WsMessage::Text(
        serde_json::json!({"type": "auth", "key": key}).to_string().into(),
    ))
    .await
    .unwrap();
    ws
}

/// 轮询等待 child_id 注册进服务器连接表。
async fn wait_registered(server: &WebSocketServer, id: &str) {
    for _ in 0..150 {
        if server.get_connection(id).is_some() {
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("connection {} never registered", id);
}

async fn make_server() -> (Arc<WebSocketServer>, u16, Arc<KeyGenerator>) {
    let key_gen = Arc::new(KeyGenerator::new());
    let server = Arc::new(WebSocketServer::new(key_gen.clone()));
    let port = server.start().await.unwrap();
    (server, port, key_gen)
}

/// 发原始垃圾字节（非 HTTP 升级请求），accept_async 必须升级失败并
/// 只打日志返回，服务器继续存活。
#[tokio::test]
async fn s7_non_websocket_connection_fails_upgrade() {
    let (server, port, _) = make_server().await;

    let mut sock = tokio::net::TcpStream::connect(("127.0.0.1", port))
        .await
        .unwrap();
    use tokio::io::AsyncWriteExt;
    sock.write_all(b"garbage not http\r\n\r\n").await.unwrap();
    sock.shutdown().await.unwrap();
    drop(sock);

    tokio::time::sleep(Duration::from_millis(300)).await;
    // 服务器不受影响。
    assert_eq!(server.get_port(), port);
    server.stop();
}

/// 对端 RST 后：写循环 send 报错退出（write error 分支），读循环拿到
/// Err 退出（read error 分支）并清理连接表。
#[tokio::test]
async fn s7_reset_connection_produces_write_and_read_errors() {
    let (server, port, key_gen) = make_server().await;
    let key = key_gen.generate("s7-rst", 1);
    let ws = rogue_connect(port, &key).await;
    wait_registered(&server, "s7-rst").await;

    // SO_LINGER=0 + drop -> RST（deprecated 警告是 tokio 对生产代码的
    // 提醒；测试里阻塞-on-drop 的风险可控且正是我们要的效果）。
    #[allow(deprecated)]
    if let MaybeTlsStream::Plain(tcp) = ws.get_ref() {
        let _ = tcp.set_linger(Some(Duration::ZERO));
    }
    drop(ws);

    // 立刻开始轮询发通知：写错误分支要求消息在连接被读循环清理之前
    // 进入写循环（先 sleep 再发只会撞 "connection not found"）。
    let mut gone = false;
    for _ in 0..150 {
        let _ = server.send_notification("s7-rst", "poke", serde_json::json!({}));
        if server.get_connection("s7-rst").is_none() {
            gone = true;
            break;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    assert!(gone, "connection should be removed after the peer reset");
    server.stop();
}

/// 连接被标记 closed 后 call_child 在发送一步失败，且 pending 项被清理。
#[tokio::test]
async fn s7_call_child_on_closed_connection_returns_send_error() {
    let (server, port, key_gen) = make_server().await;
    let key = key_gen.generate("s7-closed", 2);
    let _ws = rogue_connect(port, &key).await;
    wait_registered(&server, "s7-closed").await;

    let conn = server.get_connection("s7-closed").unwrap();
    conn.lock().await.close(); // closed 标志 -> send() 返回 Err

    let err = server
        .call_child("s7-closed", "some.method", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(
        matches!(err, WsServerError::Other(ref s) if s.contains("connection closed")),
        "unexpected error: {}",
        err
    );
    server.stop();
}

/// call_child 挂起等待期间 stop() 清空 pending -> oneshot tx 被 drop ->
/// call 返回 "response channel dropped"。
#[tokio::test]
async fn s7_call_child_pending_dropped_by_stop_errors() {
    let (server, port, key_gen) = make_server().await;
    let key = key_gen.generate("s7-drop", 3);
    let _ws = rogue_connect(port, &key).await;
    wait_registered(&server, "s7-drop").await;

    let s2 = server.clone();
    let handle = tokio::spawn(async move {
        s2.call_child("s7-drop", "never.answered", serde_json::json!({}))
            .await
    });
    // 等 call 把请求发出去并注册 pending。
    tokio::time::sleep(Duration::from_millis(200)).await;

    server.stop(); // 清 pending -> tx drop
    let result = tokio::time::timeout(Duration::from_secs(3), handle)
        .await
        .expect("call should finish promptly after stop()")
        .unwrap();
    match result {
        Err(WsServerError::Other(ref s)) if s.contains("response channel dropped") => {}
        other => panic!(
            "unexpected result: {:?}",
            other.map(|_| "<response message>")
        ),
    }
}

/// 对端永不回复时 call_child 走 30s 超时出口（paused 时钟立即快进）。
#[tokio::test(start_paused = true)]
async fn s7_call_child_times_out() {
    let (server, port, key_gen) = make_server().await;
    let key = key_gen.generate("s7-slow", 4);
    let _ws = rogue_connect(port, &key).await;
    wait_registered(&server, "s7-slow").await;

    let err = server
        .call_child("s7-slow", "never.answered", serde_json::json!({}))
        .await
        .unwrap_err();
    assert!(matches!(err, WsServerError::CallTimeout), "got: {}", err);
    server.stop();
}
