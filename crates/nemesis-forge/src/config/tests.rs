use super::*;

#[test]
fn test_default_config() {
    let config = ForgeConfig::default();
    assert!(!config.enabled);
    assert!(!config.learning.enabled);
    assert_eq!(config.collection.max_experiences, 10_000);
    assert_eq!(config.storage.max_experience_age_days, 90);
    assert_eq!(config.storage.max_report_age_days, 30);
    assert_eq!(config.storage.cleanup_interval_secs, 86400);
    assert_eq!(config.reflection.interval_secs, 21600);
    assert_eq!(config.reflection.min_experiences, 10);
    assert!(config.reflection.use_llm);
    assert_eq!(config.reflection.llm_budget_tokens, 4000);
    assert_eq!(config.artifacts.max_skills, 50);
    assert_eq!(config.artifacts.max_scripts, 100);
    assert!(config.validation.auto_validate);
    assert_eq!(config.validation.min_quality_score, 60);
    assert_eq!(config.validation.llm_max_tokens, 2000);
    assert_eq!(config.validation.timeout_secs, 60);
    assert!(config.trace.enabled);
    assert_eq!(config.trace.max_trace_age_days, 30);
    assert_eq!(config.trace.min_traces_for_analysis, 0);
    assert_eq!(config.learning.min_pattern_frequency, 5);
    assert_eq!(config.learning.min_outcome_samples, 5);
    assert_eq!(config.learning.monitor_window_days, 7);
    assert_eq!(config.learning.degrade_threshold, -0.2);
    assert_eq!(config.learning.degradation_cooldown_days, 7);
    // learn 段（批次三）：默认全关（交付后行为零变化）。
    assert!(!config.learn.enabled);
    assert_eq!(config.learn.budget_tokens, 8192);
    assert_eq!(config.learn.activation_recurrence, 3);
    assert_eq!(config.learn.model, None);
}

#[test]
fn test_config_serialization_roundtrip() {
    let config = ForgeConfig::default();
    let json = serde_json::to_string(&config).unwrap();
    let back: ForgeConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(
        back.collection.max_experiences,
        config.collection.max_experiences
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
        loaded.collection.max_experiences,
        config.collection.max_experiences
    );
    assert_eq!(
        loaded.learning.degradation_cooldown_days,
        config.learning.degradation_cooldown_days
    );
}

#[test]
fn test_load_missing_file_returns_default() {
    // tempdir 内的不存在路径——裸 `/nonexistent/...` 在 Windows 上解析到盘根。
    let dir = tempfile::tempdir().unwrap();
    let config = load_forge_config(&dir.path().join("nonexistent.json"));
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
    assert_eq!(config.collection.max_experiences, 10_000);
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
    assert_eq!(lc.min_outcome_samples, 5);
    assert_eq!(lc.monitor_window_days, 7);
    assert!((lc.degrade_threshold - (-0.2)).abs() < 0.001);
    assert_eq!(lc.degradation_cooldown_days, 7);
}

#[test]
fn test_collection_config_defaults() {
    let config = ForgeConfig::default();
    assert_eq!(config.collection.max_experiences, 10_000);
    assert_eq!(config.collection.sanitize_fields.len(), 6);
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
    // 验证轮四接线：护栏默认关（0=不限）。
    assert_eq!(config.trace.min_traces_for_analysis, 0);
}

#[test]
fn test_config_json_roundtrip_custom_values() {
    let mut config = ForgeConfig::default();
    config.enabled = true;
    config.learning.enabled = true;
    config.collection.max_experiences = 2000;
    config.learn.enabled = true;
    config.learn.budget_tokens = 4096;
    config.artifacts.max_skills = 200;

    let json = serde_json::to_string(&config).unwrap();
    let back: ForgeConfig = serde_json::from_str(&json).unwrap();
    assert!(back.enabled);
    assert!(back.learning.enabled);
    assert_eq!(back.collection.max_experiences, 2000);
    assert!(back.learn.enabled);
    assert_eq!(back.learn.budget_tokens, 4096);
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
        min_outcome_samples: 8,
        monitor_window_days: 14,
        degrade_threshold: -0.3,
        degradation_cooldown_days: 14,
    };
    let json = serde_json::to_string(&lc).unwrap();
    let back: LearningConfig = serde_json::from_str(&json).unwrap();
    assert!(back.enabled);
    assert_eq!(back.min_pattern_frequency, 10);
    assert_eq!(back.min_outcome_samples, 8);
    assert_eq!(back.monitor_window_days, 14);
    assert!((back.degrade_threshold - (-0.3)).abs() < 0.001);
    assert_eq!(back.degradation_cooldown_days, 14);
}

/// 线 A generate/execute 决策表退役（自学习铺路批次三）：存量盘上配置里
/// 的六个决策表键 + artifacts.auto_skill 由 serde 默认容忍（未知字段忽略
/// 不炸）——漂移锁，防未来有人给 struct 加 deny_unknown_fields。
#[test]
fn test_retired_learning_keys_tolerated_on_load() {
    let raw = r#"{"learning":{"enabled":true,"high_conf_threshold":0.8,"max_auto_creates":3,"max_refine_rounds":2,"llm_budget_tokens":3000,"success_template_skill_threshold":7,"error_recovery_suggest_min":0.35},"artifacts":{"auto_skill":true,"max_skills":50}}"#;
    let cfg: ForgeConfig = serde_json::from_str(raw).unwrap();
    assert!(cfg.learning.enabled);
    assert_eq!(cfg.learning.min_pattern_frequency, 5);
    assert_eq!(cfg.artifacts.max_skills, 50);

    let lc: LearningConfig = serde_json::from_str(
        r#"{"high_conf_threshold":0.9,"max_auto_creates":5,"llm_budget_tokens":3000}"#,
    )
    .unwrap();
    let d = LearningConfig::default();
    assert!(!lc.enabled);
    assert_eq!(lc.min_pattern_frequency, d.min_pattern_frequency);
    assert_eq!(lc.min_outcome_samples, d.min_outcome_samples);
    assert_eq!(lc.monitor_window_days, d.monitor_window_days);
    assert_eq!(lc.degrade_threshold, d.degrade_threshold);
    assert_eq!(lc.degradation_cooldown_days, d.degradation_cooldown_days);
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

// --- coverage additions (quality-hardening goal 冲刺) ---

#[test]
fn test_s8_collection_config_missing_fields_use_defaults() {
    // Every CollectionConfig field uses serde(default =...) — a JSON object
    // without any of them must resolve to the documented defaults.
    let cfg: CollectionConfig = serde_json::from_str("{}").unwrap();
    assert_eq!(cfg.max_experiences, 10_000);
    assert_eq!(cfg.sanitize_fields.len(), 6);
}

#[test]
fn test_s8_collection_config_unknown_keys_are_ignored() {
    // Retired keys (enabled/buffer_size/flush_interval_secs/
    // max_experiences_per_day/interval_secs, removed) in existing
    // on-disk configs must not break parsing — serde ignores unknown fields.
    let cfg: CollectionConfig = serde_json::from_str(
        r#"{"enabled": false, "buffer_size": 1, "flush_interval_secs": 99, "max_experiences_per_day": 7, "interval_secs": 300}"#,
    )
    .unwrap();
    assert_eq!(cfg.max_experiences, 10_000);
}

#[test]
fn test_s8_save_forge_config_creates_missing_parent_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("nested").join("deeper").join("config.json");
    save_forge_config(&path, &ForgeConfig::default()).unwrap();
    assert!(path.exists());
    // Round-trip to prove the written file is parseable.
    let loaded = load_forge_config(&path);
    assert_eq!(loaded.collection.max_experiences, 10_000);
}

#[test]
fn test_s8_save_forge_config_root_path_has_no_parent() {
    // A root path ("/") has no parent component: save_forge_config must take
    // the `if let Some(parent)` fall-through region, then fail to write onto
    // a directory. Deterministic Err on Windows and Unix, no side effect.
    let result = save_forge_config(std::path::Path::new("/"), &ForgeConfig::default());
    assert!(result.is_err());
}

/// BUG-20261114-022 回归锁：config.forge.json 是 RMW 活配置
/// （replay_auto_ingest / judge 阈值 / optimize 预算）——直写撕裂 +
/// load 解析失败静默回默认 + 再保存固化默认 = 用户 forge 调参永久丢失。
/// save_forge_config 必须走 `write_file_atomic`，生产代码零直写。
#[test]
fn save_forge_config_routes_through_atomic_helper() {
    let src: String = include_str!("../config.rs")
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    assert!(
        !src.contains("fs::write("),
        "config.rs 生产代码出现直写 fs::write（BUG-20261114-022 回归）：\
         forge 活配置必须走 write_file_atomic 原子落盘"
    );
    assert!(
        src.contains("nemesis_utils::write_file_atomic("),
        "save_forge_config 必须经 write_file_atomic 落盘（关键配置写入唯一权威 helper）"
    );
}

// ---- BUG-20261122-003：形状闸（visit_seq 零元素臂）回归锁 ----
//
// ForgeConfig 全字段带 default，`[]` 文档曾从 serde visit_seq 零元素臂
// 静默通过 = 「全默认 ForgeConfig」合法值——绕过 load_forge_config 的
// lenient WARN 臂，用户 forge 调参被静默无视零诊断。

#[test]
fn forge_config_from_str_rejects_array_document() {
    let err = forge_config_from_str("[]").unwrap_err();
    assert!(err.contains("must be a JSON object"), "err={err}");
    assert!(err.contains("array"), "err={err}");
}

#[test]
fn forge_config_from_str_rejects_scalar_documents() {
    for content in ["null", "\"forge\"", "42", "true"] {
        let err = forge_config_from_str(content).unwrap_err();
        assert!(
            err.contains("must be a JSON object"),
            "content={content} err={err}"
        );
    }
}

#[test]
fn forge_config_from_str_still_accepts_object_document() {
    // 最小 object（字段全 default，缺键合法）——闸不误伤。
    let cfg = forge_config_from_str(r#"{"enabled": true}"#).expect("object doc parses");
    assert!(cfg.enabled, "判别字段确从文档消费（非 vacuous Ok）");
    // 其余字段仍落 default。
    assert_eq!(cfg.collection.max_experiences, 10_000);
}

#[test]
fn forge_config_from_str_syntax_error_loud() {
    let err = forge_config_from_str("{not valid").unwrap_err();
    assert!(
        !err.contains("must be a JSON object"),
        "语法错误通道: {err}"
    );
}

/// BUG-20261128-021（CFG-08 同族横扫补位）：config.forge.json typed
/// round-trip 必须保留用户手加的未建模键。本文件是 RMW 活配置
/// （BUG-20261114-022 定性）——dashboard `forge.config.set` 走 raw Value
/// RMW 天然保真，而 CLI `forge enable/disable/optimize/replay-confirm`
/// 走 typed `save_forge_config` 全量覆写；无 flatten 时用户手编键
/// （含未来版本新键的前向兼容面）被任何一次 CLI 写回静默剥离。
/// ModelConfig.extra / SkillsFullConfig（CFG-08）/ PluginsHostConfig
/// 先例同款保真语义。
#[test]
fn test_typed_forge_save_roundtrip_preserves_user_keys() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.forge.json");
    let raw = serde_json::json!({
        "enabled": true,
        "replay_auto_ingest": true,
        "my_custom_key": { "nested": [1, 2, 3] },
        "another_future": "keep-me"
    });
    std::fs::write(&path, serde_json::to_string_pretty(&raw).unwrap()).unwrap();

    let cfg = load_forge_config(&path);
    assert!(cfg.enabled);
    assert!(cfg.replay_auto_ingest);
    save_forge_config(&path, &cfg).unwrap();

    let after: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
    assert_eq!(
        after["my_custom_key"]["nested"][2], 3,
        "用户手编嵌套键必须原样回吐"
    );
    assert_eq!(
        after["another_future"], "keep-me",
        "用户手编标量键必须原样回吐"
    );
    assert_eq!(after["enabled"], true);
    assert_eq!(after["replay_auto_ingest"], true);
}
