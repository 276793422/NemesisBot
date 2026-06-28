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

    /// Sidecar archive path for forgotten entries (`<stem>.archive.jsonl` next
    /// to the store file). Written by `delete`; never loaded back into the store.
    fn archive_path(&self) -> PathBuf {
        let dir = self.path.parent().unwrap_or_else(|| Path::new("."));
        let stem = self
            .path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("store");
        dir.join(format!("{}.archive.jsonl", stem))
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
        let removed = self.entries.write().remove(id);
        if let Some(entry) = removed {
            // Archive-on-forget: append the removed entry to a sidecar archive
            // file before flushing, so a forgotten memory stays inspectable and
            // recoverable rather than being hard-deleted. The archive file is
            // never loaded back into the active store on restart.
            let line = serde_json::to_string(&entry).unwrap_or_default();
            if !line.is_empty() {
                if let Ok(mut f) = tokio::fs::OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(self.archive_path())
                    .await
                {
                    let _ = f.write_all(line.as_bytes()).await;
                    let _ = f.write_all(b"\n").await;
                }
            }
            self.flush().await?;
            Ok(true)
        } else {
            Ok(false)
        }
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
mod tests;
