//! [`http_cap`] 测试：纯函数「中途停止」语义（计数迭代器）+ wiremock 真链路。

use super::accumulate_capped;

/// 计数迭代器：记录 next() 被调用次数（钉「超限后不再消费后续块」）。
struct CountingChunks<'a> {
    iter: std::vec::IntoIter<&'a [u8]>,
    polled: usize,
}

impl<'a> Iterator for CountingChunks<'a> {
    type Item = &'a [u8];

    fn next(&mut self) -> Option<Self::Item> {
        self.polled += 1;
        self.iter.next()
    }
}

#[test]
fn test_accumulate_ok() {
    let out = accumulate_capped([b"hello".as_slice(), b" world"].into_iter(), 100).unwrap();
    assert_eq!(out, b"hello world");
}

#[test]
fn test_accumulate_exact_cap_is_legal() {
    // 严格大于判定：total == cap 合法（cap 是上限不是 exclusive 排斥）。
    let out = accumulate_capped([b"aaaaa".as_slice()].into_iter(), 5).unwrap();
    assert_eq!(out, b"aaaaa");
}

#[test]
fn test_accumulate_over_cap_errors() {
    let err = accumulate_capped([b"aaaaa".as_slice(), b"bb"].into_iter(), 5).unwrap_err();
    assert!(err.contains("exceeds limit"), "msg: {err}");
    assert!(err.contains("aborted mid-read"), "msg: {err}");
}

#[test]
fn test_accumulate_stops_consuming_after_over_limit() {
    // 「中途停止」核心语义：第 2 块超限 → 迭代器不再被拉取第 3 块。
    // 「读完全量才查」的旧形态会把 4 块全部消费完，polled == 4 必红。
    let chunks: Vec<&[u8]> = vec![b"aa", b"bbbb", b"cc", b"dd"];
    let mut iter = CountingChunks {
        iter: chunks.into_iter(),
        polled: 0,
    };
    let err = accumulate_capped(&mut iter, 4).unwrap_err();
    assert!(err.contains("exceeds limit"));
    // aa(2) + bbbb(4) = 6 > 4 在第二块断开：polled = 2（读第 2 块的那次）。
    assert_eq!(iter.polled, 2, "must stop consuming after over-limit chunk");
}

#[test]
fn test_accumulate_empty_input() {
    let out = accumulate_capped(std::iter::empty::<&[u8]>(), 10).unwrap();
    assert!(out.is_empty());
}

#[test]
fn test_accumulate_cap_zero_rejects_any_content() {
    let err = accumulate_capped([b"x".as_slice()].into_iter(), 0).unwrap_err();
    assert!(err.contains("exceeds limit"));
    let out = accumulate_capped(std::iter::empty::<&[u8]>(), 0).unwrap();
    assert!(out.is_empty());
}

// ---- wiremock 真链路（reqwest 在 dev-dependencies，无 http feature 也可跑）----

#[tokio::test]
async fn test_read_body_capped_wire_ok() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/f"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;
    let resp = reqwest::get(format!("{}/f", server.uri())).await.unwrap();
    let out = super::read_body_capped(resp, 100).await.unwrap();
    assert_eq!(out, b"hello");
}

#[tokio::test]
async fn test_read_body_capped_wire_over_limit() {
    let server = wiremock::MockServer::start().await;
    let big = "x".repeat(100_000);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/big"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_string(big))
        .mount(&server)
        .await;
    let resp = reqwest::get(format!("{}/big", server.uri())).await.unwrap();
    let err = super::read_body_capped(resp, 1024).await.unwrap_err();
    assert!(err.contains("exceeds limit"), "msg: {err}");
    assert!(err.contains("aborted mid-read"), "msg: {err}");
}

// ---- 传输级「中途断开」观测（钉 [`super::read_body_capped`] 本体）----
//
// wiremock 只能观测最终错误消息，观测不了「读了多少」——若未来有人把
// 实现改回「全量读入后才核对」（消息形态不变），wire 测试照样绿。裸 TCP
// 服务器发 64MB body、客户端 cap=1MB：新形态在读入中途停止消费 chunk 并
// drop response（连接关闭），服务器实际写出的字数远小于 body 总量；旧
// 回归形态下客户端会把 64MB 全部拉完，服务器计数 = 全量，本测试必红。

#[tokio::test]
async fn test_read_body_capped_aborts_transport_mid_read() {
    const BODY: usize = 64 * 1024 * 1024;
    const CAP: u64 = 1024 * 1024;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        // 先读掉请求头（拿到 \r\n\r\n 为止），再写响应——规范 HTTP 时序。
        let mut req = vec![0u8; 8192];
        let mut got = 0usize;
        loop {
            let n = sock.read(&mut req[got..]).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            got += n;
            if req[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head =
            format!("HTTP/1.1 200 OK\r\ncontent-length: {BODY}\r\nconnection: close\r\n\r\n");
        if sock.write_all(head.as_bytes()).await.is_err() {
            return 0usize;
        }
        let chunk = vec![b'x'; 64 * 1024];
        let mut sent = 0usize;
        while sent < BODY {
            let n = chunk.len().min(BODY - sent);
            match sock.write_all(&chunk[..n]).await {
                Ok(()) => sent += n,
                // 客户端中途断开 → 写失败（broken pipe / connection reset）。
                Err(_) => break,
            }
        }
        sent
    });

    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    let err = super::read_body_capped(resp, CAP).await.unwrap_err();
    assert!(err.contains("exceeds limit"), "msg: {err}");

    let sent = tokio::time::timeout(std::time::Duration::from_secs(30), server)
        .await
        .expect("server task must terminate")
        .unwrap();
    // 新形态：客户端在 ~cap 处停读断开，服务器顶多再灌满 socket 缓冲
    // （loopback 通常 ≤ 几 MB），8MB 余量足够宽；旧形态：sent == 64MB。
    assert!(
        (sent as u64) < CAP + 8 * 1024 * 1024,
        "server sent {sent} bytes; client must stop consuming near cap ({CAP}), not drain all {BODY}"
    );
}

// ---- read_error_body（BUG-20261113-164：错误体有帽读取）----

#[tokio::test]
async fn test_read_error_body_small_body_untouched() {
    let server = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/e"))
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string("{\"error\":\"boom\"}"))
        .mount(&server)
        .await;
    let resp = reqwest::get(format!("{}/e", server.uri())).await.unwrap();
    let out = super::read_error_body(resp).await;
    assert_eq!(out, "{\"error\":\"boom\"}");
    assert!(!out.contains("truncated"));
}

#[tokio::test]
async fn test_read_error_body_over_cap_truncates_with_note() {
    let server = wiremock::MockServer::start().await;
    let big = "y".repeat(512 * 1024);
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .and(wiremock::matchers::path("/big"))
        .respond_with(wiremock::ResponseTemplate::new(500).set_body_string(big))
        .mount(&server)
        .await;
    let resp = reqwest::get(format!("{}/big", server.uri())).await.unwrap();
    let out = super::read_error_body(resp).await;
    assert!(out.contains("…[error body truncated]"), "must carry note");
    // 帽 256KB + 注记；512KB body 不得全量进内存。
    assert!(
        out.len() < 300 * 1024,
        "capped length must stay near cap, got {}",
        out.len()
    );
    assert!(out.starts_with('y'), "prefix must be preserved");
}

#[tokio::test]
async fn test_read_error_body_aborts_transport_mid_read() {
    // 与 read_body_capped 的传输级测试同形：64MB body + 客户端帽 256KB，
    // 服务器实际写出必须远小于全量（客户端帽化停读断开）。
    const BODY: usize = 64 * 1024 * 1024;

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let server = tokio::spawn(async move {
        let (mut sock, _) = listener.accept().await.unwrap();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let mut req = vec![0u8; 8192];
        let mut got = 0usize;
        loop {
            let n = sock.read(&mut req[got..]).await.unwrap_or(0);
            if n == 0 {
                break;
            }
            got += n;
            if req[..got].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head = format!(
            "HTTP/1.1 500 Internal Server Error\r\ncontent-length: {BODY}\r\nconnection: close\r\n\r\n"
        );
        if sock.write_all(head.as_bytes()).await.is_err() {
            return 0usize;
        }
        let chunk = vec![b'z'; 64 * 1024];
        let mut sent = 0usize;
        while sent < BODY {
            let n = chunk.len().min(BODY - sent);
            match sock.write_all(&chunk[..n]).await {
                Ok(()) => sent += n,
                Err(_) => break,
            }
        }
        sent
    });

    let resp = reqwest::get(format!("http://{addr}/")).await.unwrap();
    let out = super::read_error_body(resp).await;
    assert!(out.contains("…[error body truncated]"), "must carry note");

    let sent = tokio::time::timeout(std::time::Duration::from_secs(30), server)
        .await
        .expect("server task must terminate")
        .unwrap();
    const CAP: usize = 256 * 1024;
    assert!(
        sent < CAP + 8 * 1024 * 1024,
        "server sent {sent} bytes; client must stop consuming near cap ({CAP}), not drain all {BODY}"
    );
}
