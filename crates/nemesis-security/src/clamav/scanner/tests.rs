use super::*;
use std::path::PathBuf;
use tokio::io::AsyncBufReadExt; // read_line 所在 trait（E0599）

fn test_config() -> ScannerConfig {
    ScannerConfig {
        enabled: true,
        address: "127.0.0.1:3310".to_string(),
        scan_on_write: true,
        scan_on_download: true,
        scan_on_exec: true,
        max_file_size: 50 * 1024 * 1024,
        timeout: Duration::from_secs(60),
    }
}

fn disabled_config() -> ScannerConfig {
    ScannerConfig {
        enabled: false,
        ..test_config()
    }
}

#[test]
fn test_should_scan_write_file() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("write_file"));
}

#[test]
fn test_should_scan_edit_file() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("edit_file"));
}

#[test]
fn test_should_scan_append_file() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("append_file"));
}

#[test]
fn test_should_scan_download() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("download"));
}

#[test]
fn test_should_scan_exec() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("exec"));
}

#[test]
fn test_should_scan_execute_command() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan("execute_command"));
}

#[test]
fn test_should_scan_unknown() {
    let scanner = Scanner::new(test_config());
    assert!(!scanner.should_scan("unknown"));
    assert!(!scanner.should_scan("read_file"));
    assert!(!scanner.should_scan("list_dir"));
}

#[test]
fn test_should_scan_disabled() {
    let scanner = Scanner::new(disabled_config());
    assert!(!scanner.should_scan("write_file"));
    assert!(!scanner.should_scan("download"));
    assert!(!scanner.should_scan("exec"));
}

#[test]
fn test_should_scan_file_safe_extensions() {
    let scanner = Scanner::new(test_config());
    // Safe extensions should NOT be scanned
    assert!(!scanner.should_scan_file(&PathBuf::from("test.txt")));
    assert!(!scanner.should_scan_file(&PathBuf::from("readme.md")));
    assert!(!scanner.should_scan_file(&PathBuf::from("data.json")));
    assert!(!scanner.should_scan_file(&PathBuf::from("config.yaml")));
    assert!(!scanner.should_scan_file(&PathBuf::from("config.yml")));
    assert!(!scanner.should_scan_file(&PathBuf::from("data.xml")));
    assert!(!scanner.should_scan_file(&PathBuf::from("data.csv")));
    assert!(!scanner.should_scan_file(&PathBuf::from("app.log")));
    assert!(!scanner.should_scan_file(&PathBuf::from("app.ini")));
    assert!(!scanner.should_scan_file(&PathBuf::from("app.toml")));
    assert!(!scanner.should_scan_file(&PathBuf::from("page.html")));
    assert!(!scanner.should_scan_file(&PathBuf::from("style.css")));
    assert!(!scanner.should_scan_file(&PathBuf::from("app.js")));
    assert!(!scanner.should_scan_file(&PathBuf::from("app.ts")));
}

#[test]
fn test_should_scan_file_executable_extensions() {
    let scanner = Scanner::new(test_config());
    // Executable extensions should always be scanned
    assert!(scanner.should_scan_file(&PathBuf::from("program.exe")));
    assert!(scanner.should_scan_file(&PathBuf::from("library.dll")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.bat")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.cmd")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.ps1")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.sh")));
    assert!(scanner.should_scan_file(&PathBuf::from("lib.so")));
    assert!(scanner.should_scan_file(&PathBuf::from("lib.dylib")));
    assert!(scanner.should_scan_file(&PathBuf::from("setup.msi")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.vbs")));
    assert!(scanner.should_scan_file(&PathBuf::from("program.com")));
    assert!(scanner.should_scan_file(&PathBuf::from("screen.scr")));
    assert!(scanner.should_scan_file(&PathBuf::from("app.jar")));
    assert!(scanner.should_scan_file(&PathBuf::from("script.py")));
}

#[test]
fn test_should_scan_file_unknown_extension() {
    let scanner = Scanner::new(test_config());
    // Unknown extensions should be scanned (conservative)
    assert!(scanner.should_scan_file(&PathBuf::from("data.xyz")));
    assert!(scanner.should_scan_file(&PathBuf::from("archive.zip")));
    assert!(scanner.should_scan_file(&PathBuf::from("file")));
}

#[test]
fn test_should_scan_file_disabled() {
    let scanner = Scanner::new(disabled_config());
    // When disabled, nothing should be scanned
    assert!(!scanner.should_scan_file(&PathBuf::from("program.exe")));
    assert!(!scanner.should_scan_file(&PathBuf::from("data.xyz")));
}

#[test]
fn test_default_scanner_config_values() {
    let cfg = default_scanner_config();
    assert!(cfg.enabled);
    assert_eq!(cfg.address, "127.0.0.1:3310");
    assert!(cfg.scan_on_write);
    assert!(cfg.scan_on_download);
    assert!(cfg.scan_on_exec);
    assert_eq!(cfg.max_file_size, 50 * 1024 * 1024);
    assert_eq!(cfg.timeout, Duration::from_secs(60));
}

#[tokio::test]
async fn test_get_stats_initial() {
    let scanner = Scanner::new(test_config());
    let stats = scanner.get_stats().await;
    assert_eq!(stats.total_scans, 0);
    assert_eq!(stats.clean_scans, 0);
    assert_eq!(stats.infected_scans, 0);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.total_bytes, 0);
}

#[tokio::test]
async fn test_scan_file_disabled() {
    let scanner = Scanner::new(disabled_config());
    let result = scanner.scan_file(Path::new("/tmp/test.txt")).await.unwrap();
    assert!(!result.infected);
    assert_eq!(result.raw, "scanning disabled");
}

#[tokio::test]
async fn test_scan_content_disabled() {
    let scanner = Scanner::new(disabled_config());
    let result = scanner.scan_content(b"hello world").await.unwrap();
    assert!(!result.infected);
    assert_eq!(result.raw, "scanning disabled");
}

#[tokio::test]
async fn test_scan_directory_disabled() {
    let scanner = Scanner::new(disabled_config());
    let results = scanner.scan_directory(Path::new("/tmp")).await.unwrap();
    assert!(results.is_empty());
}

#[test]
fn test_scan_stats_default() {
    let stats = ScanStats::default();
    assert_eq!(stats.total_scans, 0);
    assert_eq!(stats.clean_scans, 0);
    assert_eq!(stats.infected_scans, 0);
    assert_eq!(stats.errors, 0);
    assert_eq!(stats.total_bytes, 0);
}

#[test]
fn test_scanner_config_debug() {
    let config = test_config();
    let debug = format!("{:?}", config);
    assert!(debug.contains("enabled"));
    assert!(debug.contains("3310"));
}

#[test]
fn test_scanner_config_clone() {
    let config = test_config();
    let cloned = config.clone();
    assert_eq!(cloned.address, config.address);
    assert_eq!(cloned.max_file_size, config.max_file_size);
}

#[tokio::test]
async fn test_scanner_new_with_client() {
    // Closed port → deterministic connection refusal, so the "no daemon" path
    // holds regardless of whether clamd is running on the test machine.
    let client = Client::new("127.0.0.1:1");
    let scanner = Scanner::new_with_client(client, test_config());
    assert!(scanner.ping().await.is_err()); // no daemon running
}

#[tokio::test]
async fn test_scan_file_too_large() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("large.bin");
    std::fs::write(&file_path, vec![0u8; 1024]).unwrap();
    let config = ScannerConfig {
        enabled: true,
        max_file_size: 100, // Very small limit
        ..test_config()
    };
    let scanner = Scanner::new(config);
    let result = scanner.scan_file(&file_path).await.unwrap();
    assert!(!result.infected);
    assert!(result.raw.contains("too large"));
}

#[tokio::test]
async fn test_scan_content_too_large() {
    let config = ScannerConfig {
        enabled: true,
        max_file_size: 10, // Very small limit
        ..test_config()
    };
    let scanner = Scanner::new(config);
    let data = vec![0u8; 100];
    let result = scanner.scan_content(&data).await.unwrap();
    assert!(!result.infected);
    assert!(result.raw.contains("too large"));
}

#[test]
fn test_should_scan_file_no_extension() {
    let scanner = Scanner::new(test_config());
    // File without extension should be scanned (unknown extension)
    assert!(scanner.should_scan_file(&PathBuf::from("Makefile")));
    assert!(scanner.should_scan_file(&PathBuf::from("README")));
}

#[test]
fn test_should_scan_file_pif_extension() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan_file(&PathBuf::from("program.pif")));
}

#[tokio::test]
async fn test_record_scan_stats() {
    let scanner = Scanner::new(test_config());
    // Manually record scans
    scanner.record_scan(100, false, false).await;
    scanner.record_scan(200, true, false).await;
    scanner.record_scan(50, false, true).await;

    let stats = scanner.get_stats().await;
    assert_eq!(stats.total_scans, 3);
    assert_eq!(stats.clean_scans, 1);
    assert_eq!(stats.infected_scans, 1);
    assert_eq!(stats.errors, 1);
    assert_eq!(stats.total_bytes, 350);
}

#[test]
fn test_scan_stats_debug() {
    let stats = ScanStats {
        total_scans: 10,
        clean_scans: 8,
        infected_scans: 1,
        errors: 1,
        total_bytes: 4096,
    };
    let debug = format!("{:?}", stats);
    assert!(debug.contains("10"));
    assert!(debug.contains("4096"));
}

#[test]
fn test_default_scanner_config_function() {
    let cfg = default_scanner_config();
    assert!(cfg.enabled);
    assert_eq!(cfg.address, "127.0.0.1:3310");
}

#[tokio::test]
async fn test_scan_file_nonexistent_when_enabled() {
    // Closed port → deterministic connection failure (independent of whether
    // clamd is running on this machine).
    let config = ScannerConfig {
        address: "127.0.0.1:1".to_string(),
        ..test_config()
    };
    let scanner = Scanner::new(config);
    let result = scanner
        .scan_file(Path::new("/tmp/nonexistent_file_for_test.txt"))
        .await;
    // Fails because nothing listens at the closed port.
    assert!(result.is_err());
}

#[test]
fn test_should_scan_file_scr_extension() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan_file(&PathBuf::from("screensaver.scr")));
}

#[test]
fn test_should_scan_file_com_extension() {
    let scanner = Scanner::new(test_config());
    assert!(scanner.should_scan_file(&PathBuf::from("program.com")));
}

// ============================================================
// Fake clamd server tests (2026-08-25 coverage push)
// ============================================================
// 覆盖 scan_file/scan_content/scan_directory/ping/shutdown 的成功路径
// （现有测试只到 disabled/too-large/connection-refused）。

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
        tokio::time::sleep(Duration::from_secs(30)).await;
        jh.abort();
    });
    addr.to_string()
}

fn scanner_on(addr: &str) -> Scanner {
    Scanner::new(ScannerConfig {
        address: addr.to_string(),
        ..test_config()
    })
}

#[tokio::test]
async fn fake_ping_success() {
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
    let scanner = scanner_on(&addr);
    scanner.ping().await.unwrap();
}

#[tokio::test]
async fn fake_shutdown_success() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if cmd == "SHUTDOWN" {
                b"BYE\n".to_vec()
            } else {
                Vec::new()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    scanner.shutdown().await.unwrap();
}

#[tokio::test]
async fn fake_scan_file_clean_records_stats() {
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
    let scanner = scanner_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("clean.bin");
    std::fs::write(&file, "payload").unwrap();
    let result = scanner.scan_file(&file).await.unwrap();
    assert!(result.clean());
    assert!(result.path.contains("clean.bin"));
    let stats = scanner.get_stats().await;
    assert_eq!(stats.total_scans, 1);
    assert_eq!(stats.clean_scans, 1);
}

#[tokio::test]
async fn fake_scan_file_infected_records_stats() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if let Some(path) = cmd.strip_prefix("SCAN ") {
                format!("{}: Test.Virus FOUND\n", path).into_bytes()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("dirty.bin");
    std::fs::write(&file, "payload").unwrap();
    let result = scanner.scan_file(&file).await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "Test.Virus");
    let stats = scanner.get_stats().await;
    assert_eq!(stats.infected_scans, 1);
    assert_eq!(stats.clean_scans, 0);
}

#[tokio::test]
async fn fake_scan_content_clean_records_bytes() {
    let addr = serve_clamd(
        StdArc::new(|_cmd: &str| Vec::new()),
        "stream: OK\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    let result = scanner.scan_content(b"abcdefghij").await.unwrap();
    assert!(result.clean());
    let stats = scanner.get_stats().await;
    assert_eq!(stats.total_scans, 1);
    assert_eq!(stats.total_bytes, 10);
}

#[tokio::test]
async fn fake_scan_content_infected() {
    let addr = serve_clamd(
        StdArc::new(|_cmd: &str| Vec::new()),
        "stream: EICAR FOUND\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    let result = scanner.scan_content(b"bad").await.unwrap();
    assert!(result.infected);
    assert_eq!(result.virus, "EICAR");
    let stats = scanner.get_stats().await;
    assert_eq!(stats.infected_scans, 1);
}

#[tokio::test]
async fn fake_scan_directory_cont_scan_records_stats() {
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if cmd.starts_with("CONTSCAN ") {
                b"a.bin: OK\nb.bin: Worm FOUND\n".to_vec()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let results = scanner.scan_directory(dir.path()).await.unwrap();
    // CONTSCAN 是单行应答命令：client 读到第一行即 break
    assert_eq!(results.len(), 1);
    assert!(results[0].clean());
    let stats = scanner.get_stats().await;
    assert_eq!(stats.total_scans, 1);
}

#[tokio::test]
async fn fake_scan_directory_multi_line_response() {
    // 多行：STATS/RELOAD 才读全 —— CONTSCAN 单行即断。此测验证多行
    // 应答也只取第一行（client 行为钉死，防回归）。
    let addr = serve_clamd(
        StdArc::new(|cmd: &str| {
            if cmd.starts_with("CONTSCAN ") {
                b"only-first-line.bin: OK\nsecond-line.bin: Trojan FOUND\n".to_vec()
            } else {
                b"PONG\n".to_vec()
            }
        }),
        "stream: OK\n",
    )
    .await;
    let scanner = scanner_on(&addr);
    let dir = tempfile::tempdir().unwrap();
    let results = scanner.scan_directory(dir.path()).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn fake_scan_file_connection_error_returns_err() {
    // enabled + 存在的文件 + 连不上（关闭端口）→ Err（不是假阴性 clean）。
    let config = ScannerConfig {
        address: "127.0.0.1:1".to_string(),
        ..test_config()
    };
    let scanner = Scanner::new(config);
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("exists.bin");
    std::fs::write(&file, "data").unwrap();
    assert!(scanner.scan_file(&file).await.is_err());
}

/// In-process mock clamd: accepts any connection, reads the request line,
/// replies with the canned payload, closes. Each connection gets one reply.
async fn clamd_mock(reply: &'static str) -> String {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        loop {
            let Ok((mut s, _)) = listener.accept().await else {
                return;
            };
            let payload = reply;
            tokio::spawn(async move {
                let mut buf = [0u8; 1024];
                let _ = s.read(&mut buf).await;
                let _ = s.write_all(payload.as_bytes()).await;
                let _ = s.shutdown().await;
            });
        }
    });
    addr.to_string()
}

#[tokio::test]
async fn scan_directory_infected_result_warns_and_counts() {
    let addr = clamd_mock("C:/tmp/bad.exe: Win32.Eicar.Test FOUND\n\n").await;
    let cfg = ScannerConfig {
        address: addr,
        ..Default::default()
    };
    let scanner = Scanner::new(cfg);
    let dir = tempfile::tempdir().unwrap();
    let results = scanner.scan_directory(dir.path()).await.unwrap();
    assert_eq!(results.len(), 1);
    assert!(results[0].infected);
    assert_eq!(results[0].virus, "Win32.Eicar.Test");
    let stats = scanner.get_stats().await;
    assert_eq!(stats.infected_scans, 1);
    assert_eq!(stats.total_scans, 1);
}

#[tokio::test]
async fn scan_file_zero_max_size_scans_via_mock() {
    // max_file_size = 0 skips the size gate entirely; scan proceeds to the
    // client against the mock daemon.
    let addr = clamd_mock("C:/x.exe: OK\n").await;
    let cfg = ScannerConfig {
        address: addr,
        max_file_size: 0,
        ..Default::default()
    };
    let scanner = Scanner::new(cfg);
    let dir = tempfile::tempdir().unwrap();
    let f = dir.path().join("x.exe");
    std::fs::write(&f, "MZ").unwrap();
    let r = scanner.scan_file(&f).await.unwrap();
    assert!(!r.infected);
    assert_eq!(scanner.get_stats().await.clean_scans, 1);
}

#[tokio::test]
async fn scan_file_missing_file_metadata_err_falls_through_to_client() {
    // Existing file does not exist → tokio::fs::metadata fails → the if-let
    // falls through the size gate and the client scan still runs.
    let addr = clamd_mock("C:/missing.exe: OK\n").await;
    let cfg = ScannerConfig {
        address: addr,
        ..Default::default()
    };
    let scanner = Scanner::new(cfg);
    let r = scanner
        .scan_file(std::path::Path::new(r"Z:\no\such\file.exe"))
        .await
        .unwrap();
    assert!(!r.infected);
}
