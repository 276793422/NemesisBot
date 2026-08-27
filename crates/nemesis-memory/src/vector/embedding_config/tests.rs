use super::*;

#[test]
fn test_default_config_valid() {
    let config = EmbeddingConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.active, "medium");
    assert_eq!(config.models.medium.dimension, 384);
    assert_eq!(config.models.medium.name, "all-MiniLM-L6-v2");
    assert!(!config.models.medium.model_url.is_empty());
    assert_eq!(config.models.large.name, "bge-base-en-v1.5");
    assert_eq!(config.models.large.dimension, 768);
    assert_eq!(config.models.small.name, "all-MiniLM-L4-v2");
    assert_eq!(config.models.small.dimension, 256);
}

#[test]
fn test_load_config_default() {
    let temp_dir = tempfile::tempdir().unwrap();
    let config = load_embedding_config(temp_dir.path());
    assert!(!config.enabled);
    assert_eq!(config.active, "medium");
    assert_eq!(config.models.medium.dimension, 384);
    assert_eq!(config.models.large.dimension, 768);
    assert_eq!(config.models.small.dimension, 256);
    // Config file should have been created
    assert!(temp_dir.path().join("config.enhanced_memory.json").exists());
}

#[test]
fn test_save_and_reload_config() {
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = load_embedding_config(temp_dir.path());
    config.active = "small".to_string();
    config.enabled = true;
    save_embedding_config(&config, temp_dir.path());

    let reloaded = load_embedding_config(temp_dir.path());
    assert!(reloaded.enabled);
    assert_eq!(reloaded.active, "small");
}

#[test]
fn test_models_config_get() {
    let config = EmbeddingConfig::default();
    assert!(config.models.get("large").is_some());
    assert!(config.models.get("medium").is_some());
    assert!(config.models.get("small").is_some());
    assert!(config.models.get("unknown").is_none());
}

#[test]
fn test_models_config_get_mut() {
    let mut config = EmbeddingConfig::default();
    let mc = config.models.get_mut("medium").unwrap();
    assert_eq!(mc.dimension, 384);
    mc.dimension = 999;
    assert_eq!(config.models.medium.dimension, 999);
}

#[test]
fn test_config_path_helper() {
    let dir = Path::new("/tmp/test");
    let path = config_path(dir);
    assert_eq!(
        path,
        std::path::PathBuf::from("/tmp/test/config.enhanced_memory.json")
    );
}

#[test]
fn test_model_config_default() {
    let mc = ModelConfig::default();
    assert!(mc.name.is_empty());
    assert_eq!(mc.dimension, 0);
    assert!(mc.model_url.is_empty());
    assert!(mc.local_model_path.is_empty());
}

#[test]
fn test_resolve_model_files_unknown_tier() {
    let config = EmbeddingConfig::default();
    let mut bad_config = config.clone();
    bad_config.active = "nonexistent".to_string();
    let temp_dir = tempfile::tempdir().unwrap();
    let result = resolve_model_files(&bad_config, temp_dir.path());
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("unknown active model"));
}

#[test]
fn test_resolve_model_files_existing_local_paths() {
    let temp_dir = tempfile::tempdir().unwrap();
    let model_dir = temp_dir.path().join("test-model");
    std::fs::create_dir_all(&model_dir).unwrap();
    // Create dummy model and tokenizer files
    std::fs::write(model_dir.join("model.onnx"), b"dummy").unwrap();
    std::fs::write(model_dir.join("tokenizer.json"), b"{}").unwrap();

    let mut config = EmbeddingConfig::default();
    config.models.medium.local_model_path =
        model_dir.join("model.onnx").to_string_lossy().to_string();
    config.models.medium.local_tokenizer_path = model_dir
        .join("tokenizer.json")
        .to_string_lossy()
        .to_string();

    let (dir, dim) = resolve_model_files(&config, temp_dir.path()).unwrap();
    assert_eq!(dim, 384);
    assert!(Path::new(&dir).exists());
}

#[test]
fn test_json_roundtrip() {
    let config = EmbeddingConfig::default();
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: EmbeddingConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.active, config.active);
    assert_eq!(parsed.enabled, config.enabled);
    assert_eq!(parsed.models.medium.name, config.models.medium.name);
}

#[test]
fn test_parse_legacy_format_enabled_only() {
    // Old config.enhanced_memory.json with just {"enabled": true}
    let json = r#"{"enabled": true}"#;
    let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    // Missing fields get defaults
    assert_eq!(config.active, "medium");
    assert_eq!(config.models.large.name, "bge-base-en-v1.5");
}

#[test]
fn test_parse_with_extra_fields() {
    let json = r#"{"enabled": true, "active": "large", "extra": "ignored", "models": {}}"#;
    let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    assert_eq!(config.active, "large");
}

#[test]
fn test_auto_inject_defaults_off_and_top_k_3() {
    // Grey-release invariant (P3.1): absent fields default to auto_inject=false
    // (message stream byte-identical to pre-feature) and top_k=3.
    let config: EmbeddingConfig = serde_json::from_str("{}").unwrap();
    assert!(!config.auto_inject);
    assert_eq!(config.auto_inject_top_k, 3);
    assert_eq!(EmbeddingConfig::default().auto_inject, false);
    assert_eq!(EmbeddingConfig::default().auto_inject_top_k, 3);

    // Explicit values survive a roundtrip.
    let json = r#"{"auto_inject": true, "auto_inject_top_k": 5}"#;
    let config: EmbeddingConfig = serde_json::from_str(json).unwrap();
    assert!(config.auto_inject);
    assert_eq!(config.auto_inject_top_k, 5);
}

// ============================================================
// R1 coverage: load/save edge arms + resolve_model_files search order
// ============================================================

#[test]
fn test_load_creates_nested_config_dir_when_absent() {
    // Config dir itself missing entirely → loader materialises it plus the
    // default JSON (never hard-fails on first boot).
    let temp_dir = tempfile::tempdir().unwrap();
    let nested = temp_dir.path().join("a/b/c");
    assert!(!nested.exists());

    let config = load_embedding_config(&nested);
    assert_eq!(config.active, "medium");
    assert!(
        nested.join("config.enhanced_memory.json").exists(),
        "loader must write default config into the fresh dir"
    );
}

#[test]
fn test_load_returns_default_on_malformed_json() {
    let temp_dir = tempfile::tempdir().unwrap();
    std::fs::write(
        temp_dir.path().join("config.enhanced_memory.json"),
        b"{definitely not json",
    )
    .unwrap();

    let config = load_embedding_config(temp_dir.path());
    // Parse failure falls back to defaults instead of failing hard.
    assert_eq!(config.active, "medium");
    assert!(!config.enabled);
}

#[test]
fn test_resolve_model_files_searches_data_dir_when_no_local_path() {
    // No local_model_path → resolver probes {data}/<model_name>/model.onnx.
    //
    // BUG #34 lesson: embedding_data_dir anchors at config_dir.parent(). A
    // bare tempdir would resolve data into the SHARED %TEMP% root and pollute
    // every other test's "model missing" premise. Always nest a private
    // ws/config layout so the whole tree stays inside this test's tmpdir.
    let base = tempfile::tempdir().unwrap();
    let config_dir = base.path().join("ws").join("config");
    std::fs::create_dir_all(&config_dir).unwrap();

    let mut config = EmbeddingConfig::default();
    config.models.medium.local_model_path.clear();
    config.models.medium.local_tokenizer_path.clear();

    let data_model_dir =
        embedding_data_dir(&config_dir).join(&config.models.medium.name);
    assert!(
        data_model_dir.starts_with(base.path()),
        "data dir must stay inside the test tmpdir, got: {}",
        data_model_dir.display()
    );
    std::fs::create_dir_all(&data_model_dir).unwrap();
    std::fs::write(data_model_dir.join("model.onnx"), b"dummy").unwrap();

    let (model_dir, dim) = resolve_model_files(&config, &config_dir).unwrap();
    assert_eq!(dim, 384);
    assert_eq!(Path::new(&model_dir), data_model_dir.as_path());
}

#[test]
fn test_resolve_model_files_searches_config_dir_as_last_resort() {
    // Neither local path nor data dir hit → falls back to config_dir itself.
    let temp_dir = tempfile::tempdir().unwrap();
    let mut config = EmbeddingConfig::default();
    config.models.medium.local_model_path.clear();
    config.models.medium.local_tokenizer_path.clear();
    std::fs::write(temp_dir.path().join("model.onnx"), b"dummy").unwrap();

    let (model_dir, dim) = resolve_model_files(&config, temp_dir.path()).unwrap();
    assert_eq!(dim, 384);
    assert_eq!(Path::new(&model_dir), temp_dir.path());
}

#[test]
fn test_resolve_model_files_rejects_bad_dimension_and_empty_name() {
    let temp_dir = tempfile::tempdir().unwrap();

    let mut bad_dim = EmbeddingConfig::default();
    bad_dim.models.medium.dimension = -1;
    let err1 = resolve_model_files(&bad_dim, temp_dir.path()).unwrap_err();
    assert!(err1.contains("invalid dimension"), "{err1}");

    let mut empty_name = EmbeddingConfig::default();
    empty_name.models.medium.name.clear();
    let err2 = resolve_model_files(&empty_name, temp_dir.path()).unwrap_err();
    assert!(err2.contains("model name is empty"), "{err2}");
}
