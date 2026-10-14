//! Forge self-learning types.

use serde::{Deserialize, Serialize};

/// Forge artifact types.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactKind {
    Skill,
    Script,
    Mcp,
}

/// Forge artifact in the registry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Artifact {
    pub id: String,
    pub name: String,
    pub kind: ArtifactKind,
    pub version: String,
    pub status: ArtifactStatus,
    pub content: String,
    pub tool_signature: Vec<String>,
    pub created_at: String,
    pub updated_at: String,
    pub usage_count: u64,
    /// Success rate (0.0 to 1.0), matching Go's `SuccessRate`.
    #[serde(default)]
    pub success_rate: f64,
    pub last_degraded_at: Option<String>,
    pub consecutive_observing_rounds: u32,
    /// 显式部署时间戳（自进化升级 F 线，2026-10-14）：实效对比 before/after
    /// 分割点。None = 回落 created_at（旧行为零变化）。SPO 迭代重部署时
    /// 由调用方刷新，使每次变异的实效窗独立。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub deployed_at: Option<String>,
    /// 最近一次实效对比快照（monitor evaluate 写回；None = 未评估）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub last_efficacy: Option<EfficacySnapshot>,
}

/// 实效对比快照（F 线）：deploy 前后成功率的窗口对比结论。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EfficacySnapshot {
    pub measured_at: String,
    pub success_before: f64,
    pub success_after: f64,
    pub samples_before: usize,
    pub samples_after: usize,
    /// monitor verdict：positive | negative | observing | insufficient_data
    pub verdict: String,
}

/// Artifact status in lifecycle.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ArtifactStatus {
    Draft,
    Active,
    Observing,
    Degraded,
    Negative,
    Archived,
}

/// Collected experience from tool usage.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct Experience {
    pub id: String,
    pub tool_name: String,
    pub input_summary: String,
    pub output_summary: String,
    pub success: bool,
    pub duration_ms: u64,
    pub timestamp: String,
    pub session_key: String,
}

/// Reflection report from analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Reflection {
    pub id: String,
    pub period_start: String,
    pub period_end: String,
    pub insights: Vec<String>,
    pub recommendations: Vec<String>,
    pub statistics: serde_json::Value,
    pub is_remote: bool,
}

/// Learning cycle record.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LearningCycle {
    pub id: String,
    pub started_at: String,
    pub completed_at: Option<String>,
    pub patterns_found: u32,
    pub actions_taken: u32,
    pub status: CycleStatus,
}

/// Learning cycle status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CycleStatus {
    Running,
    Completed,
    Failed,
}
