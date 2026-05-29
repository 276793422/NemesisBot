//! ClawHub registry client - Convex API client for the ClawHub skill registry.
//!
//! Supports:
//! - Search via ClawHub search API (vector search)
//! - List via Convex `skills:list` query
//! - Metadata via Convex `skills:getBySlug` query
//! - Download via ZIP from Convex site URL
//! - Fallback to GitHub Trees API for individual file downloads

use std::time::Duration;

use serde::Deserialize;
use tracing::debug;
use reqwest::Client;

use nemesis_types::error::{NemesisError, Result};

use crate::github_tree::download_skill_tree_from_github;
use crate::types::{validate_skill_identifier, BrowseResult, BrowseSort, InstallResult, SkillMeta, SkillSearchResult};

const DEFAULT_CLAWHUB_URL: &str = "https://clawhub.ai";
const DEFAULT_CONVEX_URL: &str = "https://wry-manatee-359.convex.cloud";

/// ClawHub registry client.
pub struct ClawHubRegistry {
    base_url: String,
    convex_url: String,
    convex_site_url: String,
    client: Client,
}

impl ClawHubRegistry {
    /// Create a new ClawHub registry with default URLs.
    pub fn new() -> Self {
        Self::with_urls(DEFAULT_CLAWHUB_URL, DEFAULT_CONVEX_URL, "")
    }

    /// Create a new ClawHub registry with custom URLs.
    pub fn with_urls(base_url: &str, convex_url: &str, convex_site_url: &str) -> Self {
        Self {
            base_url: base_url.to_string(),
            convex_url: convex_url.to_string(),
            convex_site_url: convex_site_url.to_string(),
            client: Client::builder()
                .timeout(Duration::from_secs(30))
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Create a new ClawHub registry from a ClawHubConfig.
    ///
    /// Mirrors Go's `NewClawHubRegistry(cfg)` constructor.
    pub fn new_from_config(config: &crate::types::ClawHubConfig) -> Self {
        let base_url = if config.base_url.is_empty() {
            DEFAULT_CLAWHUB_URL.to_string()
        } else {
            config.base_url.clone()
        };

        let convex_url = if config.convex_url.is_empty() {
            DEFAULT_CONVEX_URL.to_string()
        } else {
            config.convex_url.clone()
        };

        let timeout = if config.timeout_secs > 0 {
            Duration::from_secs(config.timeout_secs)
        } else {
            Duration::from_secs(30)
        };

        Self {
            base_url,
            convex_url,
            convex_site_url: config.convex_site_url.clone(),
            client: Client::builder()
                .timeout(timeout)
                .build()
                .expect("failed to build HTTP client"),
        }
    }

    /// Get the registry name.
    pub fn name(&self) -> &str {
        "clawhub"
    }

    /// Get the Convex site URL for ZIP downloads.
    fn site_url(&self) -> String {
        if !self.convex_site_url.is_empty() {
            return self.convex_site_url.clone();
        }
        self.convex_url
            .replace(".convex.cloud", ".convex.site")
    }

    /// Search for skills.
    ///
    /// Non-empty query uses the ClawHub search API (vector search).
    /// Empty query falls back to Convex `skills:list`.
    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        if query.is_empty() {
            self.search_list(limit).await
        } else {
            self.search_query(query, limit).await
        }
    }

    /// Get metadata for a specific skill by slug.
    pub async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        validate_skill_identifier(slug)
            .map_err(|e| NemesisError::Validation(format!("invalid skill slug: {}", e)))?;

        let value = self
            .call_convex("skills:getBySlug", &[("slug", slug as &str)])
            .await?;

        let detail: ConvexSkillDetail =
            serde_json::from_value(value).map_err(|e| NemesisError::Serialization(e))?;

        if detail.skill.slug.is_empty() && detail.resolved_slug.is_empty() {
            return Err(NemesisError::NotFound(format!("skill '{}' not found", slug)));
        }

        let meta_slug = if detail.skill.slug.is_empty() {
            detail.resolved_slug
        } else {
            detail.skill.slug
        };

        let version = if detail.latest_version.version.is_empty() {
            "latest".to_string()
        } else {
            detail.latest_version.version
        };

        Ok(SkillMeta {
            slug: meta_slug,
            display_name: detail.skill.display_name,
            summary: detail.skill.summary,
            latest_version: version,
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "clawhub".to_string(),
            author: detail.owner.handle,
            downloads: detail.skill.stats.downloads as i64,
        })
    }

    /// Download and install a skill.
    ///
    /// Strategy:
    /// 1. Try ZIP download from Convex site URL (primary).
    /// 2. Fallback to GitHub Trees API for individual file downloads.
    pub async fn download_and_install(
        &self,
        slug: &str,
        _version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        validate_skill_identifier(slug)
            .map_err(|e| NemesisError::Validation(format!("invalid skill slug: {}", e)))?;

        // Get full skill detail including owner handle.
        let value = self
            .call_convex("skills:getBySlug", &[("slug", slug)])
            .await?;

        let detail: ConvexSkillDetail =
            serde_json::from_value(value).map_err(|e| NemesisError::Serialization(e))?;

        if detail.owner.handle.is_empty() {
            return Err(NemesisError::NotFound(format!(
                "owner handle not found for skill '{}'",
                slug
            )));
        }

        let owner = detail.owner.handle;
        let install_version = if detail.latest_version.version.is_empty() {
            "latest".to_string()
        } else {
            detail.latest_version.version
        };

        // Strategy 1: Try ZIP download.
        if let Ok(()) = self
            .download_skill_zip(slug, target_dir)
            .await
        {
            debug!(
                "ClawHub skill installed via ZIP: slug={}, owner={}",
                slug, owner
            );
            return Ok(InstallResult {
                version: install_version,
                is_malware_blocked: false,
                is_suspicious: false,
                summary: detail.skill.summary,
            });
        }

        debug!("ZIP download failed, falling back to GitHub Trees API for {}", slug);

        // Strategy 2: Fallback to GitHub Trees API.
        let dir_prefix = format!("skills/{}/{}", owner, slug);
        download_skill_tree_from_github(
            &self.client,
            "https://api.github.com",
            "https://raw.githubusercontent.com",
            "openclaw/skills",
            "main",
            &dir_prefix,
            target_dir,
            0,
        )
        .await?;

        debug!(
            "ClawHub skill installed via GitHub Trees API: slug={}, owner={}",
            slug, owner
        );

        Ok(InstallResult {
            version: install_version,
            is_malware_blocked: false,
            is_suspicious: false,
            summary: detail.skill.summary,
        })
    }

    /// Fetch the SKILL.md content for a skill without installing it.
    ///
    /// Strategy:
    /// 1. Try ClawHub file API (primary).
    /// 2. Fallback to Convex + GitHub raw.
    pub async fn get_skill_content(&self, slug: &str) -> Result<crate::types::SkillContent> {
        validate_skill_identifier(slug)
            .map_err(|e| NemesisError::Validation(format!("invalid skill slug: {}", e)))?;

        // Strategy 1: ClawHub file API.
        let file_url = format!(
            "{}/api/v1/skills/{}/file?path=SKILL.md",
            self.base_url,
            urlencoding::encode(slug)
        );
        if let Ok(resp) = self.client.get(&file_url).send().await {
            if resp.status().is_success() {
                if let Ok(content) = resp.text().await {
                    return Ok(crate::types::SkillContent {
                        slug: slug.to_string(),
                        filename: "SKILL.md".to_string(),
                        content,
                    });
                }
            }
        }

        debug!("ClawHub file API failed, falling back to GitHub raw for {}", slug);

        // Strategy 2: Convex + GitHub raw fallback.
        let value = self.call_convex("skills:getBySlug", &[("slug", slug)]).await?;
        let detail: ConvexSkillDetail =
            serde_json::from_value(value).map_err(|e| NemesisError::Serialization(e))?;

        if detail.owner.handle.is_empty() {
            return Err(NemesisError::NotFound(format!(
                "owner handle not found for skill '{}'",
                slug
            )));
        }

        let url = format!(
            "https://raw.githubusercontent.com/openclaw/skills/main/skills/{}/SKILL.md",
            format!("{}/{}", detail.owner.handle, slug)
        );

        let resp = self.client.get(&url).send().await
            .map_err(|e| NemesisError::Other(format!("request failed: {}", e)))?;

        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!("HTTP {}", resp.status())));
        }

        let content = resp.text().await
            .map_err(|e| NemesisError::Other(format!("read failed: {}", e)))?;

        Ok(crate::types::SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content,
        })
    }

    /// Browse skills with sort and cursor-based pagination.
    ///
    /// Uses ClawHub REST API `/api/v1/skills` which supports sorting and cursors.
    pub async fn browse(
        &self,
        sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        let limit = if limit == 0 { 20 } else { limit.min(100) };
        let mut url = format!(
            "{}/api/v1/skills?sort={}&limit={}",
            self.base_url,
            sort.as_str(),
            limit,
        );
        if !cursor.is_empty() {
            url.push_str(&format!("&cursor={}", urlencoding::encode(cursor)));
        }

        let resp = self.client.get(&url).send().await
            .map_err(|e| NemesisError::Other(format!("browse request failed: {}", e)))?;

        if !resp.status().is_success() {
            let status = resp.status();
            let body = resp.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "browse failed with status {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        let browse_resp: ClawhubBrowseResponse = resp.json().await
            .map_err(|e| NemesisError::Other(format!("failed to parse browse response: {}", e)))?;

        let items: Vec<SkillSearchResult> = browse_resp.items.into_iter().map(|item| {
            SkillSearchResult {
                score: 1.0,
                slug: item.slug,
                display_name: item.display_name,
                summary: item.summary,
                version: "latest".to_string(),
                registry_name: "clawhub".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: item.stats.downloads as i64,
                truncated: false,
            }
        }).collect();

        Ok(BrowseResult {
            items,
            next_cursor: browse_resp.next_cursor,
        })
    }

    // --- Internal ---

    /// Call a Convex query function.
    async fn call_convex(
        &self,
        function_name: &str,
        args: &[(&str, &str)],
    ) -> Result<serde_json::Value> {
        let mut args_map = serde_json::Map::new();
        for (key, value) in args {
            args_map.insert(
                key.to_string(),
                serde_json::Value::String(value.to_string()),
            );
        }

        let req_body = serde_json::json!({
            "path": function_name,
            "args": serde_json::Value::Object(args_map),
            "format": "json"
        });

        let req_url = format!("{}/api/query", self.convex_url);
        let response = self
            .client
            .post(&req_url)
            .json(&req_body)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("convex request failed: {}", e)))?;

        let body: ConvexResponse = response
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to decode convex response: {}", e)))?;

        if body.status == "error" {
            return Err(NemesisError::Other(format!(
                "convex error: {}",
                body.error_message.unwrap_or_default()
            )));
        }

        Ok(body.value)
    }

    /// Search using ClawHub search API (vector search).
    async fn search_query(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<SkillSearchResult>> {
        let limit = if limit == 0 { 20 } else { limit };
        let url = format!(
            "{}/api/search?q={}&limit={}",
            self.base_url,
            urlencoding::encode(query),
            limit
        );

        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("search request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "search failed with status {}: {}",
                status,
                body.chars().take(1024).collect::<String>()
            )));
        }

        let search_resp: ClawhubSearchResponse = response
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to parse search response: {}", e)))?;

        let mut results: Vec<SkillSearchResult> = search_resp
            .results
            .into_iter()
            .map(|item| {
                // Normalize score to 0-1 range.
                let score = if item.score > 1.0 {
                    item.score / 5.0
                } else {
                    item.score
                };

                SkillSearchResult {
                    score,
                    slug: item.slug,
                    display_name: item.display_name,
                    summary: item.summary,
                    version: "latest".to_string(),
                    registry_name: "clawhub".to_string(),
                    source_repo: String::new(),
                    download_path: String::new(),
                    downloads: 0,
                    truncated: false,
                }
            })
            .collect();

        // Mark truncation if we got exactly `limit` results.
        if results.len() == limit && !results.is_empty() {
            results.last_mut().unwrap().truncated = true;
        }

        Ok(results)
    }

    /// List recent skills via Convex skills:list.
    async fn search_list(&self, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let limit = if limit == 0 { 20 } else { limit };

        let value = self
            .call_convex("skills:list", &[("limit", &limit.to_string())])
            .await?;

        let items: Vec<ConvexSkillListItem> =
            serde_json::from_value(value).map_err(|e| NemesisError::Serialization(e))?;

        let results: Vec<SkillSearchResult> = items
            .into_iter()
            .map(|item| SkillSearchResult {
                score: 1.0,
                slug: item.slug,
                display_name: item.display_name,
                summary: item.summary,
                version: "latest".to_string(),
                registry_name: "clawhub".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: item.stats.downloads as i64,
                truncated: false,
            })
            .collect();

        Ok(results)
    }

    /// Download a skill as a ZIP from the Convex site and extract it.
    async fn download_skill_zip(&self, slug: &str, target_dir: &str) -> Result<()> {
        let site_url = self.site_url();
        let download_url = format!(
            "{}/api/v1/download?slug={}",
            site_url,
            urlencoding::encode(slug)
        );

        let response = self
            .client
            .get(&download_url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ZIP download request failed: {}", e)))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(NemesisError::Other(format!(
                "ZIP download failed with status {}: {}",
                status,
                body.chars().take(512).collect::<String>()
            )));
        }

        // Check content type.
        if let Some(content_type) = response.headers().get("content-type") {
            let ct = content_type.to_str().unwrap_or("");
            if !ct.contains("zip") && !ct.contains("application/octet-stream") {
                return Err(NemesisError::Other(format!(
                    "unexpected content type for ZIP download: {}",
                    ct
                )));
            }
        }

        let body = response
            .bytes()
            .await
            .map_err(|e| NemesisError::Other(format!("failed to read ZIP response: {}", e)))?;

        if body.len() as u64 > 50 * 1024 * 1024 {
            return Err(NemesisError::Other("ZIP file too large (>50MB)".to_string()));
        }

        // Extract ZIP to target directory.
        extract_zip_to_dir(&body, target_dir)?;

        Ok(())
    }
}

impl Default for ClawHubRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Extract a ZIP archive to a directory, flattening a single top-level directory if present.
fn extract_zip_to_dir(data: &[u8], target_dir: &str) -> Result<()> {
    use std::io::Cursor;

    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| NemesisError::Other(format!("failed to read ZIP archive: {}", e)))?;

    // Collect all files to detect single top-level directory.
    let mut entries: Vec<String> = Vec::new();
    for i in 0..archive.len() {
        let file = archive.by_index(i).map_err(|e| {
            NemesisError::Other(format!("failed to read ZIP entry {}: {}", i, e))
        })?;
        entries.push(file.name().to_string());
    }

    // Determine prefix to strip (flatten single top-level dir).
    let prefix = if let Some(common_prefix) = find_common_prefix(&entries) {
        common_prefix
    } else {
        String::new()
    };

    // Re-read and extract.
    let reader = Cursor::new(data);
    let mut archive = zip::ZipArchive::new(reader)
        .map_err(|e| NemesisError::Other(format!("failed to re-read ZIP archive: {}", e)))?;

    std::fs::create_dir_all(target_dir).map_err(|e| NemesisError::Io(e))?;

    for i in 0..archive.len() {
        let mut file = archive.by_index(i).map_err(|e| {
            NemesisError::Other(format!("failed to read ZIP entry {}: {}", i, e))
        })?;

        let name = file.name().to_string();
        if name.ends_with('/') {
            continue; // Skip directories.
        }

        // Strip prefix.
        let relative = if name.starts_with(&prefix) {
            &name[prefix.len()..]
        } else {
            &name
        };

        if relative.is_empty() {
            continue;
        }

        let dest_path = std::path::Path::new(target_dir).join(relative);

        // Path traversal check.
        let canonical_target = std::path::Path::new(target_dir)
            .canonicalize()
            .unwrap_or_else(|_| std::path::PathBuf::from(target_dir));
        if let Some(parent) = dest_path.parent() {
            if let Ok(canonical_parent) = parent.canonicalize() {
                if !canonical_parent.starts_with(&canonical_target) {
                    return Err(NemesisError::Security(format!(
                        "path traversal detected: {}",
                        relative
                    )));
                }
            }
        }

        // Create parent directory.
        if let Some(parent) = dest_path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| NemesisError::Io(e))?;
        }

        // Write file.
        let mut output = std::fs::File::create(&dest_path).map_err(|e| NemesisError::Io(e))?;
        std::io::copy(&mut file, &mut output).map_err(|e| NemesisError::Io(e))?;
    }

    Ok(())
}

/// Find a common prefix directory if all entries share a single top-level directory.
fn find_common_prefix(entries: &[String]) -> Option<String> {
    if entries.is_empty() {
        return None;
    }

    // Check if all entries start with the same top-level directory.
    let first_dir = entries[0].split('/').next()?;
    if first_dir.is_empty() {
        return None;
    }

    let all_same = entries.iter().all(|e| e.starts_with(&format!("{}/", first_dir)) || e == first_dir);
    if all_same {
        Some(format!("{}/", first_dir))
    } else {
        None
    }
}

/// Check if staging_dir contains a single subdirectory at the top level.
/// If so, return the path to that subdirectory (for flattening).
/// Otherwise, return staging_dir as-is.
///
/// Mirrors Go's `flattenSingleTopDir(stagingDir)`.
#[allow(dead_code)]
fn flatten_single_top_dir(staging_dir: &std::path::Path) -> std::path::PathBuf {
    let entries: Vec<_> = std::fs::read_dir(staging_dir)
        .ok()
        .map(|rd| rd.filter_map(|e| e.ok()).collect())
        .unwrap_or_default();

    // If there's exactly one entry and it's a directory, flatten into it
    if entries.len() == 1 && entries[0].file_type().map(|t| t.is_dir()).unwrap_or(false) {
        staging_dir.join(entries[0].file_name())
    } else {
        staging_dir.to_path_buf()
    }
}

/// Move all files and directories from src_dir to dst_dir.
///
/// Mirrors Go's `moveDirContents(srcDir, dstDir)`.
#[allow(dead_code)]
fn move_dir_contents(src_dir: &std::path::Path, dst_dir: &std::path::Path) -> Result<()> {
    let entries: Vec<_> = std::fs::read_dir(src_dir)
        .map_err(|e| NemesisError::Io(e))?
        .filter_map(|e| e.ok())
        .collect();

    for entry in entries {
        let src_path = entry.path();
        let file_name = entry.file_name();
        let dst_path = dst_dir.join(&file_name);

        if entry.file_type().map(|t| t.is_dir()).unwrap_or(false) {
            std::fs::create_dir_all(&dst_path).map_err(|e| NemesisError::Io(e))?;
            move_dir_contents(&src_path, &dst_path)?;
        } else {
            let data = std::fs::read(&src_path)
                .map_err(|e| NemesisError::Other(format!("failed to read {}: {}", file_name.to_string_lossy(), e)))?;
            std::fs::write(&dst_path, &data)
                .map_err(|e| NemesisError::Other(format!("failed to write {}: {}", file_name.to_string_lossy(), e)))?;
        }
    }

    Ok(())
}

// --- Convex API types ---

/// Convex response envelope.
#[derive(Debug, Deserialize)]
struct ConvexResponse {
    status: String,
    value: serde_json::Value,
    #[serde(rename = "errorMessage")]
    error_message: Option<String>,
}

/// ClawHub search API response.
#[derive(Debug, Deserialize)]
struct ClawhubSearchResponse {
    results: Vec<ClawhubSearchItem>,
}

/// Single search result from ClawHub API.
#[derive(Debug, Deserialize)]
struct ClawhubSearchItem {
    score: f64,
    slug: String,
    #[serde(rename = "displayName")]
    display_name: String,
    summary: String,
    #[allow(dead_code)]
    version: Option<String>,
}

/// Skill list item from Convex skills:list.
#[derive(Debug, Deserialize)]
struct ConvexSkillListItem {
    slug: String,
    #[serde(rename = "displayName")]
    display_name: String,
    summary: String,
    stats: ConvexStats,
}

/// Browse response from ClawHub REST API.
#[derive(Debug, Deserialize)]
struct ClawhubBrowseResponse {
    #[serde(default)]
    items: Vec<ClawhubBrowseItem>,
    #[serde(rename = "nextCursor", default)]
    next_cursor: Option<String>,
}

/// Single item from ClawHub browse API.
#[derive(Debug, Deserialize)]
struct ClawhubBrowseItem {
    slug: String,
    #[serde(rename = "displayName", default)]
    display_name: String,
    #[serde(default)]
    summary: String,
    #[serde(default)]
    stats: ConvexStats,
}

/// Stats sub-object.
#[derive(Debug, Default, Deserialize)]
struct ConvexStats {
    #[serde(default)]
    downloads: f64,
}

/// Full skill detail from Convex skills:getBySlug.
#[derive(Debug, Deserialize)]
struct ConvexSkillDetail {
    owner: ConvexOwner,
    skill: ConvexSkill,
    #[serde(rename = "latestVersion")]
    latest_version: ConvexLatestVersion,
    #[serde(rename = "resolvedSlug")]
    resolved_slug: String,
}

/// Owner sub-object.
#[derive(Debug, Deserialize)]
struct ConvexOwner {
    handle: String,
}

/// Skill sub-object.
#[derive(Debug, Deserialize)]
struct ConvexSkill {
    slug: String,
    #[serde(rename = "displayName")]
    display_name: String,
    summary: String,
    #[allow(dead_code)]
    stats: ConvexStats,
}

/// Latest version sub-object.
#[derive(Debug, Deserialize)]
struct ConvexLatestVersion {
    version: String,
}

/// Minimal URL encoding (avoids extra dependency).
mod urlencoding {
    pub fn encode(s: &str) -> String {
        s.chars()
            .map(|c| match c {
                'A'..='Z' | 'a'..='z' | '0'..='9' | '-' | '_' | '.' | '~' => c.to_string(),
                _ => format!("%{:02X}", c as u8),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests;
