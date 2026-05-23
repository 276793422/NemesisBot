//! Memory store trait and local in-memory implementation.
//!
//! `MemoryStore` defines the async interface every backend must implement.
//! `LocalStore` is a simple in-memory `Vec<Entry>` store with word-overlap
//! scoring that approximates TF-IDF for small-scale usage.

use std::sync::Arc;

use async_trait::async_trait;
use parking_lot::RwLock;

use crate::types::{Entry, MemoryType, SearchResult, ScoredEntry};

/// Async interface for a memory storage backend.
#[async_trait]
pub trait MemoryStore: Send + Sync {
    /// Persist an entry and return its ID.
    async fn store(&self, entry: Entry) -> Result<String, String>;

    /// Search entries by free-text query, optionally filtered by type.
    async fn query(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String>;

    /// Retrieve a single entry by ID.
    async fn get(&self, id: &str) -> Result<Option<Entry>, String>;

    /// Delete an entry by ID. Returns `true` if an entry was removed.
    async fn delete(&self, id: &str) -> Result<bool, String>;

    /// List entries optionally filtered by type.
    async fn list(
        &self,
        memory_type: Option<MemoryType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, String>;

    /// Release any held resources.
    async fn close(&self) -> Result<(), String>;
}

// ---------------------------------------------------------------------------
// LocalStore
// ---------------------------------------------------------------------------

/// In-memory store backed by a `Vec<Entry>` with simple word-overlap scoring.
pub struct LocalStore {
    entries: Arc<RwLock<Vec<Entry>>>,
}

impl LocalStore {
    pub fn new() -> Self {
        Self {
            entries: Arc::new(RwLock::new(Vec::new())),
        }
    }
}

impl Default for LocalStore {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// TF-IDF-like scoring
// ---------------------------------------------------------------------------

/// Tokenise a string into lowercase words (splitting on whitespace / punctuation).
fn tokenize(text: &str) -> Vec<String> {
    text.to_lowercase()
        .split(|c: char| !c.is_alphanumeric())
        .filter(|s| !s.is_empty())
        .map(|s| s.to_string())
        .collect()
}

/// Compute a simple word-overlap score between a query and a document.
///
/// The score is the number of unique query tokens that appear in the document
/// divided by the total number of unique query tokens (Jaccard-like overlap).
fn compute_score(query_tokens: &[String], doc_tokens: &[String]) -> f64 {
    if query_tokens.is_empty() {
        return 0.0;
    }
    let doc_set: std::collections::HashSet<&str> =
        doc_tokens.iter().map(|s| s.as_str()).collect();
    let query_set: std::collections::HashSet<&str> =
        query_tokens.iter().map(|s| s.as_str()).collect();

    let overlap = query_set.intersection(&doc_set).count() as f64;
    overlap / query_set.len() as f64
}

// ---------------------------------------------------------------------------
// Async trait impl
// ---------------------------------------------------------------------------

#[async_trait]
impl MemoryStore for LocalStore {
    async fn store(&self, entry: Entry) -> Result<String, String> {
        let id = entry.id.clone();
        let mut guard = self.entries.write();
        guard.push(entry);
        Ok(id)
    }

    async fn query(
        &self,
        query: &str,
        memory_type: Option<MemoryType>,
        limit: usize,
    ) -> Result<SearchResult, String> {
        let query_tokens = tokenize(query);
        let guard = self.entries.read();

        let mut scored: Vec<ScoredEntry> = guard
            .iter()
            .filter(|e| memory_type.is_none() || e.typ == memory_type.unwrap())
            .filter_map(|e| {
                let doc_tokens = tokenize(&e.content);
                let sc = compute_score(&query_tokens, &doc_tokens);
                // Also check tags for matches.
                let tag_tokens: Vec<String> =
                    e.tags.iter().flat_map(|t| tokenize(t)).collect();
                let tag_sc = compute_score(&query_tokens, &tag_tokens);
                let final_sc = sc.max(tag_sc);
                if final_sc > 0.0 {
                    Some(ScoredEntry {
                        entry: e.clone(),
                        score: final_sc,
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
        let guard = self.entries.read();
        Ok(guard.iter().find(|e| e.id == id).cloned())
    }

    async fn delete(&self, id: &str) -> Result<bool, String> {
        let mut guard = self.entries.write();
        let before = guard.len();
        guard.retain(|e| e.id != id);
        Ok(guard.len() < before)
    }

    async fn list(
        &self,
        memory_type: Option<MemoryType>,
        limit: usize,
        offset: usize,
    ) -> Result<Vec<Entry>, String> {
        let guard = self.entries.read();
        let filtered: Vec<Entry> = guard
            .iter()
            .filter(|e| memory_type.is_none() || e.typ == memory_type.unwrap())
            .skip(offset)
            .take(limit)
            .cloned()
            .collect();
        Ok(filtered)
    }

    async fn close(&self) -> Result<(), String> {
        // Nothing to flush for an in-memory store.
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
