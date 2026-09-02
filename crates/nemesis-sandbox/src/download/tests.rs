//! Tests for `download`（Phase 3 覆盖率，2026-08-25）。
//!
//! 手搓最小 HTTP 服务器（tokio TcpListener + 裸响应）代替真 GitHub——
//! checksums 解析、SHA-256 校验/失配、无期望哈希的降级路径全走真 reqwest
//! 客户端栈。

use super::*;
use std::time::Duration;

/// 起 HTTP 服务器：对每个请求回 `status` + body（Content-Length 正确）。
async fn serve(body: &'static str, status: &'static str) -> String {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        loop {
            let (mut s, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let mut buf = vec![0u8; 4096];
            let _ = s.read(&mut buf).await;
            let resp = format!(
                "HTTP/1.1 {status}\r\nContent-Type: text/plain\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = s.write_all(resp.as_bytes()).await;
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        jh.abort();
    });
    format!("http://{addr}/file")
}

#[tokio::test]
async fn fetch_expected_sha256_parses_matching_line_case_insensitive() {
    // 大写哈希 + 多行 + 空行 + 两个空格分隔——标准 sha256sum 布局。
    // 注意：空行/其它文件行放在匹配行**之前**——函数命中即 return，放后面
    // 会被短路掉（continue 臂就盖不到了）。
    let url = serve(
        "\nOTHERHASH  other-file.exe\n\nAAAA1111BBBB2222CCCC3333DDDD4444EEEE5555FFFF6666AAAA7777BBBB8888  Sandboxie-Classic-1.0.exe\n",
        "200 OK",
    )
    .await;
    let h = fetch_expected_sha256(&url, "Sandboxie-Classic-1.0.exe")
        .await
        .unwrap();
    assert_eq!(
        h,
        "aaaa1111bbbb2222cccc3333dddd4444eeee5555ffff6666aaaa7777bbbb8888"
    );
}

#[tokio::test]
async fn fetch_expected_sha256_missing_filename_bails() {
    let url = serve("aaaa  something-else.exe\n", "200 OK").await;
    let err = fetch_expected_sha256(&url, "wanted.exe").await.unwrap_err();
    assert!(format!("{err:#}").contains("not found"), "{err:#}");
}

#[tokio::test]
async fn download_and_verify_matching_hash_writes_file() {
    // R5（2026-08-27）：装 subscriber 让 info!/warn! 的参数行（bytes.len() 等）
    // 真实求值——无 subscriber 时该行 lcov 恒 0（S6 批次钉过的机制）。
    let _log = crate::test_util::capture_logs();
    let payload = b"sandboxie-installer-bytes".to_vec();
    let expected = {
        use sha2::Digest;
        let mut h = Sha256::new();
        h.update(&payload);
        format!("{:x}", h.finalize())
    };
    // leak 成 'static —— serve 需要 'static body；payload 小且测试进程短命。
    let body: &'static str = Box::leak(String::from_utf8(payload).unwrap().into_boxed_str());
    let url = serve(body, "200 OK").await;

    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("nested").join("inst.exe");
    download_and_verify(&url, Some(&expected), &dest)
        .await
        .unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"sandboxie-installer-bytes");
}

#[tokio::test]
async fn download_and_verify_hash_mismatch_bails_without_writing() {
    let body: &'static str = "tampered-bytes";
    let url = serve(body, "200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("inst.exe");
    let err = download_and_verify(
        &url,
        // 64 个 0 —— 必然失配。
        Some(&"0".repeat(64)),
        &dest,
    )
    .await
    .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("SHA-256 mismatch"), "{msg}");
    // 失配时绝不能落盘被篡改的内容。
    assert!(!dest.exists());
}

#[tokio::test]
async fn download_and_verify_without_expected_hash_still_writes() {
    // checksums 拿不到时降级为不校验直落盘（warn 路径）。
    let _log = crate::test_util::capture_logs(); // R5：同上，warn 参数行确定性求值
    let body: &'static str = "unverified-content";
    let url = serve(body, "200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("inst.exe");
    download_and_verify(&url, None, &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"unverified-content");
}

#[tokio::test]
async fn download_release_checksums_unavailable_proceeds_unverified() {
    // checksums 404 → expected=None → installer 仍下载成功（warn 降级链）。
    let checksums_url = serve("nope", "404 Not Found").await;
    let inst_body: &'static str = "installer-payload";
    let inst_url = serve(inst_body, "200 OK").await;
    let dir = tempfile::tempdir().unwrap();
    let path = download_release(&inst_url, &checksums_url, "Sandboxie.exe", dir.path())
        .await
        .unwrap();
    assert_eq!(path, dir.path().join("Sandboxie.exe"));
    assert_eq!(std::fs::read(&path).unwrap(), b"installer-payload");
}

// ---------------------------------------------------------------------------
// S6 覆盖率批次（quality-hardening goal 2026-08-25）：装 thread-local
// subscriber 让 tracing 宏参数行（info!/warn!）真实求值。
// ---------------------------------------------------------------------------

#[tokio::test]
async fn download_logs_under_subscriber_macro_args_evaluated() {
    let _log = crate::test_util::capture_logs();
    // 成功 + 校验通过 → info! 参数行
    let body = "subscriber-verified-payload";
    let sum: String = {
        use sha2::Digest;
        let h = sha2::Sha256::digest(body.as_bytes());
        h.iter().map(|b| format!("{b:02x}")).collect()
    };
    // serve 的 body 就是下载内容本身——期望哈希必须对同一个 body 算。
    let url = serve(Box::leak(body.to_string().into_boxed_str()), "200 OK").await;
    let tmp = tempfile::tempdir().unwrap();
    let dest = tmp.path().join("sub.bin");
    download_and_verify(&url, Some(&sum), &dest).await.unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), body.as_bytes());

    // 无期望哈希 → warn! 参数行
    let url2 = serve("no-hash-body", "200 OK").await;
    let dest2 = tmp.path().join("sub2.bin");
    download_and_verify(&url2, None, &dest2).await.unwrap();
    assert_eq!(std::fs::read(&dest2).unwrap(), b"no-hash-body");
}
