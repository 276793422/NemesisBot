// rpc/server.rs 覆盖率补充测试（响应回写失败臂 369：对端 RST 后写循环
// 已断，第二条响应 send 撞上已关闭的 send 通道）。
//
// 豁免：285-286（accept 循环的 Err 臂——tokio 监听器的 accept 错误在
// 正常回环环境下不可确定性构造，属环境防御面）；329-330（TcpConn::start
// 失败臂——start 只在重复 start/已关闭/连接已被取走时失败，服务器侧
// 每条连接都是全新流，数学上不可能触达，死防御）。

use super::*;
use socket2::SockRef;
use std::time::Duration;
use tokio::io::AsyncWriteExt;

fn request_wire(id: &str) -> WireMessage {
    WireMessage {
        version: "1.0".into(),
        id: id.into(),
        msg_type: "request".into(),
        from: "node-a".into(),
        to: "node-b".into(),
        action: "CovSlow".into(),
        payload: serde_json::json!({}),
        timestamp: chrono::Local::now().timestamp(),
        error: String::new(),
    }
}

/// 响应回写失败臂（369）：对端在服务端回写前 RST → 首条响应的写循环
/// 写失败断链（551），第二条响应的 conn.send 撞上已关闭通道 → Err 臂。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn response_send_failure_after_peer_rst() {
    let server = RpcServer::new(RpcServerConfig {
        bind_address: "0.0.0.0:0".into(),
        ..Default::default()
    });
    // 慢 handler：拖住首条响应的回写，确保 RST 先于写到达。
    server.register_handler(
        "CovSlow",
        Box::new(|_| {
            std::thread::sleep(Duration::from_millis(150));
            Ok(serde_json::json!({"slow": true}))
        }),
    );
    server.start().await.unwrap();
    let addr = format!("127.0.0.1:{}", server.port());

    let mut stream = tokio::net::TcpStream::connect(&addr).await.unwrap();
    let write_frame = |id: &str| {
        let json = serde_json::to_vec(&request_wire(id)).unwrap();
        let mut buf = (json.len() as u32).to_be_bytes().to_vec();
        buf.extend_from_slice(&json);
        buf
    };
    // ① req#1 → 服务端读走并进入慢 handler。
    stream.write_all(&write_frame("r1")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    // ② req#2 → 读循环在 handler 睡眠期间把它收进 recv 通道。
    stream.write_all(&write_frame("r2")).await.unwrap();
    tokio::time::sleep(Duration::from_millis(60)).await;
    // ③ linger(0) 丢弃 → RST；此后服务端 resp#1 写必败、resp#2 send 撞死通道。
    {
        let std_stream = stream.into_std().unwrap();
        SockRef::from(&std_stream)
            .set_linger(Some(Duration::from_secs(0)))
            .unwrap();
        drop(std_stream);
    }

    // 等服务端把两条响应路径都走完（2×150ms handler + 余量）。
    tokio::time::sleep(Duration::from_millis(700)).await;

    server.stop().unwrap();
}
