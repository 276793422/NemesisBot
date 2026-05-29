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
struct SearchRequest {
    PageSize: i64,
    PageNumber: i64,
    Query: String,
    Sort: String,
    Criterion: Vec<serde_json::Value>,
    #[serde(rename = "WithTopCollection")]
    with_top_collection: bool,
}

#[derive(Debug, Deserialize)]
struct ApiResponse {
    Code: i64,
    Data: ApiData,
    Message: String,
    #[allow(dead_code)]
    Success: bool,
}

#[derive(Debug, Deserialize)]
struct ApiData {
    #[serde(default)]
    SkillList: Vec<ModelScopeSkill>,
    #[serde(default)]
    TotalCount: i64,
}

#[derive(Debug, Deserialize)]
struct ModelScopeSkill {
    #[serde(default)]
    Name: String,
    #[serde(default)]
    DisplayName: String,
    #[serde(default)]
    Description: String,
    #[serde(default)]
    DescriptionEn: String,
    #[serde(default)]
    SourceURL: String,
    #[serde(default)]
    SourceDeveloper: String,
    #[serde(default)]
    DownloadCount: i64,
    #[serde(default)]
    Visits: i64,
    #[serde(default)]
    SourceStar: i64,
    #[serde(default)]
    SourceForks: i64,
    #[serde(default)]
    Tags: Vec<String>,
    #[serde(default)]
    License: String,
    #[serde(default)]
    L1: Option<ModelScopeCategory>,
}

#[derive(Debug, Deserialize)]
struct ModelScopeCategory {
    #[serde(default)]
    #[allow(dead_code)]
    CatalogID: String,
    #[serde(default)]
    #[allow(dead_code)]
    ChineseName: String,
    #[serde(default)]
    #[allow(dead_code)]
    Name: String,
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

    async fn api_search(&self, query: &str, page: i64, page_size: i64, sort: &str) -> Result<ApiResponse> {
        let body = SearchRequest {
            PageSize: page_size,
            PageNumber: page,
            Query: query.to_string(),
            Sort: sort.to_string(),
            Criterion: vec![],
            with_top_collection: false,
        };
        let resp = self.client
            .put(&self.base_url)
            .json(&body)
            .send()
            .await
            .map_err(|e| NemesisError::Other(format!("ModelScope request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!("ModelScope HTTP {}", resp.status())));
        }
        let api: ApiResponse = resp.json().await
            .map_err(|e| NemesisError::Other(format!("ModelScope parse error: {}", e)))?;
        if api.Code != 200 {
            return Err(NemesisError::Other(format!("ModelScope API error: {}", api.Message)));
        }
        Ok(api)
    }

    fn convert_skill(s: &ModelScopeSkill) -> SkillSearchResult {
        let summary = if s.Description.is_empty() {
            s.DescriptionEn.clone()
        } else {
            s.Description.clone()
        };
        SkillSearchResult {
            score: 0.5,
            slug: s.Name.clone(),
            display_name: s.DisplayName.clone(),
            summary,
            version: "latest".to_string(),
            registry_name: "modelscope".to_string(),
            source_repo: s.SourceDeveloper.clone(),
            download_path: String::new(),
            downloads: s.DownloadCount,
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
        Ok(api.Data.SkillList.iter().map(Self::convert_skill).collect())
    }

    pub async fn get_skill_meta(&self, slug: &str) -> Result<SkillMeta> {
        validate_skill_identifier(slug).map_err(|e| NemesisError::Validation(e))?;
        let api = self.api_search(slug, 1, 1, "Default").await?;
        let skill = api.Data.SkillList.into_iter().next()
            .ok_or_else(|| NemesisError::NotFound(format!("skill '{}' not found on ModelScope", slug)))?;
        let summary = if skill.Description.is_empty() {
            skill.DescriptionEn.clone()
        } else {
            skill.Description.clone()
        };
        Ok(SkillMeta {
            slug: skill.Name.clone(),
            display_name: skill.DisplayName.clone(),
            summary,
            latest_version: "latest".to_string(),
            is_malware_blocked: false,
            is_suspicious: false,
            registry_name: "modelscope".to_string(),
            author: skill.SourceDeveloper.clone(),
            downloads: skill.DownloadCount,
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
        let skill = api.Data.SkillList.into_iter().next()
            .ok_or_else(|| NemesisError::NotFound(format!("skill '{}' not found", slug)))?;

        let raw_url = Self::source_url_to_raw(&skill.SourceURL)
            .ok_or_else(|| NemesisError::Other(format!("cannot parse SourceURL: {}", skill.SourceURL)))?;

        debug!("ModelScope download from: {}", raw_url);

        let resp = self.client.get(&raw_url).send().await
            .map_err(|e| NemesisError::Other(format!("download failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!("download HTTP {}", resp.status())));
        }
        let content = resp.text().await
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
        let skill = api.Data.SkillList.into_iter().next()
            .ok_or_else(|| NemesisError::NotFound(format!("skill '{}' not found", slug)))?;

        let raw_url = Self::source_url_to_raw(&skill.SourceURL)
            .ok_or_else(|| NemesisError::Other(format!("cannot parse SourceURL: {}", skill.SourceURL)))?;

        let resp = self.client.get(&raw_url).send().await
            .map_err(|e| NemesisError::Other(format!("request failed: {}", e)))?;
        if !resp.status().is_success() {
            return Err(NemesisError::Other(format!("HTTP {}", resp.status())));
        }
        let content = resp.text().await
            .map_err(|e| NemesisError::Other(format!("read failed: {}", e)))?;

        Ok(SkillContent {
            slug: slug.to_string(),
            filename: "SKILL.md".to_string(),
            content,
        })
    }

    pub async fn browse(&self, sort: &BrowseSort, limit: usize, cursor: &str) -> Result<BrowseResult> {
        let page = if cursor.is_empty() { 1i64 } else { cursor.parse::<i64>().unwrap_or(1) };
        let page_size = limit.min(100) as i64;
        let sort_str = match sort {
            BrowseSort::Downloads => "DownloadCount",
            BrowseSort::Updated => "GmtModify",
            _ => "Default",
        };
        let api = self.api_search("", page, page_size, sort_str).await?;
        let items: Vec<SkillSearchResult> = api.Data.SkillList.iter().map(Self::convert_skill).collect();
        let has_more = (page * page_size) < api.Data.TotalCount;
        Ok(BrowseResult {
            items,
            next_cursor: if has_more { Some((page + 1).to_string()) } else { None },
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_source_url_to_raw() {
        let url = "https://github.com/majiayu000/claude-skill-registry/tree/main/skills/data/7-debug";
        let raw = ModelScopeRegistry::source_url_to_raw(url).unwrap();
        assert_eq!(raw, "https://raw.githubusercontent.com/majiayu000/claude-skill-registry/main/skills/data/7-debug/SKILL.md");
    }

    #[test]
    fn test_source_url_to_raw_simple() {
        let url = "https://github.com/anthropics/claude-plugins-official/tree/main/plugins/skill-creator/skills/skill-creator";
        let raw = ModelScopeRegistry::source_url_to_raw(url).unwrap();
        assert_eq!(raw, "https://raw.githubusercontent.com/anthropics/claude-plugins-official/main/plugins/skill-creator/skills/skill-creator/SKILL.md");
    }

    #[test]
    fn test_source_url_invalid() {
        assert!(ModelScopeRegistry::source_url_to_raw("https://example.com/foo").is_none());
        assert!(ModelScopeRegistry::source_url_to_raw("https://github.com/owner/repo").is_none());
    }
}
