use super::*;
use crate::clamav::scanner::ScannerConfig;
use tokio::io::AsyncBufReadExt; // read_line 所在 trait（E0599）

fn make_result(path: &str, infected: bool, virus: &str) -> ClamavScanResult {
    ClamavScanResult {
        path: path.to_string(),
        infected,
        virus: virus.to_string(),
        raw: String::new(),
    }
}

#[test]
fn test_format_scan_result_none() {
    assert_eq!(format_scan_result(None), "no scan performed");
}

#[test]
fn test_format_scan_result_infected() {
    let result = make_result("/tmp/eicar.com", true, "Eicar-Signature");
    let formatted = format_scan_result(Some(&result));
    assert!(formatted.contains("INFECTED"));
    assert!(formatted.contains("/tmp/eicar.com"));
    assert!(formatted.contains("Eicar-Signature"));
}

#[test]
fn test_format_scan_result_clean() {
    let result = make_result("/tmp/safe.txt", false, "");
    let formatted = format_scan_result(Some(&result));
    assert!(formatted.contains("CLEAN"));
    assert!(formatted.contains("/tmp/safe.txt"));
}

#[tokio::test]
async fn test_scan_hook_new() {
    // Closed port → deterministic "ping fails" even when clamd is running here.
    let scanner = Arc::new(Scanner::new(ScannerConfig {
        address: "127.0.0.1:1".to_string(),
        ..ScannerConfig::default()
    }));
    let hook = ScanHook::new(scanner);
    let scanner_ref = hook.get_scanner();
    assert!(scanner_ref.ping().await.is_err()); // not running, so ping should fail
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_unknown_tool() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({});
    // Unknown tools should be allowed
    let result = hook
        .scan_tool_invocation("unknown_tool", &args)
        .await
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_write_no_content() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({});
    // write_file with no content field should be ok
    let result = hook
        .scan_tool_invocation("write_file", &args)
        .await
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_write_empty_content() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({"content": ""});
    let result = hook
        .scan_tool_invocation("write_file", &args)
        .await
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_edit_file_no_content() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({});
    let result = hook.scan_tool_invocation("edit_file", &args).await.unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_append_file_no_content() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({});
    let result = hook
        .scan_tool_invocation("append_file", &args)
        .await
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_download() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({"url": "http://example.com/file"});
    let result = hook.scan_tool_invocation("download", &args).await.unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_exec() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({"command": "ls"});
    let result = hook.scan_tool_invocation("exec", &args).await.unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_tool_invocation_execute_command() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let args = serde_json::json!({"command": "dir"});
    let result = hook
        .scan_tool_invocation("execute_command", &args)
        .await
        .unwrap();
    assert!(result);
}

#[tokio::test]
async fn test_scan_hook_scan_file_path_nonexistent() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let result = hook
        .scan_file_path(Path::new("/nonexistent/file.txt"))
        .await
        .unwrap();
    assert!(result.0); // clean
    assert!(result.1.is_none()); // no scan result
}

#[tokio::test]
async fn test_scan_hook_scan_file_path_safe_extension() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    // Create a temp file with safe extension
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("test.txt");
    std::fs::write(&file_path, "hello").unwrap();
    let result = hook.scan_file_path(&file_path).await.unwrap();
    assert!(result.0); // clean (not scanned because .txt is safe)
    assert!(result.1.is_none()); // no scan result
}

#[tokio::test]
async fn test_scan_hook_scan_downloaded_file_nonexistent() {
    let scanner = Arc::new(Scanner::new(ScannerConfig::default()));
    let hook = ScanHook::new(scanner);
    let result = hook
        .scan_downloaded_file(Path::new("/nonexistent/file.exe"))
        .await
        .unwrap();
    assert!(result.0); // clean
    assert!(result.1.is_none());
}

#[test]
fn test_format_scan_result_variants() {
    // Test all three variants of format_scan_result
    assert_eq!(format_scan_result(None), "no scan performed");

    let clean = make_result("/tmp/safe.txt", false, "");
    let formatted = format_scan_result(Some(&clean));
    assert!(formatted.contains("CLEAN"));

    let infected = make_result("/tmp/eicar.com", true, "Eicar");
    let formatted = format_scan_result(Some(&infected));
    assert!(formatted.contains("INFECTED"));
    assert!(formatted.contains("Eicar"));
}

#[tokio::test]
async fn test_health_check_fails_when_not_running() {
    // Closed port → deterministic "health_check fails" even when clamd is running here.
    let scanner = Arc::new(Scanner::new(ScannerConfig {
        address: "127.0.0.1:1".to_string(),
        ..ScannerConfig::default()
    }));
    let hook = ScanHook::new(scanner);
    assert!(hook.health_check().await.is_err());
}

// ============================================================
// Fake clamd server tests (2026-08-25 coverage push)
// ============================================================
// 协议细节见 client/tests.rs 的 serve_clamd 注释；此处为模块内独立副本
// （tests 模块互相不可见）。

use std::sync::Arc as StdArc;

/// `instream_reply`：INSTREAM 完整收到后的应答行（含换行）。
async fn serve_clamd(
    responder: StdArc<dyn Fn(&str) -> Vec<u8> + Send + Sync>,
    instream_reply: &'static str,
) -> String {
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
                let cmd = line.trim().strip_prefix('n').unwrap_or(line.trim()).to_string();
                if cmd == "INSTREAM" {
                    let mut lenbuf = [0u8; 4];
                    let mut terminated = false;
                    loop {
                        if reader.read_exact(&mut lenbuf).await.is_err() {
                            break;
                        }
                        let len = u32::from_be_bytes(lenbuf) as usize;
                        if len == 0 {
                            terminated = true;
                            break;
                        }
                        let mut chunk = vec![0u8; len];
                        if reader.read_exact(&mut chunk).await.is_err() {
                            break;
                        }
                    }
                    if terminated {
                        let _ = write_half.write_all(instream_reply.as_bytes()).await;
                    }
                } else {
                    let resp = responder(&cmd);
                    if !resp.is_empty() {
                        let _ = write_half.write_all(&resp).await;
                    }
                }
            });
        }
    });
    tokio::spawn(async move {
        tokio::time::sleep(std::time::Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

fn noop_responder() -> StdArc<dyn Fn(&str) -> Vec<u8> + Send + Sync> {
    StdArc::new(|_cmd: &str| Vec::new())
}

fn hook_on(addr: &str) -> ScanHook {
    ScanHook::new(StdArc::new(Scanner::new(ScannerConfig {
        address: addr.to_string(),
        ..ScannerConfig::default()
    })))
}

#[tokio::test]
async fn fake_health_check_success() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if cmd == "PING" {
                b"PONG\n".to_vec()
            } else {
                Vec::new()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let hook = hook_on(&addr);
    hook.health_check().await.unwrap();
}

#[tokio::test]
async fn fake_scan_file_path_clean_exe() {
    // .exe 走 should_scan_file=true → 真扫描 → clean。
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if let Some(path) = cmd.strip_prefix("SCAN ") {
                format!("{}: OK\n", path).into_bytes()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let hook = hook_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("prog.exe");
    std::fs::write(&file, "MZ binary").unwrap();
    let (clean, result) = hook.scan_file_path(&file).await.unwrap();
    assert!(clean);
    let r = result.expect("scan performed");
    assert!(r.clean());
    assert!(r.path.contains("prog.exe"));
}

#[tokio::test]
async fn fake_scan_file_path_infected_exe() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if let Some(path) = cmd.strip_prefix("SCAN ") {
                format!("{}: Win.Trojan.Evil FOUND\n", path).into_bytes()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let hook = hook_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("evil.exe");
    std::fs::write(&file, "MZ infected").unwrap();
    let (clean, result) = hook.scan_file_path(&file).await.unwrap();
    assert!(!clean);
    let r = result.expect("scan performed");
    assert!(r.infected);
    assert_eq!(r.virus, "Win.Trojan.Evil");
}

#[tokio::test]
async fn fake_scan_downloaded_file_clean_keeps_file() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if let Some(path) = cmd.strip_prefix("SCAN ") {
                format!("{}: OK\n", path).into_bytes()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let hook = hook_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("downloaded.exe");
    std::fs::write(&file, "clean installer").unwrap();
    let (clean, result) = hook.scan_downloaded_file(&file).await.unwrap();
    assert!(clean);
    assert!(result.is_some());
    assert!(file.exists(), "clean download must NOT be removed");
}

#[tokio::test]
async fn fake_scan_downloaded_file_infected_removes_file() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if let Some(path) = cmd.strip_prefix("SCAN ") {
                format!("{}: EICAR FOUND\n", path).into_bytes()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let hook = hook_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("bad.exe");
    std::fs::write(&file, "infected installer").unwrap();
    let (clean, result) = hook.scan_downloaded_file(&file).await.unwrap();
    assert!(!clean);
    let r = result.expect("scan performed");
    assert!(r.infected);
    assert_eq!(r.virus, "EICAR");
    // 关键行为：感染文件被删除
    assert!(!file.exists(), "infected download must be removed");
}

#[tokio::test]
async fn fake_scan_tool_invocation_write_content_clean() {
    // write_file 带非空 content → INSTREAM 扫描 → stream: OK → Ok(true)。
    let addr = serve_clamd(noop_responder(), "stream: OK\n").await;
    let hook = hook_on(&addr);
    let args = serde_json::json!({"path": "x.bin", "content": "hello world"});
    let ok = hook.scan_tool_invocation("write_file", &args).await.unwrap();
    assert!(ok);
}

#[tokio::test]
async fn fake_scan_tool_invocation_write_content_infected() {
    // INSTREAM 回 FOUND → Err("virus detected in content: ...")。
    let addr = serve_clamd(noop_responder(), "stream: EICAR FOUND\n").await;
    let hook = hook_on(&addr);
    let args = serde_json::json!({"path": "y.bin", "content": "X5O!P%@AP[EICAR]"});
    let err = hook.scan_tool_invocation("write_file", &args).await.unwrap_err();
    assert!(err.contains("virus detected in content"), "{err}");
    assert!(err.contains("EICAR"), "{err}");
}

#[tokio::test]
async fn fake_scan_tool_invocation_edit_file_content_infected() {
    // edit_file 同样走 scan_write_args。
    let addr = serve_clamd(noop_responder(), "stream: Bad.Virus FOUND\n").await;
    let hook = hook_on(&addr);
    let args = serde_json::json!({"content": "infected patch"});
    let err = hook.scan_tool_invocation("edit_file", &args).await.unwrap_err();
    assert!(err.contains("Bad.Virus"), "{err}");
}

#[tokio::test]
async fn scan_downloaded_file_missing_path_is_ok_and_none() {
    let scanner = crate::clamav::scanner::Scanner::new(crate::clamav::scanner::ScannerConfig::default());
    let hook = ScanHook::new(std::sync::Arc::new(scanner));
    let (clean, res) = hook
        .scan_downloaded_file(std::path::Path::new(r"Z:\definitely\missing\file.bin"))
        .await
        .unwrap();
    assert!(clean);
    assert!(res.is_none());
}
