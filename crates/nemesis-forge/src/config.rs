//! Forge configuration types.
//!
//! Defines the full configuration hierarchy for the forge self-learning system:
//! collection intervals, reflection budgets, learning parameters, etc.
//!
//! Matches Go's ForgeConfig 1:1, including:
//! - `TraceConfig` (Phase 5 conversation-level traces)
//! - All fields in CollectionConfig, StorageConfig, ReflectionConfig, etc.
//! - `LoadForgeConfig` / `SaveForgeConfig` file I/O

use serde::{Deserialize, Serialize};
use std::path::Path;

/// Top-level forge configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ForgeConfig {
    /// Whether the forge system is enabled (main switch).
    #[serde(default)]
    pub enabled: bool,

    /// Collection configuration.
    #[serde(default)]
    pub collection: CollectionConfig,

    /// Storage / retention configuration.
    #[serde(default)]
    pub storage: StorageConfig,

    /// Reflection configuration.
    #[serde(default)]
    pub reflection: ReflectionConfig,

    /// Artifact generation configuration.
    #[serde(default)]
    pub artifacts: ArtifactsConfig,

    /// Validation pipeline configuration.
    #[serde(default)]
    pub validation: ValidationConfig,

    /// Trace collection configuration (Phase 5).
    #[serde(default)]
    pub trace: TraceConfig,

    /// Learning configuration (Phase 6 closed-loop).
    #[serde(default)]
    pub learning: LearningConfig,
}

impl Default for ForgeConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            collection: CollectionConfig::default(),
            storage: StorageConfig::default(),
            reflection: ReflectionConfig::default(),
            artifacts: ArtifactsConfig::default(),
            validation: ValidationConfig::default(),
            trace: TraceConfig::default(),
            learning: LearningConfig::default(),
        }
    }
}

/// Collection subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    /// Whether experience collection is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum buffer size before flushing.
    #[serde(default = "default_buffer_size")]
    pub buffer_size: usize,

    /// Interval between flush passes (seconds).
    #[serde(default = "default_flush_interval")]
    pub flush_interval_secs: u64,

    /// Maximum experiences to keep per day (0 = unlimited).
    #[serde(default = "default_max_exp_per_day")]
    pub max_experiences_per_day: usize,

    /// Fields to sanitize when collecting experiences.
    #[serde(default = "default_sanitize_fields")]
    pub sanitize_fields: Vec<String>,

    /// Maximum experiences to keep overall.
    #[serde(default = "default_max_experiences")]
    pub max_experiences: usize,

    /// Interval between collection passes (seconds).
    #[serde(default = "default_interval")]
    pub interval_secs: u64,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            buffer_size: default_buffer_size(),
            flush_interval_secs: default_flush_interval(),
            max_experiences_per_day: default_max_exp_per_day(),
            sanitize_fields: default_sanitize_fields(),
            max_experiences: default_max_experiences(),
            interval_secs: default_interval(),
        }
    }
}

fn default_true() -> bool {
    true
}

fn default_buffer_size() -> usize {
    256
}

fn default_flush_interval() -> u64 {
    30
}

fn default_max_exp_per_day() -> usize {
    500
}

fn default_sanitize_fields() -> Vec<String> {
    vec![
        "api_key".into(),
        "token".into(),
        "password".into(),
        "secret".into(),
        "credential".into(),
        "key".into(),
    ]
}

fn default_interval() -> u64 {
    300
}

fn default_max_experiences() -> usize {
    10_000
}

/// Storage / retention configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StorageConfig {
    /// Maximum age of experience data before cleanup (days).
    #[serde(default = "default_max_exp_age")]
    pub max_experience_age_days: u64,

    /// Maximum age of reflection reports before cleanup (days).
    #[serde(default = "default_max_report_age")]
    pub max_report_age_days: u64,

    /// Interval between cleanup passes (seconds).
    #[serde(default = "default_cleanup_interval")]
    pub cleanup_interval_secs: u64,
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            max_experience_age_days: default_max_exp_age(),
            max_report_age_days: default_max_report_age(),
            cleanup_interval_secs: default_cleanup_interval(),
        }
    }
}

fn default_max_exp_age() -> u64 {
    90
}

fn default_max_report_age() -> u64 {
    30
}

fn default_cleanup_interval() -> u64 {
    86400 // 24 hours
}

/// Reflection subsystem configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReflectionConfig {
    /// Interval between reflection passes (seconds).
    #[serde(default = "default_reflection_interval")]
    pub interval_secs: u64,

    /// Minimum number of experiences required before running reflection.
    #[serde(default = "default_min_experiences")]
    pub min_experiences: usize,

    /// Whether to use LLM for semantic analysis.
    #[serde(default = "default_true")]
    pub use_llm: bool,

    /// LLM budget tokens per reflection.
    #[serde(default = "default_llm_budget")]
    pub llm_budget_tokens: u32,

    /// Maximum age of reflection reports (days).
    #[serde(default = "default_max_report_age")]
    pub max_report_age_days: u64,
}

impl Default for ReflectionConfig {
    fn default() -> Self {
        Self {
            interval_secs: default_reflection_interval(),
            min_experiences: default_min_experiences(),
            use_llm: true,
            llm_budget_tokens: default_llm_budget(),
            max_report_age_days: default_max_report_age(),
        }
    }
}

fn default_reflection_interval() -> u64 {
    21600 // 6 hours
}

fn default_min_experiences() -> usize {
    10
}

fn default_llm_budget() -> u32 {
    4000
}

/// Artifact management configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ArtifactsConfig {
    /// Whether to automatically create skills from patterns.
    #[serde(default)]
    pub auto_skill: bool,

    /// Maximum number of skills to maintain.
    #[serde(default = "default_max_skills")]
    pub max_skills: usize,

    /// Maximum number of scripts to maintain.
    #[serde(default = "default_max_scripts")]
    pub max_scripts: usize,

    /// Default status for new artifacts.
    #[serde(default = "default_status")]
    pub default_status: String,
}

impl Default for ArtifactsConfig {
    fn default() -> Self {
        Self {
            auto_skill: false,
            max_skills: default_max_skills(),
            max_scripts: default_max_scripts(),
            default_status: default_status(),
        }
    }
}

fn default_max_skills() -> usize {
    50
}

fn default_max_scripts() -> usize {
    100
}

fn default_status() -> String {
    "draft".to_string()
}

/// Validation pipeline configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ValidationConfig {
    /// Whether to auto-validate on creation.
    #[serde(default = "default_true")]
    pub auto_validate: bool,

    /// Minimum quality score (0-100) for validation to pass.
    #[serde(default = "default_min_quality")]
    pub min_quality_score: u32,

    /// Maximum LLM tokens for validation evaluation.
    #[serde(default = "default_llm_max_tokens")]
    pub llm_max_tokens: u32,

    /// Timeout for validation in seconds.
    #[serde(default = "default_timeout")]
    pub timeout_secs: u64,
}

impl Default for ValidationConfig {
    fn default() -> Self {
        Self {
            auto_validate: true,
            min_quality_score: default_min_quality(),
            llm_max_tokens: default_llm_max_tokens(),
            timeout_secs: default_timeout(),
        }
    }
}

fn default_min_quality() -> u32 {
    60
}

fn default_llm_max_tokens() -> u32 {
    2000
}

fn default_timeout() -> u64 {
    60
}

/// Trace collection configuration (Phase 5: conversation-level traces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    /// Whether trace collection is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum age of trace data before cleanup (days).
    #[serde(default = "default_max_trace_age")]
    pub max_trace_age_days: u64,

    /// Minimum number of traces required for analysis.
    #[serde(default = "default_min_traces")]
    pub min_traces_for_analysis: usize,
}

impl Default for TraceConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_trace_age_days: default_max_trace_age(),
            min_traces_for_analysis: default_min_traces(),
        }
    }
}

fn default_max_trace_age() -> u64 {
    30
}

fn default_min_traces() -> usize {
    5
}

/// Learning configuration for Phase 6 closed-loop learning.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// Whether learning is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Minimum pattern frequency to trigger action (default 5).
    #[serde(default = "default_min_freq")]
    pub min_pattern_frequency: u32,

    /// High confidence threshold for auto-creation (default 0.8).
    #[serde(default = "default_high_conf")]
    pub high_conf_threshold: f64,

    /// Maximum auto-creates per cycle (default 3).
    #[serde(default = "default_max_auto")]
    pub max_auto_creates: u32,

    /// Maximum refine iterations (default 3).
    #[serde(default = "default_max_refine")]
    pub max_refine_rounds: u32,

    /// Minimum samples for evaluation (default 5).
    #[serde(default = "default_min_samples")]
    pub min_outcome_samples: u32,

    /// Observation window in days (default 7).
    #[serde(default = "default_monitor_window")]
    pub monitor_window_days: u32,

    /// Deprecation threshold (default -0.2).
    #[serde(default = "default_degrade_threshold")]
    pub degrade_threshold: f64,

    /// Cooldown before re-deprecating (default 7 days).
    #[serde(default = "default_degradation_cooldown")]
    pub degradation_cooldown_days: u32,

    /// Token budget for Skill draft generation (default 2000).
    #[serde(default = "default_learning_llm_budget")]
    pub llm_budget_tokens: u32,
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_pattern_frequency: default_min_freq(),
            high_conf_threshold: default_high_conf(),
            max_auto_creates: default_max_auto(),
            max_refine_rounds: default_max_refine(),
            min_outcome_samples: default_min_samples(),
            monitor_window_days: default_monitor_window(),
            degrade_threshold: default_degrade_threshold(),
            degradation_cooldown_days: default_degradation_cooldown(),
            llm_budget_tokens: default_learning_llm_budget(),
        }
    }
}

fn default_min_freq() -> u32 {
    5
}

fn default_high_conf() -> f64 {
    0.8
}

fn default_max_auto() -> u32 {
    3
}

fn default_max_refine() -> u32 {
    3
}

fn default_min_samples() -> u32 {
    5
}

fn default_monitor_window() -> u32 {
    7
}

fn default_degrade_threshold() -> f64 {
    -0.2
}

fn default_degradation_cooldown() -> u32 {
    7
}

fn default_learning_llm_budget() -> u32 {
    2000
}

// ----- File I/O (matching Go's LoadForgeConfig / SaveForgeConfig) -----

/// Load forge configuration from a JSON file.
///
/// If the file does not exist or cannot be read, returns the default config.
/// If the file contains partial JSON, missing fields use defaults.
pub fn load_forge_config(path: &Path) -> ForgeConfig {
    match std::fs::read_to_string(path) {
        Ok(data) => {
            let mut config = ForgeConfig::default();
            match serde_json::from_str::<ForgeConfig>(&data) {
                Ok(loaded) => config = loaded,
                Err(e) => {
                    tracing::warn!("Failed to parse forge config: {}", e);
                }
            }
            config
        }
        Err(_) => ForgeConfig::default(),
    }
}

/// Save forge configuration to a JSON file.
pub fn save_forge_config(path: &Path, config: &ForgeConfig) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    std::fs::write(path, data)
}

#[cfg(test)]
mod tests {
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
        assert_eq!(back.collection.interval_secs, config.collection.interval_secs);
        assert_eq!(back.learning.degrade_threshold, config.learning.degrade_threshold);
        assert_eq!(back.trace.max_trace_age_days, config.trace.max_trace_age_days);
    }

    #[test]
    fn test_load_save_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("forge.json");

        let config = ForgeConfig::default();
        save_forge_config(&path, &config).unwrap();

        let loaded = load_forge_config(&path);
        assert_eq!(loaded.collection.interval_secs, config.collection.interval_secs);
        assert_eq!(loaded.learning.degradation_cooldown_days, config.learning.degradation_cooldown_days);
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
        assert!(config.collection.sanitize_fields.contains(&"api_key".to_string()));
        assert!(config.collection.sanitize_fields.contains(&"password".to_string()));
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
}
