//! R5（2026-10-24 第四次外评）回归锁：pipe roundtrip 无请求 ID——外部超时
//! 取消交换后，迟到应答滞留管道缓冲，被下一请求冒领（张冠李戴）。修复 =
//! 交换窗标志（取消路径滞留）+ `LinkUnreliable` 诚实拒绝。外评探针
//! `review_oct24_timeout_assigns_previous_read_reply_to_next_address` 的
//! 安全反转。

use super::*;

/// read_mem(0x1000) 超时（对端 100ms 后才回 "A"）→ 下一条 read_mem(0x2000)
/// 必须诚实报 `LinkUnreliable`，决不能收到 "A"（修复前形态）。
#[tokio::test]
async fn r5_timeout_poisons_link_no_stale_reply_attribution() {
    let name = format!(
        r"\\.\pipe\nemesis-r5-lock-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_nanos()
    );
    let server = create_server(&name).unwrap();
    let token = "1234567890abcdef1234567890abcdef";
    let client = tokio::spawn(async move {
        let mut stream = tokio::net::windows::named_pipe::ClientOptions::new()
            .open(&name)
            .unwrap();
        let hello = format!(
            "{{\"proto\":1,\"token\":\"{token}\",\"arch\":\"x64\",\"pid\":{}}}",
            std::process::id()
        );
        stream.write_u32_le(hello.len() as u32).await.unwrap();
        stream.write_all(hello.as_bytes()).await.unwrap();
        // 第 1 条请求（read_mem 0x1000）——迟到 100ms 才应答 "A"。
        let n = stream.read_u32_le().await.unwrap();
        let mut request = vec![0; n as usize];
        stream.read_exact(&mut request).await.unwrap();
        tokio::time::sleep(Duration::from_millis(100)).await;
        let first = br#"{"ok":true,"data":"QQ=="}"#; // base64("A")
        stream.write_u32_le(first.len() as u32).await.unwrap();
        stream.write_all(first).await.unwrap();
        // 第 2 条请求（read_mem 0x2000）——立即应答 "B"。
        let n = stream.read_u32_le().await.unwrap();
        let mut request = vec![0; n as usize];
        stream.read_exact(&mut request).await.unwrap();
        let second = br#"{"ok":true,"data":"Qg=="}"#; // base64("B")
        stream.write_u32_le(second.len() as u32).await.unwrap();
        stream.write_all(second).await.unwrap();
    });
    let mut link = finish_handshake(server, token, std::process::id(), "x64")
        .await
        .unwrap();

    // 第一条：20ms 预算 < 对端 100ms 迟到——外部超时取消交换（毒化窗口）。
    let first =
        tokio::time::timeout(Duration::from_millis(20), read_mem(&mut link, 0x1000, 1)).await;
    assert!(first.is_err(), "第一条必须超时（对端迟到 100ms）");

    // 第二条：修复前 = 冒领迟到的 "A"（Ok(b"A")）；修复后 = 诚实拒绝。
    let second = tokio::time::timeout(Duration::from_secs(2), read_mem(&mut link, 0x2000, 1))
        .await
        .expect("read_mem 应立即返回（毒化拒绝或消费应答）");
    assert!(
        matches!(second, Err(PipeError::LinkUnreliable)),
        "超时后的链路必须诚实拒绝（结果未知，勿冒领迟到应答），got: {second:?}"
    );

    // 毒化语义下服务器不会再写第二条请求——fake client 永远等不到，
    // abort 收尾（不 await：等它 = 等一个永不发生的帧）。
    client.abort();
    let _ = client.await;
}
