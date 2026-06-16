//! Additional tests for registry.rs covering cache, search aggregation,
//! install paths, and trait implementations.

use super::*;
use crate::types::{
    GitHubSourceConfig as TypesGitHubSourceConfig, InstallResult, RegistryConfig,
    SkillContent, SkillSearchResult,
};
use async_trait::async_trait;
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

// ============================================================
// Helper mock registries
// ============================================================

struct FailingRegistry;

#[async_trait]
impl SkillRegistry for FailingRegistry {
    fn name(&self) -> &str {
        "failing"
    }
    async fn search(&self, _q: &str, _l: usize) -> Result<Vec<SkillSearchResult>> {
        Err(NemesisError::Other("search failed".to_string()))
    }
    async fn get_skill_meta(&self, slug: &str) -> Result<crate::types::SkillMeta> {
        Err(NemesisError::NotFound(format!("not found: {}", slug)))
    }
    async fn download_and_install(
        &self,
        _slug: &str,
        _version: &str,
        _target: &str,
    ) -> Result<InstallResult> {
        Err(NemesisError::Other("install failed".to_string()))
    }
    async fn get_skill_content(&self, slug: &str) -> Result<SkillContent> {
        Err(NemesisError::NotFound(format!("no content: {}", slug)))
    }
    async fn browse(
        &self,
        _sort: &BrowseSort,
        _limit: usize,
        _cursor: &str,
    ) -> Result<BrowseResult> {
        Err(NemesisError::Other("browse failed".to_string()))
    }
}

struct CountingRegistry {
    name: String,
    search_calls: AtomicUsize,
}

impl CountingRegistry {
    fn new(name: &str) -> Self {
        Self {
            name: name.to_string(),
            search_calls: AtomicUsize::new(0),
        }
    }
}

#[async_trait]
impl SkillRegistry for CountingRegistry {
    fn name(&self) -> &str {
        &self.name
    }
    async fn search(&self, _q: &str, _l: usize) -> Result<Vec<SkillSearchResult>> {
        self.search_calls.fetch_add(1, Ordering::SeqCst);
        Ok(vec![SkillSearchResult {
            score: 0.9,
            slug: format!("{}-skill", self.name),
            display_name: "Test".to_string(),
            summary: "summary".to_string(),
            version: "1.0".to_string(),
            registry_name: self.name.clone(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 0,
            truncated: false,
        }])
    }
    async fn get_skill_meta(&self, _slug: &str) -> Result<crate::types::SkillMeta> {
        Ok(crate::types::SkillMeta {
            slug: "x".to_string(),
            display_name: "X".to_string(),
            summary: "summary".to_string(),
            latest_version: "1.0".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: self.name.clone(),
            author: String::new(),
            downloads: 0,
        })
    }
    async fn download_and_install(
        &self,
        _slug: &str,
        version: &str,
        _target: &str,
    ) -> Result<InstallResult> {
        Ok(InstallResult {
            version: version.to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: "ok".to_string(),
        })
    }
    async fn get_skill_content(&self, slug: &str) -> Result<SkillContent> {
        Ok(SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content: "# content".to_string(),
        })
    }
    async fn browse(&self, _sort: &BrowseSort, limit: usize, _cursor: &str) -> Result<BrowseResult> {
        Ok(BrowseResult {
            items: vec![SkillSearchResult {
                score: 0.9,
                slug: "x".to_string(),
                display_name: "X".to_string(),
                summary: "summary".to_string(),
                version: "1.0".to_string(),
                registry_name: self.name.clone(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            next_cursor: if limit > 0 { Some("next".to_string()) } else { None },
        })
    }
}

// ============================================================
// SkillRegistry trait default method tests
// ============================================================

struct DefaultOnlyRegistry;

#[async_trait]
impl SkillRegistry for DefaultOnlyRegistry {
    fn name(&self) -> &str {
        "default-only"
    }
    async fn search(&self, _q: &str, _l: usize) -> Result<Vec<SkillSearchResult>> {
        Ok(Vec::new())
    }
    async fn get_skill_meta(&self, slug: &str) -> Result<crate::types::SkillMeta> {
        Ok(crate::types::SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: String::new(),
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "default-only".to_string(),
            author: String::new(),
            downloads: 0,
        })
    }
    async fn download_and_install(
        &self,
        _slug: &str,
        version: &str,
        _target: &str,
    ) -> Result<InstallResult> {
        Ok(InstallResult {
            version: version.to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: String::new(),
        })
    }
    // get_skill_content and browse use default impls -> error
}

#[tokio::test]
async fn test_default_get_skill_content_returns_error() {
    let r = DefaultOnlyRegistry;
    let result = r.get_skill_content("x").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not implemented"));
}

#[tokio::test]
async fn test_default_browse_returns_error() {
    let r = DefaultOnlyRegistry;
    let result = r.browse(&BrowseSort::Trending, 10, "").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("not implemented"));
}

// ============================================================
// RegistryManager: concurrent fan-out + error aggregation
// ============================================================

#[tokio::test]
async fn test_search_all_with_mixed_success_and_failure() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("ok")));
    manager.add_registry(Arc::new(FailingRegistry));
    let results = manager.search_all("test", 10).await.unwrap();
    // Only the successful registry is included in results
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].registry_name, "ok");
    assert_eq!(results[0].results.len(), 1);
}

#[tokio::test]
async fn test_search_all_all_failures_returns_error() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(FailingRegistry));
    let err = manager.search_all("test", 10).await.unwrap_err();
    assert!(err.to_string().contains("all registries failed"));
}

#[tokio::test]
async fn test_search_all_combines_multiple_registries() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    manager.add_registry(Arc::new(CountingRegistry::new("b")));
    manager.add_registry(Arc::new(CountingRegistry::new("c")));
    let results = manager.search_all("test", 10).await.unwrap();
    assert_eq!(results.len(), 3);
}

#[tokio::test]
async fn test_search_merged_combines_and_sorts_by_score() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    manager.add_registry(Arc::new(CountingRegistry::new("b")));
    let results = manager.search("test", 10).await.unwrap();
    assert_eq!(results.len(), 2);
    // All have score 0.9, sort is stable
    assert!(results.iter().all(|r| r.score == 0.9));
}

#[tokio::test]
async fn test_search_merged_truncates_to_limit() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    manager.add_registry(Arc::new(CountingRegistry::new("b")));
    manager.add_registry(Arc::new(CountingRegistry::new("c")));
    let results = manager.search("test", 2).await.unwrap();
    assert_eq!(results.len(), 2);
}

// ============================================================
// Cache hit/miss behavior
// ============================================================

#[tokio::test]
async fn test_search_caches_results_on_second_call() {
    let manager = RegistryManager::new({
        let mut c = RegistryConfig::default();
        c.search_cache.enabled = true;
        c.search_cache.max_size = 10;
        c.search_cache.ttl_secs = 60;
        c
    });
    let counting = Arc::new(CountingRegistry::new("a"));
    manager.add_registry(counting.clone());

    let _ = manager.search("pdf", 10).await.unwrap();
    let first_count = counting.search_calls.load(Ordering::SeqCst);
    assert_eq!(first_count, 1);

    let _ = manager.search("pdf", 10).await.unwrap();
    let second_count = counting.search_calls.load(Ordering::SeqCst);
    // Should be cached - count doesn't increase
    assert_eq!(second_count, 1);
}

#[tokio::test]
async fn test_search_cache_not_used_when_disabled() {
    let manager = RegistryManager::new_empty();
    let counting = Arc::new(CountingRegistry::new("a"));
    manager.add_registry(counting.clone());

    let _ = manager.search("pdf", 10).await.unwrap();
    let _ = manager.search("pdf", 10).await.unwrap();
    // Cache disabled, search called twice
    assert_eq!(counting.search_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_search_cache_different_query_miss() {
    let manager = RegistryManager::new({
        let mut c = RegistryConfig::default();
        c.search_cache.enabled = true;
        c.search_cache.max_size = 10;
        c.search_cache.ttl_secs = 60;
        c
    });
    let counting = Arc::new(CountingRegistry::new("a"));
    manager.add_registry(counting.clone());

    let _ = manager.search("pdf", 10).await.unwrap();
    let _ = manager.search("csv", 10).await.unwrap();
    // Different queries -> two calls
    assert_eq!(counting.search_calls.load(Ordering::SeqCst), 2);
}

#[tokio::test]
async fn test_search_cache_stores_per_registry_groups() {
    // Even if a registry returns empty inner results, the grouped result
    // (one RegistrySearchResult per registry) is stored in the cache.
    let manager = RegistryManager::new({
        let mut c = RegistryConfig::default();
        c.search_cache.enabled = true;
        c.search_cache.max_size = 10;
        c.search_cache.ttl_secs = 60;
        c
    });
    // StubRegistry returns empty results
    manager.add_registry(Arc::new(StubRegistryProvider));

    let _ = manager.search_all("anything", 10).await.unwrap();
    let cache = manager.get_search_cache();
    let cached = cache.as_ref().unwrap().get("anything", 10);
    // grouped has 1 entry (the stub), so it IS cached.
    assert!(cached.is_some());
}

// ============================================================
// Truncation flag handling
// ============================================================

struct TruncatedRegistry;

#[async_trait]
impl SkillRegistry for TruncatedRegistry {
    fn name(&self) -> &str {
        "trunc"
    }
    async fn search(&self, _q: &str, _l: usize) -> Result<Vec<SkillSearchResult>> {
        Ok(vec![
            SkillSearchResult {
                score: 1.0,
                slug: "first".to_string(),
                display_name: "First".to_string(),
                summary: "s".to_string(),
                version: "1.0".to_string(),
                registry_name: "trunc".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
            SkillSearchResult {
                score: 0.5,
                slug: "last".to_string(),
                display_name: "Last".to_string(),
                summary: "s".to_string(),
                version: "1.0".to_string(),
                registry_name: "trunc".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: true, // last result marked truncated
            },
        ])
    }
    async fn get_skill_meta(&self, _: &str) -> Result<crate::types::SkillMeta> {
        unimplemented!()
    }
    async fn download_and_install(&self, _: &str, _: &str, _: &str) -> Result<InstallResult> {
        unimplemented!()
    }
}

#[tokio::test]
async fn test_search_all_handles_truncation_flag() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(TruncatedRegistry));
    let results = manager.search_all("test", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].registry_name, "trunc");
    // truncated flag is true (from last result)
    assert!(results[0].truncated);
    // The last item's truncated field is reset to false
    assert!(!results[0].results.last().unwrap().truncated);
}

// ============================================================
// install / get_skill_content / browse routing
// ============================================================

#[tokio::test]
async fn test_install_calls_registry() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    let dir = tempfile::tempdir().unwrap();
    let version = manager
        .install("a", "pdf", dir.path().to_str().unwrap())
        .await
        .unwrap();
    assert_eq!(version, "latest");
}

#[tokio::test]
async fn test_get_skill_content_routes_to_registry() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    let content = manager.get_skill_content("a", "pdf").await.unwrap();
    assert_eq!(content.slug, "pdf");
    assert_eq!(content.filename, "SKILL.md");
    assert!(content.content.contains("content"));
}

#[tokio::test]
async fn test_get_skill_content_missing_registry() {
    let manager = RegistryManager::new_empty();
    let err = manager.get_skill_content("nope", "pdf").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_browse_routes_to_registry() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    let result = manager.browse("a", &BrowseSort::Trending, 10, "").await.unwrap();
    assert_eq!(result.items.len(), 1);
}

#[tokio::test]
async fn test_browse_missing_registry() {
    let manager = RegistryManager::new_empty();
    let err = manager.browse("nope", &BrowseSort::Trending, 10, "").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_install_missing_registry_returns_not_found() {
    let manager = RegistryManager::new_empty();
    let err = manager.install("nope", "x", "/tmp").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

// ============================================================
// from_config: modelscope, multiple github sources
// ============================================================

#[test]
fn test_from_config_with_modelscope_enabled() {
    let mut config = RegistryConfig::default();
    config.modelscope.enabled = true;
    let manager = RegistryManager::from_config(config);
    let reg = manager.registries();
    assert_eq!(reg.len(), 1);
    assert_eq!(reg[0], "modelscope");
}

#[test]
fn test_from_config_all_three_registries() {
    let mut config = RegistryConfig::default();
    config.github.enabled = true;
    config.clawhub.enabled = true;
    config.modelscope.enabled = true;
    let manager = RegistryManager::from_config(config);
    let reg = manager.registries();
    assert_eq!(reg.len(), 3);
}

#[test]
fn test_from_config_with_disabled_modelscope() {
    let mut config = RegistryConfig::default();
    config.modelscope.enabled = false;
    let manager = RegistryManager::from_config(config);
    assert!(manager.registries().is_empty());
}

#[test]
fn test_from_config_with_disabled_clawhub() {
    let mut config = RegistryConfig::default();
    config.clawhub.enabled = false;
    let manager = RegistryManager::from_config(config);
    assert!(manager.registries().is_empty());
}

#[test]
fn test_from_config_multi_source_overrides_legacy() {
    // When github_sources has entries AND github.enabled is true,
    // multi-source takes precedence (legacy skipped)
    let mut config = RegistryConfig::default();
    config.github.enabled = true;
    config.github_sources.push(TypesGitHubSourceConfig {
        name: "multi".to_string(),
        repo: "org/multi".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    });
    let manager = RegistryManager::from_config(config);
    let reg = manager.registries();
    // Only multi-source, legacy skipped because github_sources is non-empty
    assert_eq!(reg.len(), 1);
    assert_eq!(reg[0], "multi");
}

#[test]
fn test_from_config_legacy_only_when_no_multi_source() {
    let mut config = RegistryConfig::default();
    config.github.enabled = true;
    let manager = RegistryManager::from_config(config);
    let reg = manager.registries();
    assert_eq!(reg.len(), 1);
    assert_eq!(reg[0], "github");
}

#[test]
fn test_from_config_with_cache_disabled() {
    let mut config = RegistryConfig::default();
    config.search_cache.enabled = false;
    let manager = RegistryManager::from_config(config);
    let cache = manager.get_search_cache();
    assert!(cache.is_none());
}

#[test]
fn test_from_config_zero_max_concurrent_uses_default() {
    let mut config = RegistryConfig::default();
    config.max_concurrent_searches = 0;
    let manager = RegistryManager::from_config(config);
    assert_eq!(manager.max_concurrent, DEFAULT_MAX_CONCURRENT);
}

#[test]
fn test_from_config_custom_max_concurrent() {
    let mut config = RegistryConfig::default();
    config.max_concurrent_searches = 8;
    let manager = RegistryManager::from_config(config);
    assert_eq!(manager.max_concurrent, 8);
}

// ============================================================
// add_source validation
// ============================================================

#[test]
fn test_add_source_branch_defaults_to_main() {
    let manager = RegistryManager::new_empty();
    manager.add_source("s".to_string(), "r".to_string(), None).unwrap();
    let cfg = manager.config.read();
    assert_eq!(cfg.github_sources[0].branch, "main");
}

#[test]
fn test_add_source_creates_default_index_settings() {
    let manager = RegistryManager::new_empty();
    manager.add_source("s".to_string(), "r".to_string(), None).unwrap();
    let cfg = manager.config.read();
    let s = &cfg.github_sources[0];
    assert_eq!(s.index_type, "github_api");
    assert_eq!(s.index_path, "");
    assert_eq!(s.skill_path_pattern, "skills/{slug}/SKILL.md");
    assert!(s.enabled);
}

#[test]
fn test_add_source_multiple_distinct_names() {
    let manager = RegistryManager::new_empty();
    manager.add_source("a".to_string(), "r1".to_string(), None).unwrap();
    manager.add_source("b".to_string(), "r2".to_string(), None).unwrap();
    manager.add_source("c".to_string(), "r3".to_string(), None).unwrap();
    let cfg = manager.config.read();
    assert_eq!(cfg.github_sources.len(), 3);
}

#[test]
fn test_add_source_same_name_different_repo_fails() {
    let manager = RegistryManager::new_empty();
    manager.add_source("dup".to_string(), "r1".to_string(), None).unwrap();
    let err = manager.add_source("dup".to_string(), "r2".to_string(), None).unwrap_err();
    assert!(err.to_string().contains("already exists"));
}

// ============================================================
// compute_relevance branch coverage
// ============================================================

#[test]
fn test_compute_relevance_word_match_adds_score() {
    let skill = RegistrySkill {
        slug: "tool".to_string(),
        display_name: "Multi Word Name".to_string(),
        summary: "Another word".to_string(),
        version: "1.0".to_string(),
    };
    // Multi-word query matches some words but not slug
    let score = RegistryManager::compute_relevance("multi word", &skill);
    assert!(score > 0.0);
}

#[test]
fn test_compute_relevance_full_word_match_in_summary() {
    let skill = RegistrySkill {
        slug: "x".to_string(),
        display_name: "Y".to_string(),
        summary: "this is a long summary with pdf keyword".to_string(),
        version: "1.0".to_string(),
    };
    let score = RegistryManager::compute_relevance("pdf", &skill);
    assert!(score >= 0.3);
}

#[test]
fn test_compute_relevance_multi_word_query_partial_match() {
    let skill = RegistrySkill {
        slug: "pdf".to_string(),
        display_name: "tool".to_string(),
        summary: "misc".to_string(),
        version: "1.0".to_string(),
    };
    // Query has 2 words, only 1 matches
    let score = RegistryManager::compute_relevance("pdf excel", &skill);
    assert!(score > 0.0);
}

#[test]
fn test_compute_relevance_exact_slug_match_adds_one() {
    let skill = RegistrySkill {
        slug: "pdf".to_string(),
        display_name: "other".to_string(),
        summary: "misc".to_string(),
        version: "1.0".to_string(),
    };
    let score = RegistryManager::compute_relevance("pdf", &skill);
    // slug exact match (1.0) + summary doesn't contain "pdf" -> 1.0
    assert_eq!(score, 1.0);
}

#[test]
fn test_compute_relevance_partial_slug_only() {
    let skill = RegistrySkill {
        slug: "my-pdf-tool".to_string(),
        display_name: "X".to_string(),
        summary: "Y".to_string(),
        version: "1.0".to_string(),
    };
    let score = RegistryManager::compute_relevance("pdf", &skill);
    // slug contains pdf -> 0.7
    assert!(score >= 0.7);
}

// ============================================================
// StubRegistryProvider trait coverage
// ============================================================

#[tokio::test]
async fn test_stub_get_skill_meta() {
    let stub = StubRegistryProvider;
    let meta = stub.get_skill_meta("x").await.unwrap();
    assert_eq!(meta.slug, "x");
    assert_eq!(meta.registry_name, "stub");
    assert_eq!(meta.latest_version, "latest");
}

#[tokio::test]
async fn test_stub_download_and_install() {
    let stub = StubRegistryProvider;
    let result = stub.download_and_install("x", "1.0", "/tmp").await.unwrap();
    assert_eq!(result.version, "1.0");
    assert!(!result.is_malware_blocked);
}

#[tokio::test]
async fn test_stub_browse_returns_empty() {
    let stub = StubRegistryProvider;
    let result = stub.browse(&BrowseSort::Trending, 10, "").await.unwrap();
    assert!(result.items.is_empty());
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_stub_search_returns_empty_vec() {
    let stub = StubRegistryProvider;
    let result = stub.search("anything", 10).await.unwrap();
    assert!(result.is_empty());
}

#[tokio::test]
async fn test_stub_default_get_skill_content_returns_error() {
    let stub = StubRegistryProvider;
    let result = stub.get_skill_content("x").await;
    // StubRegistryProvider doesn't override get_skill_content, uses default
    assert!(result.is_err());
}

// ============================================================
// get_registry by name
// ============================================================

#[test]
fn test_get_registry_returns_correct_one() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("alpha")));
    manager.add_registry(Arc::new(CountingRegistry::new("beta")));
    let found = manager.get_registry("beta").unwrap();
    assert_eq!(found.name(), "beta");
}

#[test]
fn test_get_registry_returns_none_for_unknown() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("alpha")));
    assert!(manager.get_registry("gamma").is_none());
}

#[test]
fn test_registries_accessor_returns_all_names() {
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(CountingRegistry::new("a")));
    manager.add_registry(Arc::new(CountingRegistry::new("b")));
    let names = manager.registries();
    assert_eq!(names.len(), 2);
    assert!(names.contains(&"a".to_string()));
    assert!(names.contains(&"b".to_string()));
}

// ============================================================
// SkillRegistry trait dispatch for concrete impls
// ============================================================

#[tokio::test]
async fn test_github_registry_trait_impl_routes_search() {
    // Use a github_api config so get_skill_meta doesn't make HTTP call
    let config = crate::github_registry::GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 1,
        max_size: 1024,
    };
    let gh = GitHubRegistry::from_source(&config);
    let trait_ref: &dyn SkillRegistry = &gh;
    let meta = trait_ref.get_skill_meta("pdf").await.unwrap();
    assert_eq!(meta.slug, "pdf");
    assert_eq!(meta.registry_name, "test");
}

#[tokio::test]
async fn test_modelscope_registry_trait_impl() {
    let ms = ModelScopeRegistry::new();
    let trait_ref: &dyn SkillRegistry = &ms;
    assert_eq!(trait_ref.name(), "modelscope");
}

#[tokio::test]
async fn test_clawhub_registry_trait_impl() {
    let ch = ClawHubRegistry::with_urls("https://base", "https://convex", "https://site");
    let trait_ref: &dyn SkillRegistry = &ch;
    assert_eq!(trait_ref.name(), "clawhub");
}
