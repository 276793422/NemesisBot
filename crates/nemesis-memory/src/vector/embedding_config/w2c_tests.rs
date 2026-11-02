//! W2c batch tests (Phase 3 / quality-hardening goal).
//!
//! Targets vector/embedding_config.rs branches not exercised by tests.rs /
//! extra_tests.rs:
//! - load_embedding_config failure arms (config dir is a file / config path is
//!   a dir / invalid JSON syntax) → EmbeddingConfig::default(), never panic
//! - save_embedding_config write failure is silent
//! - download_model_files tokenizer copy from local_tokenizer_path (offline)
//! - download_model_files real HTTP download arm via a local one-shot TCP
//!   server (reqwest::blocking against 127.0.0.1 — no external network)
//! - download failure arms: HTTP 404 + connection refused

use super::*;

/// Spawn a std-thread HTTP server that serves exactly `responses.len()`
/// requests, then exits. Each response is (status_line, body).
/// `pub(super)`: shared with cov_tests.rs (subscriber 翻绿批次).
pub(super) fn spawn_http_server(responses: Vec<(&'static str, &'static str)>) -> u16 {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        for (status, body) in responses {
            let (mut stream, _) = match listener.accept() {
                Ok(s) => s,
                Err(_) => return,
            };
            // Drain the request (GET — headers only, no body).
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let resp = format!(
                "HTTP/1.1 {}\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                status,
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
            let _ = stream.flush();
        }
    });
    port
}

/// Find a free port and abandon it (for the connection-refused arm).
/// `pub(super)` 供兄弟测试模块（extra_tests 等）复用：任何可能触发下载的
/// 测试都应把 URL 指到死端口——默认 URL 是 hf-mirror 真网，CI runner 被
/// 限流时 429 假红、本机网络好时真下载成功假绿（2026-09-01 CI 事故）。
pub(super) fn find_dead_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    port
}

fn base_config(active: &str) -> EmbeddingConfig {
    let mut c = EmbeddingConfig::default();
    c.enabled = true;
    c.active = active.to_string();
    c
}

pub(super) fn set_model_urls(config: &mut EmbeddingConfig, model_url: &str, tokenizer_url: &str) {
    let key = config.active.clone();
    let mc = config.models.get_mut(&key).unwrap();
    mc.model_url = model_url.to_string();
    mc.tokenizer_url = tokenizer_url.to_string();
    mc.dimension = 384;
    mc.name = "test-model".to_string();
}

// ============================================================
// load_embedding_config failure arms
// ============================================================

#[test]
fn load_config_dir_is_file_returns_default() {
    // config_dir itself is a FILE: create_dir_all fails (warn), the
    // subsequent read fails (warn) → default config, no panic.
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not_a_dir");
    std::fs::write(&file, "x").unwrap();

    let cfg = load_embedding_config(&file);
    assert!(!cfg.enabled);
    assert_eq!(cfg.active, "medium");
    assert_eq!(cfg.auto_inject_top_k, 3);
}

#[test]
fn load_config_path_is_dir_returns_default() {
    // {config_dir}/config.enhanced_memory.json exists as a DIRECTORY:
    // read_to_string fails → default config.
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("config.enhanced_memory.json");
    std::fs::create_dir(&cfg_file).unwrap();

    let cfg = load_embedding_config(dir.path());
    assert!(!cfg.enabled);
    assert_eq!(cfg.active, "medium");
}

#[test]
fn load_invalid_json_syntax_returns_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, "{ this is not json").unwrap();

    let cfg = load_embedding_config(dir.path());
    assert!(!cfg.enabled);
    // Defaults kick in for the malformed file.
    assert_eq!(cfg.models.medium.dimension, 384);
}

// ============================================================
// save_embedding_config write failure is silent
// ============================================================

#[test]
fn save_config_target_is_dir_no_panic() {
    let dir = tempfile::tempdir().unwrap();
    let cfg_file = dir.path().join("config.enhanced_memory.json");
    std::fs::create_dir(&cfg_file).unwrap();

    let mut cfg = base_config("small");
    cfg.enabled = true;
    // Must not panic; the directory stays untouched.
    save_embedding_config(&cfg, dir.path());
    assert!(cfg_file.is_dir());
}

// ============================================================
// download_model_files — offline tokenizer copy arm
// ============================================================

#[test]
fn download_model_files_copies_local_tokenizer_into_model_dir() {
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    // Model already present in the data dir.
    let data_dir = ws
        .path()
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("test-model");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("model.onnx"), "fake-model").unwrap();

    // Tokenizer provided via local_tokenizer_path.
    let tok_src = ws.path().join("tokenizer-src.json");
    std::fs::write(&tok_src, "TOKENIZER-BYTES").unwrap();

    let mut cfg = base_config("medium");
    {
        let mc = cfg.models.get_mut("medium").unwrap();
        mc.name = "test-model".into();
        mc.dimension = 384;
        mc.model_url = String::new();
        mc.tokenizer_url = String::new();
        mc.local_tokenizer_path = tok_src.to_string_lossy().to_string();
    }

    let (dir, dim) = download_model_files(&mut cfg, &config_dir).unwrap();
    assert_eq!(dim, 384);
    assert!(dir.contains("test-model"));
    let copied = std::fs::read_to_string(data_dir.join("tokenizer.json")).unwrap();
    assert_eq!(copied, "TOKENIZER-BYTES");
    // Nothing was downloaded → no config write-back side effect required.
    assert!(!config_dir.join("config.enhanced_memory.json").exists());
}

// ============================================================
// download_model_files — real HTTP download arm (local server)
// ============================================================

#[test]
fn download_model_files_downloads_via_http_and_saves_config() {
    let port = spawn_http_server(vec![
        ("200 OK", "MODEL-BYTES-123"),
        ("200 OK", "TOKENIZER-BYTES-45"),
    ]);
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    let mut cfg = base_config("medium");
    set_model_urls(
        &mut cfg,
        &format!("http://127.0.0.1:{}/model.onnx", port),
        &format!("http://127.0.0.1:{}/tokenizer.json", port),
    );

    let (dir, dim) = download_model_files(&mut cfg, &config_dir).unwrap();
    assert_eq!(dim, 384);
    let data_dir = std::path::Path::new(&dir);
    assert_eq!(
        std::fs::read_to_string(data_dir.join("model.onnx")).unwrap(),
        "MODEL-BYTES-123"
    );
    assert_eq!(
        std::fs::read_to_string(data_dir.join("tokenizer.json")).unwrap(),
        "TOKENIZER-BYTES-45"
    );

    // Download happened → config persisted with the resolved local paths.
    let saved = load_embedding_config(&config_dir);
    let mc = saved.models.get("medium").unwrap();
    assert!(
        mc.local_model_path.ends_with("model.onnx"),
        "got {}",
        mc.local_model_path
    );
    assert!(
        mc.local_tokenizer_path.ends_with("tokenizer.json"),
        "got {}",
        mc.local_tokenizer_path
    );
}

#[test]
fn download_model_files_http_error_propagates() {
    let port = spawn_http_server(vec![("404 Not Found", "gone")]);
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    let mut cfg = base_config("medium");
    set_model_urls(
        &mut cfg,
        &format!("http://127.0.0.1:{}/model.onnx", port),
        "",
    );

    let err = download_model_files(&mut cfg, &config_dir).unwrap_err();
    assert!(
        err.contains("HTTP 404"),
        "expected HTTP status in error, got: {}",
        err
    );
    // Failed download must not leave a model file behind.
    let data_dir = embedding_data_dir(&config_dir).join("test-model");
    assert!(!data_dir.join("model.onnx").exists());
}

#[test]
fn download_model_files_connection_refused_errors() {
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    let dead = find_dead_port();
    let mut cfg = base_config("medium");
    set_model_urls(
        &mut cfg,
        &format!("http://127.0.0.1:{}/model.onnx", dead),
        "",
    );

    let err = download_model_files(&mut cfg, &config_dir).unwrap_err();
    assert!(
        err.contains("download request failed"),
        "expected request-failure error, got: {}",
        err
    );
}

// ============================================================
// S5 coverage: tokenizer copy failure arm + download_file edges
// ============================================================

#[test]
fn download_model_files_tokenizer_copy_failure_warns_but_succeeds() {
    // local_tokenizer_path exists but is a DIRECTORY → fs::copy fails →
    // warn + continue (no tokenizer written, no download, Ok returned).
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    let data_dir = embedding_data_dir(&config_dir).join("test-model");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("model.onnx"), "fake-model").unwrap();

    let tok_dir = ws.path().join("tokenizer-is-a-dir");
    std::fs::create_dir(&tok_dir).unwrap();

    let mut cfg = base_config("medium");
    {
        let mc = cfg.models.get_mut("medium").unwrap();
        mc.name = "test-model".into();
        mc.dimension = 384;
        mc.model_url = String::new();
        mc.tokenizer_url = String::new();
        mc.local_tokenizer_path = tok_dir.to_string_lossy().to_string();
    }

    let (dir, dim) = download_model_files(&mut cfg, &config_dir).unwrap();
    assert_eq!(dim, 384);
    assert!(dir.contains("test-model"));
    // Copy failed → tokenizer.json must NOT exist in the model dir.
    assert!(!data_dir.join("tokenizer.json").exists());
}

#[test]
fn download_file_dest_exists_returns_ok_without_request() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("already.bin");
    std::fs::write(&dest, "present").unwrap();

    // Dead URL: must not even be contacted because dest already exists.
    let dead = find_dead_port();
    let url = format!("http://127.0.0.1:{}/never", dead);
    download_file(&url, &dest).unwrap();
    assert_eq!(std::fs::read_to_string(&dest).unwrap(), "present");
}

#[test]
fn download_file_parent_create_failure_errors() {
    let port = spawn_http_server(vec![("200 OK", "payload")]);
    let dir = tempfile::tempdir().unwrap();
    let blocker = dir.path().join("blocker");
    std::fs::write(&blocker, "x").unwrap();
    let dest = blocker.join("out.bin");

    let err = download_file(&format!("http://127.0.0.1:{}/f", port), &dest).unwrap_err();
    assert!(err.contains("failed to create parent dir"), "got: {err}");
}

// ============================================================
// BUG-20261031-241：零字节残骸重下 + 空响应体诚实报错 + 原子落盘
// （对齐 nemesis-voice model.rs 同族先例 BUG-20261029-146）
// ============================================================

/// 红证（download_model_files 层）：0 字节 model.onnx/tokenizer.json 是
/// 中断下载的残骸——旧代码「存在即跳过」永远不会重下（本地 http server
/// 一个请求都收不到，文件保持 0 字节），插件 init 必败且重装无效。
#[test]
fn download_model_files_redownloads_zero_byte_debris() {
    let port = spawn_http_server(vec![
        ("200 OK", "MODEL-BYTES-FRESH"),
        ("200 OK", "TOKENIZER-BYTES-FRESH"),
    ]);
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    // 预置中断下载残骸：0 字节的 model.onnx + tokenizer.json。
    let data_dir = embedding_data_dir(&config_dir).join("test-model");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("model.onnx"), b"").unwrap();
    std::fs::write(data_dir.join("tokenizer.json"), b"").unwrap();

    let mut cfg = base_config("medium");
    set_model_urls(
        &mut cfg,
        &format!("http://127.0.0.1:{}/model.onnx", port),
        &format!("http://127.0.0.1:{}/tokenizer.json", port),
    );

    let (dir, dim) = download_model_files(&mut cfg, &config_dir).unwrap();
    assert_eq!(dim, 384);
    let data_dir = std::path::Path::new(&dir);
    // 旧代码：exists 短路 → 残骸原样保留（0 字节）→ 本断言红。
    assert_eq!(
        std::fs::read(data_dir.join("model.onnx")).unwrap(),
        b"MODEL-BYTES-FRESH"
    );
    assert_eq!(
        std::fs::read(data_dir.join("tokenizer.json")).unwrap(),
        b"TOKENIZER-BYTES-FRESH"
    );
}

/// 红证（download_file 层）：0 字节 dest 不再当已下载跳过——必须发起
/// 请求（死端口 → 诚实 Err）；旧代码 dest.exists() 直接 Ok 不碰网络。
#[test]
fn download_file_zero_byte_dest_is_redownloaded_not_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("debris.bin");
    std::fs::write(&dest, b"").unwrap();

    let dead = find_dead_port();
    let url = format!("http://127.0.0.1:{}/never", dead);
    let err = download_file(&url, &dest).unwrap_err();
    assert!(
        err.contains("download request failed"),
        "zero-byte dest must be re-downloaded, got: {err}"
    );
}

/// 空响应体（200 + 0 字节）诚实报错，不落 0 字节终文件。
/// 旧代码：写入 0 字节文件并 Ok（把残骸合法化）。
#[test]
fn download_file_empty_body_errors_without_materializing_file() {
    let port = spawn_http_server(vec![("200 OK", "")]);
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("out.bin");

    let err = download_file(&format!("http://127.0.0.1:{}/f", port), &dest).unwrap_err();
    assert!(err.contains("empty"), "got: {err}");
    assert!(!dest.exists(), "empty body must not materialize a file");
}

/// resolve_model_files 对 0 字节残骸不再报 Model found——诚实
/// 「模型文件未安装」，引导用户重装而不是把晦涩的插件 init 错误抛给上层。
#[test]
fn resolve_model_files_zero_byte_model_is_not_installed() {
    let ws = tempfile::tempdir().unwrap();
    let cfg_dir = ws.path().join("config");
    std::fs::create_dir(&cfg_dir).unwrap();
    let data_dir = embedding_data_dir(&cfg_dir).join("test-model");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("model.onnx"), b"").unwrap();

    let mut cfg = base_config("medium");
    {
        let mc = cfg.models.get_mut("medium").unwrap();
        mc.name = "test-model".into();
        mc.dimension = 384;
    }

    let err = resolve_model_files(&cfg, &cfg_dir).unwrap_err();
    assert!(
        err.contains("模型文件未安装"),
        "zero-byte debris must not resolve as installed, got: {err}"
    );
}

// ---------------------------------------------------------------------------
// BUG-20261101-278：零超时下载在半死上游上永久挂死——总超时回归测试
// （blocking::ClientBuilder 无 read_timeout，走总超时兜底；测试态
// DOWNLOAD_TOTAL_TIMEOUT=400ms，等待必须秒级以内）。
// ---------------------------------------------------------------------------

/// 半死上游①：接受连接、收走请求、永不回响应头（连接保持打开）。
fn serve_head_stall() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // 单请求 mock：accept 一次后 park 挂住（park 永不返回 → 连接保持打开，
    // 客户端等响应头直到总超时触发）。
    std::thread::spawn(move || {
        use std::io::Read;
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf);
        loop {
            std::thread::park();
        }
    });
    port
}

/// 半死上游②：回完整响应头 + 半截 body 后断流（Content-Length 说谎）。
fn serve_body_stall() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    // 单请求 mock：响应头 + 10/100 字节后 park 挂住（连接保持不发余量——
    // body 读半截 stall，总超时必须兜住）。
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        let Ok((mut s, _)) = listener.accept() else {
            return;
        };
        let mut buf = [0u8; 4096];
        let _ = s.read(&mut buf);
        let head = b"HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: 100\r\nConnection: close\r\n\r\n";
        let _ = s.write_all(head);
        let _ = s.write_all(&[0xABu8; 10]);
        let _ = s.flush();
        loop {
            std::thread::park();
        }
    });
    port
}

fn assert_stall_timeout(err: &str, elapsed: std::time::Duration) {
    // 下界 ≥300ms：证明确实等了 400ms 超时窗（连接被拒等瞬时错误毫秒级
    // 就返回，属别的失败形态）；上界 <5s：证明没有永久挂死（修复前此
    // 测试会挂死到测试框架超时）。reqwest 0.12 的 Display 不含 cause 链，
    // 「超时」细节断不了言，用耗时上下界代替。
    assert!(
        elapsed >= std::time::Duration::from_millis(300),
        "耗时 {:?} 过短，不像是等了超时窗（瞬时失败属别的错误形态）",
        elapsed
    );
    assert!(
        elapsed < std::time::Duration::from_secs(5),
        "必须在总超时窗内失败，实际耗时 {:?}",
        elapsed
    );
    assert!(
        err.contains("download request failed") || err.contains("failed to read download response"),
        "须是传输层错误而非其它错误: {err}"
    );
}

#[test]
fn download_file_errors_on_head_stall_instead_of_hanging() {
    let port = serve_head_stall();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.onnx");
    let started = std::time::Instant::now();
    let err = download_file(&format!("http://127.0.0.1:{}/model.onnx", port), &dest)
        .expect_err("半死上游必须报错而非永久挂起");
    assert_stall_timeout(&err, started.elapsed());
    assert!(!dest.exists(), "失败不得落盘");
}

#[test]
fn download_file_errors_on_body_stall_instead_of_hanging() {
    let port = serve_body_stall();
    let dir = tempfile::tempdir().unwrap();
    let dest = dir.path().join("model.onnx");
    let started = std::time::Instant::now();
    let err = download_file(&format!("http://127.0.0.1:{}/model.onnx", port), &dest)
        .expect_err("body 断流必须报错而非永久挂起");
    assert_stall_timeout(&err, started.elapsed());
    assert!(!dest.exists(), "失败不得落盘");
}
