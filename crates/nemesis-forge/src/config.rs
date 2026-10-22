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
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ForgeConfig {
    /// Whether the forge system is enabled (main switch).
    #[serde(default)]
    pub enabled: bool,

    /// 失败重放自动入库开关（自进化升级 C 线，2026-10-14）：false（默认）
    /// = 重放用例进待审区需确认；true = 自动进回归集。
    #[serde(default)]
    pub replay_auto_ingest: bool,

    /// A 线行为回归可调键（自进化升级，2026-10-14）。
    #[serde(default)]
    pub test: TestConfig,

    /// D 线微优化可调键（SPO 精简版，2026-10-14）。
    #[serde(default)]
    pub optimize: OptimizeConfig,

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

/// Collection subsystem configuration.
///
/// 2026-10-22 对称清理：`enabled` / `buffer_size` / `flush_interval_secs` /
/// `max_experiences_per_day` / `interval_secs` 五个 write-only 键随聚合线
/// 移除——flush 循环、每日限额与「采集间隔」的唯一消费者就是被删的
/// collector_loop / append_aggregated（interval_secs 生前唯一消费点是 CLI
/// `forge status` 的展示打印，不驱动任何行为），`buffer_size` 与
/// `collection.enabled` 在此之前也已全仓零消费（采集开关只有 `forge.enabled`
/// 总闸一条真相源）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectionConfig {
    /// Fields to sanitize when collecting experiences.
    #[serde(default = "default_sanitize_fields")]
    pub sanitize_fields: Vec<String>,

    /// Maximum experiences to keep overall.
    #[serde(default = "default_max_experiences")]
    pub max_experiences: usize,
}

impl Default for CollectionConfig {
    fn default() -> Self {
        Self {
            sanitize_fields: default_sanitize_fields(),
            max_experiences: default_max_experiences(),
        }
    }
}

fn default_true() -> bool {
    true
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
    ///
    /// ⚠ 保留键（2026-10-15 核实未消费）：报告清理尚未实施，改它无效果。
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
    ///
    /// ⚠ 保留键（2026-10-15 核实未消费）：反思循环对 ≥1 条经验即运行，
    /// 此闸未实施——且默认值 10 已被 auto-create 序列化进存量盘上配置，
    /// 接线会让存量部署 cycle 停摆到攒够条数，动它需连默认值一起重估。
    #[serde(default = "default_min_experiences")]
    pub min_experiences: usize,

    /// Whether to use LLM in the reflection cycle (semantic analysis AND
    /// auto_skill distillation). `false` = 纯统计反思，循环内零 LLM 调用；
    /// gateway 与 CLI 同语义（CLI 侧该键决定是否挂 provider）。
    #[serde(default = "default_true")]
    pub use_llm: bool,

    /// LLM budget tokens per reflection.
    ///
    /// ⚠ 保留键（2026-10-15 核实未消费）：实际预算统一走
    /// `reflector_llm::AUX_FORGE_MAX_TOKENS`（8192，2026-09-28 事故后上调
    /// ——低预算被 thinking 系模型烧穿、真机草稿全空），接线会重引该风险。
    #[serde(default = "default_llm_budget")]
    pub llm_budget_tokens: u32,

    /// Maximum age of reflection reports (days).
    ///
    /// ⚠ 保留键（2026-10-15 核实未消费）：与 `storage.max_report_age_days`
    /// 重复且同样未实施，报告清理落地时二者需去重。
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

/// A 线行为回归可调键（自进化升级，2026-10-14）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TestConfig {
    /// judge 层缺省分阈值（用例未显式声明 threshold 时生效；0.0-1.0）。
    #[serde(default = "default_judge_threshold")]
    pub judge_threshold: f64,

    /// judge 模型：缺省 None = `agents.small_model` 通道（未配置回落主
    /// 模型）；显式别名 = 钉死该模型。回落链任一臂失败自动降下一臂，
    /// 绝不因缺配置失败。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub judge_model: Option<String>,
}

impl Default for TestConfig {
    fn default() -> Self {
        Self {
            judge_threshold: default_judge_threshold(),
            judge_model: None,
        }
    }
}

fn default_judge_threshold() -> f64 {
    0.7
}

/// D 线微优化可调键（SPO 精简版）。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OptimizeConfig {
    /// 单次改写输出 token 预算（CJK 保守 ×2 折算字符数做上限）。
    #[serde(default = "default_optimize_budget")]
    pub budget_tokens: u32,
}

impl Default for OptimizeConfig {
    fn default() -> Self {
        Self {
            budget_tokens: default_optimize_budget(),
        }
    }
}

fn default_optimize_budget() -> u32 {
    2000
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
    ///
    /// ⚠ 保留键（2026-10-15 核实未消费）：实际预算统一走
    /// `reflector_llm::AUX_FORGE_MAX_TOKENS`（8192，2026-09-28 事故后上调
    /// ——低预算被 thinking 系模型烧穿、真机两稿全空），接线会重引该风险。
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
                    tracing::warn!("[Config] Failed to parse forge config: {}", e);
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
mod tests;
