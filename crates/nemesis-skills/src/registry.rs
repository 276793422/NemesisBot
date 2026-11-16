//! Registry manager - coordinates multiple skill registries.
//!
//! Manages GitHub-based and ClawHub-based skill registries, fans out search
//! requests concurrently, and routes installs to the correct registry.

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use parking_lot::RwLock;
use tokio::sync::Semaphore;
use tracing::{debug, info, warn};

use nemesis_types::error::{NemesisError, Result};

use crate::builtin_registry::BuiltinSkillsRegistry;
use crate::clawhub_registry::ClawHubRegistry;
use crate::github_registry::{GitHubRegistry, GitHubSourceConfig};
use crate::modelscope_registry::ModelScopeRegistry;
use crate::search_cache::SearchCache;
use crate::types::{
    BrowseResult, BrowseSort, RegistryConfig, RegistrySearchResult, SkillMeta, SkillSearchResult,
};

/// Default maximum concurrent registry searches.
const DEFAULT_MAX_CONCURRENT: usize = 2;

/// 出厂第三源描述（值单一真相源，2026-11-16 拍板）：registry 面
/// （[`RegistryManager::from_config`]）与 nemesis-web SkillsFullConfig
/// 显示/RMW 面共用。repo 与 nemesis-web persona `BUILTIN_REPO` 同仓库；
/// `Skills/` 在仓库根，live 臂加技能 = 往仓库 `Skills/` 提交一个技能目录
/// （部署实例不重编可见可装）。
pub struct FactorySourceDesc {
    /// 源名（marketplace 展示与安装路由键）。
    pub name: &'static str,
    /// GitHub `owner/repo`。
    pub repo: &'static str,
    pub branch: &'static str,
    pub index_type: &'static str,
    /// 仓库根形态 pattern——listing/下载两侧均由它推导（github_registry
    /// `listing_dir`）。
    pub pattern: &'static str,
}

/// 出厂 GitHub 源值（单一真相源）。
pub const FACTORY_SOURCE: FactorySourceDesc = FactorySourceDesc {
    name: "nemesisbot",
    repo: "276793422/NemesisBot",
    branch: "main",
    index_type: "github_api",
    pattern: "Skills/{slug}/SKILL.md",
};

/// 出厂源合流（append-if-absent，语义单一真相源）：名字已存在则跳过
/// （含用户显式停用的条目——用户可关不可丢），否则追加 enabled 条目。
/// 老工作区 config 无此源也能零操作拿到（persona 出厂源同款收敛语义）。
pub fn apply_factory_sources(config: &mut RegistryConfig) {
    if config
        .github_sources
        .iter()
        .any(|s| s.name == FACTORY_SOURCE.name)
    {
        return;
    }
    config
        .github_sources
        .push(crate::types::GitHubSourceConfig {
            name: FACTORY_SOURCE.name.to_string(),
            repo: FACTORY_SOURCE.repo.to_string(),
            enabled: true,
            branch: FACTORY_SOURCE.branch.to_string(),
            index_type: FACTORY_SOURCE.index_type.to_string(),
            index_path: String::new(),
            skill_path_pattern: FACTORY_SOURCE.pattern.to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
}

/// API 元数据 JSON 成功响应体帽（BUG-20261114-046 横向收口）。skill
/// registry 各 HTTP 端点的成功响应此前无字节上限——reqwest 无默认体上限，
/// 请求超时只约束时间不约束体积（快管道超时窗口内可灌 GB 级）；错误臂
/// 早已走 `read_error_body` 有帽，成功臂是同款不对称面。用户可配第三方
/// 源与 nemesis-mcp 服务器同信任层，按同款口径收口：搜索/树/列表/commit
/// 等元数据 JSON 16MB（与 nemesis-mcp `MAX_RESPONSE_BODY_BYTES` 对齐，
/// 正常载荷 KB~MB 量级，16MB 为数十倍裕量）。
/// 2026-11-15 升 pub：nemesis-web persona shop 抓取面（handlers/persona.rs
/// tree/contents JSON）与 skills 源结构探测（handlers/skills.rs）横向接入
/// ——同一信任域同一帽值，跨 crate 单一真相源。
pub const MAX_API_JSON_BODY_BYTES: u64 = 16 * 1024 * 1024;

/// SKILL.md 文本成功响应体帽（BUG-20261114-046 横向收口）。markdown
/// 内容 KB 量级，1MB 与 github registry `DEFAULT_MAX_SIZE` 对齐。
/// 2026-11-15 升 pub：nemesis-web persona shop 原文抓取（handlers/persona.rs
/// raw 内容）横向接入同一帽值——markdown 内容面单一真相源。
pub const MAX_SKILL_CONTENT_BODY_BYTES: u64 = 1024 * 1024;

/// 技能包**聚合落盘体量**上限（单次安装实际解压/下载字节，BUG-20261114-059
/// 收编为单一真相源）。clawhub zip 解压（原私有
/// `MAX_TOTAL_UNCOMPRESSED_BYTES` 同值）、github_tree 树下载、modelscope
/// 镜像拉取三面共用：三方注册表内容是同一信任域（发布者可控 → staging），
/// 聚合上限必须同值同源，防「一个面收口、另一个面漏网」的横向分叉。
pub(crate) const MAX_SKILL_TOTAL_BYTES: u64 = 200 * 1024 * 1024;

/// 技能包**聚合文件数**上限（单次安装，BUG-20261114-059）。正常技能
/// <100 文件，2000 = 20 倍裕量。帽住小文件海啸（树 JSON 16MB 可编码
/// ~160K 条目，每条目几个字节即可耗尽 inode/目录项）——体量闸管不住
/// 条目数，两闸互补。
pub(crate) const MAX_SKILL_FILE_COUNT: usize = 2000;

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
        Err(NemesisError::Other(
            "get_skill_content not implemented".to_string(),
        ))
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
    ///
    /// `config` 按值进来后先做出厂源合流（append-if-absent，见下方注释），
    /// 再由合流后的副本构造 registries 并存回 [`Self::config`]——消费方
    /// 拿到的 config 面与 registry 面同源。
    pub fn from_config(mut config: RegistryConfig) -> Self {
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

        // Legacy 单源形态资格**先于**出厂源合流捕获：合流后 github_sources
        // 恒非空（出厂源 append-if-absent），若让 legacy 分支事后判断会永久
        // 假死——老工作区 `github.enabled` 单源形态（无 sources 数组）必须
        // 保持可注册，与出厂源叠加并存。
        let legacy_github_eligible = config.github_sources.is_empty() && config.github.enabled;

        // 出厂第三源 append-if-absent（2026-11-16 拍板；persona 先例同款）：
        // 在 github_sources 循环**之前**合流，出厂源的 GitHub registry 走
        // 既有循环构造，config 面读取方（源列表/面板/落盘）看到合流后的
        // 真相。老工作区 config 无此源也能零操作拿到。值与语义单一真相源
        // = [`FACTORY_SOURCE`] / [`apply_factory_sources`]（nemesis-web
        // SkillsFullConfig 显示/RMW 面同源消费）。
        apply_factory_sources(&mut config);

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

        // 内嵌臂恒注册（2026-11-16 拍板②同款双臂形态：live GitHub 臂在上，
        // 本臂断网/纯内网兜底；无 config 开关——恒在，列表全量上架 13 个
        // 技能，安装时统一内容安全关照常裁决，运维技能装时诚实报拦截理由）。
        registries.push(Arc::new(BuiltinSkillsRegistry::new()));

        // Legacy: if no multi-source GitHub registries, use single-source.
        //（资格在出厂源合流前捕获，见上方注释）
        if legacy_github_eligible {
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

        // ModelScope support.
        if config.modelscope.enabled {
            let ms = ModelScopeRegistry::new();
            registries.push(Arc::new(ms));
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
    pub async fn search_all(&self, query: &str, limit: usize) -> Result<Vec<RegistrySearchResult>> {
        // 1. Check cache first if enabled.
        {
            let cache = self.search_cache.read();
            if let Some(ref cache) = *cache
                && let Some(results) = cache.get(query, limit)
            {
                debug!(
                    "Search cache hit for '{}' ({} registries)",
                    query,
                    results.len()
                );
                return Ok(results);
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
                    let truncated = results.last().map(|r| r.truncated).unwrap_or(false);
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
            if let Some(ref cache) = *cache
                && !grouped.is_empty()
            {
                cache.put(query, grouped.clone());
                debug!(
                    "Search cache stored for '{}' ({} registries)",
                    query,
                    grouped.len()
                );
            }
        }

        Ok(grouped)
    }

    /// Search for skills across all registries and merge results.
    ///
    /// Results are merged and sorted by score (descending), truncated to `limit`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let grouped = self.search_all(query, limit).await?;

        let mut all_results: Vec<SkillSearchResult> =
            grouped.into_iter().flat_map(|g| g.results).collect();

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

        let result = registry
            .download_and_install(slug, "latest", target_dir)
            .await?;

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
            published_at: None,
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

    async fn browse(&self, sort: &BrowseSort, limit: usize, cursor: &str) -> Result<BrowseResult> {
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

    async fn browse(&self, sort: &BrowseSort, limit: usize, cursor: &str) -> Result<BrowseResult> {
        self.browse(sort, limit, cursor).await
    }
}

/// Implement SkillRegistry for ModelScopeRegistry.
#[async_trait]
impl SkillRegistry for ModelScopeRegistry {
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

    async fn browse(&self, sort: &BrowseSort, limit: usize, cursor: &str) -> Result<BrowseResult> {
        self.browse(sort, limit, cursor).await
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod registry_extra_tests;
