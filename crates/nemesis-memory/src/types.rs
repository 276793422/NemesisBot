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
    /// Embedding tier (kept for logging, no longer used for branching).
    #[serde(default = "default_embedding_tier")]
    pub embedding_tier: String,

    /// Path to a plugin DLL/SO (ONNX or similar).
    #[serde(default)]
    pub plugin_path: Option<String>,

    /// Config directory containing config.enhanced_memory.json (not serialized).
    #[serde(skip)]
    pub config_dir: Option<String>,

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
            plugin_path: None,
            config_dir: None,
            host_services: None,
        }
    }
}

fn default_embedding_tier() -> String {
    "plugin".to_string()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
