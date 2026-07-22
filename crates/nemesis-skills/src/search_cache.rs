//! Search cache - trigram-based similarity matching and LRU eviction.
//!
//! Provides intelligent caching for skill search results with:
//! - Trigram extraction for query fingerprinting
//! - Jaccard similarity for approximate matching
//! - LRU eviction when cache is full
//! - TTL-based expiration

use std::collections::HashMap;
use std::time::{Duration, Instant};

use parking_lot::RwLock;
use serde::{Deserialize, Serialize};

use crate::types::RegistrySearchResult;

/// A single cache entry.
#[derive(Debug, Clone)]
struct CacheEntry {
    /// Grouped search results.
    results: Vec<RegistrySearchResult>,
    /// Trigram signature for similarity matching.
    trigrams: Vec<u32>,
    /// When this entry was created.
    created_at: Instant,
    /// When this entry was last accessed.
    #[allow(dead_code)]
    last_access_at: Instant,
    /// How many times this entry has been accessed.
    #[allow(dead_code)]
    access_count: usize,
}

/// Cache statistics.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CacheStats {
    /// Current number of entries.
    pub size: usize,
    /// Maximum number of entries.
    pub max_size: usize,
    /// Number of cache hits.
    pub hit_count: usize,
    /// Number of cache misses.
    pub miss_count: usize,
    /// Cache hit rate (0.0 - 1.0).
    pub hit_rate: f64,
}

/// Search cache configuration.
#[derive(Debug, Clone)]
pub struct SearchCacheConfig {
    /// Maximum number of entries (default: 50).
    pub max_size: usize,
    /// Time-to-live (default: 5 minutes).
    pub ttl: Duration,
}

impl Default for SearchCacheConfig {
    fn default() -> Self {
        Self {
            max_size: 50,
            ttl: Duration::from_secs(300),
        }
    }
}

/// Intelligent search cache with trigram-based similarity matching.
pub struct SearchCache {
    inner: RwLock<SearchCacheInner>,
}

struct SearchCacheInner {
    cache: HashMap<String, CacheEntry>,
    lru_list: Vec<String>,
    max_size: usize,
    ttl: Duration,
    hit_count: usize,
    miss_count: usize,
}

impl SearchCache {
    /// Create a new search cache with the given configuration.
    pub fn new(config: SearchCacheConfig) -> Self {
        Self {
            inner: RwLock::new(SearchCacheInner {
                cache: HashMap::new(),
                lru_list: Vec::with_capacity(config.max_size),
                max_size: config.max_size,
                ttl: config.ttl,
                hit_count: 0,
                miss_count: 0,
            }),
        }
    }

    /// Retrieve search results from cache.
    ///
    /// First tries an exact match, then falls back to similarity matching
    /// (trigram Jaccard coefficient > 0.7 threshold).
    ///
    /// Returns `Some(results)` on hit (exact or similar), `None` on miss.
    /// Results are clamped per-registry to the given limit.
    pub fn get(&self, query: &str, limit: usize) -> Option<Vec<RegistrySearchResult>> {
        let mut inner = self.inner.write();

        // 1. Try exact match first.
        let exact_match = inner.cache.get(query).map(|entry| {
            (
                entry.created_at.elapsed() > inner.ttl,
                entry.results.clone(),
            )
        });

        if let Some((expired, results)) = exact_match {
            if expired {
                inner.cache.remove(query);
                // Fall through to try similarity match with remaining entries.
            } else {
                // Exact match found and not expired.
                inner.hit_count += 1;
                inner.update_lru(query);
                return Some(clamp_registry_results(&results, limit));
            }
        }

        // 2. Try similarity match.
        let query_trigrams = build_trigrams(query);

        // Skip similarity matching if the query is too short to have trigrams.
        if !query_trigrams.is_empty() {
            let mut best_match: Option<String> = None;
            let mut best_score = 0.0f64;

            for (key, entry) in &inner.cache {
                if key == query {
                    continue; // Skip self (exact match already tried).
                }
                if entry.created_at.elapsed() > inner.ttl {
                    continue;
                }

                let similarity = jaccard_similarity(&query_trigrams, &entry.trigrams);
                if similarity > 0.7 && similarity > best_score {
                    best_match = Some(key.clone());
                    best_score = similarity;
                }
            }

            if let Some(match_key) = best_match {
                let entry = inner.cache.get(&match_key).unwrap().clone();
                inner.update_lru(&match_key);
                inner.hit_count += 1;
                return Some(clamp_registry_results(&entry.results, limit));
            }
        }

        inner.miss_count += 1;
        None
    }

    /// Store search results in the cache.
    pub fn put(&self, query: &str, results: Vec<RegistrySearchResult>) {
        let mut inner = self.inner.write();
        let trigrams = build_trigrams(query);
        let now = Instant::now();

        let entry = CacheEntry {
            results,
            trigrams,
            created_at: now,
            last_access_at: now,
            access_count: 1,
        };

        if inner.cache.contains_key(query) {
            inner.cache.insert(query.to_string(), entry);
            inner.update_lru(query);
            return;
        }

        // Evict if full.
        if inner.cache.len() >= inner.max_size {
            inner.evict_lru();
        }

        inner.cache.insert(query.to_string(), entry);
        inner.lru_list.push(query.to_string());
    }

    /// Clear all entries.
    pub fn clear(&self) {
        let mut inner = self.inner.write();
        inner.cache.clear();
        inner.lru_list.clear();
        inner.hit_count = 0;
        inner.miss_count = 0;
    }

    /// Get cache statistics.
    pub fn stats(&self) -> CacheStats {
        let inner = self.inner.read();
        let total = inner.hit_count + inner.miss_count;
        let hit_rate = if total > 0 {
            inner.hit_count as f64 / total as f64
        } else {
            0.0
        };

        CacheStats {
            size: inner.cache.len(),
            max_size: inner.max_size,
            hit_count: inner.hit_count,
            miss_count: inner.miss_count,
            hit_rate,
        }
    }

    /// Update the LRU list by moving the key to the end.
    ///
    /// Mirrors Go `updateLRU`. Public for testing and external use.
    pub fn update_lru(&self, key: &str) {
        let mut inner = self.inner.write();
        inner.update_lru(key);
    }

    /// Evict the least recently used entry.
    ///
    /// Mirrors Go `evictLRU`. Public for testing and external use.
    pub fn evict_lru(&self) {
        let mut inner = self.inner.write();
        inner.evict_lru();
    }
}

impl SearchCacheInner {
    fn update_lru(&mut self, key: &str) {
        self.lru_list.retain(|k| k != key);
        self.lru_list.push(key.to_string());
    }

    fn evict_lru(&mut self) {
        if !self.lru_list.is_empty() {
            let lru_key = self.lru_list.remove(0);
            self.cache.remove(&lru_key);
        }
    }
}

/// Clamp each registry's results to the given limit.
///
/// Public function matching Go's `clampRegistryResults`.
pub fn clamp_registry_results(
    grouped: &[RegistrySearchResult],
    limit: usize,
) -> Vec<RegistrySearchResult> {
    if limit == 0 {
        return grouped.to_vec();
    }

    grouped
        .iter()
        .map(|g| {
            if g.results.len() > limit {
                RegistrySearchResult {
                    registry_name: g.registry_name.clone(),
                    results: g.results[..limit].to_vec(),
                    truncated: true,
                }
            } else {
                g.clone()
            }
        })
        .collect()
}

/// Build sorted, deduplicated trigrams from a string.
///
/// A trigram is a 3-byte sliding window hash: `s[i] << 16 | s[i+1] << 8 | s[i+2]`.
/// The input is lowercased for case-insensitive matching.
/// For multi-word inputs, also includes character bigrams, word unigrams, and
/// word bigrams for improved similarity matching on short queries.
pub fn build_trigrams(s: &str) -> Vec<u32> {
    let lower = s.to_lowercase();
    let bytes = lower.as_bytes();

    if bytes.len() < 3 {
        return Vec::new();
    }

    let mut features: Vec<u32> = (0..=bytes.len() - 3)
        .map(|i| (bytes[i] as u32) << 16 | (bytes[i + 1] as u32) << 8 | (bytes[i + 2] as u32))
        .collect();

    features.sort();
    features.dedup();

    // For multi-word inputs, add extra features for better similarity matching.
    let words: Vec<&str> = lower.split_whitespace().collect();
    if words.len() >= 2 {
        // Character bigrams (prefixed to avoid collision with trigrams).
        for i in 0..=bytes.len() - 2 {
            features.push(0x01000000u32 | ((bytes[i] as u32) << 8) | (bytes[i + 1] as u32));
        }

        // Word unigrams.
        for word in &words {
            let word_hash = word.chars().fold(0x02000000u32, |acc, c| {
                acc.wrapping_mul(31).wrapping_add(c as u32)
            });
            features.push(word_hash);
        }

        // Word bigrams.
        for i in 0..words.len().saturating_sub(1) {
            let h1 = words[i]
                .chars()
                .fold(0u32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u32));
            let h2 = words[i + 1]
                .chars()
                .fold(0u32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u32));
            features.push(0x03000000u32 | (h1.wrapping_mul(31).wrapping_add(h2) & 0x00FFFFFF));
        }

        features.sort();
        features.dedup();
    }

    features
}

/// Calculate similarity between two sorted feature sets using the Dice coefficient.
///
/// Dice(A, B) = 2 * |A intersection B| / (|A| + |B|)
///
/// Returns 0.0 - 1.0. Returns 1.0 if both sets are empty.
pub fn jaccard_similarity(a: &[u32], b: &[u32]) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let mut intersection = 0usize;
    let mut i = 0usize;
    let mut j = 0usize;

    while i < a.len() && j < b.len() {
        if a[i] == b[j] {
            intersection += 1;
            i += 1;
            j += 1;
        } else if a[i] < b[j] {
            i += 1;
        } else {
            j += 1;
        }
    }

    let total = a.len() + b.len();
    if total == 0 {
        return 0.0;
    }

    (2.0 * intersection as f64) / total as f64
}

/// Convert a string to lowercase (ASCII-only, efficient).
///
/// Mirrors Go `toLower` from `search_cache.go`. More efficient than
/// `String::to_lowercase` for ASCII-only matching since it avoids
/// Unicode case-folding overhead.
pub fn to_lower(s: &str) -> String {
    s.bytes()
        .map(|c| {
            if c.is_ascii_uppercase() {
                c.to_ascii_lowercase() as char
            } else {
                c as char
            }
        })
        .collect()
}

#[cfg(test)]
mod tests;
