//! http_min 有界重组读测试：真 TCP 进程内（TcpListener + TcpStream），
//! 覆盖单写基线 / 分段重组（BUG-20261126-106 回归锁）/ CL 满足即返回 /
//! 超帽 / EOF 三态 / 无 CL 静默窗 / 墙钟超时。

use super::*;
use tokio::io::AsyncWriteExt;
use tokio::net::{TcpListener, TcpStream};

/// 拉起单连接服务端：accept 一次后跑重组读，把结果回传给测试。
async fn spawn_server(
    cfg: MinReadConfig,
) -> (
    std::net::SocketAddr,
    tokio::task::JoinHandle<Result<Vec<u8>, MinReadError>>,
) {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        read_request_bounded_with(&mut stream, cfg).await
    });
    (addr, handle)
}

/// 客户端：按给定分片写请求（分片间 sleep `gap`），返回流（持有 = 连接保活；drop = EOF）。
async fn connect_and_write(
    addr: std::net::SocketAddr,
    chunks: &[&[u8]],
    gap: Duration,
) -> TcpStream {
    let mut s = TcpStream::connect(addr).await.unwrap();
    for (i, chunk) in chunks.iter().enumerate() {
        if i > 0 {
            tokio::time::sleep(gap).await;
        }
        s.write_all(chunk).await.unwrap();
        s.flush().await.unwrap();
    }
    s
}

/// 组装带 CL 的最小请求字节。
fn request_with_cl(body: &str, extra_headers: &str) -> Vec<u8> {
    format!(
        "POST /webhook/line HTTP/1.1\r\nHost: t\r\n{extra_headers}Content-Length: {}\r\n\r\n{}",
        body.len(),
        body
    )
    .into_bytes()
}

fn short_cfg() -> MinReadConfig {
    MinReadConfig {
        overall_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_millis(200),
        max_bytes: DEFAULT_MAX_BYTES,
    }
}

#[tokio::test]
async fn complete_request_single_write() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let req = request_with_cl("hello", "");
    let _client = connect_and_write(addr, &[&req], Duration::ZERO).await;
    let got = handle.await.unwrap().expect("应重组成功");
    assert_eq!(got, req);
}

/// BUG-20261126-106 回归锁：请求跨 3 个 TCP 分段（>单段 MSS 的真实形态），
/// 单次 read 只见部分请求——重组读必须拿到完整字节。
#[tokio::test]
async fn fragmented_request_reassembled() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let req = request_with_cl(" fragmented body across segments 0123456789 abcdefghij", "");
    let mid = req.len() / 3;
    let mut c1 = mid;
    while (req[c1] == b'\r' || req[c1] == b'\n') && c1 > 0 {
        c1 -= 1;
    }
    let c2 = mid * 2;
    let _client = connect_and_write(
        addr,
        &[&req[..c1], &req[c1..c2], &req[c2..]],
        Duration::from_millis(40),
    )
    .await;
    let got = handle.await.unwrap().expect("分段请求应重组成功");
    assert_eq!(got, req);
}

/// CL 声明总量超帽 → header 解析后立即 Oversize（不等 body）。
#[tokio::test]
async fn oversize_declared_content_length_fails_fast() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let head = b"POST /x HTTP/1.1\r\nHost: t\r\nContent-Length: 1000000\r\n\r\npartial".to_vec();
    let _client = connect_and_write(addr, &[&head], Duration::ZERO).await;
    match handle.await.unwrap() {
        Err(MinReadError::Oversize) => {}
        other => panic!("应 Oversize，实际 {other:?}"),
    }
}

/// 无 CL + 持续灌数据超帽 → 缓冲超帽 Oversize。
#[tokio::test]
async fn buffer_over_cap_without_cl() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let head = b"POST /x HTTP/1.1\r\nHost: t\r\n\r\n".to_vec();
    let flood = vec![b'a'; DEFAULT_MAX_BYTES + 4096];
    let _client = connect_and_write(addr, &[&head, &flood], Duration::ZERO).await;
    match handle.await.unwrap() {
        Err(MinReadError::Oversize) => {}
        other => panic!("应 Oversize，实际 {other:?}"),
    }
}

/// header 未完成即断开 → Eof。
#[tokio::test]
async fn eof_before_headers() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let partial = b"POST /x HTTP/1.1\r\nHost: t\r\n".to_vec();
    drop(connect_and_write(addr, &[&partial], Duration::ZERO).await);
    match handle.await.unwrap() {
        Err(MinReadError::Eof) => {}
        other => panic!("应 Eof，实际 {other:?}"),
    }
}

/// 带 CL 请求 body 未满即断开 → Eof（截断请求不投递）。
#[tokio::test]
async fn cl_body_truncated_by_eof() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let head =
        b"POST /x HTTP/1.1\r\nHost: t\r\nContent-Length: 100\r\n\r\nonly-20-bytes--".to_vec();
    drop(connect_and_write(addr, &[&head], Duration::ZERO).await);
    match handle.await.unwrap() {
        Err(MinReadError::Eof) => {}
        other => panic!("应 Eof，实际 {other:?}"),
    }
}

/// HTTP/1.0 形态：无 CL，EOF 是合法 body 终止符 → Ok(全量)。
#[tokio::test]
async fn no_cl_eof_terminated_http10() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let req = b"POST /x HTTP/1.0\r\nHost: t\r\n\r\nbody-until-eof".to_vec();
    drop(connect_and_write(addr, &[&req], Duration::ZERO).await);
    let got = handle.await.unwrap().expect("EOF 终止的无 CL 请求应成功");
    assert_eq!(got, req.to_vec());
}

/// 无 CL + 连接保活：静默窗耗尽即到齐返回（不等墙钟）。
#[tokio::test]
async fn no_cl_idle_drain_returns_after_quiet() {
    let cfg = MinReadConfig {
        overall_timeout: Duration::from_secs(5),
        idle_timeout: Duration::from_millis(200),
        max_bytes: DEFAULT_MAX_BYTES,
    };
    let (addr, handle) = spawn_server(cfg).await;
    let req = b"POST /x HTTP/1.1\r\nHost: t\r\n\r\nkeepalive-no-cl".to_vec();
    let client = connect_and_write(addr, &[&req], Duration::ZERO).await;
    let started = std::time::Instant::now();
    let got = handle.await.unwrap().expect("静默窗应到齐返回");
    let elapsed = started.elapsed();
    assert_eq!(got, req.to_vec());
    assert!(
        elapsed >= Duration::from_millis(150),
        "应等静默窗，实际 {elapsed:?}"
    );
    assert!(
        elapsed < Duration::from_secs(4),
        "不应等满墙钟，实际 {elapsed:?}"
    );
    drop(client);
}

/// 静默流：墙钟超时（slowloris 有界）。
#[tokio::test]
async fn overall_timeout_on_silent_stream() {
    let cfg = MinReadConfig {
        overall_timeout: Duration::from_millis(200),
        idle_timeout: Duration::from_millis(50),
        max_bytes: DEFAULT_MAX_BYTES,
    };
    let (addr, handle) = spawn_server(cfg).await;
    let _client = connect_and_write(addr, &[], Duration::ZERO).await;
    match handle.await.unwrap() {
        Err(MinReadError::Timeout) => {}
        other => panic!("应 Timeout，实际 {other:?}"),
    }
}

/// 分段 + 无 CL 静默窗：分段到达都计入，静默后到齐。
#[tokio::test]
async fn fragmented_no_cl_idle_reassembled() {
    let (addr, handle) = spawn_server(short_cfg()).await;
    let head = b"POST /x HTTP/1.1\r\nHost: t\r\n\r\n".to_vec();
    let b1 = b"chunk-one-".to_vec();
    let b2 = b"chunk-two".to_vec();
    let _client = connect_and_write(addr, &[&head, &b1, &b2], Duration::from_millis(40)).await;
    let got = handle.await.unwrap().expect("无 CL 分段应重组成功");
    let mut want = head;
    want.extend_from_slice(&b1);
    want.extend_from_slice(&b2);
    assert_eq!(got, want);
}
