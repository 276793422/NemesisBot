//! ModelScope (modelscope.cn) skill registry.
//!
//! Uses the public ModelScope dolphin API to search and browse skills.
//! No authentication required.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::{debug, warn};

use nemesis_types::error::{NemesisError, Result};

use crate::github_tree::download_skill_tree_from_github;
use crate::types::{
    BrowseResult, BrowseSort, InstallResult, SkillContent, SkillMeta, SkillSearchResult,
    validate_skill_identifier,
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// ModelScope registry.
pub struct ModelScopeRegistry {
    client: Client,
    base_url: String,
    /// Base URL for the per-skill JSON detail API (`/api/v1/skills`).
    /// `GET {content_base_url}/{path}/{name}` returns the full SKILL.md in
    /// `Data.ReadMeContent`. Separate from `base_url` (the dolphin search
    /// endpoint) so tests can mock it independently.
    content_base_url: String,
    /// Short-timeout client for the GitHub fallback (when ModelScope only
    /// mirrored a scraped SKILL.md). Keeps the install from hanging when
    /// GitHub is unreachable (e.g. behind a network that blocks api.github.com).
    github_client: Client,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "PascalCase")]
struct SearchRequest {
    page_size: i64,
    page_number: i64,
    query: String,
    sort: String,
    criterion: Vec<serde_json::Value>,
    #[serde(rename = "WithTopCollection")]
    with_top_collection: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiResponse {
    code: i64,
    data: ApiData,
    message: String,
    #[allow(dead_code)]
    success: bool,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ApiData {
    #[serde(default)]
    skill_list: Vec<ModelScopeSkill>,
    #[serde(default)]
    total_count: i64,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ModelScopeSkill {
    #[serde(default)]
    name: String,
    #[serde(default)]
    display_name: String,
    #[serde(default)]
    description: String,
    #[serde(default)]
    description_en: String,
    /// Namespace segment used to build the per-skill detail URL
    /// (`{content_base_url}/{path}/{name}`). Maps to the catalog `Path` field.
    #[serde(default)]
    path: String,
    /// Catalog `Source` field: `"github"` | `"ModelScope"` | `"clawhub"`.
    /// Used to decide whether the GitHub fallback applies.
    #[serde(default)]
    source: String,
    /// Catalog `SourceUrl` — original repo URL; a github `tree` URL for
    /// `Source:github` skills. Used by the GitHub fallback.
    #[serde(default)]
    source_url: String,
    #[serde(default)]
    source_developer: String,
    #[serde(default)]
    download_count: i64,
    #[serde(default)]
    #[allow(dead_code)]
    visits: i64,
    #[serde(default)]
    #[allow(dead_code)]
    source_star: i64,
    #[serde(default)]
    #[allow(dead_code)]
    source_forks: i64,
    #[serde(default)]
    #[allow(dead_code)]
    tags: Vec<String>,
    #[serde(default)]
    #[allow(dead_code)]
    license: String,
    #[serde(default)]
    #[allow(dead_code)]
    l1: Option<ModelScopeCategory>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ModelScopeCategory {
    #[serde(default)]
    #[allow(dead_code)]
    catalog_id: String,
    #[serde(default)]
    #[allow(dead_code)]
    chinese_name: String,
    #[serde(default)]
    #[allow(dead_code)]
    name: String,
}

/// Per-skill detail API response (`GET /api/v1/skills/{path}/{name}`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SkillDetailResponse {
    code: i64,
    data: SkillDetailData,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct SkillDetailData {
    /// Full SKILL.md text (catalog `ReadMeContent` field).
    #[serde(default)]
    read_me_content: String,
}

/// File-tree listing response (`GET .../repo/files[?Root=]`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RepoFilesResponse {
    code: i64,
    data: RepoFilesData,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RepoFilesData {
    #[serde(default)]
    files: Vec<RepoFileEntry>,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RepoFileEntry {
    #[serde(default)]
    path: String,
    /// `"tree"` (directory) or `"blob"` (file).
    #[serde(default, rename = "Type")]
    entry_type: String,
}

/// Single-file content response (`GET .../repo/raw?Revision=&FilePath=`).
#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RepoRawResponse {
    code: i64,
    data: RepoRawData,
    #[serde(default)]
    message: String,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "PascalCase")]
struct RepoRawData {
    #[serde(default)]
    content: String,
}

/// Percent-encode a URL path segment (RFC 3986 unreserved set).
/// Dependency-free; mirrors the encoding in `clawhub_registry`.
fn url_encode_component(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for &b in s.as_bytes() {
        match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' => {
                out.push(b as char)
            }
            _ => out.push_str(&format!("%{:02X}", b)),
        }
    }
    out
}

/// Percent-encode a multi-segment file path, preserving `/` separators.
/// Used for `FilePath` query values and repo sub-paths.
fn url_encode_filepath(p: &str) -> String {
    p.split('/')
        .map(url_encode_component)
        .collect::<Vec<_>>()
        .join("/")
}

/// Parse a GitHub `tree` URL into `(owner, repo, branch, path)`.
/// Returns `None` for non-github URLs or URLs that are not a `.../tree/<branch>/<path>`
/// shape (e.g. a bare repo root, or a `/blob/` file URL).
fn parse_github_tree_url(url: &str) -> Option<(&str, &str, &str, &str)> {
    let rest = url.strip_prefix("https://github.com/")?;
    let parts: Vec<&str> = rest.splitn(4, '/').collect();
    if parts.len() < 4 || parts[2] != "tree" {
        return None;
    }
    let (branch, path) = parts[3].split_once('/')?;
    if branch.is_empty() || path.is_empty() {
        return None;
    }
    Some((parts[0], parts[1], branch, path))
}

impl ModelScopeRegistry {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");
        let github_client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(10))
            .build()
            .expect("failed to create GitHub HTTP client");
        Self {
            client,
            base_url: "https://www.modelscope.cn/api/v1/dolphin/skills".to_string(),
            content_base_url: "https://www.modelscope.cn/api/v1/skills".to_string(),
            github_client,
        }
    }

    pub fn name(&self) -> &str {
        "modelscope"
    }

    async fn api_search(
        &self,
        query: &str,
        page: i64,
        page_size: i64,
        sort: &str,
    ) -> Result<ApiResponse> {
        let body = SearchRequest {
            page_size,
            page_number: page,
            query: query.to_string(),
            sort: sort.to_string(),
            criterion: vec![],
            with_top_collection: false,
        };
        let resp = self
            .client
            .put(&self.base_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!(
                "ModelScope HTTP {}",
                resp.status()
            )));
        }
        let api: ApiResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope parse error: {}", e)))?;
        if api.code != 200 {
            return Err(NemesisError::Other(format!(
                "ModelScope API error: {}",
                api.message
            )));
        }
        Ok(api)
    }

    fn convert_skill(s: &ModelScopeSkill) -> SkillSearchResult {
        let summary = if s.description.is_empty() {
            s.description_en.clone()
        } else {
            s.description.clone()
        };
        SkillSearchResult {
            score: 0.5,
            slug: s.name.clone(),
            display_name: s.display_name.clone(),
            summary,
            version: "latest".to_string(),
            registry_name: "modelscope".to_string(),
            source_repo: s.source_developer.clone(),
            download_path: String::new(),
            downloads: s.download_count,
            truncated: false,
        }
    }

    /// Fetch the full SKILL.md for a skill via the per-skill JSON detail API.
    ///
    /// `GET {content_base_url}/{path}/{name}` returns
    /// `{"Code":200,"Data":{...,"ReadMeContent":"<full SKILL.md>"}}`. This
    /// endpoint is authoritative for every catalog entry regardless of `Source`
    /// (github / ModelScope / clawhub) — ModelScope mirrors the SKILL.md — so it
    /// supersedes the old `source_url_to_raw` (which only handled github `/tree/`
    /// URLs and failed for ~75% of skills, including all ModelScope-native ones).
    async fn fetch_skill_content(&self, path: &str, name: &str) -> Result<String> {
        debug!("ModelScope fetch skill content: {}/{}", path, name);
        let url = format!(
            "{}/{}/{}",
            self.content_base_url.trim_end_matches('/'),
            url_encode_component(path),
            url_encode_component(name),
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope detail request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!(
                "ModelScope detail HTTP {} for '{}/{}'",
                resp.status(),
                path,
                name
            )));
        }
        let body: SkillDetailResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope detail parse error: {}", e)))?;
        if body.code != 200 {
            return Err(NemesisError::Other(format!(
                "ModelScope detail API error for '{}/{}': {}",
                path, name, body.message
            )));
        }
        let content = body
            .data
            .read_me_content
            .trim_start_matches('\u{feff}')
            .to_string();
        if content.is_empty() {
            return Err(NemesisError::Other(format!(
                "ModelScope returned empty ReadMeContent for '{}/{}'",
                path, name
            )));
        }
        Ok(content)
    }

    /// Build the per-skill API URL (`{content_base_url}/{path}/{name}`).
    fn skill_api_url(&self, path: &str, name: &str) -> String {
        format!(
            "{}/{}/{}",
            self.content_base_url.trim_end_matches('/'),
            url_encode_component(path),
            url_encode_component(name),
        )
    }

    /// List one level of the skill repo file tree at `root` (`""` = repo root).
    async fn list_repo_files(
        &self,
        path: &str,
        name: &str,
        root: &str,
    ) -> Result<Vec<RepoFileEntry>> {
        // Always send `?Root=` (empty for repo root); the API treats an empty
        // Root the same as no Root, and always sending it makes the root and
        // subdirectory list requests distinguishable by query param.
        let url = format!(
            "{}/repo/files?Root={}",
            self.skill_api_url(path, name),
            url_encode_filepath(root),
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope file-list request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!(
                "ModelScope file-list HTTP {} for '{}/{}'",
                resp.status(),
                path,
                name
            )));
        }
        let body: RepoFilesResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope file-list parse error: {}", e)))?;
        if body.code != 200 {
            return Err(NemesisError::Other(format!(
                "ModelScope file-list API error for '{}/{}': {}",
                path, name, body.message
            )));
        }
        Ok(body.data.files)
    }

    /// Fetch a single file's content via `/repo/raw?Revision=master&FilePath=`.
    async fn fetch_repo_raw(
        &self,
        path: &str,
        name: &str,
        file_path: &str,
    ) -> Result<String> {
        let url = format!(
            "{}/repo/raw?Revision=master&FilePath={}",
            self.skill_api_url(path, name),
            url_encode_filepath(file_path),
        );
        let resp = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope file request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!(
                "ModelScope file HTTP {} for '{}/{}'",
                resp.status(),
                path,
                file_path
            )));
        }
        let body: RepoRawResponse = resp
            .json()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope file parse error: {}", e)))?;
        if body.code != 200 {
            return Err(NemesisError::Other(format!(
                "ModelScope file API error for '{}/{}': {}",
                path, file_path, body.message
            )));
        }
        Ok(body.data.content)
    }

    /// Recursively fetch every file of a skill — SKILL.md plus its companion
    /// files (`references/`, `scripts/`, ...). ModelScope mirrors the entire
    /// skill directory, so this works for all `Source` types.
    ///
    /// The tree API does not flatten, so directories (`Type:"tree"`) are walked
    /// level by level via `Root`, and each blob is downloaded via `/repo/raw`.
    /// Returns `(relative_path, content)` pairs.
    async fn fetch_full_skill(
        &self,
        path: &str,
        name: &str,
    ) -> Result<Vec<(String, String)>> {
        let mut files = Vec::new();
        let mut roots: Vec<String> = vec![String::new()];
        while let Some(root) = roots.pop() {
            let entries = self.list_repo_files(path, name, &root).await?;
            for e in entries {
                if e.entry_type == "tree" {
                    roots.push(e.path);
                } else {
                    let content = self.fetch_repo_raw(path, name, &e.path).await?;
                    files.push((e.path, content));
                }
            }
        }
        if files.is_empty() {
            return Err(NemesisError::Other(format!(
                "ModelScope skill '{}/{}' has no files",
                path, name
            )));
        }
        Ok(files)
    }

    pub async fn search(&self, query: &str, limit: usize) -> Result<Vec<SkillSearchResult>> {
        let page_size = limit.min(50) as i64;
        let api = self.api_search(query, 1, page_size, "Default").await?;
        Ok(api
            .data
            .skill_list
            .iter()
            .map(Self::convert_skill)
            .collect())
    }

    pub async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        validate_skill_identifier(slug).map_err(|e| NemesisError::Validation(e))?;
        let api = self.api_search(slug, 1, 1, "Default").await?;
        let skill = api.data.skill_list.into_iter().next().ok_or_else(|| {
            NemesisError::NotFound(format!("skill '{}' not found on ModelScope", slug))
        })?;
        let summary = if skill.description.is_empty() {
            skill.description_en.clone()
        } else {
            skill.description.clone()
        };
        Ok(SkillMeta {
            slug: skill.name.clone(),
            display_name: skill.display_name.clone(),
            summary,
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "modelscope".to_string(),
            author: skill.source_developer.clone(),
            downloads: skill.download_count,
        })
    }

    pub async fn download_and_install(
        &self,
        slug: &str,
        _version: &str,
        target_dir: &str,
    ) -> Result<InstallResult> {
        validate_skill_identifier(slug).map_err(|e| NemesisError::Validation(e))?;
        let meta = self.get_skill_meta(slug).await?;
        let api = self.api_search(slug, 1, 1, "Default").await?;
        let skill = api
            .data
            .skill_list
            .into_iter()
            .next()
            .ok_or_else(|| NemesisError::NotFound(format!("skill '{}' not found", slug)))?;

        // Catalog search is free-text; guard against installing a near-match
        // skill (e.g. the duplicated slug "chinese-novelist") by requiring the
        // returned skill name to equal the requested slug.
        if skill.name != slug {
            return Err(NemesisError::NotFound(format!(
                "skill '{}' not found on ModelScope (search returned '{}')",
                slug, skill.name
            )));
        }

        // Fetch the FULL skill tree (SKILL.md + references/scripts/...) via the
        // per-skill repo API. ModelScope mirrors every file of the indexed skill,
        // so this works for all Source types (github / ModelScope / clawhub).
        let files = self.fetch_full_skill(&skill.path, &skill.name).await?;

        // ModelScope mirrors some GitHub-sourced skills as only a scraped
        // SKILL.md (the catalog `metadata.json` betrays this: a per-file `path`
        // plus a `downloaded_at` timestamp). When the mirror has no
        // subdirectories and the skill originates on GitHub, fetch the
        // authoritative full tree from the GitHub source so companion files
        // (references/, scripts/) are not silently dropped. Falls back to the
        // (partial) ModelScope mirror if GitHub is unreachable.
        let has_subdirs = files.iter().any(|(p, _)| p.contains('/'));
        if !has_subdirs && skill.source == "github" {
            if let Some((owner, repo, branch, gh_path)) = parse_github_tree_url(&skill.source_url) {
                let repo_str = format!("{}/{}", owner, repo);
                match download_skill_tree_from_github(
                    &self.github_client,
                    "https://api.github.com",
                    "https://raw.githubusercontent.com",
                    &repo_str,
                    branch,
                    gh_path,
                    target_dir,
                    0,
                )
                .await
                {
                    Ok(()) => {
                        debug!(
                            "installed skill '{}' full tree from GitHub (modelscope mirror was partial)",
                            slug
                        );
                        return Ok(InstallResult {
                            version: "latest".to_string(),
                            is_malware_blocked: false,
                            is_suspicious: false,
                            summary: meta.summary,
                        });
                    }
                    Err(e) => warn!(
                        "GitHub fallback for skill '{}' failed ({}); ModelScope mirror has only {} file(s)",
                        slug,
                        e,
                        files.len()
                    ),
                }
            }
        }

        let target_path = std::path::Path::new(target_dir);
        std::fs::create_dir_all(target_path)
            .map_err(|e| NemesisError::Other(format!("create dir failed: {}", e)))?;
        let canonical_target = target_path
            .canonicalize()
            .unwrap_or_else(|_| target_path.to_path_buf());

        for (rel, content) in &files {
            // Path-traversal guard: server-provided paths must stay under target.
            if rel.is_empty()
                || rel.starts_with('/')
                || rel.starts_with('\\')
                || rel.contains("..")
            {
                return Err(NemesisError::Security(format!(
                    "refusing unsafe skill file path: {}",
                    rel
                )));
            }
            let dest = target_path.join(rel);
            if let Some(parent) = dest.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| NemesisError::Other(format!("create dir failed: {}", e)))?;
                if let Ok(canonical_parent) = parent.canonicalize() {
                    if !canonical_parent.starts_with(&canonical_target) {
                        return Err(NemesisError::Security(format!(
                            "path traversal detected: {}",
                            rel
                        )));
                    }
                }
            }
            std::fs::write(&dest, content)
                .map_err(|e| NemesisError::Other(format!("write failed: {}", e)))?;
        }

        Ok(InstallResult {
            version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: meta.summary,
        })
    }

    pub async fn get_skill_content(&self, slug: &str) -> Result<SkillContent> {
        validate_skill_identifier(slug).map_err(|e| NemesisError::Validation(e))?;
        let api = self.api_search(slug, 1, 1, "Default").await?;
        let skill = api
            .data
            .skill_list
            .into_iter()
            .next()
            .ok_or_else(|| NemesisError::NotFound(format!("skill '{}' not found", slug)))?;

        if skill.name != slug {
            return Err(NemesisError::NotFound(format!(
                "skill '{}' not found on ModelScope (search returned '{}')",
                slug, skill.name
            )));
        }

        let content = self.fetch_skill_content(&skill.path, &skill.name).await?;

        Ok(SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content,
        })
    }

    pub async fn browse(
        &self,
        sort: &BrowseSort,
        limit: usize,
        cursor: &str,
    ) -> Result<BrowseResult> {
        let page = if cursor.is_empty() {
            1i64
        } else {
            cursor.parse::<i64>().unwrap_or(1)
        };
        let page_size = limit.min(100) as i64;
        let sort_str = match sort {
            BrowseSort::Downloads => "DownloadCount",
            BrowseSort::Updated => "GmtModify",
            _ => "Default",
        };
        let api = self.api_search("", page, page_size, sort_str).await?;
        let items: Vec<SkillSearchResult> = api
            .data
            .skill_list
            .iter()
            .map(Self::convert_skill)
            .collect();
        let has_more = (page * page_size) < api.data.total_count;
        Ok(BrowseResult {
            items,
            next_cursor: if has_more {
                Some((page + 1).to_string())
            } else {
                None
            },
        })
    }
}

#[cfg(test)]
mod tests;

#[cfg(test)]
mod modelscope_extra_tests;
