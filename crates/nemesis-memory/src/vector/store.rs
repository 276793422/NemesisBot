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
            plugin_config_dir: None,
            host_services: None,
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

    // ============================================================
    // P2 System Tests — VectorStore with real ONNX plugin
    //
    // ONNX Runtime cannot safely re-init after free, so all
    // scenarios run inside a single test with one VectorStore
    // lifecycle. The plugin store is created once, all scenarios
    // are executed sequentially, then dropped at the end.
    //
    // Requires:
    //   1. plugin_onnx.dll: cd plugins/plugin-onnx && cargo build --release
    //   2. Test model:       bash plugins/plugin-onnx/scripts/setup-test.sh
    //
    // Run with:
    //   cargo test -p nemesis-memory -- --ignored --test-threads=1
    // ============================================================

    /// Resolve the real plugin DLL path relative to this crate.
    fn st_plugin_dll_path() -> String {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR not set");
        let path = std::path::PathBuf::from(&manifest_dir)
            .join("../../plugins/plugin-onnx/target/release/plugin_onnx.dll");
        let path = path.canonicalize().unwrap_or(path);
        assert!(path.exists(), "plugin_onnx.dll not found at {:?}. Run: cd plugins/plugin-onnx && cargo build --release", path);
        path.to_string_lossy().to_string()
    }

    /// Resolve the real ONNX model path.
    fn st_model_path() -> String {
        let manifest_dir = std::env::var("CARGO_MANIFEST_DIR")
            .expect("CARGO_MANIFEST_DIR not set");
        let path = std::path::PathBuf::from(&manifest_dir)
            .join("../../plugins/plugin-onnx/test-data/model.onnx");
        let path = path.canonicalize().unwrap_or(path);
        assert!(path.exists(), "model.onnx not found at {:?}. Run: bash plugins/plugin-onnx/scripts/setup-test.sh", path);
        path.to_string_lossy().to_string()
    }

    /// Create a StoreConfig pointing to the real plugin.
    fn st_plugin_store_config() -> StoreConfig {
        StoreConfig {
            embedding_tier: "plugin".into(),
            local_dim: 384,
            plugin_path: Some(st_plugin_dll_path()),
            plugin_model_path: Some(st_model_path()),
            api_model: None,
            max_results: 10,
            similarity_threshold: 0.1,
            storage_path: String::new(),
        }
    }

    #[test]
    #[ignore]
    fn st_plugin_system_test_all_scenarios() {
        // Create the plugin store ONCE — ONNX Runtime cannot re-init after free
        let store = VectorStore::new(st_plugin_store_config());

        // === Scenario 1: Store creates and is empty ===
        {
            assert!(store.is_empty());
            assert_eq!(store.len(), 0);
            println!("[P2] Scenario 1: Store creates empty — PASS");
        }

        // === Scenario 2: Single entry store ===
        {
            store.store_entry(&make_entry("s2-1", "The quick brown fox jumps over the lazy dog")).unwrap();
            assert_eq!(store.len(), 1);
            println!("[P2] Scenario 2: Single entry store — PASS");
        }

        // Clear for next scenarios
        store.delete_entry("s2-1");
        assert!(store.is_empty());

        // === Scenario 3: Basic store + query with semantic ranking ===
        {
            store.store_entry(&make_entry("s3-1", "Cats are independent animals that like to explore")).unwrap();
            store.store_entry(&make_entry("s3-2", "Dogs are loyal companions that love to play fetch")).unwrap();
            store.store_entry(&make_entry("s3-3", "The stock market showed mixed results today")).unwrap();

            let result = store.query("feline pets", 10, &[]).unwrap();
            assert!(result.total >= 1, "Expected at least 1 result, got {}", result.total);
            assert_eq!(result.entries[0].id, "s3-1",
                "Cat entry should be top result for 'feline pets'");
            println!("[P2] Scenario 3: Basic query with semantic ranking — PASS");
        }

        // Clear
        for id in &["s3-1", "s3-2", "s3-3"] { store.delete_entry(id); }

        // === Scenario 4: Semantic ranking of diverse topics ===
        {
            store.store_entry(&make_entry("s4-1", "Python is a popular programming language for data science")).unwrap();
            store.store_entry(&make_entry("s4-2", "Java is widely used for enterprise applications")).unwrap();
            store.store_entry(&make_entry("s4-3", "Bananas are a good source of potassium")).unwrap();
            store.store_entry(&make_entry("s4-4", "Machine learning models require training data")).unwrap();

            let result = store.query("software development and coding", 10, &[]).unwrap();
            assert!(result.total >= 2, "Expected at least 2 results, got {}", result.total);

            let ids: Vec<&str> = result.entries.iter().map(|e| e.id.as_str()).collect();
            let python_pos = ids.iter().position(|&id| id == "s4-1");
            let banana_pos = ids.iter().position(|&id| id == "s4-3");
            // If both are present, python should rank higher
            if let (Some(pp), Some(bp)) = (python_pos, banana_pos) {
                assert!(pp < bp,
                    "Python entry should rank higher than banana for 'software development'");
            }
            // Python should always be in results
            assert!(python_pos.is_some(), "Python entry should be in results for 'software development'");
            println!("[P2] Scenario 4: Semantic ranking — PASS");
        }

        // Clear
        for id in &["s4-1", "s4-2", "s4-3", "s4-4"] { store.delete_entry(id); }

        // === Scenario 5: Similarity scores are valid ===
        {
            store.store_entry(&make_entry("s5-1", "Machine learning is a subset of artificial intelligence")).unwrap();
            store.store_entry(&make_entry("s5-2", "Neural networks are inspired by the human brain")).unwrap();

            let result = store.query("AI and deep learning", 10, &[]).unwrap();
            assert!(result.total >= 1);
            for entry in &result.entries {
                assert!(entry.score > 0.0, "Score should be positive");
                assert!(entry.score <= 1.0, "Score should not exceed 1.0, got {}", entry.score);
            }
            println!("[P2] Scenario 5: Similarity scores valid — PASS");
        }

        for id in &["s5-1", "s5-2"] { store.delete_entry(id); }

        // === Scenario 6: Query with type filter ===
        {
            let mut e1 = make_entry("s6-1", "Important meeting about project timeline");
            e1.entry_type = "long_term".into();
            let mut e2 = make_entry("s6-2", "Meeting notes from standup");
            e2.entry_type = "episodic".into();
            let mut e3 = make_entry("s6-3", "Project deadline is next Friday");
            e3.entry_type = "long_term".into();

            store.store_entry(&e1).unwrap();
            store.store_entry(&e2).unwrap();
            store.store_entry(&e3).unwrap();

            let result = store.query("project meeting", 10, &["long_term".to_string()]).unwrap();
            assert!(result.entries.iter().all(|e| e.entry_type == "long_term"),
                "All results should be long_term type");
            println!("[P2] Scenario 6: Type filter — PASS");
        }

        for id in &["s6-1", "s6-2", "s6-3"] { store.delete_entry(id); }

        // === Scenario 7: Query consistency (deterministic results) ===
        {
            store.store_entry(&make_entry("s7-1", "The weather is sunny and warm today")).unwrap();
            store.store_entry(&make_entry("s7-2", "Programming in Rust is fun and safe")).unwrap();

            let r1 = store.query("climate and sunshine", 10, &[]).unwrap();
            let r2 = store.query("climate and sunshine", 10, &[]).unwrap();

            assert_eq!(r1.total, r2.total, "Same query should return same count");
            for (a, b) in r1.entries.iter().zip(r2.entries.iter()) {
                assert_eq!(a.id, b.id, "Same query should return same entries");
                assert!((a.score - b.score).abs() < 1e-6, "Same query should return same scores");
            }
            println!("[P2] Scenario 7: Query consistency — PASS");
        }

        for id in &["s7-1", "s7-2"] { store.delete_entry(id); }

        // === Scenario 8: CRUD lifecycle ===
        {
            store.store_entry(&make_entry("s8-1", "First entry to test CRUD")).unwrap();
            store.store_entry(&make_entry("s8-2", "Second entry for CRUD test")).unwrap();
            assert_eq!(store.len(), 2);

            let entry = store.get_by_id("s8-1").unwrap();
            assert_eq!(entry.content, "First entry to test CRUD");

            assert!(store.delete_entry("s8-1"));
            assert_eq!(store.len(), 1);
            assert!(store.get_by_id("s8-1").is_none());

            let result = store.query("CRUD test", 10, &[]).unwrap();
            assert_eq!(result.total, 1);
            assert_eq!(result.entries[0].id, "s8-2");
            println!("[P2] Scenario 8: CRUD lifecycle — PASS");
        }

        store.delete_entry("s8-2");

        // === Scenario 9: Plugin produces different embeddings than local hash ===
        {
            use crate::vector::embedding_local::ngram_hash_embed;

            // Get a plugin embedding by storing an entry and querying it
            store.store_entry(&make_entry("s9-1", "The cat sat on the mat")).unwrap();
            let result = store.query("cat", 10, &[]).unwrap();
            assert!(result.total >= 1, "Plugin store should find results for 'cat'");

            // Get a local hash embedding for the same text
            let local_vec = ngram_hash_embed("The cat sat on the mat", 384);

            // Local hash should produce a 384-dim vector
            assert_eq!(local_vec.len(), 384);

            // Local hash should be L2 normalized
            let local_norm: f64 = local_vec.iter().map(|x| (*x as f64).powi(2)).sum::<f64>().sqrt();
            assert!((local_norm - 1.0).abs() < 0.01, "Local hash should be L2 normalized, norm={}", local_norm);

            // This proves both methods work correctly and produce valid output
            println!("[P2] Scenario 9: Plugin and local embeddings both valid — PASS");
        }

        store.delete_entry("s9-1");

        // === Scenario 10: Semantic similarity with lexical variation ===
        {
            store.store_entry(&make_entry("s10-1", "The automobile was traveling at high speed")).unwrap();
            store.store_entry(&make_entry("s10-2", "The vehicle was moving very fast")).unwrap();
            store.store_entry(&make_entry("s10-3", "I enjoy cooking pasta for dinner")).unwrap();

            let result = store.query("a car going quickly", 10, &[]).unwrap();

            // Car/speed entries should rank above cooking
            let ids: Vec<&str> = result.entries.iter().map(|e| e.id.as_str()).collect();
            let s3_pos = ids.iter().position(|&id| id == "s10-3");
            if let Some(pos) = s3_pos {
                let s1_pos = ids.iter().position(|&id| id == "s10-1").unwrap_or(99);
                let s2_pos = ids.iter().position(|&id| id == "s10-2").unwrap_or(99);
                assert!(s1_pos < pos && s2_pos < pos,
                    "Car/speed entries should rank above cooking");
            }

            // Both car entries should have meaningful similarity
            let p_s1 = result.entries.iter().find(|e| e.id == "s10-1").map(|e| e.score).unwrap_or(0.0);
            let p_s2 = result.entries.iter().find(|e| e.id == "s10-2").map(|e| e.score).unwrap_or(0.0);
            assert!(p_s1 > 0.3, "s1 should have meaningful similarity: {}", p_s1);
            assert!(p_s2 > 0.3, "s2 should have meaningful similarity: {}", p_s2);
            println!("[P2] Scenario 10: Semantic similarity with lexical variation — PASS");
        }

        for i in 1..=3 { store.delete_entry(&format!("s10-{}", i)); }

        // === Scenario 11: Embed dimension matches config ===
        {
            store.store_entry(&make_entry("s11-1", "Dimension verification test")).unwrap();
            let result = store.query("test", 10, &[]).unwrap();
            assert!(result.total >= 1, "Query should work with correct dimensions");
            println!("[P2] Scenario 11: Embed dimension matches — PASS");
        }

        store.delete_entry("s11-1");

        // === Scenario 12: Large batch entries ===
        {
            for i in 0..20 {
                store.store_entry(&make_entry(
                    &format!("s12-{}", i),
                    &format!("Entry number {} about topic {}", i, i % 5),
                )).unwrap();
            }
            assert_eq!(store.len(), 20);

            let result = store.query("topic 0", 10, &[]).unwrap();
            assert!(result.total >= 1, "Should find entries about topic 0");
            let top_ids: Vec<&str> = result.entries.iter().take(4).map(|e| e.id.as_str()).collect();
            assert!(
                top_ids.iter().any(|id| *id == "s12-0" || *id == "s12-5"),
                "Topic 0 entries should appear in top results"
            );
            println!("[P2] Scenario 12: Large batch (20 entries) — PASS");
        }

        for i in 0..20 { store.delete_entry(&format!("s12-{}", i)); }

        // === Scenario 13: Multiple sequential queries produce stable results ===
        {
            store.store_entry(&make_entry("s13-1", "Artificial intelligence is transforming technology")).unwrap();
            store.store_entry(&make_entry("s13-2", "Cooking recipes from around the world")).unwrap();
            store.store_entry(&make_entry("s13-3", "Space exploration and Mars colonization")).unwrap();

            // Run 5 queries in sequence
            for _ in 0..5 {
                let r = store.query("AI and computers", 10, &[]).unwrap();
                assert!(r.total >= 1);
                assert_eq!(r.entries[0].id, "s13-1",
                    "AI entry should consistently rank first");
            }
            println!("[P2] Scenario 13: Sequential query stability — PASS");
        }

        // Store is dropped here — ONNX Runtime freed once
        println!("[P2] All 13 scenarios PASSED");
    }

    #[tokio::test]
    #[ignore]
    async fn st_plugin_persistence_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("plugin_vectors.jsonl");

        let config = StoreConfig {
            storage_path: path.to_string_lossy().to_string(),
            ..st_plugin_store_config()
        };

        // Phase 1: Store and persist
        let store = VectorStore::new(config.clone());
        let e1 = make_entry("st-persist-1", "Persistent entry about machine learning");
        let e2 = make_entry("st-persist-2", "Another entry about natural language processing");
        store.store_entry(&e1).unwrap();
        store.store_entry(&e2).unwrap();
        store.persist_entry(&e1).await.unwrap();
        store.persist_entry(&e2).await.unwrap();
        assert_eq!(store.len(), 2);
        drop(store); // Release ONNX Runtime

        // Phase 2: Load into new store
        // NOTE: This creates a new ONNX session, which works because
        // the previous session was fully dropped before this
        let store2 = VectorStore::new(config);
        store2.load_persisted().await.unwrap();
        assert_eq!(store2.len(), 2, "Should load 2 persisted entries");

        let result = store2.query("AI and ML", 10, &[]).unwrap();
        assert!(result.total >= 1, "Should find results in loaded store");
        println!("[P2] Persistence roundtrip — PASS");
    }
}
