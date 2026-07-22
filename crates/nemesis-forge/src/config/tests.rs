use super::*;

#[test]
fn test_default_config() {
    let config = ForgeConfig::default();
    assert!(!config.enabled);
    assert!(!config.learning.enabled);
    assert!(config.collection.enabled);
    assert_eq!(config.collection.buffer_size, 256);
    assert_eq!(config.collection.flush_interval_secs, 30);
    assert_eq!(config.collection.max_experiences_per_day, 500);
    assert_eq!(config.collection.interval_secs, 300);
    assert_eq!(config.storage.max_experience_age_days, 90);
    assert_eq!(config.storage.max_report_age_days, 30);
    assert_eq!(config.storage.cleanup_interval_secs, 86400);
    assert_eq!(config.reflection.interval_secs, 21600);
    assert_eq!(config.reflection.min_experiences, 10);
    assert!(config.reflection.use_llm);
    assert_eq!(config.reflection.llm_budget_tokens, 4000);
    assert!(!config.artifacts.auto_skill);
    assert_eq!(config.artifacts.max_skills, 50);
    assert_eq!(config.artifacts.max_scripts, 100);
    assert!(config.validation.auto_validate);
    assert_eq!(config.validation.min_quality_score, 60);
    assert_eq!(config.validation.llm_max_tokens, 2000);
    assert_eq!(config.validation.timeout_secs, 60);
    assert!(config.trace.enabled);
    assert_eq!(config.trace.max_trace_age_days, 30);
    assert_eq!(config.trace.min_traces_for_analysis, 5);
    assert_eq!(config.learning.min_pattern_frequency, 5);
    assert_eq!(config.learning.high_conf_threshold, 0.8);
    assert_eq!(config.learning.max_auto_creates, 3);
    assert_eq!(config.learning.max_refine_rounds, 3);
    assert_eq!(config.learning.min_outcome_samples, 5);
    assert_eq!(config.learning.monitor_window_days, 7);
    assert_eq!(config.learning.degrade_threshold, -0.2);
    assert_eq!(config.learning.degradation_cooldown_days, 7);
    assert_eq!(config.learning.llm_budget_tokens, 2000);
}

#[test]
fn test_config_serialization_roundtrip() {
    let config = ForgeConfig::default();
    let json = serde_json::to_string(&config).unwrap();
    let back: ForgeConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.collection.interval_secs,
        config.collection.interval_secs
    );
    assert_eq!(
        back.learning.degrade_threshold,
        config.learning.degrade_threshold
    );
    assert_eq!(
        back.trace.max_trace_age_days,
        config.trace.max_trace_age_days
    );
}

#[test]
fn test_load_save_config() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.json");

    let config = ForgeConfig::default();
    save_forge_config(&path, &config).unwrap();

    let loaded = load_forge_config(&path);
    assert_eq!(
        loaded.collection.interval_secs,
        config.collection.interval_secs
    );
    assert_eq!(
        loaded.learning.degradation_cooldown_days,
        config.learning.degradation_cooldown_days
    );
}

#[test]
fn test_load_missing_file_returns_default() {
    let config = load_forge_config(std::path::Path::new("/nonexistent/forge.json"));
    assert!(!config.enabled);
}

#[test]
fn test_load_partial_json_uses_defaults() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.json");
    std::fs::write(&path, r#"{"enabled": true}"#).unwrap();

    let config = load_forge_config(&path);
    assert!(config.enabled);
    // Other fields should have defaults
    assert!(config.collection.enabled);
    assert_eq!(config.collection.buffer_size, 256);
}

#[test]
fn test_sanitize_fields_default() {
    let config = ForgeConfig::default();
    assert!(
        config
            .collection
            .sanitize_fields
            .contains(&"api_key".to_string())
    );
    assert!(
        config
            .collection
            .sanitize_fields
            .contains(&"password".to_string())
    );
    assert_eq!(config.collection.sanitize_fields.len(), 6);
}

// --- Additional config tests ---

#[test]
fn test_config_enabled_flag() {
    let mut config = ForgeConfig::default();
    assert!(!config.enabled);
    config.enabled = true;
    assert!(config.enabled);
}

#[test]
fn test_learning_config_defaults() {
    let lc = LearningConfig::default();
    assert!(!lc.enabled);
    assert_eq!(lc.min_pattern_frequency, 5);
    assert!((lc.high_conf_threshold - 0.8).abs() < 0.001);
    assert_eq!(lc.max_auto_creates, 3);
    assert_eq!(lc.max_refine_rounds, 3);
    assert_eq!(lc.min_outcome_samples, 5);
    assert_eq!(lc.monitor_window_days, 7);
    assert!((lc.degrade_threshold - (-0.2)).abs() < 0.001);
    assert_eq!(lc.degradation_cooldown_days, 7);
    assert_eq!(lc.llm_budget_tokens, 2000);
}

#[test]
fn test_collection_config_defaults() {
    let config = ForgeConfig::default();
    assert!(config.collection.enabled);
    assert_eq!(config.collection.buffer_size, 256);
    assert_eq!(config.collection.flush_interval_secs, 30);
    assert_eq!(config.collection.max_experiences_per_day, 500);
    assert_eq!(config.collection.interval_secs, 300);
}

#[test]
fn test_storage_config_defaults() {
    let config = ForgeConfig::default();
    assert_eq!(config.storage.max_experience_age_days, 90);
    assert_eq!(config.storage.max_report_age_days, 30);
    assert_eq!(config.storage.cleanup_interval_secs, 86400);
}

#[test]
fn test_reflection_config_defaults() {
    let config = ForgeConfig::default();
    assert_eq!(config.reflection.interval_secs, 21600);
    assert_eq!(config.reflection.min_experiences, 10);
    assert!(config.reflection.use_llm);
    assert_eq!(config.reflection.llm_budget_tokens, 4000);
    assert_eq!(config.reflection.max_report_age_days, 30);
}

#[test]
fn test_artifacts_config_defaults() {
    let config = ForgeConfig::default();
    assert!(!config.artifacts.auto_skill);
    assert_eq!(config.artifacts.max_skills, 50);
    assert_eq!(config.artifacts.max_scripts, 100);
    assert_eq!(config.artifacts.default_status, "draft");
}

#[test]
fn test_validation_config_defaults() {
    let config = ForgeConfig::default();
    assert!(config.validation.auto_validate);
    assert_eq!(config.validation.min_quality_score, 60);
    assert_eq!(config.validation.llm_max_tokens, 2000);
    assert_eq!(config.validation.timeout_secs, 60);
}

#[test]
fn test_trace_config_defaults() {
    let config = ForgeConfig::default();
    assert!(config.trace.enabled);
    assert_eq!(config.trace.max_trace_age_days, 30);
    assert_eq!(config.trace.min_traces_for_analysis, 5);
}

#[test]
fn test_config_json_roundtrip_custom_values() {
    let mut config = ForgeConfig::default();
    config.enabled = true;
    config.learning.enabled = true;
    config.learning.high_conf_threshold = 0.95;
    config.collection.max_experiences_per_day = 1000;
    config.artifacts.auto_skill = true;
    config.artifacts.max_skills = 200;

    let json = serde_json::to_string(&config).unwrap();
    let back: ForgeConfig = serde_json::from_str(&json).unwrap();
    assert!(back.enabled);
    assert!(back.learning.enabled);
    assert!((back.learning.high_conf_threshold - 0.95).abs() < 0.001);
    assert_eq!(back.collection.max_experiences_per_day, 1000);
    assert!(back.artifacts.auto_skill);
    assert_eq!(back.artifacts.max_skills, 200);
}

#[test]
fn test_load_invalid_json_returns_default() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("forge.json");
    std::fs::write(&path, "this is not json").unwrap();
    let config = load_forge_config(&path);
    assert!(!config.enabled);
}

#[test]
fn test_save_creates_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("subdir").join("nested").join("forge.json");
    let config = ForgeConfig::default();
    save_forge_config(&path, &config).unwrap();
    assert!(path.exists());
}

#[test]
fn test_learning_config_serialization() {
    let lc = LearningConfig {
        enabled: true,
        min_pattern_frequency: 10,
        high_conf_threshold: 0.9,
        max_auto_creates: 5,
        max_refine_rounds: 2,
        min_outcome_samples: 8,
        monitor_window_days: 14,
        degrade_threshold: -0.3,
        degradation_cooldown_days: 14,
        llm_budget_tokens: 3000,
    };
    let json = serde_json::to_string(&lc).unwrap();
    let back: LearningConfig = serde_json::from_str(&json).unwrap();
    assert!(back.enabled);
    assert_eq!(back.min_pattern_frequency, 10);
    assert!((back.high_conf_threshold - 0.9).abs() < 0.001);
}

#[test]
fn test_reflection_config_serialization() {
    let rc = ReflectionConfig {
        interval_secs: 3600,
        min_experiences: 5,
        use_llm: false,
        llm_budget_tokens: 1000,
        max_report_age_days: 14,
    };
    let json = serde_json::to_string(&rc).unwrap();
    let back: ReflectionConfig = serde_json::from_str(&json).unwrap();
    assert!(!back.use_llm);
    assert_eq!(back.interval_secs, 3600);
}
