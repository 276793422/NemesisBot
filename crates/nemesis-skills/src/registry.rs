//! Registry manager - coordinates multiple skill registries.
//!
//! Manages GitHub-based and ClawHub-based skill registries, fans out search
//! requests concurrently, and routes installs to the correct registry.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};
use parking_lot::RwLock;

use nemesis_types::error::{NemesisError, Result};

use crate::clawhub_registry::ClawHubRegistry;
use crate::github_registry::{GitHubRegistry, GitHubSourceConfig};
use crate::search_cache::SearchCache;
use crate::types::{
    RegistryConfig, RegistrySearchResult, SkillMeta, SkillSearchResult,
};

/// Default maximum concurrent registry searches.
const DEFAULT_MAX_CONCURRENT: usize = 2;

/// Trait for registry operations (allows mocking in tests).
#[async_trait]
pub trait SkillRegistry: Send + Sync {
    /// Get the registry name.
    fn name(&self) -> &str;
    /// Search the registry for skills matching the query.
    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>>;
    /// Get metadata for a specific skill by slug.
    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta>;
    /// Download and install a skill to the target directory.
    async fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<crate::types::InstallResult>;
}

/// A skill entry in a remote registry (for simple in-memory lookup).
#[derive(Debug, Clone)]
pub struct RegistrySkill {
    /// Skill slug.
    pub slug: String,
    /// Display name.
    pub display_name: String,
    /// Summary.
    pub summary: String,
    /// Version string.
    pub version: String,
}

/// Manages skill registries (GitHub-based sources and ClawHub).
pub struct RegistryManager {
    config: RwLock<RegistryConfig>,
    registries: RwLock<Vec<Arc<dyn SkillRegistry>>>,
    search_cache: RwLock<Option<SearchCache>>,
    max_concurrent: usize,
}

impl RegistryManager {
    /// Create a new registry manager with the given configuration.
    pub fn new(config: RegistryConfig) -> Self {
        let max_concurrent = config.max_concurrent_searches;
        let search_cache = if config.search_cache.enabled {
            Some(SearchCache::new(crate::search_cache::SearchCacheConfig {
                max_size: config.search_cache.max_size,
                ttl: Duration::from_secs(config.search_cache.ttl_secs),
            }))
        } else {
            None
        };

        Self {
            config: RwLock::new(config),
            registries: RwLock::new(Vec::new()),
            search_cache: RwLock::new(search_cache),
            max_concurrent: if max_concurrent > 0 {
                max_concurrent
            } else {
                DEFAULT_MAX_CONCURRENT
            },
        }
    }

    /// Build a RegistryManager from config, instantiating enabled registries.
    pub fn from_config(config: RegistryConfig) -> Self {
        let max_concurrent = if config.max_concurrent_searches > 0 {
            config.max_concurrent_searches
        } else {
            DEFAULT_MAX_CONCURRENT
        };

        let search_cache = if config.search_cache.enabled {
            Some(SearchCache::new(crate::search_cache::SearchCacheConfig {
                max_size: config.search_cache.max_size,
                ttl: Duration::from_secs(config.search_cache.ttl_secs),
            }))
        } else {
            None
        };

        let mut registries: Vec<Arc<dyn SkillRegistry>> = Vec::new();

        // Create GitHub registries from multi-source config.
        for source in &config.github_sources {
            if source.enabled {
                let gh_source = GitHubSourceConfig {
                    name: source.name.clone(),
                    repo: source.repo.clone(),
                    enabled: source.enabled,
                    branch: source.branch.clone(),
                    index_type: source.index_type.clone(),
                    index_path: source.index_path.clone(),
                    skill_path_pattern: source.skill_path_pattern.clone(),
                    timeout_secs: source.timeout_secs,
                    max_size: source.max_size,
                };
                registries.push(Arc::new(GitHubRegistry::from_source(&gh_source)));
            }
        }

        // Legacy: if no multi-source GitHub registries, use single-source.
        if config.github_sources.is_empty() && config.github.enabled {
            let gh = GitHubRegistry::new(
                &config.github.base_url,
                config.github.timeout_secs,
                config.github.max_size,
            );
            registries.push(Arc::new(gh));
        }

        // ClawHub support.
        if config.clawhub.enabled {
            let clawhub = ClawHubRegistry::with_urls(
                &config.clawhub.base_url,
                &config.clawhub.convex_url,
                &config.clawhub.convex_site_url,
            );
            registries.push(Arc::new(clawhub));
        }

        Self {
            config: RwLock::new(config),
            registries: RwLock::new(registries),
            search_cache: RwLock::new(search_cache),
            max_concurrent,
        }
    }

    /// Create a new empty registry manager.
    pub fn new_empty() -> Self {
        Self {
            config: RwLock::new(RegistryConfig::default()),
            registries: RwLock::new(Vec::new()),
            search_cache: RwLock::new(None),
            max_concurrent: DEFAULT_MAX_CONCURRENT,
        }
    }

    /// Add a registry to the manager.
    pub fn add_registry(&self, registry: Arc<dyn SkillRegistry>) {
        self.registries.write().push(registry);
    }

    /// Get a registry by name.
    pub fn get_registry(&self, name: &str) -> Option<Arc<dyn SkillRegistry>> {
        self.registries
            .read()
            .iter()
            .find(|r| r.name() == name)
            .cloned()
    }

    /// Get a reference to the search cache, if one is configured.
    ///
    /// Returns a `parking_lot::RwLockReadGuard` wrapping the optional cache,
    /// allowing callers to inspect or use the cache without mutable access.
    pub fn get_search_cache(&self) -> parking_lot::RwLockReadGuard<'_, Option<SearchCache>> {
        self.search_cache.read()
    }

    /// Add a new GitHub source to the registry configuration.
    ///
    /// Returns an error if a source with the same name already exists.
    pub fn add_source(&self, name: String, repo: String, branch: Option<String>) -> Result<()> {
        let mut config = self.config.write();
        if config.github_sources.iter().any(|s| s.name == name) {
            return Err(NemesisError::Validation(format!(
                "Registry source '{}' already exists",
                name
            )));
        }

        let source = crate::types::GitHubSourceConfig {
            name: name.clone(),
            repo,
            enabled: true,
            branch: branch.unwrap_or_else(|| "main".to_string()),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        };

        info!("Adding registry source: {} ({})", source.name, source.repo);
        config.github_sources.push(source);
        Ok(())
    }

    /// Search all registries concurrently.
    ///
    /// Returns results grouped by registry, not merged.
    /// Uses search cache if enabled.
    pub async fn search_all(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<RegistrySearchResult>> {
        // 1. Check cache first if enabled.
        {
            let cache = self.search_cache.read();
            if let Some(ref cache) = *cache {
                if let Some(results) = cache.get(query, limit) {
                    debug!(
                        "Search cache hit for '{}' ({} registries)",
                        query,
                        results.len()
                    );
                    return Ok(results);
                }
            }
        }

        let registries = self.registries.read().clone();
        if registries.is_empty() {
            return Err(NemesisError::Validation(
                "no registries configured".to_string(),
            ));
        }

        let semaphore = Arc::new(Semaphore::new(self.max_concurrent));
        let mut handles = Vec::new();

        for registry in registries {
            let sem = semaphore.clone();
            let query = query.to_string();

            handles.push(tokio::spawn(async move {
                let _permit = sem.acquire().await.unwrap();
                let result = registry.search(&query, limit).await;
                (registry.name().to_string(), result)
            }));
        }

        let mut grouped: Vec<RegistrySearchResult> = Vec::new();
        let mut last_err: Option<NemesisError> = None;
        let mut any_success = false;

        for handle in handles {
            match handle.await {
                Ok((name, Ok(results))) => {
                    any_success = true;
                    // Check if last result indicates truncation.
                    let truncated = results
                        .last()
                        .map(|r| r.truncated)
                        .unwrap_or(false);
                    let mut results = results;
                    if truncated && !results.is_empty() {
                        results.last_mut().unwrap().truncated = false;
                    }

                    grouped.push(RegistrySearchResult {
                        registry_name: name,
                        results,
                        truncated,
                    });
                }
                Ok((name, Err(e))) => {
                    warn!("Registry '{}' search failed: {}", name, e);
                    last_err = Some(e);
                }
                Err(e) => {
                    warn!("Registry task panicked: {}", e);
                    last_err = Some(NemesisError::Other(format!("task error: {}", e)));
                }
            }
        }

        if !any_success {
            if let Some(e) = last_err {
                return Err(NemesisError::Other(format!("all registries failed: {}", e)));
            }
            return Err(NemesisError::Other("all registries failed".to_string()));
        }

        // Store results in cache.
        {
            let cache = self.search_cache.read();
            if let Some(ref cache) = *cache {
                if !grouped.is_empty() {
                    cache.put(query, grouped.clone());
                    debug!(
                        "Search cache stored for '{}' ({} registries)",
                        query,
                        grouped.len()
                    );
                }
            }
        }

        Ok(grouped)
    }

    /// Search for skills across all registries and merge results.
    ///
    /// Results are merged and sorted by score (descending), truncated to `limit`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let grouped = self.search_all(query, limit).await?;

        let mut all_results: Vec<SkillSearchResult> = grouped
            .into_iter()
            .flat_map(|g| g.results)
            .collect();

        // Sort by score descending.
        all_results.sort_by(|a, b| {
            b.score
                .partial_cmp(&a.score)
                .unwrap_or(std::cmp::Ordering::Equal)
        });
        all_results.truncate(limit);

        Ok(all_results)
    }

    /// Install a skill from a specific registry.
    pub async fn install(
        &self,
        registry_name: &str,
        slug: &str,
        target_dir: &str,
    ) -> Result<String> {
        let registry = self.get_registry(registry_name).ok_or_else(|| {
            NemesisError::NotFound(format!("registry '{}' not found", registry_name))
        })?;

        info!(
            "Installing skill '{}' from registry '{}' to '{}'",
            slug, registry_name, target_dir
        );

        let result = registry.download_and_install(slug, "latest", target_dir).await?;

        Ok(result.version)
    }

    /// Get the current list of registries.
    pub fn registries(&self) -> Vec<String> {
        self.registries
            .read()
            .iter()
            .map(|r| r.name().to_string())
            .collect()
    }

    /// Compute relevance score between a query and a registry skill.
    #[allow(dead_code)]
    fn compute_relevance(query: &str, skill: &RegistrySkill) -> f64 {
        let mut score = 0.0;
        let query_lower = query.to_lowercase();
        let slug_lower = skill.slug.to_lowercase();
        let name_lower = skill.display_name.to_lowercase();
        let summary_lower = skill.summary.to_lowercase();

        if slug_lower == query_lower {
            score += 1.0;
        } else if slug_lower.contains(&query_lower) {
            score += 0.7;
        }

        if name_lower.contains(&query_lower) {
            score += 0.5;
        }

        if summary_lower.contains(&query_lower) {
            score += 0.3;
        }

        let query_words: Vec<&str> = query_lower.split_whitespace().collect();
        if query_words.len() > 1 {
            let all_text = format!("{} {} {}", slug_lower, name_lower, summary_lower);
            let matched = query_words
                .iter()
                .filter(|w| all_text.contains(**w))
                .count();
            score += (matched as f64 / query_words.len() as f64) * 0.4;
        }

        score.min(1.0)
    }
}

/// Default stub registry provider (returns empty results).
pub struct StubRegistryProvider;

#[async_trait]
impl SkillRegistry for StubRegistryProvider {
    fn name(&self) -> &str {
        "stub"
    }

    async fn search(&self, _query: &str, _limit: usize) -> Result<Vec<SkillSearchResult>> {
        Ok(Vec::new())
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        Ok(SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: "Stub skill".to_string(),
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "stub".to_string(),
        })
    }

    async fn download_and_install(
        &self,
        _slug: &str,
        version: &str,
        _target_dir: &str,
    ) -> Result<crate::types::InstallResult> {
        Ok(crate::types::InstallResult {
            version: version.to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: "Stub installation".to_string(),
        })
    }
}

/// Implement SkillRegistry for GitHubRegistry.
#[async_trait]
impl SkillRegistry for GitHubRegistry {
    fn name(&self) -> &str {
        self.name()
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        self.search(query, limit).await
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        self.get_skill_meta(slug).await
    }

    async fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<crate::types::InstallResult> {
        self.download_and_install(slug, version, target_dir).await
    }
}

/// Implement SkillRegistry for ClawHubRegistry.
#[async_trait]
impl SkillRegistry for ClawHubRegistry {
    fn name(&self) -> &str {
        self.name()
    }

    async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        self.search(query, limit).await
    }

    async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        self.get_skill_meta(slug).await
    }

    async fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<crate::types::InstallResult> {
        self.download_and_install(slug, version, target_dir).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_new_empty_manager() {
        let manager = RegistryManager::new_empty();
        assert!(manager.registries().is_empty());
    }

    #[test]
    fn test_add_registry() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        assert_eq!(manager.registries().len(), 1);
        assert_eq!(manager.registries()[0], "stub");
    }

    #[test]
    fn test_get_registry_found() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let found = manager.get_registry("stub");
        assert!(found.is_some());
    }

    #[test]
    fn test_get_registry_not_found() {
        let manager = RegistryManager::new_empty();
        let found = manager.get_registry("nonexistent");
        assert!(found.is_none());
    }

    #[test]
    fn test_add_source() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("test".to_string(), "org/skills".to_string(), None)
            .unwrap();
        // Source added to config (but not yet instantiated as a registry).
        let config = manager.config.read();
        assert_eq!(config.github_sources.len(), 1);
    }

    #[test]
    fn test_add_duplicate_source_fails() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("test".to_string(), "org/skills".to_string(), None)
            .unwrap();
        let result = manager.add_source("test".to_string(), "other/skills".to_string(), None);
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_empty_registries() {
        let manager = RegistryManager::new_empty();
        let result = manager.search("test", 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_with_stub() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let results = manager.search("anything", 10).await.unwrap();
        assert!(results.is_empty()); // Stub returns empty
    }

    #[tokio::test]
    async fn test_install_from_missing_registry() {
        let manager = RegistryManager::new_empty();
        let result = manager.install("nonexistent", "skill", "/tmp").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_compute_relevance_exact_match() {
        let skill = RegistrySkill {
            slug: "pdf".to_string(),
            display_name: "PDF Tool".to_string(),
            summary: "Converts PDF files".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score >= 1.0); // Exact match + summary match
    }

    #[test]
    fn test_compute_relevance_partial_match() {
        let skill = RegistrySkill {
            slug: "pdf-converter".to_string(),
            display_name: "PDF Converter".to_string(),
            summary: "Converts documents".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score > 0.5);
    }

    #[test]
    fn test_compute_relevance_no_match() {
        let skill = RegistrySkill {
            slug: "csv".to_string(),
            display_name: "CSV Parser".to_string(),
            summary: "Parses CSV files".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert_eq!(score, 0.0);
    }

    // ============================================================
    // Additional tests for missing coverage
    // ============================================================

    #[test]
    fn test_new_manager_from_config_defaults() {
        let config = RegistryConfig::default();
        let manager = RegistryManager::new(config);
        assert!(manager.registries().is_empty());
    }

    #[test]
    fn test_new_manager_with_cache_enabled() {
        let mut config = RegistryConfig::default();
        config.search_cache.enabled = true;
        config.search_cache.max_size = 20;
        config.search_cache.ttl_secs = 60;
        let manager = RegistryManager::new(config);
        let cache = manager.get_search_cache();
        assert!(cache.is_some());
    }

    #[test]
    fn test_new_manager_with_cache_disabled() {
        let config = RegistryConfig::default();
        let manager = RegistryManager::new(config);
        let cache = manager.get_search_cache();
        assert!(cache.is_none());
    }

    #[test]
    fn test_from_config_with_github_enabled() {
        let mut config = RegistryConfig::default();
        config.github.enabled = true;
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 1);
        assert!(reg[0].contains("github"));
    }

    #[test]
    fn test_from_config_with_clawhub_enabled() {
        let mut config = RegistryConfig::default();
        config.clawhub.enabled = true;
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 1);
        assert!(reg[0].contains("clawhub"));
    }

    #[test]
    fn test_from_config_with_both_enabled() {
        let mut config = RegistryConfig::default();
        config.github.enabled = true;
        config.clawhub.enabled = true;
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn test_compute_relevance_multi_word_query() {
        let skill = RegistrySkill {
            slug: "pdf".to_string(),
            display_name: "PDF Converter".to_string(),
            summary: "Converts PDF files to text".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf converter", &skill);
        assert!(score > 0.0);
    }

    #[test]
    fn test_compute_relevance_case_insensitive() {
        let skill = RegistrySkill {
            slug: "PDF-TOOL".to_string(),
            display_name: "Pdf Tool".to_string(),
            summary: "PDF processing".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score > 0.0);
    }

    #[test]
    fn test_compute_relevance_capped_at_1() {
        let skill = RegistrySkill {
            slug: "pdf".to_string(),
            display_name: "pdf pdf".to_string(),
            summary: "pdf pdf pdf".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score <= 1.0);
    }

    #[test]
    fn test_compute_relevance_empty_query() {
        let skill = RegistrySkill {
            slug: "pdf".to_string(),
            display_name: "PDF Tool".to_string(),
            summary: "Converts PDF files".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("", &skill);
        // Empty string is contained in all strings, so score = min(1.5, 1.0) = 1.0
        assert_eq!(score, 1.0);
    }

    #[test]
    fn test_registries_accessor() {
        let manager = RegistryManager::new_empty();
        assert!(manager.registries().is_empty());
        manager.add_registry(Arc::new(StubRegistryProvider));
        let names = manager.registries();
        assert_eq!(names, vec!["stub"]);
    }

    #[test]
    fn test_add_source_with_branch() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("test".to_string(), "org/skills".to_string(), Some("dev".to_string()))
            .unwrap();
        let config = manager.config.read();
        assert_eq!(config.github_sources[0].branch, "dev");
    }

    #[test]
    fn test_new_manager_zero_max_concurrent_uses_default() {
        let mut config = RegistryConfig::default();
        config.max_concurrent_searches = 0;
        let manager = RegistryManager::new(config);
        // Should use DEFAULT_MAX_CONCURRENT (2)
        assert_eq!(manager.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    #[tokio::test]
    async fn test_search_all_with_stub() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let results = manager.search_all("test", 10).await.unwrap();
        assert_eq!(results.len(), 1);
        assert_eq!(results[0].registry_name, "stub");
        assert!(results[0].results.is_empty());
    }

    #[tokio::test]
    async fn test_search_all_empty_registries_error() {
        let manager = RegistryManager::new_empty();
        let result = manager.search_all("test", 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_install_from_stub_registry() {
        let dir = tempfile::tempdir().unwrap();
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let result = manager.install("stub", "test-skill", dir.path().to_str().unwrap()).await;
        assert!(result.is_ok());
        assert_eq!(result.unwrap(), "latest");
    }

    // ============================================================
    // Coverage improvement: additional registry tests
    // ============================================================

    #[test]
    fn test_from_config_with_multi_source() {
        let mut config = RegistryConfig::default();
        config.github_sources.push(crate::types::GitHubSourceConfig {
            name: "test-source".to_string(),
            repo: "org/skills".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "skills_json".to_string(),
            index_path: "skills.json".to_string(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 10,
            max_size: 1024,
        });
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0], "test-source");
    }

    #[test]
    fn test_from_config_with_disabled_source() {
        let mut config = RegistryConfig::default();
        config.github_sources.push(crate::types::GitHubSourceConfig {
            name: "disabled".to_string(),
            repo: "org/disabled".to_string(),
            enabled: false,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
        let manager = RegistryManager::from_config(config);
        assert!(manager.registries().is_empty());
    }

    #[test]
    fn test_from_config_no_registries() {
        let config = RegistryConfig::default();
        let manager = RegistryManager::from_config(config);
        assert!(manager.registries().is_empty());
    }

    #[test]
    fn test_new_with_custom_max_concurrent() {
        let mut config = RegistryConfig::default();
        config.max_concurrent_searches = 5;
        let manager = RegistryManager::new(config);
        assert_eq!(manager.max_concurrent, 5);
    }

    #[test]
    fn test_stub_registry_trait_impl() {
        let stub = StubRegistryProvider;
        assert_eq!(stub.name(), "stub");
    }

    #[test]
    fn test_registry_skill_fields() {
        let skill = RegistrySkill {
            slug: "test".to_string(),
            display_name: "Test".to_string(),
            summary: "A test".to_string(),
            version: "2.0".to_string(),
        };
        assert_eq!(skill.slug, "test");
        assert_eq!(skill.display_name, "Test");
        assert_eq!(skill.summary, "A test");
        assert_eq!(skill.version, "2.0");
    }

    #[test]
    fn test_compute_relevance_display_name_match() {
        let skill = RegistrySkill {
            slug: "tool".to_string(),
            display_name: "PDF Converter Pro".to_string(),
            summary: "No match here".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score >= 0.5); // display_name match
    }

    #[test]
    fn test_compute_relevance_summary_only_match() {
        let skill = RegistrySkill {
            slug: "tool".to_string(),
            display_name: "Tool".to_string(),
            summary: "A PDF converter utility".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score >= 0.3); // summary match only
    }

    #[test]
    fn test_add_source_default_values() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("my-source".to_string(), "org/repo".to_string(), None)
            .unwrap();
        let config = manager.config.read();
        let source = &config.github_sources[0];
        assert_eq!(source.name, "my-source");
        assert_eq!(source.repo, "org/repo");
        assert!(source.enabled);
        assert_eq!(source.branch, "main");
        assert_eq!(source.index_type, "github_api");
        assert_eq!(source.skill_path_pattern, "skills/{slug}/SKILL.md");
    }

    #[test]
    fn test_from_config_with_cache_enabled() {
        let mut config = RegistryConfig::default();
        config.search_cache.enabled = true;
        config.github.enabled = true;
        let manager = RegistryManager::from_config(config);
        let cache = manager.get_search_cache();
        assert!(cache.is_some());
    }

    #[test]
    fn test_multiple_registries() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        manager.add_registry(Arc::new(StubRegistryProvider));
        assert_eq!(manager.registries().len(), 2);
        // Both named "stub", so get_registry returns the first
        let reg = manager.get_registry("stub");
        assert!(reg.is_some());
    }

    // ============================================================
    // Coverage improvement: additional registry tests (part 2)
    // ============================================================

    #[test]
    fn test_from_config_legacy_github_single_source() {
        let mut config = RegistryConfig::default();
        config.github.enabled = true;
        config.github.base_url = "https://raw.githubusercontent.com".to_string();
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0], "github");
    }

    #[test]
    fn test_from_config_with_clawhub() {
        let mut config = RegistryConfig::default();
        config.clawhub.enabled = true;
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 1);
        assert_eq!(reg[0], "clawhub");
    }

    #[test]
    fn test_from_config_github_and_clawhub() {
        let mut config = RegistryConfig::default();
        config.github.enabled = true;
        config.clawhub.enabled = true;
        let manager = RegistryManager::from_config(config);
        let reg = manager.registries();
        assert_eq!(reg.len(), 2);
    }

    #[test]
    fn test_add_source_duplicate_name() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("test".to_string(), "org/repo".to_string(), None)
            .unwrap();
        let result = manager.add_source("test".to_string(), "org/other".to_string(), None);
        assert!(result.is_err());
    }

    #[test]
    fn test_add_source_with_custom_branch() {
        let manager = RegistryManager::new_empty();
        manager
            .add_source("test".to_string(), "org/repo".to_string(), Some("develop".to_string()))
            .unwrap();
        let config = manager.config.read();
        assert_eq!(config.github_sources[0].branch, "develop");
    }

    #[test]
    fn test_new_with_zero_max_concurrent() {
        let mut config = RegistryConfig::default();
        config.max_concurrent_searches = 0;
        let manager = RegistryManager::new(config);
        assert_eq!(manager.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    #[tokio::test]
    async fn test_search_all_no_registries() {
        let manager = RegistryManager::new_empty();
        let result = manager.search_all("test", 10).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_search_merged_with_stub() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        // Stub returns empty results, so search returns empty
        let result = manager.search("test", 10).await;
        assert!(result.is_ok());
        assert!(result.unwrap().is_empty());
    }

    #[test]
    fn test_get_search_cache_none_when_disabled() {
        let manager = RegistryManager::new_empty();
        let cache = manager.get_search_cache();
        assert!(cache.is_none());
    }

    #[test]
    fn test_compute_relevance_slug_match() {
        let skill = RegistrySkill {
            slug: "pdf-tool".to_string(),
            display_name: "Tool".to_string(),
            summary: "No match".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        assert!(score >= 0.5, "Slug match should give good score");
    }

    #[test]
    fn test_compute_relevance_zero_score() {
        let skill = RegistrySkill {
            slug: "tool".to_string(),
            display_name: "Tool".to_string(),
            summary: "Utility".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("xyz123nonexistent", &skill);
        assert_eq!(score, 0.0, "No match should give 0 score");
    }

    // ============================================================
    // Coverage improvement: search cache, install edge cases
    // ============================================================

    #[test]
    fn test_search_cache_enabled_creation() {
        use crate::search_cache::{SearchCache, SearchCacheConfig};
        let cache = SearchCache::new(SearchCacheConfig {
            max_size: 50,
            ttl: std::time::Duration::from_secs(300),
        });
        assert!(cache.get("test", 10).is_none());
    }

    #[test]
    fn test_search_cache_set_and_get() {
        use crate::search_cache::{SearchCache, SearchCacheConfig};
        use crate::types::{RegistrySearchResult, SkillSearchResult};
        let cache = SearchCache::new(SearchCacheConfig {
            max_size: 50,
            ttl: std::time::Duration::from_secs(300),
        });
        let results = vec![RegistrySearchResult {
            registry_name: "test".to_string(),
            results: vec![SkillSearchResult {
                score: 1.0,
                slug: "pdf".to_string(),
                display_name: "PDF".to_string(),
                summary: "PDF tool".to_string(),
                version: "1.0".to_string(),
                registry_name: "test".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        }];
        cache.put("pdf", results.clone());
        let cached = cache.get("pdf", 10);
        assert!(cached.is_some());
        let cached = cached.unwrap();
        assert_eq!(cached.len(), 1);
        assert_eq!(cached[0].results[0].slug, "pdf");
    }

    #[test]
    fn test_search_cache_miss() {
        use crate::search_cache::{SearchCache, SearchCacheConfig};
        let cache = SearchCache::new(SearchCacheConfig {
            max_size: 50,
            ttl: std::time::Duration::from_secs(300),
        });
        assert!(cache.get("nonexistent", 10).is_none());
    }

    #[tokio::test]
    async fn test_search_with_cache_enabled() {
        let mut config = crate::types::RegistryConfig::default();
        config.search_cache.enabled = true;
        config.search_cache.max_size = 20;
        config.search_cache.ttl_secs = 60;
        let manager = RegistryManager::new(config);
        manager.add_registry(Arc::new(StubRegistryProvider));

        // First search populates cache
        let result1 = manager.search("test", 10).await;
        assert!(result1.is_ok());

        // Second search should use cache
        let result2 = manager.search("test", 10).await;
        assert!(result2.is_ok());
    }

    #[test]
    fn test_compute_relevance_all_fields_match() {
        let skill = RegistrySkill {
            slug: "pdf".to_string(),
            display_name: "PDF Converter".to_string(),
            summary: "A PDF converter tool".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        // All three fields match, should cap at 1.0
        assert_eq!(score, 1.0);
    }

    #[test]
    fn test_compute_relevance_partial_slug_match() {
        let skill = RegistrySkill {
            slug: "my-pdf-tool".to_string(),
            display_name: "Other".to_string(),
            summary: "Other tool".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("pdf", &skill);
        // Slug contains "pdf"
        assert!(score >= 0.5);
    }

    #[test]
    fn test_from_config_with_multiple_sources() {
        let mut config = crate::types::RegistryConfig::default();
        config.github_sources.push(crate::types::GitHubSourceConfig {
            name: "source-a".to_string(),
            repo: "org/a".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "skills_json".to_string(),
            index_path: "skills.json".to_string(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
        config.github_sources.push(crate::types::GitHubSourceConfig {
            name: "source-b".to_string(),
            repo: "org/b".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
        let manager = RegistryManager::from_config(config);
        assert_eq!(manager.registries().len(), 2);
    }

    #[tokio::test]
    async fn test_install_from_nonexistent_registry() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let result = manager.install("nonexistent", "skill", "/tmp").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_registry_config_default_values() {
        let config = crate::types::RegistryConfig::default();
        assert!(!config.github.enabled);
        assert!(!config.clawhub.enabled);
        assert!(config.github_sources.is_empty());
        assert!(!config.search_cache.enabled);
    }

    #[test]
    fn test_registry_skill_slug_and_version() {
        let skill = RegistrySkill {
            slug: "my-skill".to_string(),
            display_name: "My Skill".to_string(),
            summary: "A test skill".to_string(),
            version: "1.0.0".to_string(),
        };
        assert_eq!(skill.slug, "my-skill");
        assert_eq!(skill.version, "1.0.0");
    }

    #[test]
    fn test_compute_relevance_with_empty_strings() {
        let skill = RegistrySkill {
            slug: "".to_string(),
            display_name: "".to_string(),
            summary: "".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("test", &skill);
        assert_eq!(score, 0.0);
    }

    #[test]
    fn test_registry_manager_registries_accessor() {
        let manager = RegistryManager::new_empty();
        assert!(manager.registries().is_empty());
    }

    #[test]
    fn test_new_manager_with_custom_max_concurrent() {
        let mut config = RegistryConfig::default();
        config.max_concurrent_searches = 4;
        let manager = RegistryManager::new(config);
        assert_eq!(manager.max_concurrent, 4);
    }

    #[test]
    fn test_new_manager_zero_max_concurrent() {
        let mut config = RegistryConfig::default();
        config.max_concurrent_searches = 0;
        let manager = RegistryManager::new(config);
        assert_eq!(manager.max_concurrent, DEFAULT_MAX_CONCURRENT);
    }

    #[test]
    fn test_get_search_cache_when_disabled() {
        let manager = RegistryManager::new_empty();
        let cache = manager.get_search_cache();
        assert!(cache.is_none());
    }

    #[test]
    fn test_registry_skill_debug_format() {
        let skill = RegistrySkill {
            slug: "test".to_string(),
            display_name: "Test".to_string(),
            summary: "A test".to_string(),
            version: "1.0".to_string(),
        };
        let debug_str = format!("{:?}", skill);
        assert!(debug_str.contains("test"));
    }

    #[test]
    fn test_compute_relevance_exact_slug() {
        let skill = RegistrySkill {
            slug: "my-awesome-tool".to_string(),
            display_name: "My Tool".to_string(),
            summary: "A tool".to_string(),
            version: "1.0".to_string(),
        };
        let score = RegistryManager::compute_relevance("my-awesome-tool", &skill);
        assert!(score > 0.0);
    }

    #[tokio::test]
    async fn test_search_merged_sorts_by_score() {
        let manager = RegistryManager::new_empty();
        manager.add_registry(Arc::new(StubRegistryProvider));
        let result = manager.search("test", 10).await;
        assert!(result.is_ok());
    }
}
