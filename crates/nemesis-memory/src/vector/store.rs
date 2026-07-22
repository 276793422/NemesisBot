//! Vector store - semantic search using local embeddings.
//!
//! Provides in-memory vector search with JSONL persistence. Entries are
//! embedded using the configured embedding function and stored for
//! similarity-based retrieval.

use std::collections::HashMap;
use std::path::PathBuf;

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::VectorConfig;
use crate::vector::embedding::new_embedding_func;

/// Configuration for the vector store.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoreConfig {
    /// Embedding tier.
    pub embedding_tier: String,
    /// Plugin path.
    pub plugin_path: Option<String>,
    /// Config directory containing config.enhanced_memory.json.
    #[serde(skip)]
    pub config_dir: Option<String>,
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
            embedding_tier: "plugin".into(),
            plugin_path: None,
            config_dir: None,
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

/// Compute cosine similarity between two vectors.
pub fn cosine_similarity(a: &[f32], b: &[f32]) -> f64 {
    if a.len() != b.len() || a.is_empty() {
        return 0.0;
    }

    let mut dot = 0.0f64;
    let mut norm_a = 0.0f64;
    let mut norm_b = 0.0f64;

    for i in 0..a.len() {
        dot += a[i] as f64 * b[i] as f64;
        norm_a += a[i] as f64 * a[i] as f64;
        norm_b += b[i] as f64 * b[i] as f64;
    }

    if norm_a == 0.0 || norm_b == 0.0 {
        return 0.0;
    }

    dot / (norm_a.sqrt() * norm_b.sqrt())
}

impl VectorStore {
    /// Create a new vector store.
    ///
    /// Returns an error if the embedding function cannot be created
    /// (e.g., plugin DLL not found, model files missing).
    pub fn new(config: StoreConfig) -> Result<Self, String> {
        let persist_path = if config.storage_path.is_empty() {
            PathBuf::from("memory/vector/vector_store.jsonl")
        } else {
            PathBuf::from(&config.storage_path)
        };

        let vector_config = VectorConfig {
            embedding_tier: config.embedding_tier.clone(),
            plugin_path: config.plugin_path.clone(),
            config_dir: config.config_dir.clone(),
            host_services: None,
        };

        let embed = new_embedding_func(&vector_config)
            .map_err(|e| format!("Failed to create embedding function: {}", e))?;

        let store = Self {
            docs: RwLock::new(Vec::new()),
            embed,
            config,
            persist_path,
        };

        // Load persisted data
        // (In a real async context we'd await this, but since new() is sync
        // we skip auto-loading here and provide load_persisted() separately)

        Ok(store)
    }

    /// Create a new vector store with a pre-built embedding function.
    ///
    /// This is a test-only constructor that allows sharing a single ONNX
    /// plugin across multiple VectorStore instances.
    #[cfg(any(test, feature = "test-fixture"))]
    pub fn new_from_embed(
        embed: Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync>,
        config: StoreConfig,
    ) -> Self {
        let persist_path = if config.storage_path.is_empty() {
            PathBuf::from("memory/vector/vector_store.jsonl")
        } else {
            PathBuf::from(&config.storage_path)
        };
        let store = Self {
            docs: RwLock::new(Vec::new()),
            embed,
            config,
            persist_path,
        };
        let _ = store.load_persisted_sync();
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

        let limit = if limit <= 0 {
            self.config.max_results
        } else {
            limit
        };
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
        self.docs
            .read()
            .iter()
            .find(|d| d.entry.id == id)
            .map(|d| d.entry.clone())
    }

    /// Delete an entry by ID. Returns true if found.
    /// Persists the deletion to disk by rewriting the JSONL file.
    pub fn delete_entry(&self, id: &str) -> bool {
        let mut docs = self.docs.write();
        let before = docs.len();
        docs.retain(|d| d.entry.id != id);
        let found = docs.len() < before;
        if found {
            self.rewrite_persist_file(&docs);
        }
        found
    }

    /// Rewrite the entire JSONL persist file from the current docs.
    fn rewrite_persist_file(&self, docs: &[IndexedDoc]) {
        use std::io::Write;
        let tmp_path = self.persist_path.with_extension("jsonl.tmp");
        let file = std::fs::File::create(&tmp_path);
        match file {
            Ok(mut f) => {
                for doc in docs {
                    if let Ok(line) = serde_json::to_string(&doc.entry) {
                        let _ = writeln!(f, "{}", line);
                    }
                }
                drop(f);
                let _ = std::fs::rename(&tmp_path, &self.persist_path);
            }
            Err(_) => {
                let _ = std::fs::remove_file(&tmp_path);
            }
        }
    }

    /// List entries with optional type filter and pagination.
    pub fn list_entries(&self, type_filter: &[String], offset: usize, limit: usize) -> QueryResult {
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

    /// Load persisted entries from JSONL (async version).
    pub async fn load_persisted(&self) -> Result<(), String> {
        self.load_persisted_sync()
    }

    /// Load persisted entries from JSONL (sync version).
    pub fn load_persisted_sync(&self) -> Result<(), String> {
        if !self.persist_path.exists() {
            return Ok(());
        }

        let content = std::fs::read_to_string(&self.persist_path).map_err(|e| e.to_string())?;

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

    /// Persist an entry to JSONL (async version).
    pub async fn persist_entry(&self, entry: &VectorEntry) -> Result<(), String> {
        Self::persist_entry_sync_inner(&self.persist_path, entry)
    }

    /// Persist an entry to JSONL (sync version for use from sync contexts).
    pub fn persist_entry_sync(&self, entry: &VectorEntry) -> Result<(), String> {
        Self::persist_entry_sync_inner(&self.persist_path, entry)
    }

    /// Shared implementation for persist.
    fn persist_entry_sync_inner(
        persist_path: &std::path::Path,
        entry: &VectorEntry,
    ) -> Result<(), String> {
        if let Some(parent) = persist_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let mut file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(persist_path)
            .map_err(|e| e.to_string())?;

        let mut line = serde_json::to_string(entry).map_err(|e| e.to_string())?;
        line.push('\n');
        use std::io::Write;
        file.write_all(line.as_bytes()).map_err(|e| e.to_string())?;

        Ok(())
    }
}

#[cfg(test)]
mod extra_tests;

#[cfg(test)]
mod tests;
