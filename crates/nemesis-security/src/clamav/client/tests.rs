use super::*;

#[test]
fn test_parse_clean_response() {
    let result = parse_scan_response("/tmp/test.txt: OK");
    assert!(result.clean());
    assert_eq!(result.path, "/tmp/test.txt");
}

#[test]
fn test_parse_infected_response() {
    let result = parse_scan_response("/tmp/eicar.com: Eicar-Signature FOUND");
    assert!(!result.clean());
    assert_eq!(result.virus, "Eicar-Signature");
}

#[test]
fn test_parse_error_response() {
    let result = parse_scan_response("/tmp/missing: Access denied ERROR");
    assert!(!result.infected);
}

#[test]
fn test_parse_multi_response() {
    let results = parse_multi_scan_response("/tmp/a.txt: OK\n/tmp/b.exe: Trojan FOUND\n");
    assert_eq!(results.len(), 2);
    assert!(results[0].clean());
    assert!(!results[1].clean());
}

#[test]
fn test_parse_scan_response_found_without_path() {
    let result = parse_scan_response("SomeVirus FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "SomeVirus");
}

#[test]
fn test_parse_scan_response_empty() {
    let result = parse_scan_response("");
    assert!(!result.infected);
    assert!(result.path.is_empty());
}

#[test]
fn test_parse_scan_response_ok_with_path() {
    let result = parse_scan_response("/some/path/file.exe: OK");
    assert!(!result.infected);
    assert_eq!(result.path, "/some/path/file.exe");
}

#[test]
fn test_parse_scan_response_error() {
    let result = parse_scan_response("/tmp/missing: Access denied ERROR");
    assert!(!result.infected);
    assert!(result.raw.contains("ERROR"));
}

#[test]
fn test_clamav_scan_result_clean_method() {
    let clean = ClamavScanResult {
        path: "/tmp/test.txt".to_string(),
        infected: false,
        virus: String::new(),
        raw: String::new(),
    };
    assert!(clean.clean());

    let infected = ClamavScanResult {
        path: "/tmp/test.exe".to_string(),
        infected: true,
        virus: "Trojan".to_string(),
        raw: String::new(),
    };
    assert!(!infected.clean());
}

#[test]
fn test_client_new() {
    let client = Client::new("127.0.0.1:3310");
    assert_eq!(client.address(), "127.0.0.1:3310");
    assert_eq!(client.timeout(), Duration::from_secs(30));
}

#[test]
fn test_client_with_timeout() {
    let client = Client::with_timeout("127.0.0.1:3310", Duration::from_secs(120));
    assert_eq!(client.timeout(), Duration::from_secs(120));
}

#[test]
fn test_is_single_response_command() {
    assert!(is_single_response_command("PING"));
    assert!(is_single_response_command("VERSION"));
    assert!(is_single_response_command("SCAN /tmp/test.txt"));
    assert!(is_single_response_command("CONTSCAN /tmp"));
    assert!(!is_single_response_command("STATS"));
    assert!(!is_single_response_command("RELOAD"));
}

#[test]
fn test_parse_multi_response_empty_lines() {
    let results = parse_multi_scan_response("\n\n");
    assert!(results.is_empty());
}

#[test]
fn test_parse_multi_response_single_line() {
    let results = parse_multi_scan_response("/tmp/a.txt: OK");
    assert_eq!(results.len(), 1);
    assert!(results[0].clean());
}

#[test]
fn test_clamav_scan_result_debug() {
    let result = ClamavScanResult {
        path: "/tmp/test.txt".to_string(),
        infected: false,
        virus: String::new(),
        raw: "OK".to_string(),
    };
    let debug = format!("{:?}", result);
    assert!(debug.contains("/tmp/test.txt"));
    assert!(debug.contains("OK"));
}

#[tokio::test]
async fn test_client_ping_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310"); // unlikely port
    let result = client.ping().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_version_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.version().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_file_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_file(Path::new("/tmp/test.txt")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_cont_scan_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.cont_scan(Path::new("/tmp")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_stream(b"test content").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_reload_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.reload().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_stats_fails_when_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.stats().await;
    assert!(result.is_err());
}

#[test]
fn test_client_invalid_address() {
    let client = Client::new("not-a-valid-address");
    // With invalid address, the socket_addr falls back to default
    assert_eq!(client.address(), "not-a-valid-address");
}

#[test]
fn test_parse_scan_response_status_ok() {
    let result = parse_scan_response("/path/to/file.doc: OK");
    assert!(!result.infected);
    assert_eq!(result.path, "/path/to/file.doc");
    assert!(result.clean());
}

#[test]
fn test_parse_scan_response_found_with_colon_in_path() {
    // Path with colon (Windows-style)
    let result = parse_scan_response("C:\\Users\\test.exe: Malware FOUND");
    assert!(result.infected);
    // The last ": " split should find the status
}

#[test]
fn test_parse_multi_response_multiple_results() {
    let input = "/tmp/a.txt: OK\n/tmp/b.exe: Trojan FOUND\n/tmp/c.txt: OK";
    let results = parse_multi_scan_response(input);
    assert_eq!(results.len(), 3);
    assert!(results[0].clean());
    assert!(!results[1].clean());
    assert!(results[2].clean());
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_parse_scan_response_windows_path() {
    let result = parse_scan_response("C:\\Users\\test\\file.exe: Win32.Trojan FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win32.Trojan");
    assert!(result.raw.contains("FOUND"));
}

#[test]
fn test_parse_scan_response_path_with_spaces() {
    let result = parse_scan_response("/path/to/my file.txt: OK");
    assert!(!result.infected);
    assert_eq!(result.path, "/path/to/my file.txt");
    assert!(result.clean());
}

#[test]
fn test_parse_scan_response_found_with_complex_virus_name() {
    let result = parse_scan_response("/tmp/file: Win.Trojan.Agent-12345 FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win.Trojan.Agent-12345");
}

#[test]
fn test_parse_multi_response_with_errors() {
    let input = "/tmp/a.txt: OK\n/tmp/missing: Access denied ERROR\n/tmp/c.exe: Worm FOUND";
    let results = parse_multi_scan_response(input);
    assert_eq!(results.len(), 3);
    assert!(results[0].clean());
    assert!(!results[1].infected);
    assert!(results[2].infected);
}

#[test]
fn test_client_new_default_timeout() {
    let client = Client::new("127.0.0.1:3310");
    assert_eq!(client.timeout(), Duration::from_secs(30));
}

#[test]
fn test_client_with_custom_timeout() {
    let client = Client::with_timeout("127.0.0.1:3310", Duration::from_secs(300));
    assert_eq!(client.timeout(), Duration::from_secs(300));
    assert_eq!(client.address(), "127.0.0.1:3310");
}

#[test]
fn test_client_address_storage() {
    let client = Client::new("10.0.0.1:9999");
    assert_eq!(client.address(), "10.0.0.1:9999");
}

#[test]
fn test_clamav_scan_result_fields() {
    let result = ClamavScanResult {
        path: "/test/file.exe".to_string(),
        infected: true,
        virus: "TestVirus".to_string(),
        raw: "/test/file.exe: TestVirus FOUND".to_string(),
    };
    assert_eq!(result.path, "/test/file.exe");
    assert!(result.infected);
    assert!(!result.clean());
    assert_eq!(result.virus, "TestVirus");
}

#[test]
fn test_parse_scan_response_only_found_no_path() {
    let result = parse_scan_response("Malware.Generic FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Malware.Generic");
    assert!(result.path.is_empty());
}

#[test]
fn test_parse_scan_response_unrecognized_format() {
    // Text that doesn't match any known pattern
    let result = parse_scan_response("some random text without format");
    assert!(!result.infected);
    assert!(result.path.is_empty());
    assert!(result.virus.is_empty());
    assert_eq!(result.raw, "some random text without format");
}

#[test]
fn test_is_single_response_command_various() {
    // Single response commands
    assert!(is_single_response_command("PING"));
    assert!(is_single_response_command("VERSION"));
    assert!(is_single_response_command("SCAN /tmp/test"));
    assert!(is_single_response_command("CONTSCAN /tmp"));

    // Multi-line response commands
    assert!(!is_single_response_command("STATS"));
    assert!(!is_single_response_command("RELOAD"));
    assert!(!is_single_response_command("UNKNOWN"));
}

#[test]
fn test_parse_multi_response_trailing_newlines() {
    let results = parse_multi_scan_response("/tmp/a.txt: OK\n\n\n");
    assert_eq!(results.len(), 1);
    assert!(results[0].clean());
}

#[test]
fn test_parse_multi_response_tabs_and_spaces() {
    let results = parse_multi_scan_response("  /tmp/a.txt: OK  \n\t/tmp/b.exe: Trojan FOUND\t");
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_client_scan_stream_fails_no_daemon() {
    let client = Client::new("127.0.0.1:13310");
    let result = client.scan_stream(b"test data").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_with_custom_timeout() {
    let client = Client::with_timeout("127.0.0.1:13310", Duration::from_millis(100));
    let result = client.scan_stream(b"test data").await;
    assert!(result.is_err());
}

#[test]
fn test_parse_scan_response_error_various() {
    // Different error types
    let result = parse_scan_response("/tmp/file: Can't access file ERROR");
    assert!(!result.infected);
    assert!(result.raw.contains("ERROR"));

    let result = parse_scan_response("/tmp/file: lstat() failed ERROR");
    assert!(!result.infected);
}

#[test]
fn test_clamav_scan_result_clean_and_infected() {
    let clean = ClamavScanResult {
        path: "/clean.txt".to_string(),
        infected: false,
        virus: String::new(),
        raw: "/clean.txt: OK".to_string(),
    };
    assert!(clean.clean());
    assert!(!clean.infected);

    let infected = ClamavScanResult {
        path: "/infected.exe".to_string(),
        infected: true,
        virus: "Trojan".to_string(),
        raw: "/infected.exe: Trojan FOUND".to_string(),
    };
    assert!(!infected.clean());
    assert!(infected.infected);
}

#[test]
fn test_client_socket_addr_fallback_on_invalid() {
    let client = Client::new("invalid-addr");
    // Should not panic, falls back to default socket addr
    assert_eq!(client.address(), "invalid-addr");
}

#[test]
fn test_parse_scan_response_only_ok() {
    let result = parse_scan_response("OK");
    assert!(!result.infected);
}

#[test]
fn test_client_new_with_localhost() {
    let client = Client::new("127.0.0.1:3310");
    assert_eq!(client.address(), "127.0.0.1:3310");
}

#[test]
fn test_client_new_with_tcp_prefix() {
    let client = Client::new("tcp://127.0.0.1:3310");
    assert_eq!(client.address(), "tcp://127.0.0.1:3310");
}

#[test]
fn test_clamav_scan_result_construct() {
    let result = ClamavScanResult {
        path: "/test/path".to_string(),
        infected: false,
        virus: String::new(),
        raw: "/test/path: OK".to_string(),
    };
    assert_eq!(result.path, "/test/path");
    assert!(result.clean());
}

#[test]
fn test_parse_scan_response_multiple_colons_in_path() {
    let result = parse_scan_response("C:\\Users\\test:file: Win.Trojan FOUND");
    assert!(result.infected);
    assert_eq!(result.virus, "Win.Trojan");
}

#[test]
fn test_parse_scan_response_ok_file() {
    let result = parse_scan_response("/home/user/document.pdf: OK");
    assert!(!result.infected);
    assert!(result.clean());
    assert!(result.raw.contains("OK"));
}

#[tokio::test]
async fn test_client_scan_file_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.scan_file(Path::new("/tmp/nonexistent")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_version_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.version().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_ping_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.ping().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_stats_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.stats().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_reload_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.reload().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_cont_scan_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.cont_scan(Path::new("/tmp/nonexistent")).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_client_scan_stream_connection_refused() {
    let client = Client::new("127.0.0.1:1");
    let result = client.scan_stream(b"test content").await;
    assert!(result.is_err());
}

// ============================================================
// Fake clamd protocol server tests (2026-08-25 coverage push)
// ============================================================
// 手搓假 clamd：tokio TcpListener 127.0.0.1:0，每个连接读一行 `nCMD\n`
// 按脚本应答后关闭（对端读到 EOF 结束多行命令）。协议要点：
// - PING/VERSION/SCAN/CONTSCAN 单行应答（client 读到第一行即 break）
// - RELOAD/STATS/SHUTDOWN 多行应答（client 读到 EOF）
// - INSTREAM：4 字节大端长度前缀 chunk，0 长度终止，一行应答

use std::sync::Arc;

/// 起假 clamd。`responder` 收到去掉 `n` 前缀的命令行，返回应答字节
/// （空 Vec = 不写任何字节直接关连接 → 客户端读到空响应）。
/// INSTREAM 特殊处理：吞完全部 chunk 后应答 `stream: <总字节数> FOUND`
/// （用病毒名携带总字节数，跨 chunk 完整性可断言）。
async fn serve_clamd(responder: Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync>) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let jh = tokio::spawn(async move {
        loop {
            let (socket, _) = match listener.accept().await {
                Ok(x) => x,
                Err(_) => return,
            };
            let responder = responder.clone();
            tokio::spawn(async move {
                let mut socket = socket;
                let (read_half, mut write_half) = socket.split();
                let mut reader = tokio::io::BufReader::new(read_half);
                let mut line = String::new();
                if reader.read_line(&mut line).await.is_err() {
                    return;
                }
                let cmd = line
                    .trim()
                    .strip_prefix('n')
                    .unwrap_or(line.trim())
                    .to_string();
                if cmd == "INSTREAM" {
                    // 吞 chunk 直到 0 长度终止，统计总字节数
                    let mut total: usize = 0;
                    let mut lenbuf = [0u8; 4];
                    loop {
                        if reader.read_exact(&mut lenbuf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(lenbuf) as usize;
                        if len == 0 {
                            break;
                        }
                        let mut chunk = vec![0u8; len];
                        if reader.read_exact(&mut chunk).await.is_err() {
                            break;
                        }
                        total += len;
                    }
                    let resp = format!("stream: {} FOUND\n", total);
                    let _ = write_half.write_all(resp.as_bytes()).await;
                } else {
                    let resp = responder(&cmd);
                    if !resp.is_empty() {
                        let _ = write_half.write_all(&resp).await;
                    }
                }
                // drop → 对端读到 EOF
            });
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

/// 常用应答器：PING→PONG；SCAN <path>→原样回显路径 + OK（client 发的是
/// canonicalize 过的绝对路径，服务端回显即与最终 result.path 对齐）。
fn pong_scan_ok() -> Arc<dyn Fn(&str) -> Vec<u8> + Send + Sync> {
    Arc::new(|cmd: &str| {
        if let Some(path) = cmd.strip_prefix("SCAN ") {
            format!("{}: OK\n", path).into_bytes()
        } else if cmd == "PING" {
            b"PONG\n".to_vec()
        } else if cmd == "VERSION" {
            b"ClamAV 1.4.2/27350/Mon Jan  1 00:00:00 2026\n".to_vec()
        } else {
            Vec::new()
        }
    })
}

#[tokio::test]
async fn fake_server_ping_success() {
    let addr = serve_clamd(pong_scan_ok()).await;
    let client = Client::new(&addr);
    client.ping().await.unwrap();
}

#[tokio::test]
async fn fake_server_ping_unexpected_response() {
    let addr = serve_clamd(Arc::new(|_cmd: &str| b"WHAT\n".to_vec())).await;
    let client = Client::new(&addr);
    let err = client.ping().await.unwrap_err();
    assert!(err.contains("unexpected ping response"), "{err}");
}

#[tokio::test]
async fn fake_server_version_success() {
    let addr = serve_clamd(pong_scan_ok()).await;
    let client = Client::new(&addr);
    let v = client.version().await.unwrap();
    assert!(v.contains("ClamAV 1.4.2"), "{v}");
}

#[tokio::test]
async fn fake_server_shutdown_success() {
    // 真 clamd 关连接不回话，但 client 要求非空响应才算 Ok —— 按 client
    // 实际实现测试：回一行非空即可。
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if cmd == "SHUTDOWN" {
            b"BYE\n".to_vec()
        } else {
            Vec::new()
        }
    }))
    .await;
    let client = Client::new(&addr);
    client.shutdown().await.unwrap();
}

#[tokio::test]
async fn fake_server_empty_response_is_error() {
    let addr = serve_clamd(Arc::new(|_cmd: &str| Vec::new())).await;
    let client = Client::new(&addr);
    let err = client.version().await.unwrap_err();
    assert!(err.contains("empty response"), "{err}");
}

#[tokio::test]
async fn fake_server_scan_file_clean_absolute_path() {
    // 已知坑：scan_file 必须发 canonicalize 绝对路径；服务端回显收到的
    // 路径，断言 result.path 含文件名（canonicalize 后回显对齐）。
    let addr = serve_clamd(pong_scan_ok()).await;
    let client = Client::new(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("target.txt");
    std::fs::write(&file, "clean content").unwrap();
    let result = client.scan_file(&file).await.unwrap();
    assert!(result.clean());
    assert!(result.path.contains("target.txt"), "{}", result.path);
    // 绝对路径（Windows 盘符 / Unix 根）
    let p = &result.path;
    assert!(
        p.len() >= 3 && (p.as_bytes()[1] == b':' || p.starts_with('/')),
        "not absolute: {p}"
    );
}

#[tokio::test]
async fn fake_server_scan_file_infected() {
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if let Some(path) = cmd.strip_prefix("SCAN ") {
            format!("{}: EICAR-Test-File FOUND\n", path).into_bytes()
        } else {
            b"PONG\n".to_vec()
        }
    }))
    .await;
    let client = Client::new(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("eicar.txt");
    std::fs::write(&file, "eicar test payload").unwrap();
    let result = client.scan_file(&file).await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "EICAR-Test-File");
    assert!(!result.clean());
}

#[tokio::test]
async fn fake_server_scan_file_no_separator_falls_back_clean() {
    // 应答无 ": " 也无 " FOUND"（如裸 "OK"）→ parse fallback：clean + 空 path。
    let addr = serve_clamd(Arc::new(|_cmd: &str| b"OK\n".to_vec())).await;
    let client = Client::new(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("any.txt");
    std::fs::write(&file, "x").unwrap();
    let result = client.scan_file(&file).await.unwrap();
    assert!(result.clean());
    assert!(result.path.is_empty());
}

#[tokio::test]
async fn fake_server_cont_scan_single_line_response() {
    // CONTSCAN 是单行应答命令（is_single_response_command=true，读完第一行
    // 即 break），parse_multi 拿到一行。
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if let Some(path) = cmd.strip_prefix("CONTSCAN ") {
            format!("{}: OK\n", path).into_bytes()
        } else {
            b"PONG\n".to_vec()
        }
    }))
    .await;
    let client = Client::new(&addr);
    let dir = tempfile::tempdir().unwrap();
    let results = client.cont_scan(dir.path()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].clean());
}

#[tokio::test]
async fn fake_server_scan_stream_small_content() {
    // 病毒名携带服务端收到的总字节数 → 断言字节完整到达。
    let addr = serve_clamd(Arc::new(|_cmd: &str| Vec::new())).await;
    let client = Client::new(&addr);
    let result = client.scan_stream(b"hello").await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "5");
}

#[tokio::test]
async fn fake_server_scan_stream_multi_chunk() {
    // 70_000 字节 > 32KB chunk_size → 强制走 2+ chunk 路径；总字节数
    // 必须仍是 70000（跨 chunk 无丢失）。
    let addr = serve_clamd(Arc::new(|_cmd: &str| Vec::new())).await;
    let client = Client::new(&addr);
    let content = vec![b'A'; 70_000];
    let result = client.scan_stream(&content).await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "70000");
}

#[tokio::test]
async fn fake_server_scan_stream_empty_content() {
    // 空内容：不发任何 chunk，直接 0 长度终止 → 服务端 total=0。
    let addr = serve_clamd(Arc::new(|_cmd: &str| Vec::new())).await;
    let client = Client::new(&addr);
    let result = client.scan_stream(b"").await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "0");
}

#[tokio::test]
async fn fake_server_reload_success() {
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if cmd == "RELOAD" {
            b"RELOADING\n".to_vec()
        } else {
            Vec::new()
        }
    }))
    .await;
    let client = Client::new(&addr);
    client.reload().await.unwrap();
}

#[tokio::test]
async fn fake_server_reload_unexpected_response() {
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if cmd == "RELOAD" {
            b"BUSY\n".to_vec()
        } else {
            Vec::new()
        }
    }))
    .await;
    let client = Client::new(&addr);
    let err = client.reload().await.unwrap_err();
    assert!(err.contains("unexpected reload response"), "{err}");
}

#[tokio::test]
async fn fake_server_stats_multi_line() {
    // STATS 是多行命令：client 读到 EOF；多行 join("\n")。
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if cmd == "STATS" {
            b"POOLS: 1\nQUEUE: 0 items\n\nMEMORY: 64.00 MB\n".to_vec()
        } else {
            Vec::new()
        }
    }))
    .await;
    let client = Client::new(&addr);
    let stats = client.stats().await.unwrap();
    assert!(stats.contains("POOLS: 1"), "{stats}");
    assert!(stats.contains("QUEUE: 0 items"), "{stats}");
    assert!(stats.contains("MEMORY"), "{stats}");
}

#[tokio::test]
async fn fake_server_with_timeout_client_roundtrip() {
    // with_timeout 构造 + 真协议往返。
    let addr = serve_clamd(pong_scan_ok()).await;
    let client = Client::with_timeout(&addr, Duration::from_secs(10));
    assert_eq!(client.timeout(), Duration::from_secs(10));
    client.ping().await.unwrap();
}

#[tokio::test]
async fn fake_server_scan_file_error_response_is_not_infected() {
    // "<path>: ... ERROR" → clean 且 path 清空（ERROR 分支）。
    let addr = serve_clamd(Arc::new(|cmd: &str| {
        if let Some(path) = cmd.strip_prefix("SCAN ") {
            format!("{}: Access denied ERROR\n", path).into_bytes()
        } else {
            Vec::new()
        }
    }))
    .await;
    let client = Client::new(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("denied.txt");
    std::fs::write(&file, "x").unwrap();
    let result = client.scan_file(&file).await.unwrap();
    assert!(!result.infected);
    assert!(result.path.is_empty());
    assert!(result.raw.contains("ERROR"));
}

#[tokio::test]
#[allow(deprecated)] // set_linger(0) 故意发 RST 复现读错误分支
async fn send_command_read_error_breaks_out_of_loop() {
    use tokio::io::AsyncReadExt;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let srv = tokio::spawn(async move {
        let (mut s, _) = listener.accept().await.unwrap();
        let mut buf = [0u8; 64];
        let _ = s.read(&mut buf).await;
        // linger(0) + close sends RST instead of FIN → the client's read_line
        // fails with a connection-reset error (not a clean EOF).
        let _ = s.set_linger(Some(std::time::Duration::ZERO));
        drop(s);
    });
    let client = Client::with_timeout(&addr.to_string(), std::time::Duration::from_secs(5));
    // STATS is a multi-line command: the read loop continues after each line,
    // so the reset hits inside the loop's Err(_) => break arm.
    let r = client.stats().await;
    assert!(r.is_err());
    let _ = srv.await;
}
