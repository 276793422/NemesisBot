//! Additional coverage tests for embedding_config.
//!
//! Focuses on:
//! - Tier selection (default_active, get/get_mut on each tier)
//! - resolve_model_files error and success paths
//! - download_model_files no-op / non-existent tier
//! - embedding_data_dir parent/child resolution
//! - default_config_json roundtrip
//! - Partial JSON parsing (missing tiers)
//! - save_embedding_config with read-only dir

use super::*;

// ============================================================
// Defaults
// ============================================================

#[test]
fn test_default_active_value() {
    assert_eq!(default_active(), "medium");
}

#[test]
fn test_models_config_default_returns_all_tiers() {
    let m = ModelsConfig::default();
    assert_eq!(m.large.name, "bge-base-en-v1.5");
    assert_eq!(m.medium.name, "all-MiniLM-L6-v2");
    assert_eq!(m.small.name, "all-MiniLM-L4-v2");
}

#[test]
fn test_embedding_config_default_disabled() {
    let c = EmbeddingConfig::default();
    assert!(!c.enabled);
}

#[test]
fn test_default_large_dimensions() {
    let m = default_large();
    assert_eq!(m.dimension, 768);
    assert!(!m.model_url.is_empty());
    assert!(!m.tokenizer_url.is_empty());
}

#[test]
fn test_default_medium_dimensions_and_size() {
    let m = default_medium();
    assert_eq!(m.dimension, 384);
    assert_eq!(m.model_size, 90405214);
    assert_eq!(m.tokenizer_size, 466247);
}

#[test]
fn test_default_small_dimensions() {
    let m = default_small();
    assert_eq!(m.dimension, 256);
    assert_eq!(m.model_size, 60000000);
}

#[test]
fn test_model_config_default_is_all_empty() {
    let m = ModelConfig::default();
    assert!(m.name.is_empty());
    assert_eq!(m.dimension, 0);
    assert_eq!(m.model_size, 0);
    assert_eq!(m.tokenizer_size, 0);
    assert!(m.local_model_path.is_empty());
    assert!(m.local_tokenizer_path.is_empty());
}

// ============================================================
// default_config_json
// ============================================================

#[test]
fn test_default_config_json_is_valid() {
    let json = default_config_json();
    assert!(!json.is_empty());
    let parsed: EmbeddingConfig = serde_json::from_str(&json).unwrap();
    assert!(!parsed.enabled);
    assert_eq!(parsed.active, "medium");
    assert_eq!(parsed.models.large.dimension, 768);
    assert_eq!(parsed.models.medium.dimension, 384);
    assert_eq!(parsed.models.small.dimension, 256);
}

#[test]
fn test_default_config_json_contains_expected_urls() {
    let json = default_config_json();
    assert!(json.contains("bge-base-en-v1.5"));
    assert!(json.contains("all-MiniLM-L6-v2"));
    assert!(json.contains("all-MiniLM-L4-v2"));
    assert!(json.contains("hf-mirror.com"));
}

// ============================================================
// config_path
// ============================================================

#[test]
fn test_config_path_root() {
    let p = config_path(Path::new("/"));
    assert!(p.ends_with("config.enhanced_memory.json"));
}

#[test]
fn test_config_path_relative() {
    let p = config_path(Path::new("relative/dir"));
    assert!(p.ends_with("config.enhanced_memory.json"));
}

#[test]
fn test_config_path_empty_dir() {
    let p = config_path(Path::new(""));
    assert!(p.ends_with("config.enhanced_memory.json"));
}

// ============================================================
// ModelsConfig::get / get_mut
// ============================================================

#[test]
fn test_models_get_mut_large() {
    let mut m = ModelsConfig::default();
    let mc = m.get_mut("large").unwrap();
    mc.dimension = 1000;
    assert_eq!(m.large.dimension, 1000);
}

#[test]
fn test_models_get_mut_small() {
    let mut m = ModelsConfig::default();
    let mc = m.get_mut("small").unwrap();
    mc.name = "custom-small".into();
    assert_eq!(m.small.name, "custom-small");
}

#[test]
fn test_models_get_mut_unknown_returns_none() {
    let mut m = ModelsConfig::default();
    assert!(m.get_mut("unknown").is_none());
}

#[test]
fn test_models_get_returns_correct_refs() {
    let m = ModelsConfig::default();
    assert_eq!(m.get("large").unwrap().dimension, 768);
    assert_eq!(m.get("medium").unwrap().dimension, 384);
    assert_eq!(m.get("small").unwrap().dimension, 256);
}

// ============================================================
// embedding_data_dir
// ============================================================

#[test]
fn test_embedding_data_dir_with_parent() {
    // config_dir = workspace/config → parent = workspace.
    let cd = Path::new("/workspace/config");
    let dd = embedding_data_dir(cd);
    assert_eq!(dd, std::path::PathBuf::from("/workspace/tools/memory/data/embedding"));
}

#[test]
fn test_embedding_data_dir_no_parent() {
    // When config_dir has no parent, fallback uses config_dir itself.
    let cd = Path::new("config");
    let dd = embedding_data_dir(cd);
    // Just verify it ends with the expected suffix.
    assert!(dd.ends_with("tools/memory/data/embedding"));
}

#[test]
fn test_embedding_data_dir_relative() {
    let cd = Path::new("./ws/config");
    let dd = embedding_data_dir(cd);
    assert!(dd.ends_with("tools/memory/data/embedding"));
}

// ============================================================
// resolve_model_files
// ============================================================

#[test]
fn test_resolve_model_files_unknown_tier_error_message() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "nonexistent_tier".into();
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_model_files(&cfg, dir.path()).unwrap_err();
    assert!(err.contains("unknown active model tier"));
    assert!(err.contains("nonexistent_tier"));
}

#[test]
fn test_resolve_model_files_invalid_dimension() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "medium".into();
    cfg.models.medium.dimension = 0;
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_model_files(&cfg, dir.path()).unwrap_err();
    assert!(err.contains("invalid dimension"));
}

#[test]
fn test_resolve_model_files_negative_dimension() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "small".into();
    cfg.models.small.dimension = -10;
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_model_files(&cfg, dir.path()).unwrap_err();
    assert!(err.contains("invalid dimension"));
}

#[test]
fn test_resolve_model_files_empty_name() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "medium".into();
    cfg.models.medium.name = String::new();
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_model_files(&cfg, dir.path()).unwrap_err();
    assert!(err.contains("model name is empty"));
}

#[test]
fn test_resolve_model_files_not_downloaded_error() {
    let cfg = EmbeddingConfig::default();
    let dir = tempfile::tempdir().unwrap();
    let err = resolve_model_files(&cfg, dir.path()).unwrap_err();
    assert!(err.contains("模型文件未安装") || err.contains("not found") || err.contains("install"));
}

#[test]
fn test_resolve_model_files_finds_local_model_path() {
    let dir = tempfile::tempdir().unwrap();
    let model_dir = dir.path().join("custom-model-dir");
    std::fs::create_dir_all(&model_dir).unwrap();
    let model_file = model_dir.join("model.onnx");
    std::fs::write(&model_file, b"dummy").unwrap();

    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.local_model_path = model_file.to_string_lossy().to_string();
    let (resolved, dim) = resolve_model_files(&cfg, dir.path()).unwrap();
    assert_eq!(dim, 384);
    // Resolved dir should be the parent of model.onnx.
    assert!(Path::new(&resolved).ends_with("custom-model-dir"));
}

#[test]
fn test_resolve_model_files_finds_data_dir_model() {
    // Use a nested tempdir so the parent is unique (avoids cross-test
    // contamination via shared OS temp dir).
    let outer = tempfile::tempdir().unwrap();
    let cfg_dir = outer.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Create the model under {parent}/tools/memory/data/embedding/{model_name}/model.onnx
    let parent = outer.path();
    let model_dir = parent
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("all-MiniLM-L6-v2");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model.onnx"), b"x").unwrap();

    let cfg = EmbeddingConfig::default();
    let (resolved, dim) = resolve_model_files(&cfg, &cfg_dir).unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&resolved).exists());
}

#[test]
fn test_resolve_model_files_finds_config_dir_model() {
    // Use a nested tempdir so the parent is unique.
    let outer = tempfile::tempdir().unwrap();
    let cfg_dir = outer.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Put model directly in config dir.
    std::fs::write(cfg_dir.join("model.onnx"), b"x").unwrap();

    let cfg = EmbeddingConfig::default();
    let (resolved, dim) = resolve_model_files(&cfg, &cfg_dir).unwrap();
    assert_eq!(dim, 384);
    // Resolved should equal cfg_dir.
    assert_eq!(Path::new(&resolved), &cfg_dir);
}

#[test]
fn test_resolve_model_files_local_path_takes_precedence() {
    // When local_model_path exists AND data_dir also has model.onnx,
    // local_model_path should be used.
    let outer = tempfile::tempdir().unwrap();
    let cfg_dir = outer.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    let parent = outer.path();
    let data_dir_model = parent
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("all-MiniLM-L6-v2");
    std::fs::create_dir_all(&data_dir_model).unwrap();
    std::fs::write(data_dir_model.join("model.onnx"), b"data").unwrap();

    let local_dir = cfg_dir.join("local-overrides");
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(local_dir.join("model.onnx"), b"local").unwrap();

    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.local_model_path = local_dir.join("model.onnx").to_string_lossy().to_string();
    let (resolved, _) = resolve_model_files(&cfg, &cfg_dir).unwrap();
    assert_eq!(
        std::fs::read(Path::new(&resolved).join("model.onnx")).unwrap(),
        b"local".to_vec()
    );
}

// ============================================================
// download_model_files (no actual download — uses pre-existing files)
// ============================================================

#[test]
fn test_download_model_files_unknown_tier_errors() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "nonexistent".into();
    let dir = tempfile::tempdir().unwrap();
    let err = download_model_files(&mut cfg, dir.path()).unwrap_err();
    assert!(err.contains("unknown active model tier"));
}

#[test]
fn test_download_model_files_invalid_dimension_errors() {
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "medium".into();
    cfg.models.medium.dimension = 0;
    let dir = tempfile::tempdir().unwrap();
    let err = download_model_files(&mut cfg, dir.path()).unwrap_err();
    assert!(err.contains("invalid dimension"));
}

#[test]
fn test_download_model_files_empty_name_errors() {
    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.name = String::new();
    let dir = tempfile::tempdir().unwrap();
    let err = download_model_files(&mut cfg, dir.path()).unwrap_err();
    assert!(err.contains("model name is empty"));
}

#[test]
fn test_download_model_files_no_url_no_local_errors() {
    // No local file, no model in cfg_dir, no data_dir model, empty URL.
    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.model_url = String::new();
    let dir = tempfile::tempdir().unwrap();
    let err = download_model_files(&mut cfg, dir.path()).unwrap_err();
    assert!(err.contains("model file not found") || err.contains("no URL"));
}

#[test]
fn test_download_model_files_finds_existing_in_config_dir() {
    // Use a nested tempdir so parent is isolated.
    let outer = tempfile::tempdir().unwrap();
    let cfg_dir = outer.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Place model.onnx in config_dir, no download should happen.
    std::fs::write(cfg_dir.join("model.onnx"), b"existing").unwrap();

    let mut cfg = EmbeddingConfig::default();
    let (resolved, dim) = download_model_files(&mut cfg, &cfg_dir).unwrap();
    assert_eq!(dim, 384);
    assert_eq!(Path::new(&resolved), &cfg_dir);
    // local_model_path not updated (no download).
    assert!(cfg.models.medium.local_model_path.is_empty());
}

#[test]
fn test_download_model_files_finds_data_dir_model() {
    // Use a nested tempdir so parent is isolated.
    let outer = tempfile::tempdir().unwrap();
    let cfg_dir = outer.path().join("cfg");
    std::fs::create_dir_all(&cfg_dir).unwrap();
    // Place model in {parent}/tools/memory/data/embedding/{name}/model.onnx.
    let parent = outer.path();
    let model_dir = parent
        .join("tools")
        .join("memory")
        .join("data")
        .join("embedding")
        .join("all-MiniLM-L6-v2");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model.onnx"), b"x").unwrap();
    // Also write tokenizer.json so download isn't attempted.
    std::fs::write(model_dir.join("tokenizer.json"), b"{}").unwrap();

    let mut cfg = EmbeddingConfig::default();
    let (resolved, dim) = download_model_files(&mut cfg, &cfg_dir).unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&resolved).exists());
}

#[test]
fn test_download_model_files_with_local_model_path_no_download() {
    let cfg_dir = tempfile::tempdir().unwrap();
    let local_dir = cfg_dir.path().join("local-model");
    std::fs::create_dir_all(&local_dir).unwrap();
    std::fs::write(local_dir.join("model.onnx"), b"local-model-bytes").unwrap();

    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.local_model_path = local_dir.join("model.onnx").to_string_lossy().to_string();
    let (resolved, _) = download_model_files(&mut cfg, cfg_dir.path()).unwrap();
    // No download → local_model_path unchanged.
    assert!(cfg.models.medium.local_model_path.ends_with("model.onnx"));
    assert!(Path::new(&resolved).exists());
}

// ============================================================
// load_embedding_config edge cases
// ============================================================

#[test]
fn test_load_embedding_config_partial_json_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    // Only enabled present; rest defaults.
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();
    let cfg = load_embedding_config(dir.path());
    assert!(cfg.enabled);
    // Missing models → default models.
    assert_eq!(cfg.models.medium.dimension, 384);
}

#[test]
fn test_load_embedding_config_active_only() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"active": "large"}"#).unwrap();
    let cfg = load_embedding_config(dir.path());
    assert_eq!(cfg.active, "large");
    assert!(!cfg.enabled);
}

#[test]
fn test_load_embedding_config_empty_string_active() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    std::fs::write(&path, r#"{"active": ""}"#).unwrap();
    let cfg = load_embedding_config(dir.path());
    assert_eq!(cfg.active, "");
}

#[test]
fn test_load_embedding_config_array_value_invalid() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    // Wrong type for enabled → parse fails → default.
    std::fs::write(&path, r#"{"enabled": [1,2,3]}"#).unwrap();
    let cfg = load_embedding_config(dir.path());
    assert!(!cfg.enabled);
}

#[test]
fn test_load_embedding_config_with_full_models_section() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.enhanced_memory.json");
    let json = r#"{
        "enabled": true,
        "active": "medium",
        "models": {
            "large": {"name": "x", "dimension": 1024, "model_url": "http://x", "model_size": 1, "tokenizer_url": "http://t", "tokenizer_size": 1, "local_model_path": "", "local_tokenizer_path": ""},
            "medium": {"name": "y", "dimension": 512, "model_url": "", "model_size": 0, "tokenizer_url": "", "tokenizer_size": 0, "local_model_path": "", "local_tokenizer_path": ""},
            "small": {"name": "z", "dimension": 128, "model_url": "", "model_size": 0, "tokenizer_url": "", "tokenizer_size": 0, "local_model_path": "", "local_tokenizer_path": ""}
        }
    }"#;
    std::fs::write(&path, json).unwrap();
    let cfg = load_embedding_config(dir.path());
    assert_eq!(cfg.models.large.dimension, 1024);
    assert_eq!(cfg.models.medium.dimension, 512);
    assert_eq!(cfg.models.small.dimension, 128);
}

// ============================================================
// save_embedding_config round-trips
// ============================================================

#[test]
fn test_save_embedding_config_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let cfg = EmbeddingConfig::default();
    save_embedding_config(&cfg, dir.path());
    let path = dir.path().join("config.enhanced_memory.json");
    assert!(path.exists());
    let content = std::fs::read_to_string(&path).unwrap();
    assert!(content.contains("\"enabled\""));
    assert!(content.contains("\"active\""));
}

#[test]
fn test_save_then_load_preserves_active() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = EmbeddingConfig::default();
    cfg.active = "large".into();
    cfg.enabled = true;
    save_embedding_config(&cfg, dir.path());
    let loaded = load_embedding_config(dir.path());
    assert_eq!(loaded.active, "large");
    assert!(loaded.enabled);
}

#[test]
fn test_save_then_load_preserves_local_paths() {
    let dir = tempfile::tempdir().unwrap();
    let mut cfg = EmbeddingConfig::default();
    cfg.models.medium.local_model_path = "/some/path/model.onnx".into();
    cfg.models.medium.local_tokenizer_path = "/some/path/tok.json".into();
    save_embedding_config(&cfg, dir.path());
    let loaded = load_embedding_config(dir.path());
    assert_eq!(loaded.models.medium.local_model_path, "/some/path/model.onnx");
    assert_eq!(loaded.models.medium.local_tokenizer_path, "/some/path/tok.json");
}

#[test]
fn test_save_embedding_config_overwrites_existing() {
    let dir = tempfile::tempdir().unwrap();

    let mut cfg1 = EmbeddingConfig::default();
    cfg1.active = "small".into();
    save_embedding_config(&cfg1, dir.path());

    let mut cfg2 = EmbeddingConfig::default();
    cfg2.active = "large".into();
    save_embedding_config(&cfg2, dir.path());

    let loaded = load_embedding_config(dir.path());
    assert_eq!(loaded.active, "large");
}

// ============================================================
// JSON serialization edge cases
// ============================================================

#[test]
fn test_embedding_config_serialize_pretty() {
    let cfg = EmbeddingConfig::default();
    let json = serde_json::to_string_pretty(&cfg).unwrap();
    assert!(json.contains("\"enabled\": false"));
    assert!(json.contains("\"active\": \"medium\""));
}

#[test]
fn test_models_config_serialize() {
    let m = ModelsConfig::default();
    let json = serde_json::to_string(&m).unwrap();
    let parsed: ModelsConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.large.dimension, 768);
}

#[test]
fn test_model_config_serialize_with_paths() {
    let mut mc = ModelConfig::default();
    mc.local_model_path = "/x/model.onnx".into();
    mc.local_tokenizer_path = "/x/tok.json".into();
    let json = serde_json::to_string(&mc).unwrap();
    let parsed: ModelConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.local_model_path, "/x/model.onnx");
    assert_eq!(parsed.local_tokenizer_path, "/x/tok.json");
}

#[test]
fn test_model_config_with_u64_max_size() {
    let mut mc = ModelConfig::default();
    mc.model_size = u64::MAX;
    mc.tokenizer_size = u64::MAX;
    let json = serde_json::to_string(&mc).unwrap();
    let parsed: ModelConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.model_size, u64::MAX);
    assert_eq!(parsed.tokenizer_size, u64::MAX);
}

#[test]
fn test_models_config_get_mut_returns_correct_tier() {
    let mut m = ModelsConfig::default();
    let orig_large_name = m.large.name.clone();
    let mc = m.get_mut("large").unwrap();
    assert_eq!(mc.name, orig_large_name);
    mc.dimension = 999;
    assert_eq!(m.large.dimension, 999);
}

// ============================================================
// resolve_model_files large tier
// ============================================================

#[test]
fn test_resolve_model_files_large_tier_in_config_dir() {
    let cfg_dir = tempfile::tempdir().unwrap();
    std::fs::write(cfg_dir.path().join("model.onnx"), b"x").unwrap();

    let mut cfg = EmbeddingConfig::default();
    cfg.active = "large".into();
    let (resolved, dim) = resolve_model_files(&cfg, cfg_dir.path()).unwrap();
    assert_eq!(dim, 768);
    assert!(Path::new(&resolved).exists());
}

#[test]
fn test_resolve_model_files_small_tier_in_config_dir() {
    let cfg_dir = tempfile::tempdir().unwrap();
    std::fs::write(cfg_dir.path().join("model.onnx"), b"x").unwrap();

    let mut cfg = EmbeddingConfig::default();
    cfg.active = "small".into();
    let (resolved, dim) = resolve_model_files(&cfg, cfg_dir.path()).unwrap();
    assert_eq!(dim, 256);
    assert!(Path::new(&resolved).exists());
}
