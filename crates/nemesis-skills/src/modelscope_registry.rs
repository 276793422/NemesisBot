//! ModelScope (modelscope.cn) skill registry.
//!
//! Uses the public ModelScope dolphin API to search and browse skills.
//! No authentication required.

use reqwest::Client;
use serde::{Deserialize, Serialize};
use tracing::debug;

use nemesis_types::error::{NemesisError, Result};

use crate::types::{
    BrowseResult, BrowseSort, InstallResult, SkillContent, SkillMeta, SkillSearchResult,
    validate_skill_identifier,
};

const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36";

/// ModelScope registry.
pub struct ModelScopeRegistry {
    client: Client,
    base_url: String,
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

impl ModelScopeRegistry {
    pub fn new() -> Self {
        let client = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(std::time::Duration::from_secs(30))
            .build()
            .expect("failed to create HTTP client");
        Self {
            client,
            base_url: "https://www.modelscope.cn/api/v1/dolphin/skills".to_string(),
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

    fn source_url_to_raw(source_url: &str) -> Option<String> {
        let url = source_url.strip_prefix("https://github.com/")?;
        let parts: Vec<&str> = url.splitn(4, '/').collect();
        if parts.len() < 4 || parts[2] != "tree" {
            return None;
        }
        let owner = parts[0];
        let repo = parts[1];
        let rest = parts[3];
        let branch_path: Vec<&str> = rest.splitn(2, '/').collect();
        if branch_path.len() < 2 {
            return None;
        }
        let branch = branch_path[0];
        let path = branch_path[1];
        Some(format!(
            "https://raw.githubusercontent.com/{}/{}/{}/{}/SKILL.md",
            owner, repo, branch, path
        ))
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

        let raw_url = Self::source_url_to_raw(&skill.source_url).ok_or_else(|| {
            NemesisError::Other(format!("cannot parse SourceURL: {}", skill.source_url))
        })?;

        debug!("ModelScope download from: {}", raw_url);

        let resp = self
            .client
            .get(&raw_url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("download failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!(
                "download HTTP {}",
                resp.status()
            )));
        }
        let content = resp
            .text()
            .await
            .map_err(|e| NemesisError::Other(format!("read failed: {}", e)))?;

        let target_path = std::path::Path::new(target_dir);
        std::fs::create_dir_all(target_path)
            .map_err(|e| NemesisError::Other(format!("create dir failed: {}", e)))?;
        std::fs::write(target_path.join("SKILL.md"), &content)
            .map_err(|e| NemesisError::Other(format!("write failed: {}", e)))?;

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

        let raw_url = Self::source_url_to_raw(&skill.source_url).ok_or_else(|| {
            NemesisError::Other(format!("cannot parse SourceURL: {}", skill.source_url))
        })?;

        let resp = self
            .client
            .get(&raw_url)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!("HTTP {}", resp.status())));
        }
        let content = resp
            .text()
            .await
            .map_err(|e| NemesisError::Other(format!("read failed: {}", e)))?;

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
