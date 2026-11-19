//! BUG-20261119-184 回归锁：安全中间件网络出口总超时行为锁。
//!
//! 挂起服务器（accept 后收下请求但零字节响应）+ 短超时 client 实跑超时闸
//! ——修复前 builder 只挂重定向策略无 timeout，慢/恶意服务器让 send()
//! 无限挂死；修复后 ~1s 内诚实报错。测试缝 = `middleware_http_client_with_timeout`
//! （与 BUG-20261119-183 的 impl 缝同型；生产四出口经 `middleware_http_client`
//! 以 const 30s 委托同款 builder，编译期保证同源）。

use std::io::Read as _;
use std::time::Duration;

/// 挂起 HTTP 服务器：accept 后收下请求字节但**不回任何响应**、连接挂住
/// （独立 OS 线程阻塞 accept，不依赖 tokio::net feature）。
fn spawn_stalled_server() -> std::net::SocketAddr {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind local server");
    let addr = listener.local_addr().expect("local_addr");
    std::thread::spawn(move || {
        if let Ok((mut sock, _)) = listener.accept() {
            let mut buf = [0u8; 1024];
            let _ = sock.read(&mut buf); // 收请求不响应
            std::thread::sleep(Duration::from_secs(60)); // 连接挂住到测试窗外
        }
    });
    addr
}

#[tokio::test]
async fn middleware_http_client_timeout_fires_on_stalled_server() {
    let addr = spawn_stalled_server();
    let client =
        super::SecureNetworkWrapper::middleware_http_client_with_timeout(Duration::from_secs(1));
    let start = std::time::Instant::now();
    let result = client.get(format!("http://{addr}/")).send().await;
    let elapsed = start.elapsed();
    assert!(result.is_err(), "stalled server must yield an error");
    assert!(
        elapsed < Duration::from_secs(15),
        "timeout must fire near the 1s budget, took {elapsed:?}"
    );
    assert!(
        elapsed >= Duration::from_millis(400),
        "fired suspiciously early (not the timeout path): {elapsed:?}"
    );
}
