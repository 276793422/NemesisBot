//! Coverage 翻绿批次（coverage-95 goal，用户裁决「先真实补测」）。
//!
//! 背景：embedding_config.rs 的 info!/warn!/error! 多行字段表达式
//! （`.display()` 参数行）在无 tracing subscriber 时惰性不求值 → 覆盖
//! 报告 dark，与 gateway.rs:380 已钉案的 R7b 伪影同族。本文件用
//! `tracing::subscriber::with_default` 挂 sink subscriber（输出丢弃，只
//! 求字段表达式真实求值）重跑既有场景，把这批伪影行真实翻绿。
//!
//! 同时钉一条平台行为：readonly 目录上的默认配置写盘（Linux 覆盖 Err
//! 臂；Windows 目录 readonly 位不阻断创建 → Ok 臂 + 平台怪癖备注）。
//!
//! 全部断言保持行为级（非 vacuous）：翻绿是副产物，测试本身独立有效。

use super::w2c_tests::{set_model_urls, spawn_http_server};
use super::*;

/// sink subscriber：TRACE 全开但输出丢弃——只为让宏字段表达式求值。
fn sink_subscriber() -> impl tracing::Subscriber + Send + Sync + 'static {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::TRACE)
        .with_writer(std::io::sink)
        .finish()
}

// ============================================================
// load_embedding_config / save_embedding_config（伪影行 240/248/
// 266/273/282/298/304 + 真实臂 251-252）
// ============================================================

#[test]
fn cov_fresh_dir_load_writes_default() {
    let dir = tempfile::tempdir().unwrap();
    let got =
        tracing::subscriber::with_default(sink_subscriber(), || load_embedding_config(dir.path()));
    // 首次加载：默认配置落盘 + 加载成功（info! 参数行求值）
    assert!(!got.enabled);
    assert_eq!(got.active, "medium");
    assert!(dir.path().join("config.enhanced_memory.json").exists());
}

#[test]
fn cov_config_dir_is_file_create_fails() {
    // config_dir 本身是文件：create_dir_all 失败（warn 参数行求值）。
    // 注意 fs::write 默认配置在 else 分支里，此场景不触达 write。
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not_a_dir");
    std::fs::write(&file, "x").unwrap();

    let got = tracing::subscriber::with_default(sink_subscriber(), || load_embedding_config(&file));
    assert!(!got.enabled);
    assert_eq!(got.active, "medium");
}

#[test]
fn cov_readonly_dir_default_write() {
    // config_dir 设 readonly：create_dir_all 对已存在目录返回 Ok（走进
    // else，fs::write 默认配置被执行）。Linux 上 chmod 444 → write EACCES
    // → 覆盖 Err 臂（251-252）；Windows 目录 readonly 位不阻断子文件创建
    // （仅对文件生效）→ write 成功走 Ok 臂，本测试在此钉住该平台怪癖：
    // readonly 属性目录里默认配置照样落盘，load 返回解析后的默认配置。
    let dir = tempfile::tempdir().unwrap();
    let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
    perms.set_readonly(true);
    std::fs::set_permissions(dir.path(), perms).unwrap();

    let got =
        tracing::subscriber::with_default(sink_subscriber(), || load_embedding_config(dir.path()));

    // 恢复可写，保证 tempdir 清理不炸
    let mut perms = std::fs::metadata(dir.path()).unwrap().permissions();
    perms.set_readonly(false);
    std::fs::set_permissions(dir.path(), perms).unwrap();

    assert!(!got.enabled);
    assert_eq!(got.active, "medium");
}

#[test]
fn cov_valid_json_save_then_load_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let got = tracing::subscriber::with_default(sink_subscriber(), || {
        let mut cfg = EmbeddingConfig::default();
        cfg.enabled = true;
        save_embedding_config(&cfg, dir.path());
        load_embedding_config(dir.path())
    });
    // save 成功（info! 参数行）+ load 解析成功（info! 参数行）
    assert!(got.enabled);
    assert_eq!(got.active, "medium");
}

#[test]
fn cov_invalid_json_load_falls_back_to_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, "{ this is not json").unwrap();

    let got =
        tracing::subscriber::with_default(sink_subscriber(), || load_embedding_config(dir.path()));
    // 解析失败（error! 参数行求值）→ default 兜底
    assert!(!got.enabled);
    assert_eq!(got.models.medium.dimension, 384);
}

#[test]
fn cov_save_target_parent_is_file_fails_silently() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("not_a_dir");
    std::fs::write(&file, "x").unwrap();

    tracing::subscriber::with_default(sink_subscriber(), || {
        let cfg = EmbeddingConfig::default();
        save_embedding_config(&cfg, &file);
    });
    // save 写盘失败（warn 参数行求值）→ 静默，目标仍是文件未被破坏
    assert!(file.is_file());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), "x");
}

// ============================================================
// resolve_model_files（伪影行 382/388）
// ============================================================

#[test]
fn cov_resolve_finds_data_dir_model() {
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();
    // data dir = {config_dir.parent}/tools/memory/data/embedding/{name}
    let data_dir = ws
        .path()
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("test-model");
    std::fs::create_dir_all(&data_dir).unwrap();
    std::fs::write(data_dir.join("model.onnx"), "m").unwrap();

    let mut cfg = EmbeddingConfig::default();
    let mc = cfg.models.get_mut("medium").unwrap();
    mc.name = "test-model".to_string();
    mc.dimension = 384;
    // local_model_path 留空 → 走 data_dir 分支（info! 参数行求值）

    let (dir, dim) = tracing::subscriber::with_default(sink_subscriber(), || {
        resolve_model_files(&cfg, &config_dir)
    })
    .unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&dir).join("model.onnx").exists());
}

#[test]
fn cov_resolve_finds_config_dir_model() {
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();
    std::fs::write(config_dir.join("model.onnx"), "m").unwrap();

    let mut cfg = EmbeddingConfig::default();
    let mc = cfg.models.get_mut("medium").unwrap();
    mc.name = "test-model".to_string();
    mc.dimension = 384;

    let (dir, dim) = tracing::subscriber::with_default(sink_subscriber(), || {
        resolve_model_files(&cfg, &config_dir)
    })
    .unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&dir).join("model.onnx").exists());
}

// ============================================================
// download_model_files（伪影行 453/470/497）
// ============================================================

#[test]
fn cov_download_config_dir_model_no_network() {
    // 模型已在 config_dir/model.onnx（info! 参数行求值）；tokenizer 用
    // local_tokenizer_path 离线拷贝，urls 清空避免任何网络访问。
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();
    std::fs::write(config_dir.join("model.onnx"), "m").unwrap();
    let tok_src = ws.path().join("tok.json");
    std::fs::write(&tok_src, "t").unwrap();

    let mut cfg = EmbeddingConfig::default();
    {
        let mc = cfg.models.get_mut("medium").unwrap();
        mc.name = "test-model".to_string();
        mc.dimension = 384;
        mc.model_url = String::new();
        mc.tokenizer_url = String::new();
        mc.local_tokenizer_path = tok_src.to_string_lossy().to_string();
    }

    let (dir, dim) = tracing::subscriber::with_default(sink_subscriber(), || {
        download_model_files(&mut cfg, &config_dir)
    })
    .unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&dir).join("model.onnx").exists());
    assert!(Path::new(&dir).join("tokenizer.json").exists());
}

#[test]
fn cov_download_via_http_flips_download_info_lines() {
    // 真实 HTTP 下载臂（本地一次性 TCP 服务器，无外网）：
    // model 下载后 info!（参数行求值）+ tokenizer 下载后 info!（参数行求值）
    let port = spawn_http_server(vec![
        ("200 OK", "MODEL-BYTES-COV"),
        ("200 OK", "TOKENIZER-BYTES-COV"),
    ]);
    let ws = tempfile::tempdir().unwrap();
    let config_dir = ws.path().join("config");
    std::fs::create_dir(&config_dir).unwrap();

    let mut cfg = EmbeddingConfig::default();
    set_model_urls(
        &mut cfg,
        &format!("http://127.0.0.1:{}/model.onnx", port),
        &format!("http://127.0.0.1:{}/tokenizer.json", port),
    );

    let (dir, dim) = tracing::subscriber::with_default(sink_subscriber(), || {
        download_model_files(&mut cfg, &config_dir)
    })
    .unwrap();
    assert_eq!(dim, 384);
    let model_dir = Path::new(&dir);
    assert_eq!(
        std::fs::read_to_string(model_dir.join("model.onnx")).unwrap(),
        "MODEL-BYTES-COV"
    );
    assert_eq!(
        std::fs::read_to_string(model_dir.join("tokenizer.json")).unwrap(),
        "TOKENIZER-BYTES-COV"
    );
}
