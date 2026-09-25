// download.rs 覆盖率补充（wave 5）：fetch_expected_sha256 解析/未命中 bail、
// download_and_verify 校验成功 / 无哈希 warn / 哈希不匹配 bail。
//
// 纪律：不发外网——reqwest 打到本机一次性 TCP HTTP 服务（127.0.0.1 随机
// 端口，单连接单响应后关闭）。

use super::*;
use sha2::{Digest, Sha256};
use std::io::Write as _;

/// 起一个一次性 HTTP 服务（std 线程，独立于测试的 tokio runtime）：接受一条
/// 连接、读走请求头、回 200 + body、graceful 关闭。
async fn serve_once(body: &'static [u8]) -> String {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap();
    std::thread::spawn(move || {
        let (mut sock, _) = listener.accept().expect("one connection");
        let mut buf = [0u8; 8192];
        let mut filled = 0;
        // 读到请求头结束（\r\n\r\n）为止，避免半请求时抢答。
        loop {
            let n = std::io::Read::read(&mut sock, &mut buf[filled..]).expect("read request");
            filled += n;
            if n == 0 || buf[..filled].windows(4).any(|w| w == b"\r\n\r\n") {
                break;
            }
        }
        let head = format!(
            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
            body.len()
        );
        let _ = sock.write_all(head.as_bytes());
        let _ = sock.write_all(body);
        let _ = sock.shutdown(std::net::Shutdown::Both);
    });
    format!("http://{addr}/fixture")
}

#[tokio::test]
async fn fetch_expected_sha256_parses_hash_line() {
    let url = serve_once(b"deadbeefcafe  sandbox.exe\nother  unused.exe\n").await;
    let got = fetch_expected_sha256(&url, "sandbox.exe")
        .await
        .expect("hash line present");
    assert_eq!(got, "deadbeefcafe");
}

#[tokio::test]
async fn fetch_expected_sha256_bails_when_filename_absent() {
    let url = serve_once(b"deadbeef  other.exe\n").await;
    let err = fetch_expected_sha256(&url, "sandbox.exe")
        .await
        .expect_err("filename missing from checksums");
    assert!(
        err.to_string().contains("not found in checksums file"),
        "{err}"
    );
}

#[tokio::test]
async fn download_and_verify_succeeds_with_matching_hash() {
    let payload: &'static [u8] = b"sandboxie installer fixture bytes";
    let mut h = Sha256::new();
    h.update(payload);
    let expected = format!("{:x}", h.finalize());
    let url = serve_once(payload).await;

    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("installer.exe");
    download_and_verify(&url, Some(&expected), &dest)
        .await
        .expect("matching hash downloads fine");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}

#[tokio::test]
async fn download_and_verify_warns_without_expected_hash() {
    let payload: &'static [u8] = b"unverified bytes";
    let url = serve_once(payload).await;
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("installer.exe");
    download_and_verify(&url, None, &dest)
        .await
        .expect("no expected hash = download without verification");
    assert_eq!(std::fs::read(&dest).unwrap(), payload);
}

#[tokio::test]
async fn download_and_verify_bails_on_hash_mismatch() {
    let payload: &'static [u8] = b"tampered bytes";
    let url = serve_once(payload).await;
    let dest_dir = tempfile::tempdir().unwrap();
    let dest = dest_dir.path().join("installer.exe");
    let err = download_and_verify(&url, Some(&"0".repeat(64)), &dest)
        .await
        .expect_err("hash mismatch must bail");
    assert!(err.to_string().contains("SHA-256 mismatch"), "{err}");
    assert!(!dest.exists(), "mismatched payload must not be written");
}
