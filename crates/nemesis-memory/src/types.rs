//! Memory data types and configuration.
//!
//! Re-exports core types from `nemesis_types::memory` and defines extended
//! types for search results, vector configuration, and graph/episodic entries.

use std::collections::HashMap;

use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

// Re-export the shared memory types so consumers only need this crate.
pub use nemesis_types::memory::{MemoryEntry, MemoryQueryResult, MemoryType};

/// Extended memory entry with metadata and timestamps using proper chrono types.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Entry {
    /// Unique identifier (UUID v4).
    pub id: String,
    /// Memory type classification.
    #[serde(rename = "type")]
    pub typ: MemoryType,
    /// Main content body.
    pub content: String,
    /// Arbitrary key-value metadata.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Free-form tags for categorisation.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Optional relevance / quality score in [0, 1].
    pub score: Option<f64>,
    /// When this entry was created.
    pub created_at: DateTime<Utc>,
    /// When this entry was last updated.
    pub updated_at: DateTime<Utc>,
}

impl Entry {
    /// Create a new entry with auto-generated ID and current timestamps.
    pub fn new(typ: MemoryType, content: String) -> Self {
        let now = Utc::now();
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            typ,
            content,
            metadata: HashMap::new(),
            tags: Vec::new(),
            score: None,
            created_at: now,
            updated_at: now,
        }
    }

    /// Builder-style method to attach tags.
    pub fn with_tags(mut self, tags: Vec<String>) -> Self {
        self.tags = tags;
        self
    }

    /// Builder-style method to attach metadata.
    pub fn with_metadata(mut self, metadata: HashMap<String, String>) -> Self {
        self.metadata = metadata;
        self
    }

    /// Builder-style method to set score.
    pub fn with_score(mut self, score: f64) -> Self {
        self.score = Some(score);
        self
    }
}

/// Result of a memory search query.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    /// Matching entries sorted by relevance (best first).
    pub entries: Vec<ScoredEntry>,
    /// Total number of matches (may exceed `entries.len()` if truncated).
    pub total: usize,
}

/// A memory entry together with its computed relevance score.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ScoredEntry {
    pub entry: Entry,
    pub score: f64,
}

/// Configuration for the optional vector search subsystem.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorConfig {
    /// Embedding tier: "auto" | "plugin" | "api" | "local".
    #[serde(default = "default_embedding_tier")]
    pub embedding_tier: String,

    /// Dimensionality of the local hash embedding.
    #[serde(default = "default_local_dim")]
    pub local_dim: usize,

    /// Path to a plugin DLL/SO (ONNX or similar).
    #[serde(default)]
    pub plugin_path: Option<String>,

    /// Path to the ONNX model file.
    #[serde(default)]
    pub plugin_model_path: Option<String>,

    /// Provider API embedding model name.
    #[serde(default)]
    pub api_model: Option<String>,

    /// Config directory for unified plugin interface (not serialized).
    #[serde(skip)]
    pub plugin_config_dir: Option<String>,

    /// Host services pointer for unified plugin interface (not serialized).
    #[serde(skip)]
    pub host_services: Option<*const nemesis_plugin::HostServices>,
}

// SAFETY: The HostServices pointer is read-only and valid for process lifetime.
unsafe impl Send for VectorConfig {}
unsafe impl Sync for VectorConfig {}

impl Default for VectorConfig {
    fn default() -> Self {
        Self {
            embedding_tier: default_embedding_tier(),
            local_dim: default_local_dim(),
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
            plugin_config_dir: None,
            host_services: None,
        }
    }
}

fn default_embedding_tier() -> String {
    "auto".to_string()
}

fn default_local_dim() -> usize {
    256
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entry_new_generates_valid_id() {
        let entry = Entry::new(MemoryType::LongTerm, "test content".to_string());
        assert!(!entry.id.is_empty());
        // UUID v4 format: 8-4-4-4-12
        assert_eq!(entry.id.len(), 36);
        assert_eq!(entry.typ, MemoryType::LongTerm);
        assert_eq!(entry.content, "test content");
    }

    #[test]
    fn entry_builder_methods_work() {
        let mut meta = HashMap::new();
        meta.insert("source".to_string(), "test".to_string());

        let entry = Entry::new(MemoryType::ShortTerm, "hello".to_string())
            .with_tags(vec!["greeting".to_string(), "test".to_string()])
            .with_metadata(meta)
            .with_score(0.95);

        assert_eq!(entry.tags.len(), 2);
        assert_eq!(entry.metadata.get("source").unwrap(), "test");
        assert!((entry.score.unwrap() - 0.95).abs() < f64::EPSILON);
        assert!(entry.created_at <= entry.updated_at);
    }

    #[test]
    fn memory_type_display() {
        assert_eq!(MemoryType::ShortTerm.to_string(), "short_term");
        assert_eq!(MemoryType::LongTerm.to_string(), "long_term");
        assert_eq!(MemoryType::Episodic.to_string(), "episodic");
        assert_eq!(MemoryType::Graph.to_string(), "graph");
        assert_eq!(MemoryType::Daily.to_string(), "daily");
    }

    #[test]
    fn vector_config_default_values() {
        let config = VectorConfig::default();
        assert_eq!(config.embedding_tier, "auto");
        assert_eq!(config.local_dim, 256);
        assert!(config.plugin_path.is_none());
        assert!(config.plugin_model_path.is_none());
        assert!(config.api_model.is_none());
    }

    #[test]
    fn entry_serialization_roundtrip() {
        let mut meta = HashMap::new();
        meta.insert("key1".to_string(), "value1".to_string());
        let entry = Entry::new(MemoryType::LongTerm, "test content".to_string())
            .with_tags(vec!["tag1".to_string(), "tag2".to_string()])
            .with_metadata(meta)
            .with_score(0.85);

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, entry.id);
        assert_eq!(deserialized.typ, entry.typ);
        assert_eq!(deserialized.content, entry.content);
        assert_eq!(deserialized.tags, entry.tags);
        assert!((deserialized.score.unwrap() - 0.85).abs() < f64::EPSILON);
        assert_eq!(deserialized.metadata.get("key1").unwrap(), "value1");
    }

    #[test]
    fn entry_default_metadata_and_tags() {
        let entry = Entry::new(MemoryType::ShortTerm, "hello".to_string());
        assert!(entry.metadata.is_empty());
        assert!(entry.tags.is_empty());
        assert!(entry.score.is_none());
    }

    #[test]
    fn entry_with_score_zero() {
        let entry = Entry::new(MemoryType::Daily, "daily note".to_string()).with_score(0.0);
        assert_eq!(entry.score.unwrap(), 0.0);
    }

    #[test]
    fn entry_with_score_one() {
        let entry = Entry::new(MemoryType::Episodic, "episodic".to_string()).with_score(1.0);
        assert_eq!(entry.score.unwrap(), 1.0);
    }

    #[test]
    fn entry_different_memory_types() {
        let types = vec![
            MemoryType::ShortTerm,
            MemoryType::LongTerm,
            MemoryType::Episodic,
            MemoryType::Graph,
            MemoryType::Daily,
        ];
        for mt in types {
            let entry = Entry::new(mt, format!("content for {:?}", mt));
            assert_eq!(entry.typ, mt);
        }
    }

    #[test]
    fn entry_unique_ids() {
        let e1 = Entry::new(MemoryType::LongTerm, "a".to_string());
        let e2 = Entry::new(MemoryType::LongTerm, "b".to_string());
        assert_ne!(e1.id, e2.id);
    }

    #[test]
    fn entry_timestamps_set() {
        let entry = Entry::new(MemoryType::LongTerm, "ts test".to_string());
        assert!(entry.created_at <= chrono::Utc::now());
        assert!(entry.updated_at <= chrono::Utc::now());
        assert!(entry.created_at <= entry.updated_at);
    }

    #[test]
    fn search_result_serialization() {
        let entry = Entry::new(MemoryType::LongTerm, "test".to_string());
        let sr = SearchResult {
            entries: vec![ScoredEntry { entry, score: 0.95 }],
            total: 1,
        };
        let json = serde_json::to_string(&sr).unwrap();
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.total, 1);
        assert_eq!(deserialized.entries.len(), 1);
        assert!((deserialized.entries[0].score - 0.95).abs() < f64::EPSILON);
    }

    #[test]
    fn search_result_empty() {
        let sr = SearchResult {
            entries: vec![],
            total: 0,
        };
        assert!(sr.entries.is_empty());
        assert_eq!(sr.total, 0);
        let json = serde_json::to_string(&sr).unwrap();
        let deserialized: SearchResult = serde_json::from_str(&json).unwrap();
        assert!(deserialized.entries.is_empty());
    }

    #[test]
    fn scored_entry_ordering() {
        let e1 = Entry::new(MemoryType::LongTerm, "a".to_string());
        let e2 = Entry::new(MemoryType::LongTerm, "b".to_string());
        let s1 = ScoredEntry { entry: e1, score: 0.9 };
        let s2 = ScoredEntry { entry: e2, score: 0.5 };
        assert!(s1.score > s2.score);
    }

    #[test]
    fn vector_config_custom() {
        let config = VectorConfig {
            embedding_tier: "plugin".to_string(),
            local_dim: 512,
            plugin_path: Some("/path/to/plugin".to_string()),
            plugin_model_path: Some("/path/to/model".to_string()),
            api_model: Some("text-embedding-3-small".to_string()),
            plugin_config_dir: None,
            host_services: None,
        };
        assert_eq!(config.embedding_tier, "plugin");
        assert_eq!(config.local_dim, 512);
        assert!(config.plugin_path.is_some());
    }

    #[test]
    fn vector_config_serialization_roundtrip() {
        let config = VectorConfig {
            embedding_tier: "local".to_string(),
            local_dim: 128,
            plugin_path: None,
            plugin_model_path: None,
            api_model: Some("my-model".to_string()),
            plugin_config_dir: None,
            host_services: None,
        };
        let json = serde_json::to_string(&config).unwrap();
        let deserialized: VectorConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.embedding_tier, "local");
        assert_eq!(deserialized.local_dim, 128);
        assert_eq!(deserialized.api_model, Some("my-model".to_string()));
    }

    #[test]
    fn entry_with_empty_tags() {
        let entry = Entry::new(MemoryType::LongTerm, "no tags".to_string()).with_tags(vec![]);
        assert!(entry.tags.is_empty());
    }

    #[test]
    fn entry_with_empty_metadata() {
        let entry = Entry::new(MemoryType::LongTerm, "no meta".to_string()).with_metadata(HashMap::new());
        assert!(entry.metadata.is_empty());
    }

    #[test]
    fn entry_content_with_special_chars() {
        let content = "Hello\n\t\"world\"\r\n{'key': 'value'}";
        let entry = Entry::new(MemoryType::LongTerm, content.to_string());
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content, content);
    }

    #[test]
    fn entry_content_unicode() {
        let content = "日本語テスト 🎉 Ñoño";
        let entry = Entry::new(MemoryType::LongTerm, content.to_string());
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: Entry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.content, content);
    }

    #[test]
    fn entry_content_very_long() {
        let content = "a".repeat(1_000_000);
        let entry = Entry::new(MemoryType::LongTerm, content.clone());
        assert_eq!(entry.content.len(), 1_000_000);
    }
}
