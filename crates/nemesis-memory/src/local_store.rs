//! JSONL-backed local store with TF-IDF-like keyword matching.
//!
//! Mirrors the Go `localStore` type. Entries are persisted in a single `.jsonl`
//! file (one JSON object per line). Text search uses a simplified TF-IDF scoring
//! algorithm that considers term frequency in the entry content, tags, and
//! metadata values.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::io::AsyncWriteExt;

use crate::store::MemoryStore;
use crate::types::{Entry, MemoryType, SearchResult, ScoredEntry};

// ---------------------------------------------------------------------------
// TfIdfLocalStore
// ---------------------------------------------------------------------------

/// A JSONL-file-backed memory store with TF-IDF keyword matching.
///
/// All entries are held in memory for fast queries and flushed to a single
/// `.jsonl` file on every write. On construction the file is loaded (or
/// created if absent).
pub struct TfIdfLocalStore {
    path: PathBuf,
    entries: RwLock<HashMap<String, Entry>>,
}

impl TfIdfLocalStore {
    /// Create (or load) a local store backed by `path`.
    ///
    /// If the file does not exist a fresh store is returned.
    pub async fn new(path: impl AsRef<Path>) -> Result<Self, String> {
        let path = path.as_ref().to_path_buf();
        let entries = RwLock::new(HashMap::new());

        let store = Self { path, entries };
        store.load().await?;
        Ok(store)
    }

    /// Read the JSONL file into memory (skipping malformed lines).
    async fn load(&self) -> Result<(), String> {
        if !self.path.exists() {
            return Ok(());
        }

        let data = tokio::fs::read_to_string(&self.path)
            .await
            .map_err(|e| format!("Failed to read store file: {e}"))?;

        let mut guard = self.entries.write();
        for line in data.lines() {
            let line = line.trim();
            if line.is_empty() {
                continue;
            }
            if let Ok(entry) = serde_json::from_str::<Entry>(line) {
                guard.insert(entry.id.clone(), entry);
            }
        }

        Ok(())
    }

    /// Flush all entries back to the JSONL file.
    ///
    /// The in-memory lock is released before performing async I/O so that
    /// the resulting future is `Send`.
    async fn flush(&self) -> Result<(), String> {
        // Snapshot entries under the read lock, then release it.
        let snapshot: Vec<Vec<u8>> = {
            let guard = self.entries.read();
            let mut buf = Vec::with_capacity(guard.len() * 256);
            for entry in guard.values() {
                let mut line = serde_json::to_string(entry)
                    .map_err(|e| format!("Failed to serialize entry: {e}"))?;
                line.push('\n');
                buf.extend_from_slice(line.as_bytes());
            }
            // Collect into a single buffer so we drop the lock.
            vec![buf]
        };

        // Ensure parent directory exists.
        if let Some(parent) = self.path.parent() {
            tokio::fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create store directory: {e}"))?;
        }

        let mut file = tokio::fs::File::create(&self.path)
            .await
            .map_err(|e| format!("Failed to create store file: {e}"))?;

        file.write_all(&snapshot[0])
            .await
            .map_err(|e| format!("Failed to write store file: {e}"))?;

        file.flush()
            .await
            .map_err(|e| format!("Failed to flush store file: {e}"))?;

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Async trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl MemoryStore for TfIdfLocalStore {
    async fn store(&self, entry: Entry) -> Result<String, String> {
        let id = entry.id.clone();
        self.entries.write().insert(id.clone(), entry);
        self.flush().await?;
        Ok(id)
    }

    async fn query(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String> {
        let limit = if limit == 0 { 10 } else { limit };
        let query_tokens = tokenize(query);

        if query_tokens.is_empty() {
            return Ok(SearchResult {
                entries: Vec::new(),
                total: 0,
            });
        }

        // Take a snapshot of entries under read lock, then drop it.
        let entries_snapshot: Vec<Entry> = {
            let guard = self.entries.read();
            guard
                .values()
                .filter(|e| memory_type.is_none() || e.typ == memory_type.unwrap())
                .cloned()
                .collect()
        };

        let total_docs = entries_snapshot.len();

        // Build document frequency map (owned strings for safety).
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        for entry in &entries_snapshot {
            let doc_tokens = entry_tokens(entry);
            let mut seen = std::collections::HashSet::new();
            for token in &doc_tokens {
                if seen.insert(token.clone()) {
                    *doc_freq.entry(token.clone()).or_insert(0) += 1;
                }
            }
        }

        // Score each entry using TF-IDF cosine similarity.
        let mut scored: Vec<ScoredEntry> = entries_snapshot
            .into_iter()
            .filter_map(|entry| {
                let doc_tokens = entry_tokens(&entry);
                let score = tfidf_score(&query_tokens, &doc_tokens, total_docs, &doc_freq);
                if score > 0.0 {
                    Some(ScoredEntry {
                        entry,
                        score,
                    })
                } else {
                    None
                }
            })
            .collect();

        // Sort descending by score.
        scored.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));

        let total = scored.len();
        let entries = scored.into_iter().take(limit).collect();

        Ok(SearchResult { entries, total })
    }

    async fn get(&self, id: &str) -> Result<Option<Entry>, String> {
        Ok(self.entries.read().get(id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<bool, String> {
        let removed = self.entries.write().remove(id).is_some();
        if removed {
            self.flush().await?;
        }
        Ok(removed)
    }

    async fn list(
        &self,
        memory_type: Option<MemoryType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, String> {
        let guard = self.entries.read();
        let mut matched: Vec<Entry> = guard
            .values()
            .filter(|e| memory_type.is_none() || e.typ == memory_type.unwrap())
            .cloned()
            .collect();

        // Sort by created_at descending (most recent first), matching Go behavior.
        matched.sort_by(|a, b| b.created_at.cmp(&a.created_at));

        let total = matched.len();
        let offset = offset.min(total);
        let result: Vec<Entry> = matched
            .into_iter()
            .skip(offset)
            .take(if limit > 0 { limit } else { usize::MAX })
            .collect();

        Ok(result)
    }

    async fn close(&self) -> Result<(), String> {
        // Data is flushed on every write; nothing extra to do.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// TF-IDF scoring helpers
// ---------------------------------------------------------------------------

/// Tokenize text into lowercase words, replacing punctuation with spaces.
///
/// This mirrors the Go `tokenize` function which uses `unicode.IsPunct`
/// and `unicode.IsSymbol` as separators.
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_punctuation()
                || c.is_whitespace()
                || (!c.is_alphanumeric() && c != '_')
            {
                ' '
            } else {
                c
            }
        })
        .collect::<String>()
        .split_whitespace()
        .map(|s| s.to_string())
        .collect()
}

/// Extract all searchable tokens from an entry (content + tags + metadata values).
fn entry_tokens(entry: &Entry) -> Vec<String> {
    let mut text = entry.content.to_lowercase();

    // Include tags.
    for tag in &entry.tags {
        text.push(' ');
        text.push_str(&tag.to_lowercase());
    }

    // Include metadata values.
    for value in entry.metadata.values() {
        text.push(' ');
        text.push_str(&value.to_lowercase());
    }

    tokenize(&text)
}

/// Compute TF-IDF cosine similarity between query tokens and document tokens.
///
/// - **TF** (term frequency): count of term in document, log-normalised.
/// - **IDF** (inverse document frequency): `log(N / df)` where N = total docs.
/// - **Score**: cosine similarity between the TF-IDF vectors.
fn tfidf_score(
    query_tokens: &[String],
    doc_tokens: &[String],
    total_docs: usize,
    doc_freq: &HashMap<String, usize>,
) -> f64 {
    if query_tokens.is_empty() || doc_tokens.is_empty() || total_docs == 0 {
        return 0.0;
    }

    // Build TF maps.
    let mut query_tf: HashMap<&str, f64> = HashMap::new();
    for token in query_tokens {
        *query_tf.entry(token.as_str()).or_insert(0.0) += 1.0;
    }

    let mut doc_tf: HashMap<&str, f64> = HashMap::new();
    for token in doc_tokens {
        *doc_tf.entry(token.as_str()).or_insert(0.0) += 1.0;
    }

    // Compute TF-IDF vectors and cosine similarity in one pass.
    let n = total_docs as f64;
    let mut dot_product = 0.0f64;
    let mut query_norm = 0.0f64;

    for (term, &tf) in &query_tf {
        let df = doc_freq.get(*term).copied().unwrap_or(0) as f64;
        if df == 0.0 {
            // Term does not appear in any filtered document.
            continue;
        }
        let idf = (n / df).ln() + 1.0; // smoothed IDF
        let query_weight = (1.0 + tf.ln()) * idf;
        let doc_tf_val = doc_tf.get(term).copied().unwrap_or(0.0);
        // Only contribute to dot product if the term appears in the document.
        if doc_tf_val > 0.0 {
            let doc_weight = (1.0 + doc_tf_val.ln()) * idf;
            dot_product += query_weight * doc_weight;
        }
        query_norm += query_weight * query_weight;
    }

    // Compute document vector norm (only terms that appear in this doc).
    let mut doc_norm = 0.0f64;
    for (term, &tf) in &doc_tf {
        if tf == 0.0 {
            continue;
        }
        let df = doc_freq.get(*term).copied().unwrap_or(0) as f64;
        if df == 0.0 {
            continue;
        }
        let idf = (n / df).ln() + 1.0;
        let weight = (1.0 + tf.ln()) * idf;
        doc_norm += weight * weight;
    }

    if query_norm == 0.0 || doc_norm == 0.0 {
        return 0.0;
    }

    dot_product / (query_norm.sqrt() * doc_norm.sqrt())
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::Entry;

    fn make_entry(typ: MemoryType, content: &str) -> Entry {
        Entry::new(typ, content.to_string())
    }

    #[test]
    fn test_tokenize_basic() {
        let tokens = tokenize("Hello, World! This is a test.");
        assert_eq!(tokens, vec!["hello", "world", "this", "is", "a", "test"]);
    }

    #[test]
    fn test_tokenize_empty() {
        let tokens = tokenize("");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tokenize_punctuation_only() {
        let tokens = tokenize("!!! ??? ...");
        assert!(tokens.is_empty());
    }

    #[test]
    fn test_tokenize_unicode() {
        let tokens = tokenize("Hello world");
        assert!(tokens.contains(&"hello".to_string()));
        assert!(tokens.contains(&"world".to_string()));
    }

    #[tokio::test]
    async fn test_local_store_new_creates_fresh() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();
        // File doesn't exist yet -- no error.
        assert!(!path.exists());
        let entries = store.list(None, 100, 0).await.unwrap();
        assert!(entries.is_empty());
    }

    #[tokio::test]
    async fn test_store_and_get() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        let entry = make_entry(MemoryType::LongTerm, "Paris is the capital of France");
        let id = store.store(entry).await.unwrap();

        let retrieved = store.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "Paris is the capital of France");
        assert_eq!(retrieved.typ, MemoryType::LongTerm);
    }

    #[tokio::test]
    async fn test_store_persists_to_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");

        let id = {
            let store = TfIdfLocalStore::new(&path).await.unwrap();
            let entry = make_entry(MemoryType::LongTerm, "persisted content");
            let id = store.store(entry).await.unwrap();
            assert!(path.exists());
            id
        };

        // Load again from disk.
        let store2 = TfIdfLocalStore::new(&path).await.unwrap();
        let retrieved = store2.get(&id).await.unwrap().unwrap();
        assert_eq!(retrieved.content, "persisted content");
    }

    #[tokio::test]
    async fn test_delete_removes_entry() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        let entry = make_entry(MemoryType::ShortTerm, "temporary");
        let id = store.store(entry).await.unwrap();

        let deleted = store.delete(&id).await.unwrap();
        assert!(deleted);

        let gone = store.get(&id).await.unwrap();
        assert!(gone.is_none());

        // Deleting again returns false.
        let deleted_again = store.delete(&id).await.unwrap();
        assert!(!deleted_again);
    }

    #[tokio::test]
    async fn test_query_finds_relevant() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        store.store(make_entry(MemoryType::LongTerm, "The cat sat on the mat")).await.unwrap();
        store.store(make_entry(MemoryType::LongTerm, "Dogs love to play fetch")).await.unwrap();
        store.store(make_entry(MemoryType::ShortTerm, "Cat food is expensive")).await.unwrap();

        let result = store.query("cat", None, 10).await.unwrap();
        assert_eq!(result.total, 2);
        // Both cat entries should be present.
        assert!(result
            .entries
            .iter()
            .all(|e| e.entry.content.contains("Cat") || e.entry.content.contains("cat")));
    }

    #[tokio::test]
    async fn test_query_with_type_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        store.store(make_entry(MemoryType::LongTerm, "cat info long")).await.unwrap();
        store.store(make_entry(MemoryType::ShortTerm, "cat info short")).await.unwrap();

        let result = store.query("cat", Some(MemoryType::ShortTerm), 10).await.unwrap();
        assert_eq!(result.total, 1);
        assert_eq!(result.entries[0].entry.typ, MemoryType::ShortTerm);
    }

    #[tokio::test]
    async fn test_query_empty_tokens_returns_empty() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        store.store(make_entry(MemoryType::LongTerm, "some content")).await.unwrap();

        let result = store.query("!!!", None, 10).await.unwrap();
        assert_eq!(result.total, 0);
    }

    #[tokio::test]
    async fn test_list_with_pagination() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        for i in 0..5 {
            store.store(make_entry(MemoryType::LongTerm, &format!("entry {i}"))).await.unwrap();
        }

        let page1 = store.list(None, 2, 0).await.unwrap();
        assert_eq!(page1.len(), 2);

        let page2 = store.list(None, 2, 2).await.unwrap();
        assert_eq!(page2.len(), 2);

        let page3 = store.list(None, 2, 4).await.unwrap();
        assert_eq!(page3.len(), 1);

        let page4 = store.list(None, 2, 10).await.unwrap();
        assert!(page4.is_empty());
    }

    #[tokio::test]
    async fn test_list_with_type_filter() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        store.store(make_entry(MemoryType::LongTerm, "long term entry")).await.unwrap();
        store.store(make_entry(MemoryType::ShortTerm, "short term entry")).await.unwrap();
        store.store(make_entry(MemoryType::LongTerm, "another long term")).await.unwrap();

        let result = store.list(Some(MemoryType::LongTerm), 10, 0).await.unwrap();
        assert_eq!(result.len(), 2);
        assert!(result.iter().all(|e| e.typ == MemoryType::LongTerm));
    }

    #[tokio::test]
    async fn test_query_uses_tags_and_metadata() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();

        // Entry with matching tag but no matching content.
        let entry = Entry::new(MemoryType::LongTerm, "programming language".to_string())
            .with_tags(vec!["rust".to_string()]);
        store.store(entry).await.unwrap();

        let result = store.query("rust", None, 10).await.unwrap();
        assert_eq!(result.total, 1);
    }

    #[tokio::test]
    async fn test_close_is_noop() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("store.jsonl");
        let store = TfIdfLocalStore::new(&path).await.unwrap();
        assert!(store.close().await.is_ok());
    }

    #[test]
    fn test_tfidf_score_identical_docs() {
        let tokens = vec!["hello".to_string(), "world".to_string()];
        let total_docs = 5;
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        doc_freq.insert("hello".to_string(), 3);
        doc_freq.insert("world".to_string(), 2);

        let score = tfidf_score(&tokens, &tokens, total_docs, &doc_freq);
        // Identical documents should score ~1.0.
        assert!((score - 1.0).abs() < 0.01, "Expected ~1.0, got {score}");
    }

    #[test]
    fn test_tfidf_score_no_overlap() {
        let query = vec!["cat".to_string()];
        let doc = vec!["dog".to_string()];
        let total_docs = 5;
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        doc_freq.insert("cat".to_string(), 2);
        doc_freq.insert("dog".to_string(), 3);

        let score = tfidf_score(&query, &doc, total_docs, &doc_freq);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_tfidf_score_partial_overlap() {
        let query = vec!["cat".to_string(), "mat".to_string()];
        let doc = vec!["cat".to_string(), "sat".to_string()];
        let total_docs = 10;
        let mut doc_freq: HashMap<String, usize> = HashMap::new();
        doc_freq.insert("cat".to_string(), 5);
        doc_freq.insert("mat".to_string(), 3);
        doc_freq.insert("sat".to_string(), 4);

        let score = tfidf_score(&query, &doc, total_docs, &doc_freq);
        assert!(score > 0.0 && score < 1.0, "Expected (0, 1), got {score}");
    }
}
