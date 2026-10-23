//! Forge-specific types for the self-learning framework.
//!
//! Re-exports core types from nemesis-types and adds forge-level wrappers.

use serde::{Deserialize, Serialize};

// Re-export the canonical types from nemesis-types so consumers can use
// `nemesis_forge::types::Experience` etc.
pub use nemesis_types::forge::{Artifact, ArtifactKind, ArtifactStatus, Experience, Reflection};

/// Simple wrapper used by the collector for deduplication and storage.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectedExperience {
    pub experience: Experience,
    /// SHA-256 hex digest of (tool_name + ":" + sorted(arg_keys)), matching Go's ComputePatternHash.
    pub dedup_hash: String,
}

/// Summary statistics produced by the reflector during analysis.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExperienceStats {
    pub total_count: usize,
    pub success_count: usize,
    pub failure_count: usize,
    pub avg_duration_ms: f64,
    pub tool_counts: std::collections::HashMap<String, ToolStats>,
}

/// Per-tool statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolStats {
    pub count: usize,
    pub success_count: usize,
    pub avg_duration_ms: f64,
}

/// Configuration for the collector.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CollectorConfig {
    /// Maximum number of experiences to keep in memory.
    pub max_size: usize,
    /// Path to the JSONL persistence file (empty = no persistence).
    pub persistence_path: String,
}

impl Default for CollectorConfig {
    fn default() -> Self {
        Self {
            max_size: 10_000,
            persistence_path: String::new(),
        }
    }
}

/// Configuration for the registry.
#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct RegistryConfig {
    /// Path to the JSON index file.
    pub index_path: String,
}

// `AggregatedExperience`（聚合记录）2026-10-22 整体移除：聚合写入线
// （Collector::flush → aggregates.jsonl）与其读者面（ExperienceStore 的
// read_aggregated/get_top_patterns 家族）全仓零生产调用，是 write-only
// 死数据（审计 P2-a）。经验分析消费 `CollectedExperience`（read_all/
// read_recent）。

#[cfg(test)]
mod tests;
