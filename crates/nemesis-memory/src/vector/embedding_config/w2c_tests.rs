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
fn spawn_http_server(responses: Vec<(&'static str, &'static str)>) -> u16 {
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
fn find_dead_port() -> u16 {
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

fn set_model_urls(config: &mut EmbeddingConfig, model_url: &str, tokenizer_url: &str) {
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
    assert!(mc.local_model_path.ends_with("model.onnx"), "got {}", mc.local_model_path);
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
    assert!(
        err.contains("failed to create parent dir"),
        "got: {err}"
    );
}
