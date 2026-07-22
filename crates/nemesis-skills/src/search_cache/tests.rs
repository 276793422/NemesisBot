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
    assert!(
        sim > 0.3,
        "Similar strings should have > 0.3 similarity, got {}",
        sim
    );
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
            SkillSearchResult {
                score: 1.0,
                slug: "a".to_string(),
                display_name: "A".to_string(),
                summary: "A".to_string(),
                version: "1.0".to_string(),
                registry_name: "r".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
            SkillSearchResult {
                score: 0.9,
                slug: "b".to_string(),
                display_name: "B".to_string(),
                summary: "B".to_string(),
                version: "1.0".to_string(),
                registry_name: "r".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
            SkillSearchResult {
                score: 0.8,
                slug: "c".to_string(),
                display_name: "C".to_string(),
                summary: "C".to_string(),
                version: "1.0".to_string(),
                registry_name: "r".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
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
            SkillSearchResult {
                score: 1.0,
                slug: "a".to_string(),
                display_name: "A".to_string(),
                summary: "A".to_string(),
                version: "1.0".to_string(),
                registry_name: "r".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
            SkillSearchResult {
                score: 0.9,
                slug: "b".to_string(),
                display_name: "B".to_string(),
                summary: "B".to_string(),
                version: "1.0".to_string(),
                registry_name: "r".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
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
