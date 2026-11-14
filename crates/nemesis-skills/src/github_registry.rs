//! GitHub registry client - searches and installs skills from GitHub repositories.
//!
//! Supports two index types:
//! - `skills_json`: Fetches a `skills.json` index file and searches within it.
//! - `github_api`: Uses the GitHub Contents/Trees API for directory-based discovery.
//!
//! Supports three directory structure patterns:
//! - Two-layer: `skills/{slug}/SKILL.md`
//! - Three-layer: `skills/{author}/{slug}/SKILL.md`
//! - Root: `{slug}/SKILL.md`

use std::time::Duration;

use reqwest::Client;
use serde::Deserialize;
use tracing::{debug, warn};

use nemesis_types::error::{NemesisError, Result};

use crate::github_tree::download_skill_tree_from_github;
use crate::types::{
    BrowseResult, BrowseSort, InstallResult, SkillMeta, SkillSearchResult, contains_ci,
    validate_skill_identifier,
};

/// Default GitHub API timeout.
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(30);
/// Default max response size (1 MB).
const DEFAULT_MAX_SIZE: u64 = 1024 * 1024;
/// Browser-like User-Agent for GitHub API requests.
const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// GitHub registry client.
pub struct GitHubRegistry {
    base_url: String,
    #[allow(dead_code)]
    timeout: Duration,
    max_size: u64,
    client: Client,
    repo: String,
    branch: String,
    index_type: String,
    index_path: String,
    skill_path_pattern: String,
    registry_name: String,
    github_api_url: String,
}

impl GitHubRegistry {
    /// Create a new GitHub registry from a legacy single-source config.
    pub fn new(base_url: &str, timeout_secs: u64, max_size: u64) -> Self {
        let base_url = if base_url.is_empty() {
            "https://raw.githubusercontent.com".to_string()
        } else {
            // BUG-20261113-162：尾斜杠归一——用户从文档复制
            // `https://raw.githubusercontent.com/` 会拼出 `//{repo}` 双斜杠
            // 路径（raw 端点 404）。同族先例 = BUG-20261109-012
            // （telegram/bluesky/mastodon）+ modelscope 构造期归一。
            base_url.trim_end_matches('/').to_string()
        };

        let timeout = if timeout_secs > 0 {
            Duration::from_secs(timeout_secs)
        } else {
            DEFAULT_TIMEOUT
        };

        let max_size = if max_size > 0 {
            max_size
        } else {
            DEFAULT_MAX_SIZE
        };

        Self {
            base_url,
            timeout,
            max_size,
            client: Client::builder()
                .timeout(timeout)
                .user_agent(USER_AGENT)
                .build()
                .expect("failed to build HTTP client"),
            repo: "276793422/nemesisbot-skills".to_string(),
            branch: "main".to_string(),
            index_type: "skills_json".to_string(),
            index_path: "skills.json".to_string(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            registry_name: String::new(),
            github_api_url: "https://api.github.com".to_string(),
        }
    }

    /// Create a new GitHub registry from a per-source config.
    pub fn from_source(source: &GitHubSourceConfig) -> Self {
        let timeout = if source.timeout_secs > 0 {
            Duration::from_secs(source.timeout_secs)
        } else {
            DEFAULT_TIMEOUT
        };

        let max_size = if source.max_size > 0 {
            source.max_size
        } else {
            DEFAULT_MAX_SIZE
        };

        let branch = if source.branch.is_empty() {
            "main".to_string()
        } else {
            source.branch.clone()
        };

        Self {
            base_url: "https://raw.githubusercontent.com".to_string(),
            timeout,
            max_size,
            client: Client::builder()
                .timeout(timeout)
                .user_agent(USER_AGENT)
                .build()
                .expect("failed to build HTTP client"),
            repo: source.repo.clone(),
            branch,
            index_type: source.index_type.clone(),
            index_path: source.index_path.clone(),
            skill_path_pattern: source.skill_path_pattern.clone(),
            registry_name: source.name.clone(),
            github_api_url: "https://api.github.com".to_string(),
        }
    }

    /// Get the registry name.
    pub fn name(&self) -> &str {
        if self.registry_name.is_empty() {
            "github"
        } else {
            &self.registry_name
        }
    }

    /// Get the GitHub API base URL, defaulting to https://api.github.com.
    ///
    /// Mirrors Go's `apiBaseURL()` method.
    pub fn api_base_url(&self) -> &str {
        if self.github_api_url.is_empty() {
            "https://api.github.com"
        } else {
            &self.github_api_url
        }
    }

    /// Search for skills in the registry.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        match self.index_type.as_str() {
            "github_api" => self.search_github_api(query, limit).await,
            _ => self.search_skills_json(query, limit).await,
        }
    }

    /// Get metadata for a specific skill by slug.
    pub async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        validate_skill_identifier(slug)
            .map_err(|e| NemesisError::Validation(format!("invalid slug '{}': {}", slug, e)))?;

        if self.index_type == "skills_json" {
            // BUG-20261113-162：raw URL 构造单点（尾斜杠归一 + 路径段
            // percent-encoding，`#`/`?` 不再沦为 fragment/query 分隔符）。
            let url = crate::url_enc::raw_content_url(
                &self.base_url,
                &self.repo,
                &self.branch,
                &self.index_path,
            );

            let body = self.do_get(&url).await?;
            let skills: Vec<GithubSkill> =
                serde_json::from_slice(&body).map_err(NemesisError::Serialization)?;

            for skill in &skills {
                if skill.name == slug {
                    return Ok(SkillMeta {
                        slug: skill.name.clone(),
                        display_name: skill.name.clone(),
                        summary: skill.description.clone(),
                        latest_version: "latest".to_string(),
                        is_malware_blocked: false,
                        is_suspicious: false,
                        registry_name: self.name().to_string(),
                        author: skill.author.clone().unwrap_or_default(),
                        downloads: 0,
                        published_at: None,
                    });
                }
            }

            return Err(NemesisError::NotFound(format!("skill not found: {}", slug)));
        }

        // For github_api, return basic metadata.
        Ok(SkillMeta {
            slug: slug.to_string(),
            display_name: slug.to_string(),
            summary: format!("Skill from {}", self.repo),
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: self.name().to_string(),
            author: String::new(),
            downloads: 0,
            published_at: None,
        })
    }

    /// Download and install a skill.
    pub async fn download_and_install(
        &self,
        slug: &str,
        version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        validate_skill_identifier(slug)
            .map_err(|e| NemesisError::Validation(format!("invalid slug '{}': {}", slug, e)))?;

        // Fetch metadata.
        let meta = self
            .get_skill_meta(slug)
            .await
            .unwrap_or_else(|_| SkillMeta {
                slug: slug.to_string(),
                display_name: slug.to_string(),
                summary: String::new(),
                latest_version: version.to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                registry_name: self.name().to_string(),
                author: String::new(),
                downloads: 0,
                published_at: None,
            });

        let install_version = if version.is_empty() {
            if meta.latest_version.is_empty() {
                "main".to_string()
            } else {
                meta.latest_version.clone()
            }
        } else {
            version.to_string()
        };

        // Use Trees API to download the full skill directory.
        // BUG-20261111-066：{author} 模式 + bare slug 先反查 author 拼复合键
        // （resolve_install_slug）——否则 skill_dir_prefix 恒 None 落 legacy
        // fallback，字面 {author} 留 URL 必 404。
        let install_slug = self.resolve_install_slug(slug).await;
        if !self.repo.is_empty()
            && !self.skill_path_pattern.is_empty()
            && let Some(dir_prefix) = self.skill_dir_prefix(&install_slug)
        {
            download_skill_tree_from_github(
                &self.client,
                self.api_base_url(),
                &self.base_url,
                &self.repo,
                &self.branch,
                &dir_prefix,
                target_dir,
                0,
            )
            .await?;

            return Ok(InstallResult {
                version: install_version,
                is_malware_blocked: false,
                is_suspicious: false,
                summary: meta.summary,
            });
        }

        // Legacy fallback: download only SKILL.md.
        let url = self.build_skill_url(&install_slug);
        let body = self.do_get(&url).await?;

        std::fs::create_dir_all(target_dir).map_err(NemesisError::Io)?;
        let skill_path = std::path::Path::new(target_dir).join("SKILL.md");
        std::fs::write(&skill_path, &body).map_err(NemesisError::Io)?;

        Ok(InstallResult {
            version: install_version,
            is_malware_blocked: false,
            is_suspicious: false,
            summary: meta.summary,
        })
    }

    /// Fetch the SKILL.md content for a skill without installing it.
    pub async fn get_skill_content(&self, slug: &str) -> Result<crate::types::SkillContent> {
        // BUG-20261111-067：预览面此前裸插 build_skill_url 无任何校验——
        // `..`/多段路径可探配置仓库内任意路径。分段校验（本面只拼 URL 不落
        // 盘，逐段 validator）：裸 slug（search 产出形态）走三段式反查
        // （BUG-20261111-066，否则字面 {author} 进 URL 必 404）；
        // author/slug 复合形态两段各过 validator 后直用（预览调用方的
        // 显式 author 形态，免一次树反查）；`..`/空段/三段以上拒绝。
        let install_slug = match slug.split_once('/') {
            None => {
                crate::types::validate_skill_identifier(slug).map_err(|e| {
                    NemesisError::Validation(format!("invalid slug '{}': {}", slug, e))
                })?;
                self.resolve_install_slug(slug).await
            }
            Some((author, base)) => {
                for seg in [author, base] {
                    crate::types::validate_skill_identifier(seg).map_err(|e| {
                        NemesisError::Validation(format!("invalid slug '{}': {}", slug, e))
                    })?;
                }
                slug.to_string()
            }
        };
        let url = self.build_skill_url(&install_slug);
        let bytes = self.do_get(&url).await?;
        let content = String::from_utf8(bytes)
            .map_err(|e| NemesisError::Other(format!("invalid utf8: {}", e)))?;
        Ok(crate::types::SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content,
        })
    }

    /// Browse all skills with client-side pagination.
    ///
    /// GitHub has no server-side pagination for skill listing. We fetch all
    /// skills and paginate on the client side using an offset-based cursor.
    pub async fn browse(
        &self,
        _sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        let limit = if limit == 0 { 20 } else { limit };

        // Fetch all skills (empty query = list all).
        let all = self.search("", 1000).await?;

        // Parse cursor as offset.
        let offset = if let Some(rest) = cursor.strip_prefix("offset:") {
            rest.parse::<usize>().unwrap_or(0)
        } else {
            0
        };

        let items: Vec<SkillSearchResult> = all.into_iter().skip(offset).take(limit).collect();

        let next_offset = offset + items.len();
        let next_cursor = if items.len() == limit {
            Some(format!("offset:{}", next_offset))
        } else {
            None
        };

        Ok(BrowseResult { items, next_cursor })
    }

    // --- Internal methods ---

    /// Search using skills.json index file.
    async fn search_skills_json(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SkillSearchResult>> {
        // BUG-20261113-162：raw URL 构造单点（同 get_skill_meta）。
        let url = crate::url_enc::raw_content_url(
            &self.base_url,
            &self.repo,
            &self.branch,
            &self.index_path,
        );

        let body = self.do_get(&url).await?;
        let skills: Vec<GithubSkill> =
            serde_json::from_slice(&body).map_err(NemesisError::Serialization)?;

        let mut results = Vec::new();
        for skill in &skills {
            if results.len() >= limit {
                break;
            }

            if contains_ci(&skill.name, query) || contains_ci(&skill.description, query) {
                results.push(SkillSearchResult {
                    score: 1.0,
                    slug: skill.name.clone(),
                    display_name: skill.name.clone(),
                    summary: skill.description.clone(),
                    version: "latest".to_string(),
                    registry_name: self.name().to_string(),
                    source_repo: self.repo.clone(),
                    download_path: String::new(),
                    downloads: 0,
                    truncated: false,
                });
            }
        }

        Ok(results)
    }

    /// Search using GitHub Contents API (two-layer).
    async fn search_two_layer(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        // BUG-20261114-043：repo 段 percent-encoding + api_base 尾斜杠归一
        // （BUG-20261113-162 横向漏点——同文件 raw URL 三处已修，API URL
        // 两处漏网；`#`/`?` 沦为 fragment/query 分隔符指向错误资源）。
        let api_url = format!(
            "{}/repos/{}/contents/skills",
            self.api_base_url().trim_end_matches('/'),
            crate::url_enc::percent_encode_path(&self.repo)
        );

        let response = self
            .client
            .get(&api_url)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to list skills: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = nemesis_utils::http_cap::read_error_body(response).await;
            return Err(NemesisError::Other(format!(
                "GitHub API HTTP {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        // BUG-20261114-046：成功响应体 16MB 帽（错误臂已帽的不对称面收口）。
        let body_bytes = nemesis_utils::http_cap::read_body_capped(
            response,
            crate::registry::MAX_API_JSON_BODY_BYTES,
        )
        .await
        .map_err(NemesisError::Other)?;
        let entries: Vec<GitHubContentEntry> =
            serde_json::from_slice(&body_bytes).map_err(|e| {
                NemesisError::Other(format!("failed to parse directory listing: {}", e))
            })?;

        let mut results = Vec::new();
        for entry in &entries {
            if results.len() >= limit {
                break;
            }
            if entry.entry_type != "dir" {
                continue;
            }

            let slug = &entry.name;
            if contains_ci(slug, query) {
                let download_path = self.skill_path_pattern.replace("{slug}", slug);
                results.push(SkillSearchResult {
                    score: 1.0,
                    slug: slug.clone(),
                    display_name: slug.clone(),
                    summary: format!("Skill from {}", self.repo),
                    version: "latest".to_string(),
                    registry_name: self.name().to_string(),
                    source_repo: self.repo.clone(),
                    download_path,
                    downloads: 0,
                    truncated: false,
                });
            }
        }

        Ok(results)
    }

    /// 拉取仓库全树（Trees API recursive）——search_three_layer 与安装期
    /// slug→author 反查（[`Self::resolve_author`]）共用。
    async fn fetch_repo_tree(&self) -> Result<GithubTreeResponse> {
        // BUG-20261114-043：repo/branch 段 percent-encoding + api_base 尾斜杠
        // 归一（BUG-20261113-162 横向漏点；分支名可合法含 `#`，裸插值在
        // `?recursive=1` 前被 fragment 切断。github_tree.rs 同款 URL 形态
        // 162 轮已修，本处漏网）。与 github_tree.rs 保持同一构造形态。
        let api_url = format!(
            "{}/repos/{}/git/trees/{}?recursive=1",
            self.api_base_url().trim_end_matches('/'),
            crate::url_enc::percent_encode_path(&self.repo),
            crate::url_enc::percent_encode_component(&self.branch)
        );

        let response = self
            .client
            .get(&api_url)
            .header("Accept", "application/vnd.github.v3+json")
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to fetch tree: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = nemesis_utils::http_cap::read_error_body(response).await;
            return Err(NemesisError::Other(format!(
                "GitHub API HTTP {}: {}",
                status,
                body.chars().take(4096).collect::<String>()
            )));
        }

        // BUG-20261114-046：成功响应体 16MB 帽（大仓库 Trees API 载荷
        // MB 量级，16MB 数十倍裕量）。
        let body_bytes = nemesis_utils::http_cap::read_body_capped(
            response,
            crate::registry::MAX_API_JSON_BODY_BYTES,
        )
        .await
        .map_err(NemesisError::Other)?;
        serde_json::from_slice(&body_bytes)
            .map_err(|e| NemesisError::Other(format!("failed to parse tree: {}", e)))
    }

    /// Search using GitHub Trees API (three-layer: skills/{author}/{slug}/SKILL.md).
    async fn search_three_layer(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SkillSearchResult>> {
        let tree_response = self.fetch_repo_tree().await?;

        let mut results = Vec::new();
        let mut seen_slugs = std::collections::HashSet::new();

        for entry in &tree_response.tree {
            if results.len() >= limit {
                break;
            }

            if entry.entry_type != "blob" {
                continue;
            }

            let path = &entry.path;
            if !path.starts_with("skills/") || !path.ends_with("/SKILL.md") {
                continue;
            }

            // Extract author/slug from "skills/{author}/{slug}/SKILL.md".
            let inner = path
                .strip_prefix("skills/")
                .unwrap()
                .strip_suffix("/SKILL.md")
                .unwrap();
            let parts: Vec<&str> = inner.splitn(2, '/').collect();
            if parts.len() != 2 {
                continue;
            }
            let (author, slug) = (parts[0], parts[1]);
            if author.is_empty() || slug.is_empty() {
                continue;
            }
            if seen_slugs.contains(slug) {
                continue;
            }

            if !query.is_empty() && !contains_ci(slug, query) {
                continue;
            }

            seen_slugs.insert(slug.to_string());
            let download_path = self
                .skill_path_pattern
                .replace("{author}", author)
                .replace("{slug}", slug);

            results.push(SkillSearchResult {
                score: 1.0,
                slug: slug.to_string(),
                display_name: slug.to_string(),
                summary: format!("Skill from {}", self.repo),
                version: "latest".to_string(),
                registry_name: self.name().to_string(),
                source_repo: self.repo.clone(),
                download_path,
                downloads: 0,
                truncated: false,
            });
        }

        if tree_response.truncated == Some(true) {
            warn!(
                "GitHub tree truncated, results may be incomplete for {}",
                self.repo
            );
        }

        Ok(results)
    }

    /// Search using GitHub API (dispatches to two-layer or three-layer).
    async fn search_github_api(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        if self.is_three_layer_pattern() {
            self.search_three_layer(query, limit).await
        } else {
            self.search_two_layer(query, limit).await
        }
    }

    /// Check if the skill path pattern uses three layers (contains {author}).
    fn is_three_layer_pattern(&self) -> bool {
        self.skill_path_pattern.contains("{author}")
    }

    /// 三段式源安装期 slug→author 反查（BUG-20261111-066）。
    ///
    /// search 产出的 slug 按安装目录名纪律不带 author（`/` 被
    /// `validate_skill_identifier` 拒绝），author 只活在
    /// `SkillSearchResult.download_path` 里且从未被安装路径消费——
    /// `{author}` 模式源（默认配置的 openclaw）在安装路径
    /// `skill_dir_prefix` 恒 None、legacy fallback 把字面 `{author}` 留在
    /// URL → GitHub raw 404，整源搜索可见但不可装。修：安装期拉同一棵树，
    /// 按与 search 相同的遍历序取首个 `skills/{author}/{slug}/SKILL.md`
    /// 精确命中的 author（确定性一致：同一响应、同一顺序，search 去重保留
    /// 的也是首个）。拉树失败/无命中 = None（回落 bare slug 旧行为）。
    async fn resolve_author(&self, slug: &str) -> Option<String> {
        let tree = self.fetch_repo_tree().await.ok()?;
        let suffix = format!("/{slug}/SKILL.md");
        for entry in &tree.tree {
            if entry.entry_type != "blob" {
                continue;
            }
            let Some(inner) = entry.path.strip_prefix("skills/") else {
                continue;
            };
            let Some(author) = inner.strip_suffix(&suffix) else {
                continue;
            };
            if author.is_empty() || author.contains('/') {
                continue;
            }
            return Some(author.to_string());
        }
        None
    }

    /// 安装期 slug 解析：`{author}` 模式 + bare slug（search 产出形态）先
    /// 反查 author 拼成 `author/slug` 复合键——否则 `skill_dir_prefix` 恒
    /// None 落 legacy fallback，字面 `{author}` 留 URL 必 404
    /// （BUG-20261111-066）。两段式模式 / 已带 `/` 的复合键原样透传。
    async fn resolve_install_slug(&self, slug: &str) -> String {
        if self.is_three_layer_pattern() && !slug.contains('/') {
            match self.resolve_author(slug).await {
                Some(author) => format!("{author}/{slug}"),
                None => slug.to_string(),
            }
        } else {
            slug.to_string()
        }
    }

    /// Calculate the directory prefix for a skill from the path pattern.
    fn skill_dir_prefix(&self, slug: &str) -> Option<String> {
        let path = if self.skill_path_pattern.contains("{author}") {
            let parts: Vec<&str> = slug.splitn(2, '/').collect();
            if parts.len() == 2 {
                self.skill_path_pattern
                    .replace("{author}", parts[0])
                    .replace("{slug}", parts[1])
            } else {
                return None; // Can't determine author from slug alone.
            }
        } else {
            self.skill_path_pattern.replace("{slug}", slug)
        };

        // Remove trailing filename (e.g., "/SKILL.md").
        path.rfind('/')
            .map(|last_slash| path[..last_slash].to_string())
    }

    /// Build the download URL for a skill.
    fn build_skill_url(&self, slug: &str) -> String {
        let path = if self.skill_path_pattern.contains("{author}") {
            let parts: Vec<&str> = slug.splitn(2, '/').collect();
            if parts.len() == 2 {
                self.skill_path_pattern
                    .replace("{author}", parts[0])
                    .replace("{slug}", parts[1])
            } else {
                self.skill_path_pattern.replace("{slug}", slug)
            }
        } else {
            self.skill_path_pattern.replace("{slug}", slug)
        };

        // BUG-20261113-162：raw URL 构造单点（尾斜杠归一 + `#`/`?` 等分段
        // percent-encoding——分支名与文件名均可合法容纳这两个 URL 保留字）。
        crate::url_enc::raw_content_url(&self.base_url, &self.repo, &self.branch, &path)
    }

    /// Perform an HTTP GET request with error handling.
    ///
    /// BUG-20261107-005：与 [`crate::github_tree::download_file`] 同款旧
    /// 形态收口——`bytes()` 全量读入后才核对 `self.max_size`，换用共享
    /// 流式累计闸，超限在读入中途即断。
    async fn do_get(&self, url: &str) -> Result<Vec<u8>> {
        debug!("GitHub GET: {}", url);

        let response = self
            .client
            .get(url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = nemesis_utils::http_cap::read_error_body(response).await;
            return Err(NemesisError::Other(format!(
                "HTTP {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        nemesis_utils::http_cap::read_body_capped(response, self.max_size)
            .await
            .map_err(NemesisError::Other)
    }

    /// Download all files under the given directory prefix from GitHub
    /// using the shared Trees API download logic.
    ///
    /// Mirrors Go's `downloadSkillTree()` method that delegates to the shared
    /// `DownloadSkillTreeFromGitHub` function.
    pub async fn download_skill_tree(&self, dir_prefix: &str, target_dir: &str) -> Result<()> {
        download_skill_tree_from_github(
            &self.client,
            self.api_base_url(),
            &self.base_url,
            &self.repo,
            &self.branch,
            dir_prefix,
            target_dir,
            self.max_size,
        )
        .await
    }

    /// Create a new GitHub registry from a GitHubConfig (legacy single-source config).
    ///
    /// Mirrors Go's `NewGitHubRegistry(cfg)` constructor.
    pub fn new_from_config(config: &crate::types::GitHubConfig) -> Self {
        let base_url = if config.base_url.is_empty() {
            "https://raw.githubusercontent.com".to_string()
        } else {
            // BUG-20261114-043：与 [`Self::new`] 同源尾斜杠归一——162 修复轮
            // 只补了 new，本构造器漏网。当前 join 点均经 raw_content_url
            // （use-site trim）行为惰性，归一属构造器对称性防御。
            config.base_url.trim_end_matches('/').to_string()
        };

        let timeout = if config.timeout_secs > 0 {
            Duration::from_secs(config.timeout_secs)
        } else {
            DEFAULT_TIMEOUT
        };

        let max_size = if config.max_size > 0 {
            config.max_size
        } else {
            DEFAULT_MAX_SIZE
        };

        Self {
            base_url,
            timeout,
            max_size,
            client: Client::builder()
                .timeout(timeout)
                .user_agent(USER_AGENT)
                .build()
                .expect("failed to build HTTP client"),
            repo: "276793422/nemesisbot-skills".to_string(),
            branch: "main".to_string(),
            index_type: "skills_json".to_string(),
            index_path: "skills.json".to_string(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            registry_name: String::new(),
            github_api_url: "https://api.github.com".to_string(),
        }
    }

    /// Set the GitHub API URL.
    pub fn set_github_api_url(&mut self, url: &str) {
        self.github_api_url = url.to_string();
    }
}

/// Per-source GitHub configuration (used in from_source).
#[derive(Debug, Clone)]
pub struct GitHubSourceConfig {
    pub name: String,
    pub repo: String,
    pub enabled: bool,
    pub branch: String,
    pub index_type: String,
    pub index_path: String,
    pub skill_path_pattern: String,
    pub timeout_secs: u64,
    pub max_size: u64,
}

// --- JSON types ---

/// Skill entry from skills.json.
#[derive(Debug, Deserialize)]
struct GithubSkill {
    name: String,
    description: String,
    #[allow(dead_code)]
    repository: Option<String>,
    #[allow(dead_code)]
    author: Option<String>,
    #[allow(dead_code)]
    tags: Option<Vec<String>>,
}

/// Content entry from GitHub Contents API.
#[derive(Debug, Deserialize)]
struct GitHubContentEntry {
    name: String,
    #[serde(rename = "type")]
    entry_type: String,
    #[allow(dead_code)]
    path: String,
}

/// GitHub Trees API response.
#[derive(Debug, Deserialize)]
struct GithubTreeResponse {
    #[allow(dead_code)]
    sha: Option<String>,
    tree: Vec<GithubTreeEntry>,
    truncated: Option<bool>,
}

/// A single tree entry.
#[derive(Debug, Deserialize)]
struct GithubTreeEntry {
    path: String,
    #[serde(rename = "type")]
    entry_type: String,
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod github_extra_tests;

#[cfg(test)]
mod w4b_tests;
