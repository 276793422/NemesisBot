//! Vector store - semantic search using local embeddings.
//!
//! Provides in-memory vector search with JSONL persistence. Entries are
//! embedded using the configured embedding function and stored for
//! similarity-based retrieval.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;

use crate::types::VectorConfig;
use crate::vector::embedding::new_embedding_func;
use crate::vector::embedding_local::cosine_similarity;

/// Configuration for the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Embedding tier.
    pub embedding_tier: String,
    /// Local hash dimension.
    pub local_dim: usize,
    /// Plugin path.
    pub plugin_path: Option<String>,
    /// Plugin model path.
    pub plugin_model_path: Option<String>,
    /// API model name.
    pub api_model: Option<String>,
    /// Maximum results per query.
    pub max_results: usize,
    /// Similarity threshold [0, 1].
    pub similarity_threshold: f64,
    /// Storage path for JSONL persistence.
    pub storage_path: String,
}

impl Default for StoreConfig {
    fn default() -> Self {
        Self {
            embedding_tier: "local".into(),
            local_dim: 256,
            plugin_path: None,
            plugin_model_path: None,
            api_model: None,
            max_results: 10,
            similarity_threshold: 0.7,
            storage_path: String::new(),
        }
    }
}

/// A memory entry for the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VectorEntry {
    /// Unique ID.
    pub id: String,
    /// Entry type.
    #[serde(rename = "type")]
    pub entry_type: String,
    /// Content text.
    pub content: String,
    /// Metadata key-value pairs.
    #[serde(default)]
    pub metadata: HashMap<String, String>,
    /// Tags.
    #[serde(default)]
    pub tags: Vec<String>,
    /// Similarity score (set during queries).
    #[serde(default)]
    pub score: f64,
    /// Creation time.
    pub created_at: String,
    /// Update time.
    pub updated_at: String,
}

/// Result of a vector search query.
#[derive(Debug, Clone)]
pub struct QueryResult {
    /// Matching entries.
    pub entries: Vec<VectorEntry>,
    /// Total matches.
    pub total: usize,
    /// Original query.
    pub query: String,
}

/// An in-memory document with pre-computed embedding.
struct IndexedDoc {
    entry: VectorEntry,
    embedding: Vec<f32>,
}

/// The vector store provides semantic search over embedded entries.
pub struct VectorStore {
    docs: RwLock<Vec<IndexedDoc>>,
    embed: Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync>,
    config: StoreConfig,
    persist_path: PathBuf,
}

impl VectorStore {
    /// Create a new vector store.
    pub fn new(config: StoreConfig) -> Self {
        let persist_path = if config.storage_path.is_empty() {
            PathBuf::from("memory/vector/vector_store.jsonl")
        } else {
            PathBuf::from(&config.storage_path)
        };

        let vector_config = VectorConfig {
            embedding_tier: config.embedding_tier.clone(),
            local_dim: config.local_dim,
            plugin_path: config.plugin_path.clone(),
            plugin_model_path: config.plugin_model_path.clone(),
            api_model: config.api_model.clone(),
        };

        let embed = new_embedding_func(&vector_config);

        let store = Self {
            docs: RwLock::new(Vec::new()),
            embed,
            config,
            persist_path,
        };

        // Load persisted data
        // (In a real async context we'd await this, but since new() is sync
        // we skip auto-loading here and provide load_persisted() separately)

        store
    }

    /// Store an entry in the vector store.
    pub fn store_entry(&self, entry: &VectorEntry) -> Result<(), String> {
        let embedding = (self.embed)(&entry.content)?;

        let doc = IndexedDoc {
            entry: entry.clone(),
            embedding,
        };

        self.docs.write().push(doc);
        Ok(())
    }

    /// Query the vector store for similar entries.
    pub fn query(
        &self,
        query: &str,
        limit: usize,
        type_filter: &[String],
    ) -> Result<QueryResult, String> {
        let query_embedding = (self.embed)(query)?;

        let limit = if limit <= 0 { self.config.max_results } else { limit };
        let threshold = self.config.similarity_threshold;

        let docs = self.docs.read();

        let mut scored: Vec<(f64, &IndexedDoc)> = docs
            .iter()
            .filter(|doc| {
                if type_filter.is_empty() {
                    true
                } else {
                    type_filter.contains(&doc.entry.entry_type)
                }
            })
            .filter_map(|doc| {
                let sim = cosine_similarity(&query_embedding, &doc.embedding);
                if sim >= threshold {
                    Some((sim, doc))
                } else {
                    None
                }
            })
            .collect();

        // Sort by similarity descending
        scored.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));

        let total = scored.len();
        let entries = scored
            .into_iter()
            .take(limit)
            .map(|(score, doc)| {
                let mut entry = doc.entry.clone();
                entry.score = score;
                entry
            })
            .collect();

        Ok(QueryResult {
            entries,
            total,
            query: query.to_string(),
        })
    }

    /// Get an entry by ID.
    pub fn get_by_id(&self, id: &str) -> Option<VectorEntry> {
        self.docs.read().iter().find(|d| d.entry.id == id).map(|d| d.entry.clone())
    }

    /// Delete an entry by ID.
    pub fn delete_entry(&self, id: &str) -> bool {
        let mut docs = self.docs.write();
        let before = docs.len();
        docs.retain(|d| d.entry.id != id);
        docs.len() < before
    }

    /// List entries with optional type filter and pagination.
    pub fn list_entries(
        &self,
        type_filter: &[String],
        offset: usize,
        limit: usize,
    ) -> QueryResult {
        let docs = self.docs.read();
        let filtered: Vec<&IndexedDoc> = docs
            .iter()
            .filter(|doc| {
                if type_filter.is_empty() {
                    true
                } else {
                    type_filter.contains(&doc.entry.entry_type)
                }
            })
            .collect();

        let total = filtered.len();
        let entries = filtered
            .into_iter()
            .skip(offset)
            .take(if limit > 0 { limit } else { usize::MAX })
            .map(|doc| doc.entry.clone())
            .collect();

        QueryResult {
            entries,
            total,
            query: String::new(),
        }
    }

    /// Return the number of stored entries.
    pub fn len(&self) -> usize {
        self.docs.read().len()
    }

    /// Return whether the store is empty.
    pub fn is_empty(&self) -> bool {
        self.docs.read().is_empty()
    }

    /// Load persisted entries from JSONL.
    pub async fn load_persisted(&self) -> Result<(), String> {
        if !self.persist_path.exists() {
            return Ok(());
        }

        let content = tokio::fs::read_to_string(&self.persist_path)
            .await
            .map_err(|e| e.to_string())?;

        for line in content.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<VectorEntry>(line) {
                let _ = self.store_entry(&entry);
            }
        }

        Ok(())
    }

    /// Persist an entry to JSONL.
    pub async fn persist_entry(&self, entry: &VectorEntry) -> Result<(), String> {
        if let Some(parent) = self.persist_path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| e.to_string())?;
        }

        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.persist_path)
            .await
            .map_err(|e| e.to_string())?;

        let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        line.push('\n');
        file.write_all(line.as_bytes()).await.map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_entry(id: &str, content: &str) -> VectorEntry {
        VectorEntry {
            id: id.into(),
            entry_type: "long_term".into(),
            content: content.into(),
            metadata: HashMap::new(),
            tags: vec![],
            score: 0.0,
            created_at: chrono::Utc::now().to_rfc3339(),
            updated_at: chrono::Utc::now().to_rfc3339(),
        }
    }

    #[test]
    fn test_store_and_query() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            ..Default::default()
        };
        let store = VectorStore::new(config);

        store.store_entry(&make_entry("1", "The cat sat on the mat")).unwrap();
        store.store_entry(&make_entry("2", "Dogs love to play fetch")).unwrap();
        store.store_entry(&make_entry("3", "Cat food is expensive")).unwrap();

        let result = store.query("cat", 10, &[]).unwrap();
        assert!(result.total >= 2, "Expected at least 2 results for 'cat', got {}", result.total);
    }

    #[test]
    fn test_get_by_id() {
        let store = VectorStore::new(StoreConfig::default());
        store.store_entry(&make_entry("test-id", "hello")).unwrap();

        let entry = store.get_by_id("test-id").unwrap();
        assert_eq!(entry.content, "hello");
    }

    #[test]
    fn test_delete_entry() {
        let store = VectorStore::new(StoreConfig::default());
        store.store_entry(&make_entry("del-me", "bye")).unwrap();
        assert_eq!(store.len(), 1);

        assert!(store.delete_entry("del-me"));
        assert!(store.is_empty());
    }

    #[test]
    fn test_list_entries() {
        let store = VectorStore::new(StoreConfig::default());
        store.store_entry(&make_entry("1", "a")).unwrap();
        store.store_entry(&make_entry("2", "b")).unwrap();
        store.store_entry(&make_entry("3", "c")).unwrap();

        let result = store.list_entries(&[], 1, 1);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.total, 3);
    }

    #[tokio::test]
    async fn test_persist_and_load() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("vectors.jsonl");
        let config = StoreConfig {
            storage_path: path.to_string_lossy().to_string(),
            ..Default::default()
        };

        let store = VectorStore::new(config);
        let entry = make_entry("persist-1", "persisted content");
        store.store_entry(&entry).unwrap();
        store.persist_entry(&entry).await.unwrap();

        assert!(path.exists());

        // Load into a new store
        let config2 = StoreConfig {
            storage_path: path.to_string_lossy().to_string(),
            ..Default::default()
        };
        let store2 = VectorStore::new(config2);
        store2.load_persisted().await.unwrap();
        assert!(!store2.is_empty());
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_store_config_default() {
        let config = StoreConfig::default();
        assert_eq!(config.embedding_tier, "local");
        assert_eq!(config.local_dim, 256);
        assert!(config.plugin_path.is_none());
        assert!(config.api_model.is_none());
        assert_eq!(config.max_results, 10);
        assert!((config.similarity_threshold - 0.7).abs() < f64::EPSILON);
    }

    #[test]
    fn test_store_is_empty_initially() {
        let store = VectorStore::new(StoreConfig::default());
        assert!(store.is_empty());
        assert_eq!(store.len(), 0);
    }

    #[test]
    fn test_store_get_by_id_not_found() {
        let store = VectorStore::new(StoreConfig::default());
        assert!(store.get_by_id("nonexistent").is_none());
    }

    #[test]
    fn test_delete_nonexistent_entry() {
        let store = VectorStore::new(StoreConfig::default());
        assert!(!store.delete_entry("nonexistent"));
    }

    #[test]
    fn test_query_empty_store() {
        let store = VectorStore::new(StoreConfig::default());
        let result = store.query("anything", 10, &[]).unwrap();
        assert_eq!(result.entries.len(), 0);
        assert_eq!(result.total, 0);
        assert_eq!(result.query, "anything");
    }

    #[test]
    fn test_query_with_type_filter() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            ..Default::default()
        };
        let store = VectorStore::new(config);

        let mut e1 = make_entry("1", "cat content");
        e1.entry_type = "long_term".into();
        let mut e2 = make_entry("2", "cat other");
        e2.entry_type = "episodic".into();

        store.store_entry(&e1).unwrap();
        store.store_entry(&e2).unwrap();

        let result = store.query("cat", 10, &["long_term".to_string()]).unwrap();
        assert!(result.entries.iter().all(|e| e.entry_type == "long_term"));
    }

    #[test]
    fn test_query_with_limit_zero_uses_default() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            max_results: 2,
            ..Default::default()
        };
        let store = VectorStore::new(config);

        for i in 0..5 {
            store.store_entry(&make_entry(&format!("{}", i), "similar content")).unwrap();
        }

        let result = store.query("content", 0, &[]).unwrap();
        assert!(result.entries.len() <= 2);
    }

    #[test]
    fn test_list_entries_empty() {
        let store = VectorStore::new(StoreConfig::default());
        let result = store.list_entries(&[], 0, 10);
        assert!(result.entries.is_empty());
        assert_eq!(result.total, 0);
    }

    #[test]
    fn test_list_entries_with_type_filter() {
        let store = VectorStore::new(StoreConfig::default());

        let mut e1 = make_entry("1", "a");
        e1.entry_type = "long_term".into();
        let mut e2 = make_entry("2", "b");
        e2.entry_type = "episodic".into();

        store.store_entry(&e1).unwrap();
        store.store_entry(&e2).unwrap();

        let result = store.list_entries(&["episodic".to_string()], 0, 10);
        assert_eq!(result.entries.len(), 1);
        assert_eq!(result.entries[0].entry_type, "episodic");
    }

    #[test]
    fn test_list_entries_pagination() {
        let store = VectorStore::new(StoreConfig::default());
        for i in 0..10 {
            store.store_entry(&make_entry(&format!("{}", i), &format!("entry {}", i))).unwrap();
        }

        let page1 = store.list_entries(&[], 0, 3);
        assert_eq!(page1.entries.len(), 3);
        assert_eq!(page1.total, 10);

        let page2 = store.list_entries(&[], 3, 3);
        assert_eq!(page2.entries.len(), 3);
        assert_eq!(page2.total, 10);
    }

    #[test]
    fn test_list_entries_no_limit() {
        let store = VectorStore::new(StoreConfig::default());
        for i in 0..5 {
            store.store_entry(&make_entry(&format!("{}", i), &format!("entry {}", i))).unwrap();
        }

        let result = store.list_entries(&[], 0, 0);
        assert_eq!(result.entries.len(), 5);
    }

    #[test]
    fn test_vector_entry_serialization() {
        let entry = make_entry("test-ser", "serialize me");
        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.id, "test-ser");
        assert_eq!(deserialized.content, "serialize me");
        assert_eq!(deserialized.entry_type, "long_term");
    }

    #[test]
    fn test_vector_entry_with_metadata() {
        let mut entry = make_entry("meta-1", "with metadata");
        entry.metadata.insert("source".into(), "test".into());
        entry.metadata.insert("count".into(), "42".into());

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.metadata.get("source").unwrap(), "test");
        assert_eq!(deserialized.metadata.get("count").unwrap(), "42");
    }

    #[test]
    fn test_vector_entry_with_tags() {
        let mut entry = make_entry("tag-1", "tagged entry");
        entry.tags.push("important".into());
        entry.tags.push("review".into());

        let json = serde_json::to_string(&entry).unwrap();
        let deserialized: VectorEntry = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.tags, vec!["important", "review"]);
    }

    #[test]
    fn test_store_multiple_entries_same_content() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            ..Default::default()
        };
        let store = VectorStore::new(config);

        store.store_entry(&make_entry("1", "same content")).unwrap();
        store.store_entry(&make_entry("2", "same content")).unwrap();
        store.store_entry(&make_entry("3", "same content")).unwrap();

        assert_eq!(store.len(), 3);
        let result = store.query("same content", 10, &[]).unwrap();
        assert_eq!(result.entries.len(), 3);
    }

    #[test]
    fn test_store_similarity_threshold() {
        let config = StoreConfig {
            similarity_threshold: 0.99, // Very high threshold
            ..Default::default()
        };
        let store = VectorStore::new(config);

        store.store_entry(&make_entry("1", "completely unique text about xyz")).unwrap();

        let result = store.query("something entirely different abc", 10, &[]).unwrap();
        // With 0.99 threshold, unrelated content should not match
        assert_eq!(result.total, 0);
    }

    #[test]
    fn test_query_results_sorted_by_score() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            ..Default::default()
        };
        let store = VectorStore::new(config);

        store.store_entry(&make_entry("1", "cat cat cat cat cat")).unwrap();
        store.store_entry(&make_entry("2", "dog dog dog dog dog")).unwrap();
        store.store_entry(&make_entry("3", "cat and dog together")).unwrap();

        let result = store.query("cat", 10, &[]).unwrap();
        for i in 1..result.entries.len() {
            assert!(result.entries[i - 1].score >= result.entries[i].score);
        }
    }

    #[tokio::test]
    async fn test_persist_multiple_entries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("multi.jsonl");
        let config = StoreConfig {
            storage_path: path.to_string_lossy().to_string(),
            ..Default::default()
        };

        let store = VectorStore::new(config);
        for i in 0..5 {
            let entry = make_entry(&format!("e{}", i), &format!("content {}", i));
            store.store_entry(&entry).unwrap();
            store.persist_entry(&entry).await.unwrap();
        }

        // Verify file has 5 lines
        let content = tokio::fs::read_to_string(&path).await.unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.trim().is_empty()).collect();
        assert_eq!(lines.len(), 5);
    }

    #[tokio::test]
    async fn test_load_nonexistent_file() {
        let config = StoreConfig {
            storage_path: "/nonexistent/path/store.jsonl".to_string(),
            ..Default::default()
        };
        let store = VectorStore::new(config);
        // Should return Ok since file doesn't exist
        let result = store.load_persisted().await;
        assert!(result.is_ok());
        assert!(store.is_empty());
    }

    #[tokio::test]
    async fn test_load_corrupted_jsonl() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("corrupt.jsonl");

        // Write a mix of valid and invalid JSON lines
        tokio::fs::write(&path, "invalid json line\n{\"id\":\"ok\",\"type\":\"t\",\"content\":\"c\",\"created_at\":\"2024-01-01\",\"updated_at\":\"2024-01-01\"}\n\n")
            .await
            .unwrap();

        let config = StoreConfig {
            storage_path: path.to_string_lossy().to_string(),
            ..Default::default()
        };
        let store = VectorStore::new(config);
        store.load_persisted().await.unwrap();

        // Only the valid entry should be loaded
        assert_eq!(store.len(), 1);
        assert_eq!(store.get_by_id("ok").unwrap().content, "c");
    }

    #[test]
    fn test_delete_entry_updates_len() {
        let store = VectorStore::new(StoreConfig::default());
        store.store_entry(&make_entry("1", "a")).unwrap();
        store.store_entry(&make_entry("2", "b")).unwrap();
        store.store_entry(&make_entry("3", "c")).unwrap();
        assert_eq!(store.len(), 3);

        store.delete_entry("2");
        assert_eq!(store.len(), 2);
        assert!(store.get_by_id("2").is_none());
    }

    #[test]
    fn test_store_entry_with_empty_content() {
        let store = VectorStore::new(StoreConfig::default());
        let entry = make_entry("empty", "");
        let result = store.store_entry(&entry);
        assert!(result.is_ok());
    }

    #[test]
    fn test_query_result_fields() {
        let config = StoreConfig {
            similarity_threshold: 0.1,
            ..Default::default()
        };
        let store = VectorStore::new(config);
        store.store_entry(&make_entry("1", "hello world")).unwrap();

        let result = store.query("hello", 5, &[]).unwrap();
        assert_eq!(result.query, "hello");
        assert!(result.total >= 1);
        assert!(result.entries[0].score > 0.0);
    }
}
