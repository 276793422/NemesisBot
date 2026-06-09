//! Skill installer - downloads, validates, and installs skills.
//!
//! Handles:
//! - GitHub-based skill download
//! - Pre-install lint + quality checks
//! - Post-install validation
//! - Origin tracking metadata
//! - Registry-based installation

use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Duration;

use tracing::{debug, warn};

use nemesis_types::error::{NemesisError, Result};

use crate::security_check::check_skill_security;
use crate::types::{
    AvailableSkill, InstallResult, SecurityCheckResult, SkillOrigin,
};

/// Skill installer that manages downloading and installing skills from registries.
pub struct SkillInstaller {
    workspace: PathBuf,
    registry_manager: Option<crate::registry::RegistryManager>,
    github_base_url: String,
    last_security_check: Mutex<Option<SecurityCheckResult>>,
}

impl SkillInstaller {
    /// Create a new installer for the given workspace directory.
    pub fn new(workspace: &str) -> Self {
        Self {
            workspace: PathBuf::from(workspace),
            registry_manager: None,
            github_base_url: "https://raw.githubusercontent.com".to_string(),
            last_security_check: Mutex::new(None),
        }
    }

    /// Set the registry manager for advanced installation features.
    pub fn set_registry_manager(&mut self, manager: crate::registry::RegistryManager) {
        self.registry_manager = Some(manager);
    }

    /// Set the GitHub base URL (for testing).
    pub fn set_github_base_url(&mut self, url: &str) {
        self.github_base_url = url.to_string();
    }

    /// Check whether a registry manager is configured.
    pub fn has_registry_manager(&self) -> bool {
        self.registry_manager.is_some()
    }

    /// Get a reference to the registry manager, if configured.
    ///
    /// Returns `Some(&RegistryManager)` if a registry manager has been set,
    /// or `None` otherwise.
    pub fn get_registry_manager(&self) -> Option<&crate::registry::RegistryManager> {
        self.registry_manager.as_ref()
    }

    /// Get the result from the most recent security check.
    pub fn last_security_check(&self) -> Option<SecurityCheckResult> {
        self.last_security_check
            .lock()
            .unwrap()
            .clone()
    }

    /// Store the result from the most recent security check.
    ///
    /// Mirrors Go `setLastSecurityCheck`.
    #[allow(dead_code)]
    fn set_last_security_check(&self, result: SecurityCheckResult) {
        let mut last = self.last_security_check.lock().unwrap();
        *last = Some(result);
    }

    /// Install a skill from a named registry (Go-compatible signature).
    ///
    /// Like `install` but returns only success/failure without the detailed
    /// `InstallResult`. This matches the Go `InstallFromRegistry` error-only
    /// return convention.
    pub async fn install_from_registry(
        &self,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<()> {
        self.install(registry_name, slug, version).await?;
        Ok(())
    }

    /// Get the workspace path.
    pub fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Check whether a registry with the given name exists.
    pub fn has_registry(&self, name: &str) -> bool {
        self.registry_manager
            .as_ref()
            .map(|rm| rm.get_registry(name).is_some())
            .unwrap_or(false)
    }

    /// Search all configured registries for skills matching the query.
    ///
    /// Results are returned grouped by registry source.
    pub async fn search_all(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::RegistrySearchResult>> {
        let manager = self.registry_manager.as_ref().ok_or_else(|| {
            NemesisError::Validation("registry manager not configured".to_string())
        })?;
        manager.search_all(query, limit).await
    }

    /// Flatten grouped search results into a single list.
    ///
    /// Utility for callers that don't need per-registry grouping.
    pub fn flatten_search_results(
        grouped: &[crate::types::RegistrySearchResult],
    ) -> Vec<crate::types::SkillSearchResult> {
        grouped
            .iter()
            .flat_map(|g| g.results.clone())
            .collect()
    }

    /// Install a skill from a named registry.
    ///
    /// Downloads the skill, runs security checks, and writes origin tracking.
    /// Mirrors Go `InstallFromRegistry`.
    pub async fn install(
        &self,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<InstallResult> {
        let manager = self.registry_manager.as_ref().ok_or_else(|| {
            NemesisError::Validation("registry manager not configured".to_string())
        })?;

        let skill_dir = self.workspace.join("skills").join(slug);

        if skill_dir.exists() {
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                slug
            )));
        }

        let registry = manager.get_registry(registry_name).ok_or_else(|| {
            NemesisError::NotFound(format!("registry '{}' not found", registry_name))
        })?;

        let dl_result = registry
            .download_and_install(slug, version, &skill_dir.to_string_lossy())
            .await?;

        // Check if the download result indicates malware or suspicious content.
        if dl_result.is_malware_blocked {
            let _ = std::fs::remove_dir_all(&skill_dir);
            return Err(NemesisError::Security(format!(
                "skill '{}' was blocked as malware",
                slug
            )));
        }

        if dl_result.is_suspicious {
            warn!("Warning: Skill '{}' is marked as suspicious", slug);
        }

        // Security check on installed content.
        let skill_md_path = skill_dir.join("SKILL.md");
        if skill_md_path.exists() {
            if let Ok(content) = std::fs::read_to_string(&skill_md_path) {
                let check_result = check_skill_security(&content, slug, "");
                {
                    let mut last = self.last_security_check.lock().unwrap();
                    *last = Some(check_result.clone());
                }

                if check_result.blocked {
                    let _ = std::fs::remove_dir_all(&skill_dir);
                    return Err(NemesisError::Security(format!(
                        "skill '{}' blocked by security check: {}",
                        slug, check_result.block_reason
                    )));
                }

                if !check_result.lint_result.passed {
                    warn!(
                        "Security warnings for '{}' (score: {:.0}/100, {} issues)",
                        slug,
                        check_result.lint_result.score * 100.0,
                        check_result.lint_result.warnings.len()
                    );
                }

                if let Some(ref quality) = check_result.quality_score {
                    debug!(
                        "Quality score for '{}': {:.0}/100",
                        slug, quality.overall
                    );
                }
            }
        }

        // Write origin tracking.
        if let Err(e) = self.write_origin_tracking(
            &skill_dir.to_string_lossy(),
            registry_name,
            slug,
            &dl_result.version,
        ) {
            warn!("failed to write origin tracking: {}", e);
        }

        debug!(
            "Skill '{}' (version {}) installed from registry '{}'",
            slug, dl_result.version, registry_name
        );

        Ok(InstallResult {
            version: dl_result.version,
            is_malware_blocked: dl_result.is_malware_blocked,
            is_suspicious: dl_result.is_suspicious,
            summary: dl_result.summary,
        })
    }

    /// Install a skill from a GitHub repository.
    ///
    /// Downloads the SKILL.md file from the repository's main branch,
    /// runs security checks, and installs to the workspace skills directory.
    pub async fn install_from_github(&self, repo: &str) -> Result<()> {
        let skill_name = Path::new(repo)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| repo.to_string());

        let skill_dir = self.workspace.join("skills").join(&skill_name);

        if skill_dir.exists() {
            return Err(NemesisError::Validation(format!(
                "skill '{}' already exists",
                skill_name
            )));
        }

        let url = format!("{}/{}/main/SKILL.md", self.github_base_url, repo);

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| NemesisError::Other(format!("failed to create HTTP client: {}", e)))?;

        let response = client.get(&url).send().await.map_err(|e| {
            NemesisError::Other(format!("failed to fetch skill: {}", e))
        })?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(NemesisError::Other(format!(
                "failed to fetch skill: HTTP {}",
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            NemesisError::Other(format!("failed to read response: {}", e))
        })?;

        // Create skill directory.
        std::fs::create_dir_all(&skill_dir).map_err(|e| NemesisError::Io(e))?;

        // Write SKILL.md atomically.
        let skill_path = skill_dir.join("SKILL.md");
        std::fs::write(&skill_path, &body).map_err(|e| {
            // Clean up on failure.
            let _ = std::fs::remove_dir_all(&skill_dir);
            NemesisError::Io(e)
        })?;

        // Security check.
        let check_result = check_skill_security(&body, &skill_name, "");
        {
            let mut last = self.last_security_check.lock().unwrap();
            *last = Some(check_result.clone());
        }

        if check_result.blocked {
            let _ = std::fs::remove_dir_all(&skill_dir);
            return Err(NemesisError::Security(format!(
                "skill '{}' blocked by security check: {}",
                skill_name, check_result.block_reason
            )));
        }

        if !check_result.lint_result.warnings.is_empty() {
            warn!(
                "skill has security warnings (score: {:.2}, {} issues)",
                check_result.lint_result.score,
                check_result.lint_result.warnings.len()
            );
        }

        if let Some(ref quality) = check_result.quality_score {
            if quality.overall < 40.0 {
                warn!(
                    "skill has low quality score (score: {:.0}/100)",
                    quality.overall
                );
            }
        }

        debug!("Installed skill '{}' from GitHub", skill_name);
        Ok(())
    }

    /// Uninstall a skill by name.
    pub fn uninstall(&self, skill_name: &str) -> Result<()> {
        let skill_dir = self.workspace.join("skills").join(skill_name);

        if !skill_dir.exists() {
            return Err(NemesisError::NotFound(format!(
                "skill '{}' not found",
                skill_name
            )));
        }

        std::fs::remove_dir_all(&skill_dir).map_err(|e| NemesisError::Io(e))?;
        debug!("Uninstalled skill '{}'", skill_name);
        Ok(())
    }

    /// List available skills from configured registries.
    pub async fn list_available_skills(&self) -> Result<Vec<AvailableSkill>> {
        // If registry manager is configured, use it for better results
        if let Some(manager) = &self.registry_manager {
            return self.list_available_skills_from_registry(manager).await;
        }

        // Fallback to original GitHub implementation
        self.list_available_skills_from_github().await
    }

    /// Search across all configured registries.
    ///
    /// Results are returned grouped by registry source.
    /// Alias for `search_all` matching the Go API naming.
    pub async fn search_registries(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::types::RegistrySearchResult>> {
        self.search_all(query, limit).await
    }

    /// List available skills using the registry manager.
    ///
    /// Uses `search_all` to get grouped results, then flattens them,
    /// matching the Go `listAvailableSkillsFromRegistry` implementation.
    async fn list_available_skills_from_registry(
        &self,
        manager: &crate::registry::RegistryManager,
    ) -> Result<Vec<AvailableSkill>> {
        let grouped = manager.search_all("", 100).await.unwrap_or_default();

        // Flatten grouped results into a single list of skills.
        let all_results = SkillInstaller::flatten_search_results(&grouped);

        Ok(all_results
            .into_iter()
            .map(|r| AvailableSkill {
                name: r.slug,
                repository: String::new(),
                description: r.summary,
                author: String::new(),
                tags: vec![r.registry_name],
            })
            .collect())
    }

    /// List available skills by fetching from GitHub skills repository.
    ///
    /// Fetches the skills.json index file from the default GitHub repository.
    async fn list_available_skills_from_github(&self) -> Result<Vec<AvailableSkill>> {
        let url = "https://raw.githubusercontent.com/276793422/nemesisbot-skills/main/skills.json";

        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(15))
            .build()
            .map_err(|e| NemesisError::Other(format!("failed to create HTTP client: {}", e)))?;

        let response = client.get(url).send().await.map_err(|e| {
            NemesisError::Other(format!("failed to fetch skills list: {}", e))
        })?;

        if response.status() != reqwest::StatusCode::OK {
            return Err(NemesisError::Other(format!(
                "failed to fetch skills list: HTTP {}",
                response.status()
            )));
        }

        let body = response.text().await.map_err(|e| {
            NemesisError::Other(format!("failed to read response: {}", e))
        })?;

        let skills: Vec<AvailableSkill> =
            serde_json::from_str(&body).map_err(|e| {
                NemesisError::Other(format!("failed to parse skills list: {}", e))
            })?;

        Ok(skills)
    }

    /// Write origin tracking metadata to the skill directory.
    fn write_origin_tracking(
        &self,
        skill_dir: &str,
        registry_name: &str,
        slug: &str,
        version: &str,
    ) -> Result<()> {
        let origin = SkillOrigin {
            version: 1,
            registry: registry_name.to_string(),
            slug: slug.to_string(),
            installed_version: version.to_string(),
            installed_at: chrono::Local::now().timestamp(),
        };

        let data = serde_json::to_string_pretty(&origin)
            .map_err(|e| NemesisError::Serialization(e))?;

        let origin_path = Path::new(skill_dir).join(".skill-origin.json");
        std::fs::write(&origin_path, data).map_err(|e| NemesisError::Io(e))?;

        debug!(
            "Wrote origin tracking for '{}' from '{}' version '{}'",
            slug, registry_name, version
        );
        Ok(())
    }

    /// Read origin tracking metadata for a skill.
    pub fn get_origin_tracking(&self, skill_name: &str) -> Result<SkillOrigin> {
        let origin_path = self
            .workspace
            .join("skills")
            .join(skill_name)
            .join(".skill-origin.json");

        if !origin_path.exists() {
            return Err(NemesisError::NotFound(format!(
                "origin file not found for skill '{}'",
                skill_name
            )));
        }

        let data = std::fs::read_to_string(&origin_path).map_err(|e| NemesisError::Io(e))?;
        let origin: SkillOrigin =
            serde_json::from_str(&data).map_err(|e| NemesisError::Serialization(e))?;

        Ok(origin)
    }
}

#[cfg(test)]
mod tests;
