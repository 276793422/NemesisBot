//! Forge configuration types.
//!
//! Defines the full configuration hierarchy for the forge self-learning system:
//! collection intervals, reflection budgets, learning parameters, etc.
//!
//! Matches Go's ForgeConfig 1:1, including:
//! - `TraceConfig` (conversation-level traces)
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

    /// 失败重放自动入库开关（自进化升级 C 线）：false（默认）
    /// = 重放用例进待审区需确认；true = 自动进回归集。
    #[serde(default)]
    pub replay_auto_ingest: bool,

    /// A 线行为回归可调键（自进化升级）。
    #[serde(default)]
    pub test: TestConfig,

    /// D 线微优化可调键（SPO 精简版）。
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

    /// Trace collection configuration.
    #[serde(default)]
    pub trace: TraceConfig,

    /// Learning configuration (closed-loop).
    #[serde(default)]
    pub learning: LearningConfig,

    /// 学习对话配置（自学习铺路批次三）：四阶段对话引擎总闸与预算。
    #[serde(default)]
    pub learn: LearnConfig,
}

/// Collection subsystem configuration.
///
/// 对称清理：`enabled` / `buffer_size` / `flush_interval_secs` /
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
    /// ⚠ 保留键（核实未消费）：报告清理尚未实施，改它无效果。
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
    /// ⚠ 保留键（核实未消费）：反思循环对 ≥1 条经验即运行，
    /// 此闸未实施——且默认值 10 已被 auto-create 序列化进存量盘上配置，
    /// 接线会让存量部署 cycle 停摆到攒够条数，动它需连默认值一起重估。
    #[serde(default = "default_min_experiences")]
    pub min_experiences: usize,

    /// Whether to use LLM in the reflection cycle (semantic analysis AND
    /// the learning dialogue). `false` = 纯统计反思，循环内零 LLM 调用
    /// （learn.enabled 开而此键关时自动对话跳过并 warn——语义分析关而
    /// 对话开会自相矛盾）；gateway 与 CLI 同语义（CLI 侧该键决定是否挂
    /// provider）。auto_skill 蒸馏已退役（2026-11-24 批次三）。
    #[serde(default = "default_true")]
    pub use_llm: bool,

    /// LLM budget tokens per reflection.
    ///
    /// ⚠ 保留键（核实未消费）：实际预算统一走
    /// `reflector_llm::AUX_FORGE_MAX_TOKENS`（8192， 事故后上调
    /// ——低预算被 thinking 系模型烧穿、真机草稿全空），接线会重引该风险。
    #[serde(default = "default_llm_budget")]
    pub llm_budget_tokens: u32,

    /// Maximum age of reflection reports (days).
    ///
    /// ⚠ 保留键（核实未消费）：与 `storage.max_report_age_days`
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

/// A 线行为回归可调键（自进化升级）。
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

/// Trace collection configuration (conversation-level traces).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TraceConfig {
    /// Whether trace collection is enabled.
    #[serde(default = "default_true")]
    pub enabled: bool,

    /// Maximum age of trace data before cleanup (days).
    #[serde(default = "default_max_trace_age")]
    pub max_trace_age_days: u64,

    /// Minimum number of traces required for analysis (0 = no floor).
    ///
    /// 验证轮四接线：此前该键零消费者（死键）。窗口内轨迹少
    /// 于 N 条不做实效分析（样本太少结论即噪声）；0 = 不设下限（护栏
    /// 默认关）。
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
    0 // 护栏默认关（0=不限）； 验证轮四接线——此前该键零消费者
}

/// Learning configuration for the closed-loop 实效层（F 线：实效评估 +
/// 置信度调整 + 降级摘除）。
///
/// 线 A generate/execute 决策表已随自学习铺路批次三退役——六个决策表键
/// （high_conf_threshold / max_auto_creates / max_refine_rounds /
/// llm_budget_tokens / success_template_skill_threshold /
/// error_recovery_suggest_min）从结构体删除，存量盘上配置里的旧键由
/// serde 默认容忍（未知字段忽略不炸）；学什么、学成什么形态归学习对话
/// 引擎（`learn` 段），路由权不再在 Rust 决策表。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningConfig {
    /// Whether learning is enabled.
    #[serde(default)]
    pub enabled: bool,

    /// Minimum pattern frequency to trigger action (default 5).
    #[serde(default = "default_min_freq")]
    pub min_pattern_frequency: u32,

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
}

impl Default for LearningConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            min_pattern_frequency: default_min_freq(),
            min_outcome_samples: default_min_samples(),
            monitor_window_days: default_monitor_window(),
            degrade_threshold: default_degrade_threshold(),
            degradation_cooldown_days: default_degradation_cooldown(),
        }
    }
}

fn default_min_freq() -> u32 {
    5
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

/// 学习对话配置（自学习铺路批次三）：四阶段对话引擎（提议→定形→对账→
/// 定稿）的总闸与预算。模型解析链 = judge 链同构新臂（`learn.model` 显式
/// → `agents.small_model` → 主模型），见 nemesis-web `resolve_learn_caller`。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearnConfig {
    /// 学习对话总闸（forge.enabled 之上二级闸；默认关 = 交付后行为零变化
    /// ——自动周期不跑对话；CLI/WSAPI 手动触发是显式意志，不受此闸约束）。
    #[serde(default)]
    pub enabled: bool,

    /// 单次对话 token 预算：作为每阶段 LLM 调用的 max_tokens 下传；证据
    /// 包体积另由证据包组装器字符预算封顶（超限截证据包不截轮次）。
    #[serde(default = "default_learn_budget")]
    pub budget_tokens: u32,

    /// 同指纹复现转 active 阈值（InsightStore::with_recurrence 接线点）。
    #[serde(default = "default_activation_recurrence")]
    pub activation_recurrence: u32,

    /// 对话引擎显式模型别名；空 = `agents.small_model` → 主模型回落链。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
}

impl Default for LearnConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            budget_tokens: default_learn_budget(),
            activation_recurrence: default_activation_recurrence(),
            model: None,
        }
    }
}

fn default_learn_budget() -> u32 {
    8192
}

fn default_activation_recurrence() -> u32 {
    3
}

// ----- File I/O (matching Go's LoadForgeConfig / SaveForgeConfig) -----

/// Load forge configuration from a JSON file.
///
/// If the file does not exist or cannot be read, returns the default config.
/// If the file contains partial JSON, missing fields use defaults.
pub fn load_forge_config(path: &Path) -> ForgeConfig {
    match nemesis_utils::fsutil::read_config_string(path) {
        Ok(data) => {
            let mut config = ForgeConfig::default();
            match forge_config_from_str(&data) {
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

/// 解析（形状闸 + typed 解析），lenient loader 的统一解析通道。
///
/// BUG-20261122-003（BUG-20261120-030 同族横扫补位）：ForgeConfig 全字段
/// 带 default，`[]` 文档走 serde visit_seq 零元素臂 = 「全默认
/// ForgeConfig」**静默成功**——绕过下方 load_forge_config 的 lenient WARN
/// 臂（用户 forge 调参被静默无视，零诊断）。函数文档自述的「partial
/// JSON, missing fields use defaults」语义只覆盖 object 文档缺键；数组/
/// 标量是形状错，必须 Err 进 WARN 臂。语义单一真相源 =
/// nemesis_utils::json_shape。
pub(crate) fn forge_config_from_str(data: &str) -> std::result::Result<ForgeConfig, String> {
    let value: serde_json::Value = serde_json::from_str(data).map_err(|e| e.to_string())?;
    nemesis_utils::json_shape::ensure_object(&value, "forge config file")?;
    serde_json::from_value(value).map_err(|e| e.to_string())
}

/// Save forge configuration to a JSON file.
///
/// 原子落盘走 `write_file_atomic`（BUG-20261114-022）：本文件是 RMW 活配置
/// （replay_auto_ingest / judge 阈值 / optimize 预算），直写撕裂 + 下次
/// load 解析失败静默回默认 + 再保存把默认固化 = 用户 forge 调参永久丢失。
/// helper 自带 mkdir/唯一临时名/sync_all 先于 rename/失败清理。
pub fn save_forge_config(path: &Path, config: &ForgeConfig) -> std::io::Result<()> {
    let data = serde_json::to_string_pretty(config)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
    nemesis_utils::write_file_atomic(&path.to_string_lossy(), data.as_bytes(), 0o600)
        .map_err(std::io::Error::other)
}

#[cfg(test)]
mod tests;
