//! Forge-specific types for the self-learning framework.
//!
//! Re-exports core types from nemesis-types and adds forge-level wrappers.

use serde::{Deserialize, Serialize};

// Re-export the canonical types from nemesis-types so consumers can use
// `nemesis_forge::types::Experience` etc.
pub use nemesis_types::forge::{
    Artifact, ArtifactKind, ArtifactStatus, Experience, Reflection,
};

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
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RegistryConfig {
    /// Path to the JSON index file.
    pub index_path: String,
}

impl Default for RegistryConfig {
    fn default() -> Self {
        Self {
            index_path: String::from("forge_registry.json"),
        }
    }
}

/// Aggregated experience record (deduplicated pattern).
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AggregatedExperience {
    /// Pattern hash for deduplication.
    pub pattern_hash: String,
    /// Tool name.
    pub tool_name: String,
    /// Number of occurrences.
    pub count: u64,
    /// Average duration in milliseconds.
    pub avg_duration_ms: i64,
    /// Success rate (0.0 - 1.0).
    pub success_rate: f64,
    /// Last seen timestamp (ISO 8601).
    pub last_seen: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_collected_experience_serialization_roundtrip() {
        let exp = Experience {
            id: "exp-1".into(),
            tool_name: "file_read".into(),
            input_summary: "read config.json".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 42,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "sess-abc".into(),
        };
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "abc123".into(),
        };
        let json = serde_json::to_string(&ce).unwrap();
        let back: CollectedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dedup_hash, "abc123");
        assert_eq!(back.experience.tool_name, "file_read");
    }

    #[test]
    fn test_experience_stats_default_values() {
        let stats = ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: std::collections::HashMap::new(),
        };
        assert_eq!(stats.total_count, 0);
        assert!(stats.tool_counts.is_empty());
    }

    #[test]
    fn test_collector_config_default() {
        let cfg = CollectorConfig::default();
        assert_eq!(cfg.max_size, 10_000);
        assert!(cfg.persistence_path.is_empty());
    }

    #[test]
    fn test_registry_config_default() {
        let cfg = RegistryConfig::default();
        assert_eq!(cfg.index_path, "forge_registry.json");
    }

    #[test]
    fn test_collected_experience_clone() {
        let exp = Experience {
            id: "exp-clone".into(),
            tool_name: "test_tool".into(),
            input_summary: "input".into(),
            output_summary: "output".into(),
            success: true,
            duration_ms: 50,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_key: "sess".into(),
        };
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "hash123".into(),
        };
        let cloned = ce.clone();
        assert_eq!(cloned.dedup_hash, ce.dedup_hash);
        assert_eq!(cloned.experience.tool_name, ce.experience.tool_name);
    }

    #[test]
    fn test_aggregated_experience_serialization() {
        let agg = AggregatedExperience {
            pattern_hash: "abc123".into(),
            tool_name: "file_read".into(),
            count: 42,
            avg_duration_ms: 150,
            success_rate: 0.95,
            last_seen: "2026-04-29T12:00:00Z".into(),
        };
        let json = serde_json::to_string(&agg).unwrap();
        let back: AggregatedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pattern_hash, "abc123");
        assert_eq!(back.count, 42);
        assert!((back.success_rate - 0.95).abs() < 0.001);
    }

    #[test]
    fn test_tool_stats_serialization() {
        let stats = ToolStats {
            count: 10,
            success_count: 8,
            avg_duration_ms: 123.4,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: ToolStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.count, 10);
        assert_eq!(back.success_count, 8);
    }

    #[test]
    fn test_experience_stats_with_tools() {
        let mut tool_counts = std::collections::HashMap::new();
        tool_counts.insert("file_read".to_string(), ToolStats {
            count: 5,
            success_count: 5,
            avg_duration_ms: 100.0,
        });
        tool_counts.insert("file_write".to_string(), ToolStats {
            count: 3,
            success_count: 2,
            avg_duration_ms: 200.0,
        });

        let stats = ExperienceStats {
            total_count: 8,
            success_count: 7,
            failure_count: 1,
            avg_duration_ms: 137.5,
            tool_counts,
        };

        assert_eq!(stats.tool_counts.len(), 2);
        assert_eq!(stats.tool_counts.get("file_read").unwrap().count, 5);
        assert_eq!(stats.tool_counts.get("file_write").unwrap().success_count, 2);
    }

    #[test]
    fn test_collector_config_serialization_roundtrip() {
        let cfg = CollectorConfig {
            max_size: 5000,
            persistence_path: "/tmp/exp.jsonl".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CollectorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_size, 5000);
        assert_eq!(back.persistence_path, "/tmp/exp.jsonl");
    }

    #[test]
    fn test_registry_config_serialization_roundtrip() {
        let cfg = RegistryConfig {
            index_path: "custom_index.json".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RegistryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.index_path, "custom_index.json");
    }

    // --- Additional types tests ---

    #[test]
    fn test_experience_serialization_roundtrip() {
        let exp = Experience {
            id: "exp-roundtrip".into(),
            tool_name: "file_read".into(),
            input_summary: "read config.json".into(),
            output_summary: "ok".into(),
            success: true,
            duration_ms: 42,
            timestamp: "2026-04-29T00:00:00Z".into(),
            session_key: "sess-abc".into(),
        };
        let json = serde_json::to_string(&exp).unwrap();
        let back: Experience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.id, "exp-roundtrip");
        assert_eq!(back.tool_name, "file_read");
        assert!(back.success);
        assert_eq!(back.duration_ms, 42);
    }

    #[test]
    fn test_experience_failure_serialization() {
        let exp = Experience {
            id: "exp-fail".into(),
            tool_name: "file_write".into(),
            input_summary: "write /etc/passwd".into(),
            output_summary: "permission denied".into(),
            success: false,
            duration_ms: 10,
            timestamp: "2026-04-29T01:00:00Z".into(),
            session_key: "sess-xyz".into(),
        };
        let json = serde_json::to_string(&exp).unwrap();
        let back: Experience = serde_json::from_str(&json).unwrap();
        assert!(!back.success);
        assert_eq!(back.output_summary, "permission denied");
    }

    #[test]
    fn test_collected_experience_json_roundtrip() {
        let exp = Experience {
            id: "exp-json".into(),
            tool_name: "exec".into(),
            input_summary: "run build".into(),
            output_summary: "success".into(),
            success: true,
            duration_ms: 5000,
            timestamp: "2026-05-01T00:00:00Z".into(),
            session_key: "sess-json".into(),
        };
        let ce = CollectedExperience {
            experience: exp,
            dedup_hash: "sha256:abcdef1234567890".into(),
        };
        let json = serde_json::to_string_pretty(&ce).unwrap();
        let back: CollectedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.dedup_hash, "sha256:abcdef1234567890");
        assert_eq!(back.experience.tool_name, "exec");
        assert_eq!(back.experience.duration_ms, 5000);
    }

    #[test]
    fn test_experience_stats_zero_values() {
        let stats = ExperienceStats {
            total_count: 0,
            success_count: 0,
            failure_count: 0,
            avg_duration_ms: 0.0,
            tool_counts: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: ExperienceStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_count, 0);
        assert_eq!(back.avg_duration_ms, 0.0);
    }

    #[test]
    fn test_experience_stats_large_values() {
        let stats = ExperienceStats {
            total_count: 1_000_000,
            success_count: 990_000,
            failure_count: 10_000,
            avg_duration_ms: 1234.56,
            tool_counts: std::collections::HashMap::new(),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: ExperienceStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.total_count, 1_000_000);
        assert_eq!(back.failure_count, 10_000);
    }

    #[test]
    fn test_tool_stats_multiple_tools() {
        let mut tool_counts = std::collections::HashMap::new();
        tool_counts.insert("file_read".to_string(), ToolStats {
            count: 100,
            success_count: 98,
            avg_duration_ms: 15.5,
        });
        tool_counts.insert("file_write".to_string(), ToolStats {
            count: 50,
            success_count: 45,
            avg_duration_ms: 25.0,
        });
        tool_counts.insert("exec".to_string(), ToolStats {
            count: 20,
            success_count: 15,
            avg_duration_ms: 500.0,
        });
        let stats = ExperienceStats {
            total_count: 170,
            success_count: 158,
            failure_count: 12,
            avg_duration_ms: 100.0,
            tool_counts: tool_counts.clone(),
        };
        let json = serde_json::to_string(&stats).unwrap();
        let back: ExperienceStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.tool_counts.len(), 3);
        assert_eq!(back.tool_counts.get("exec").unwrap().count, 20);
    }

    #[test]
    fn test_collector_config_custom_values() {
        let cfg = CollectorConfig {
            max_size: 100,
            persistence_path: "/custom/path.jsonl".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CollectorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.max_size, 100);
        assert_eq!(back.persistence_path, "/custom/path.jsonl");
    }

    #[test]
    fn test_collector_config_empty_persistence() {
        let cfg = CollectorConfig {
            max_size: 500,
            persistence_path: String::new(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: CollectorConfig = serde_json::from_str(&json).unwrap();
        assert!(back.persistence_path.is_empty());
    }

    #[test]
    fn test_registry_config_custom_path() {
        let cfg = RegistryConfig {
            index_path: "/tmp/forge_index.json".into(),
        };
        let json = serde_json::to_string(&cfg).unwrap();
        let back: RegistryConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(back.index_path, "/tmp/forge_index.json");
    }

    #[test]
    fn test_aggregated_experience_all_fields() {
        let agg = AggregatedExperience {
            pattern_hash: "sha256:abc123".into(),
            tool_name: "file_read".into(),
            count: 100,
            avg_duration_ms: 150,
            success_rate: 0.95,
            last_seen: "2026-05-01T12:00:00Z".into(),
        };
        let json = serde_json::to_string(&agg).unwrap();
        let back: AggregatedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.pattern_hash, "sha256:abc123");
        assert_eq!(back.count, 100);
        assert!((back.success_rate - 0.95).abs() < 0.001);
        assert_eq!(back.last_seen, "2026-05-01T12:00:00Z");
    }

    #[test]
    fn test_aggregated_experience_zero_success_rate() {
        let agg = AggregatedExperience {
            pattern_hash: "hash".into(),
            tool_name: "failing_tool".into(),
            count: 10,
            avg_duration_ms: 200,
            success_rate: 0.0,
            last_seen: "2026-01-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&agg).unwrap();
        let back: AggregatedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.success_rate, 0.0);
    }

    #[test]
    fn test_aggregated_experience_perfect_success_rate() {
        let agg = AggregatedExperience {
            pattern_hash: "hash".into(),
            tool_name: "perfect_tool".into(),
            count: 50,
            avg_duration_ms: 10,
            success_rate: 1.0,
            last_seen: "2026-06-01T00:00:00Z".into(),
        };
        let json = serde_json::to_string(&agg).unwrap();
        let back: AggregatedExperience = serde_json::from_str(&json).unwrap();
        assert_eq!(back.success_rate, 1.0);
    }

    #[test]
    fn test_tool_stats_serialization_edge_cases() {
        let ts = ToolStats {
            count: 0,
            success_count: 0,
            avg_duration_ms: 0.0,
        };
        let json = serde_json::to_string(&ts).unwrap();
        let back: ToolStats = serde_json::from_str(&json).unwrap();
        assert_eq!(back.count, 0);
    }

    #[test]
    fn test_collected_experience_equality_by_value() {
        let exp = Experience {
            id: "same".into(),
            tool_name: "t".into(),
            input_summary: "i".into(),
            output_summary: "o".into(),
            success: true,
            duration_ms: 1,
            timestamp: "2026-01-01T00:00:00Z".into(),
            session_key: "s".into(),
        };
        let ce1 = CollectedExperience { experience: exp.clone(), dedup_hash: "h".into() };
        let ce2 = CollectedExperience { experience: exp, dedup_hash: "h".into() };
        assert_eq!(ce1.dedup_hash, ce2.dedup_hash);
        assert_eq!(ce1.experience.id, ce2.experience.id);
    }

    #[test]
    fn test_artifact_status_variants() {
        use nemesis_types::forge::ArtifactStatus;
        let statuses = [
            ArtifactStatus::Draft,
            ArtifactStatus::Active,
            ArtifactStatus::Observing,
            ArtifactStatus::Degraded,
            ArtifactStatus::Negative,
            ArtifactStatus::Archived,
        ];
        for i in 0..statuses.len() {
            for j in (i+1)..statuses.len() {
                assert_ne!(statuses[i], statuses[j]);
            }
        }
    }

    #[test]
    fn test_artifact_kind_variants() {
        use nemesis_types::forge::ArtifactKind;
        let kinds = [
            ArtifactKind::Skill,
            ArtifactKind::Script,
            ArtifactKind::Mcp,
        ];
        for i in 0..kinds.len() {
            for j in (i+1)..kinds.len() {
                assert_ne!(kinds[i], kinds[j]);
            }
        }
    }

    #[test]
    fn test_collector_config_default_values() {
        let cfg = CollectorConfig::default();
        assert_eq!(cfg.max_size, 10_000);
        assert!(cfg.persistence_path.is_empty());
    }

    #[test]
    fn test_registry_config_default_values() {
        let cfg = RegistryConfig::default();
        assert_eq!(cfg.index_path, "forge_registry.json");
    }
}
