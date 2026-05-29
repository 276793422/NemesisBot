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
    BrowseResult, BrowseSort, RegistryConfig, RegistrySearchResult, SkillMeta, SkillSearchResult,
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
    /// Fetch the SKILL.md content for a skill without installing it.
    async fn get_skill_content(&self, _slug: &str) -> Result<crate::types::SkillContent> {
        Err(NemesisError::Other("get_skill_content not implemented".to_string()))
    }
    /// Browse skills with sort and cursor-based pagination.
    async fn browse(
        &self,
        _sort: &BrowseSort,
        _limit: usize,
        _cursor: &str,
    ) -> Result<BrowseResult> {
        Err(NemesisError::Other("browse not implemented".to_string()))
    }
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

    /// Fetch SKILL.md content for a skill from a specific registry.
    pub async fn get_skill_content(
        &self,
        registry_name: &str,
        slug: &str,
    ) -> Result<crate::types::SkillContent> {
        let registry = self.get_registry(registry_name).ok_or_else(|| {
            NemesisError::NotFound(format!("registry '{}' not found", registry_name))
        })?;
        registry.get_skill_content(slug).await
    }

    /// Browse skills from a specific registry with sort and pagination.
    pub async fn browse(
        &self,
        registry_name: &str,
        sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        let registry = self.get_registry(registry_name).ok_or_else(|| {
            NemesisError::NotFound(format!("registry '{}' not found", registry_name))
        })?;
        registry.browse(sort, limit, cursor).await
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
            author: String::new(),
            downloads: 0,
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

    async fn browse(
        &self,
        _sort: &BrowseSort,
        _limit: usize,
        _cursor: &str,
    ) -> Result<BrowseResult> {
        Ok(BrowseResult {
            items: Vec::new(),
            next_cursor: None,
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

    async fn get_skill_content(&self, slug: &str) -> Result<crate::types::SkillContent> {
        self.get_skill_content(slug).await
    }

    async fn browse(
        &self,
        sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        self.browse(sort, limit, cursor).await
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

    async fn get_skill_content(&self, slug: &str) -> Result<crate::types::SkillContent> {
        self.get_skill_content(slug).await
    }

    async fn browse(
        &self,
        sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        self.browse(sort, limit, cursor).await
    }
}

#[cfg(test)]
mod tests;
