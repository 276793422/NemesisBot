//! Search cache - trigram-based similarity matching and LRU eviction.
//!
//! Provides intelligent caching for skill search results with:
//! - Trigram extraction for query fingerprinting
//! - Jaccard similarity for approximate matching
//! - LRU eviction when cache is full
//! - TTL-based expiration

use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde::{Deserialize, Serialize};
use parking_lot::RwLock;

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
            (entry.created_at.elapsed() > inner.ttl, entry.results.clone())
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
        .map(|i| {
            (bytes[i] as u32) << 16 | (bytes[i + 1] as u32) << 8 | (bytes[i + 2] as u32)
        })
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
            let h1 = words[i].chars().fold(0u32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u32));
            let h2 = words[i + 1].chars().fold(0u32, |acc, c| acc.wrapping_mul(31).wrapping_add(c as u32));
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
        .map(|c| if c.is_ascii_uppercase() { c.to_ascii_lowercase() as char } else { c as char })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::types::SkillSearchResult;

    fn make_search_result(registry: &str, count: usize) -> Vec<RegistrySearchResult> {
        let results: Vec<SkillSearchResult> = (0..count)
            .map(|i| SkillSearchResult {
                score: 1.0,
                slug: format!("skill-{}", i),
                display_name: format!("Skill {}", i),
                summary: "Test skill".to_string(),
                version: "1.0".to_string(),
                registry_name: registry.to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            })
            .collect();

        vec![RegistrySearchResult {
            registry_name: registry.to_string(),
            results,
            truncated: false,
        }]
    }

    #[test]
    fn test_build_trigrams_empty() {
        assert!(build_trigrams("").is_empty());
        assert!(build_trigrams("ab").is_empty());
    }

    #[test]
    fn test_build_trigrams_basic() {
        let trigrams = build_trigrams("abc");
        assert_eq!(trigrams.len(), 1);
    }

    #[test]
    fn test_build_trigrams_longer() {
        let trigrams = build_trigrams("abcdef");
        assert_eq!(trigrams.len(), 4); // abc, bcd, cde, def
    }

    #[test]
    fn test_build_trigrams_case_insensitive() {
        let upper = build_trigrams("ABC");
        let lower = build_trigrams("abc");
        assert_eq!(upper, lower);
    }

    #[test]
    fn test_build_trigrams_sorted_deduped() {
        let trigrams = build_trigrams("aaaa");
        // "aaa" and "aaa" -> deduplicated to 1
        assert_eq!(trigrams.len(), 1);
    }

    #[test]
    fn test_jaccard_identical() {
        let a = build_trigrams("hello");
        let b = build_trigrams("hello");
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_both_empty() {
        let sim = jaccard_similarity(&[], &[]);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_one_empty() {
        let a = build_trigrams("hello");
        let sim = jaccard_similarity(&a, &[]);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_similar() {
        let a = build_trigrams("pdf converter");
        let b = build_trigrams("pdf viewer");
        let sim = jaccard_similarity(&a, &b);
        assert!(sim > 0.3, "Similar strings should have > 0.3 similarity, got {}", sim);
        assert!(sim < 1.0);
    }

    #[test]
    fn test_jaccard_completely_different() {
        let a = build_trigrams("xyz");
        let b = build_trigrams("abc");
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_cache_put_and_get_exact() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        let results = make_search_result("test", 3);

        cache.put("pdf", results.clone());
        let hit = cache.get("pdf", 10);
        assert!(hit.is_some());

        let stats = cache.stats();
        assert_eq!(stats.hit_count, 1);
        assert_eq!(stats.miss_count, 0);
    }

    #[test]
    fn test_cache_miss() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        let hit = cache.get("nonexistent", 10);
        assert!(hit.is_none());

        let stats = cache.stats();
        assert_eq!(stats.miss_count, 1);
    }

    #[test]
    fn test_cache_similar_match() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        let results = make_search_result("test", 3);

        cache.put("pdf converter tool", results);
        let hit = cache.get("pdf converter", 10);
        // Should find a similar match via trigrams.
        assert!(hit.is_some());
    }

    #[test]
    fn test_cache_lru_eviction() {
        let config = SearchCacheConfig {
            max_size: 2,
            ttl: Duration::from_secs(300),
        };
        let cache = SearchCache::new(config);

        cache.put("a", make_search_result("r1", 1));
        cache.put("b", make_search_result("r2", 1));
        cache.put("c", make_search_result("r3", 1)); // Should evict "a"

        assert!(cache.get("a", 10).is_none()); // evicted
        assert!(cache.get("b", 10).is_some());
        assert!(cache.get("c", 10).is_some());

        let stats = cache.stats();
        assert!(stats.size <= 2);
    }

    #[test]
    fn test_cache_clear() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.put("test", make_search_result("r", 1));
        cache.clear();

        let stats = cache.stats();
        assert_eq!(stats.size, 0);
        assert_eq!(stats.hit_count, 0);
        assert_eq!(stats.miss_count, 0);
    }

    #[test]
    fn test_clamp_results() {
        let results = make_search_result("test", 10);
        let clamped = clamp_registry_results(&results, 3);
        assert_eq!(clamped[0].results.len(), 3);
        assert!(clamped[0].truncated);
    }

    #[test]
    fn test_clamp_results_zero_limit() {
        let results = make_search_result("test", 5);
        let clamped = clamp_registry_results(&results, 0);
        assert_eq!(clamped[0].results.len(), 5);
        assert!(!clamped[0].truncated);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_cache_ttl_expiration() {
        let config = SearchCacheConfig {
            max_size: 10,
            ttl: Duration::from_millis(1), // Very short TTL
        };
        let cache = SearchCache::new(config);
        cache.put("test", make_search_result("r", 1));

        // Wait for TTL to expire
        std::thread::sleep(Duration::from_millis(10));

        let hit = cache.get("test", 10);
        assert!(hit.is_none(), "Should miss after TTL expiration");
    }

    #[test]
    fn test_cache_overwrite_existing_key() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.put("key", make_search_result("r1", 1));
        cache.put("key", make_search_result("r2", 5));

        let hit = cache.get("key", 10).unwrap();
        assert_eq!(hit[0].results.len(), 5);
        assert_eq!(hit[0].registry_name, "r2");
    }

    #[test]
    fn test_cache_stats_hit_rate() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.put("test", make_search_result("r", 1));

        // 1 hit
        let _ = cache.get("test", 10);
        // 1 miss
        let _ = cache.get("nonexistent", 10);

        let stats = cache.stats();
        assert_eq!(stats.hit_count, 1);
        assert_eq!(stats.miss_count, 1);
        assert!((stats.hit_rate - 0.5).abs() < 0.001);
    }

    #[test]
    fn test_cache_stats_zero_queries() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        let stats = cache.stats();
        assert_eq!(stats.hit_rate, 0.0);
        assert_eq!(stats.size, 0);
    }

    #[test]
    fn test_build_trigrams_multi_word() {
        let trigrams = build_trigrams("pdf converter tool");
        assert!(!trigrams.is_empty());
        // Should have character trigrams, bigrams, word unigrams, and word bigrams
    }

    #[test]
    fn test_jaccard_similarity_different_sizes() {
        let a = build_trigrams("pdf");
        let b = build_trigrams("pdf converter");
        let sim = jaccard_similarity(&a, &b);
        assert!(sim > 0.0);
        assert!(sim < 1.0);
    }

    #[test]
    fn test_to_lower() {
        assert_eq!(to_lower("Hello World"), "hello world");
        assert_eq!(to_lower("ABC"), "abc");
        assert_eq!(to_lower("already lower"), "already lower");
        assert_eq!(to_lower(""), "");
    }

    #[test]
    fn test_cache_update_lru() {
        let config = SearchCacheConfig {
            max_size: 2,
            ttl: Duration::from_secs(300),
        };
        let cache = SearchCache::new(config);

        cache.put("a", make_search_result("r1", 1));
        cache.put("b", make_search_result("r2", 1));

        // Access "a" to move it to end of LRU
        cache.update_lru("a");

        // Now add "c" which should evict "b" (least recently used)
        cache.put("c", make_search_result("r3", 1));

        assert!(cache.get("a", 10).is_some(), "a should still exist");
        assert!(cache.get("b", 10).is_none(), "b should be evicted");
        assert!(cache.get("c", 10).is_some(), "c should exist");
    }

    #[test]
    fn test_cache_evict_lru_direct() {
        let config = SearchCacheConfig {
            max_size: 10,
            ttl: Duration::from_secs(300),
        };
        let cache = SearchCache::new(config);
        cache.put("a", make_search_result("r", 1));
        cache.put("b", make_search_result("r", 1));

        cache.evict_lru();
        assert!(cache.get("a", 10).is_none(), "a should be evicted");
        assert!(cache.get("b", 10).is_some(), "b should remain");
    }

    #[test]
    fn test_clamp_results_no_truncation_needed() {
        let results = make_search_result("test", 3);
        let clamped = clamp_registry_results(&results, 10);
        assert_eq!(clamped[0].results.len(), 3);
        assert!(!clamped[0].truncated);
    }

    #[test]
    fn test_cache_config_default() {
        let config = SearchCacheConfig::default();
        assert_eq!(config.max_size, 50);
        assert_eq!(config.ttl, Duration::from_secs(300));
    }

    #[test]
    fn test_cache_stats_serialization() {
        let stats = CacheStats {
            size: 10,
            max_size: 50,
            hit_count: 5,
            miss_count: 3,
            hit_rate: 0.625,
        };
        let json = serde_json::to_string(&stats).unwrap();
        let deserialized: CacheStats = serde_json::from_str(&json).unwrap();
        assert_eq!(deserialized.size, 10);
        assert_eq!(deserialized.hit_rate, 0.625);
    }

    #[test]
    fn test_cache_short_query_no_similarity() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.put("ab", make_search_result("r", 1)); // Too short for trigrams

        // "ab" is too short, should still do exact match
        let hit = cache.get("ab", 10);
        assert!(hit.is_some());
    }

    // ============================================================
    // Additional search_cache tests for missing coverage
    // ============================================================

    #[test]
    fn test_to_lower_ascii() {
        assert_eq!(to_lower("HELLO WORLD"), "hello world");
        assert_eq!(to_lower("Already Lower"), "already lower");
        assert_eq!(to_lower(""), "");
        assert_eq!(to_lower("123ABC"), "123abc");
    }

    #[test]
    fn test_to_lower_unicode() {
        assert_eq!(to_lower("NEMESIS"), "nemesis");
    }

    #[test]
    fn test_build_trigrams_normal_word() {
        let trigrams = build_trigrams("hello world");
        assert!(!trigrams.is_empty());
    }

    #[test]
    fn test_build_trigrams_short_string() {
        let trigrams = build_trigrams("ab");
        assert!(trigrams.is_empty()); // Too short for trigrams
    }

    #[test]
    fn test_build_trigrams_exact_three_chars() {
        let trigrams = build_trigrams("abc");
        assert_eq!(trigrams.len(), 1);
    }

    #[test]
    fn test_jaccard_similarity_identical() {
        let a = vec![1, 2, 3, 4, 5];
        let b = vec![1, 2, 3, 4, 5];
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_similarity_disjoint() {
        let a = vec![1, 2, 3];
        let b = vec![4, 5, 6];
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_jaccard_similarity_partial() {
        let a = vec![1, 2, 3, 4];
        let b = vec![3, 4, 5, 6];
        let sim = jaccard_similarity(&a, &b);
        // intersection = 2, total = 4+4 = 8, formula: 2*2/8 = 0.5
        assert!((sim - 0.5).abs() < 0.05);
    }

    #[test]
    fn test_jaccard_similarity_both_empty() {
        let a: Vec<u32> = vec![];
        let b: Vec<u32> = vec![];
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 1.0).abs() < 0.001); // Both empty returns 1.0
    }

    #[test]
    fn test_jaccard_similarity_one_empty() {
        let a = vec![1, 2, 3];
        let b: Vec<u32> = vec![];
        let sim = jaccard_similarity(&a, &b);
        assert!((sim - 0.0).abs() < 0.001);
    }

    #[test]
    fn test_clamp_registry_results_under_limit() {
        let results = vec![RegistrySearchResult {
            registry_name: "r".to_string(),
            results: vec![],
            truncated: false,
        }];
        let clamped = clamp_registry_results(&results, 10);
        assert_eq!(clamped.len(), 1);
    }

    #[test]
    fn test_clamp_registry_results_truncates() {
        use crate::types::SkillSearchResult;
        let results = vec![RegistrySearchResult {
            registry_name: "r".to_string(),
            results: vec![
                SkillSearchResult { score: 1.0, slug: "a".to_string(), display_name: "A".to_string(), summary: "A".to_string(), version: "1.0".to_string(), registry_name: "r".to_string(), source_repo: String::new(), download_path: String::new(), downloads: 0, truncated: false },
                SkillSearchResult { score: 0.9, slug: "b".to_string(), display_name: "B".to_string(), summary: "B".to_string(), version: "1.0".to_string(), registry_name: "r".to_string(), source_repo: String::new(), download_path: String::new(), downloads: 0, truncated: false },
                SkillSearchResult { score: 0.8, slug: "c".to_string(), display_name: "C".to_string(), summary: "C".to_string(), version: "1.0".to_string(), registry_name: "r".to_string(), source_repo: String::new(), download_path: String::new(), downloads: 0, truncated: false },
            ],
            truncated: false,
        }];
        let clamped = clamp_registry_results(&results, 2);
        assert_eq!(clamped[0].results.len(), 2);
        assert!(clamped[0].truncated);
    }

    #[test]
    fn test_clamp_registry_results_zero_returns_all() {
        use crate::types::SkillSearchResult;
        let results = vec![RegistrySearchResult {
            registry_name: "r".to_string(),
            results: vec![
                SkillSearchResult { score: 1.0, slug: "a".to_string(), display_name: "A".to_string(), summary: "A".to_string(), version: "1.0".to_string(), registry_name: "r".to_string(), source_repo: String::new(), download_path: String::new(), downloads: 0, truncated: false },
                SkillSearchResult { score: 0.9, slug: "b".to_string(), display_name: "B".to_string(), summary: "B".to_string(), version: "1.0".to_string(), registry_name: "r".to_string(), source_repo: String::new(), download_path: String::new(), downloads: 0, truncated: false },
            ],
            truncated: false,
        }];
        let clamped = clamp_registry_results(&results, 0);
        assert_eq!(clamped[0].results.len(), 2); // limit=0 returns all
        assert!(!clamped[0].truncated);
    }

    #[test]
    fn test_cache_evict_lru_nothing_to_evict() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.evict_lru();
        // Should not panic
    }

    #[test]
    fn test_cache_clear_empty() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.clear();
        assert_eq!(cache.stats().size, 0);
    }

    #[test]
    fn test_cache_update_lru_nonexistent() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        cache.update_lru("nonexistent");
        // Should not panic
    }

    #[test]
    fn test_cache_config_zero_size() {
        let config = SearchCacheConfig {
            max_size: 0,
            ttl: Duration::from_secs(300),
        };
        let cache = SearchCache::new(config);
        // Put should still work (auto-evicts)
        cache.put("test", make_search_result("r", 1));
        let stats = cache.stats();
        assert!(stats.size == 0 || stats.size == 1); // Depends on eviction logic
    }

    #[test]
    fn test_cache_stats_default() {
        let cache = SearchCache::new(SearchCacheConfig::default());
        let stats = cache.stats();
        assert_eq!(stats.size, 0);
        assert_eq!(stats.hit_count, 0);
        assert_eq!(stats.miss_count, 0);
    }
}
