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
    assert_eq!(path, std::path::PathBuf::from("/tmp/test/config.enhanced_memory.json"));
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
    config.models.medium.local_model_path = model_dir.join("model.onnx").to_string_lossy().to_string();
    config.models.medium.local_tokenizer_path = model_dir.join("tokenizer.json").to_string_lossy().to_string();

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
