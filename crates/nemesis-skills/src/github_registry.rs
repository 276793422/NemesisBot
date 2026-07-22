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
            base_url.to_string()
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
            let url = format!(
                "{}/{}/{}/{}",
                self.base_url, self.repo, self.branch, self.index_path
            );

            let body = self.do_get(&url).await?;
            let skills: Vec<GithubSkill> =
                serde_json::from_slice(&body).map_err(|e| NemesisError::Serialization(e))?;

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
        if !self.repo.is_empty() && !self.skill_path_pattern.is_empty() {
            if let Some(dir_prefix) = self.skill_dir_prefix(slug) {
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
        }

        // Legacy fallback: download only SKILL.md.
        let url = self.build_skill_url(slug);
        let body = self.do_get(&url).await?;

        std::fs::create_dir_all(target_dir).map_err(|e| NemesisError::Io(e))?;
        let skill_path = std::path::Path::new(target_dir).join("SKILL.md");
        std::fs::write(&skill_path, &body).map_err(|e| NemesisError::Io(e))?;

        Ok(InstallResult {
            version: install_version,
            is_malware_blocked: false,
            is_suspicious: false,
            summary: meta.summary,
        })
    }

    /// Fetch the SKILL.md content for a skill without installing it.
    pub async fn get_skill_content(&self, slug: &str) -> Result<crate::types::SkillContent> {
        let url = self.build_skill_url(slug);
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
        let url = format!(
            "{}/{}/{}/{}",
            self.base_url, self.repo, self.branch, self.index_path
        );

        let body = self.do_get(&url).await?;
        let skills: Vec<GithubSkill> =
            serde_json::from_slice(&body).map_err(|e| NemesisError::Serialization(e))?;

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
        let api_url = format!(
            "{}/repos/{}/contents/skills",
            self.api_base_url(),
            self.repo
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
            let body = response.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "GitHub API HTTP {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        let entries: Vec<GitHubContentEntry> = response.json().await.map_err(|e| {
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

    /// Search using GitHub Trees API (three-layer: skills/{author}/{slug}/SKILL.md).
    async fn search_three_layer(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SkillSearchResult>> {
        let api_url = format!(
            "{}/repos/{}/git/trees/{}?recursive=1",
            self.api_base_url(),
            self.repo,
            self.branch
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
            let body = response.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "GitHub API HTTP {}: {}",
                status,
                body.chars().take(4096).collect::<String>()
            )));
        }

        let tree_response: GithubTreeResponse = response
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to parse tree: {}", e)))?;

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
        if let Some(last_slash) = path.rfind('/') {
            Some(path[..last_slash].to_string())
        } else {
            None
        }
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

        format!("{}/{}/{}/{}", self.base_url, self.repo, self.branch, path)
    }

    /// Perform an HTTP GET request with error handling.
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
            let body = response.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "HTTP {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        let body = response
            .bytes()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to read response: {}", e)))?;

        if body.len() as u64 > self.max_size {
            return Err(NemesisError::Other(format!(
                "response too large: {} bytes (max {})",
                body.len(),
                self.max_size
            )));
        }

        Ok(body.to_vec())
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
            config.base_url.clone()
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
