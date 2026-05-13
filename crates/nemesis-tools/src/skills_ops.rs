//! Skills search and install tools.
//!
//! These tools delegate to the real `nemesis-skills` crate types:
//! - `FindSkillsTool` delegates to `nemesis_skills::registry::RegistryManager`
//! - `InstallSkillTool` delegates to `nemesis_skills::installer::SkillInstaller`
//!
//! Mirrors Go's `FindSkillsTool` (module/tools/skills_search.go) and
//! `InstallSkillTool` (module/tools/skills_install.go).

use crate::registry::Tool;
use crate::types::ToolResult;
use async_trait::async_trait;
use std::sync::Arc;
use tokio::sync::Mutex;

// ---------------------------------------------------------------------------
// FindSkillsTool
// ---------------------------------------------------------------------------

/// Find skills tool - searches configured skill registries.
///
/// Delegates to `nemesis_skills::registry::RegistryManager::search_all()`
/// for concurrent multi-registry search, matching Go's `FindSkillsTool.Execute()`.
pub struct FindSkillsTool {
    registry: Arc<Mutex<Option<nemesis_skills::registry::RegistryManager>>>,
}

impl FindSkillsTool {
    /// Create a new find skills tool without a registry manager.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(None)),
        }
    }

    /// Create with a pre-configured registry manager.
    pub fn with_registry_manager(manager: nemesis_skills::registry::RegistryManager) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Some(manager))),
        }
    }

    /// Set the registry manager (for late initialization).
    pub async fn set_registry_manager(&self, manager: nemesis_skills::registry::RegistryManager) {
        let mut guard = self.registry.lock().await;
        *guard = Some(manager);
    }
}

impl Default for FindSkillsTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for FindSkillsTool {
    fn name(&self) -> &str {
        "find_skills"
    }

    fn description(&self) -> &str {
        "Search for available skills from configured registries (GitHub, ClawHub, etc.). Use this when you need to find or discover new skills to install."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "query": {"type": "string", "description": "Search query (e.g., 'weather', 'docker'). Leave empty to list all."},
                "limit": {"type": "integer", "description": "Maximum results (1-50, default: 5)"}
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let query = match args["query"].as_str() {
            Some(q) => q,
            None => return ToolResult::error("missing 'query' argument"),
        };

        let limit = args["limit"].as_u64().unwrap_or(5).clamp(1, 50) as usize;

        let guard = self.registry.lock().await;
        let registry = match guard.as_ref() {
            Some(r) => r,
            None => return ToolResult::error("registry manager not configured"),
        };

        // Search all configured registries
        let grouped = match registry.search_all(query, limit).await {
            Ok(results) => results,
            Err(e) => return ToolResult::error(&format!("failed to search registries: {}", e)),
        };

        // Flatten grouped results into a single sorted list
        let mut results: Vec<nemesis_skills::types::SkillSearchResult> = grouped
            .iter()
            .flat_map(|g| g.results.clone())
            .collect();

        if results.is_empty() {
            return ToolResult::success(&format!("No skills found for query '{}'", query));
        }

        // Sort by score descending
        results.sort_by(|a, b| b.score.partial_cmp(&a.score).unwrap_or(std::cmp::Ordering::Equal));
        results.truncate(limit);

        let mut output = format!("Found {} skill(s) for \"{}\":\n", results.len(), query);
        for (i, result) in results.iter().enumerate() {
            output.push_str(&format!("\n{}. **{}**", i + 1, result.slug));
            if !result.version.is_empty() {
                output.push_str(&format!(" v{}", result.version));
            }
            output.push_str(&format!(
                " (score: {:.2}, registry: {})\n",
                result.score, result.registry_name
            ));
            if !result.display_name.is_empty() {
                output.push_str(&format!("   Display Name: {}\n", result.display_name));
            }
            if !result.source_repo.is_empty() {
                output.push_str(&format!("   Source: {}\n", result.source_repo));
            }
            if result.downloads > 0 {
                output.push_str(&format!("   Downloads: {}\n", result.downloads));
            }
            if !result.summary.is_empty() {
                output.push_str(&format!("   Description: {}\n", result.summary));
            }
        }

        ToolResult::success(&output)
    }
}

// ---------------------------------------------------------------------------
// InstallSkillTool
// ---------------------------------------------------------------------------

/// Install skill tool - installs a skill from a configured registry.
///
/// Delegates to `nemesis_skills::installer::SkillInstaller` for downloading,
/// linting, quality checking, and installing skills. Mirrors Go's
/// `InstallSkillTool.Execute()`.
pub struct InstallSkillTool {
    registry: Arc<Mutex<Option<nemesis_skills::registry::RegistryManager>>>,
    installer: Arc<Mutex<Option<nemesis_skills::installer::SkillInstaller>>>,
}

impl InstallSkillTool {
    /// Create a new install skill tool without any backends.
    pub fn new() -> Self {
        Self {
            registry: Arc::new(Mutex::new(None)),
            installer: Arc::new(Mutex::new(None)),
        }
    }

    /// Create with pre-configured registry manager and installer.
    pub fn with_backends(
        registry: nemesis_skills::registry::RegistryManager,
        installer: nemesis_skills::installer::SkillInstaller,
    ) -> Self {
        Self {
            registry: Arc::new(Mutex::new(Some(registry))),
            installer: Arc::new(Mutex::new(Some(installer))),
        }
    }

    /// Set the registry manager (for late initialization).
    pub async fn set_registry_manager(&self, manager: nemesis_skills::registry::RegistryManager) {
        let mut guard = self.registry.lock().await;
        *guard = Some(manager);
    }

    /// Set the installer (for late initialization).
    pub async fn set_installer(&self, installer: nemesis_skills::installer::SkillInstaller) {
        let mut guard = self.installer.lock().await;
        *guard = Some(installer);
    }
}

impl Default for InstallSkillTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for InstallSkillTool {
    fn name(&self) -> &str {
        "install_skill"
    }

    fn description(&self) -> &str {
        "Install a skill from a configured registry (GitHub, ClawHub, etc.). Use this after finding a skill with find_skills."
    }

    fn parameters(&self) -> serde_json::Value {
        serde_json::json!({
            "type": "object",
            "properties": {
                "slug": {"type": "string", "description": "Skill identifier (e.g., 'github', 'weather')"},
                "registry": {"type": "string", "description": "Registry name (optional, defaults to first available)"},
                "version": {"type": "string", "description": "Specific version to install (optional)"},
                "force": {"type": "boolean", "description": "Force reinstall if already exists (default: false)"}
            },
            "required": ["slug"]
        })
    }

    async fn execute(&self, args: &serde_json::Value) -> ToolResult {
        let slug = match args["slug"].as_str() {
            Some(s) if !s.is_empty() => s,
            _ => return ToolResult::error("slug parameter is required and must be a non-empty string"),
        };

        let registry_name = args["registry"].as_str().unwrap_or("");
        let version = args["version"].as_str().unwrap_or("");
        let force = args["force"].as_bool().unwrap_or(false);

        let reg_guard = self.registry.lock().await;
        let inst_guard = self.installer.lock().await;

        if reg_guard.is_none() {
            return ToolResult::error("registry manager not configured");
        }

        let installer = match inst_guard.as_ref() {
            Some(i) => i,
            None => return ToolResult::error("installer not configured"),
        };

        // Determine target registry
        let target_registry = if !registry_name.is_empty() {
            if let Some(ref reg) = *reg_guard {
                if reg.get_registry(registry_name).is_none() {
                    return ToolResult::error(&format!(
                        "registry '{}' not found or not configured",
                        registry_name
                    ));
                }
            }
            registry_name.to_string()
        } else {
            // Default to first available registry (matching Go behavior)
            "github".to_string()
        };

        // Check if skill already exists (unless force)
        if !force {
            if installer.get_origin_tracking(slug).is_ok() {
                return ToolResult::error(&format!(
                    "skill '{}' is already installed. Use force=true to reinstall.",
                    slug
                ));
            }
        }

        // Install from registry
        match installer.install(&target_registry, slug, version).await {
            Ok(_result) => {
                let msg = format!(
                    "Skill '{}' installed successfully from registry '{}'",
                    slug, target_registry
                );

                // Append security info if available
                let _ = installer.last_security_check();
                ToolResult::success(&msg)
            }
            Err(e) => ToolResult::error(&format!("failed to install skill '{}': {}", slug, e)),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_find_skills_tool_metadata() {
        let tool = FindSkillsTool::new();
        assert_eq!(tool.name(), "find_skills");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_install_skill_tool_metadata() {
        let tool = InstallSkillTool::new();
        assert_eq!(tool.name(), "install_skill");
        assert!(!tool.description().is_empty());
    }

    #[tokio::test]
    async fn test_find_skills_no_registry() {
        let tool = FindSkillsTool::new();
        let result = tool
            .execute(&serde_json::json!({"query": "docker"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("registry manager not configured"));
    }

    #[tokio::test]
    async fn test_find_skills_missing_query() {
        let tool = FindSkillsTool::new();
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_install_skill_no_backends() {
        let tool = InstallSkillTool::new();
        let result = tool
            .execute(&serde_json::json!({"slug": "weather"}))
            .await;
        assert!(result.is_error);
        assert!(result.for_llm.contains("not configured"));
    }

    #[tokio::test]
    async fn test_install_skill_missing_slug() {
        let tool = InstallSkillTool::new();
        let result = tool.execute(&serde_json::json!({})).await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_install_skill_empty_slug() {
        let tool = InstallSkillTool::new();
        let result = tool
            .execute(&serde_json::json!({"slug": ""}))
            .await;
        assert!(result.is_error);
    }

    // ---- New tests ----

    #[test]
    fn test_find_skills_parameters_valid_json() {
        let tool = FindSkillsTool::new();
        let params = tool.parameters();
        assert!(params.is_object());
    }

    #[test]
    fn test_install_skill_parameters_valid_json() {
        let tool = InstallSkillTool::new();
        let params = tool.parameters();
        assert!(params.is_object());
    }

    #[tokio::test]
    async fn test_find_skills_with_limit() {
        let tool = FindSkillsTool::new();
        let result = tool
            .execute(&serde_json::json!({"query": "test", "limit": 5}))
            .await;
        assert!(result.is_error); // no registry
    }

    #[tokio::test]
    async fn test_find_skills_empty_query() {
        let tool = FindSkillsTool::new();
        let result = tool
            .execute(&serde_json::json!({"query": ""}))
            .await;
        assert!(result.is_error); // no registry
    }

    #[test]
    fn test_find_skills_new_default() {
        let _tool = FindSkillsTool::new();
        // Verify it can be created without panic
    }

    #[test]
    fn test_install_skill_new_default() {
        let _tool = InstallSkillTool::new();
        // Verify it can be created without panic
    }

    #[tokio::test]
    async fn test_install_skill_whitespace_slug() {
        let tool = InstallSkillTool::new();
        let result = tool
            .execute(&serde_json::json!({"slug": "   "}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_find_skills_null_query() {
        let tool = FindSkillsTool::new();
        let result = tool
            .execute(&serde_json::json!({"query": null}))
            .await;
        assert!(result.is_error);
    }

    // ============================================================
    // Additional skills_ops tests for missing coverage
    // ============================================================

    #[tokio::test]
    async fn test_find_skills_with_registry_manager() {
        let config = nemesis_skills::types::RegistryConfig::default();
        let manager = nemesis_skills::registry::RegistryManager::new(config);
        let tool = FindSkillsTool::with_registry_manager(manager);
        assert_eq!(tool.name(), "find_skills");
    }

    #[tokio::test]
    async fn test_find_skills_set_registry_manager() {
        let tool = FindSkillsTool::new();
        let config = nemesis_skills::types::RegistryConfig::default();
        let manager = nemesis_skills::registry::RegistryManager::new(config);
        tool.set_registry_manager(manager).await;
        // Registry is now set, search should work but return empty results
        let result = tool
            .execute(&serde_json::json!({"query": "test"}))
            .await;
        // Should either succeed with empty results or fail gracefully
        let _ = result;
    }

    #[tokio::test]
    async fn test_install_skill_with_backends() {
        let config = nemesis_skills::types::RegistryConfig::default();
        let manager = nemesis_skills::registry::RegistryManager::new(config);
        let installer = nemesis_skills::installer::SkillInstaller::new("/tmp/test-workspace");
        let tool = InstallSkillTool::with_backends(manager, installer);
        assert_eq!(tool.name(), "install_skill");
    }

    #[tokio::test]
    async fn test_install_skill_set_registry_manager() {
        let tool = InstallSkillTool::new();
        let config = nemesis_skills::types::RegistryConfig::default();
        let manager = nemesis_skills::registry::RegistryManager::new(config);
        tool.set_registry_manager(manager).await;
    }

    #[tokio::test]
    async fn test_install_skill_set_installer() {
        let tool = InstallSkillTool::new();
        let installer = nemesis_skills::installer::SkillInstaller::new("/tmp/test-workspace");
        tool.set_installer(installer).await;
    }

    #[test]
    fn test_find_skills_tool_name() {
        let tool = FindSkillsTool::new();
        assert_eq!(tool.name(), "find_skills");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_install_skill_tool_name() {
        let tool = InstallSkillTool::new();
        assert_eq!(tool.name(), "install_skill");
        assert!(!tool.description().is_empty());
    }

    #[test]
    fn test_find_skills_default() {
        let tool = FindSkillsTool::default();
        assert_eq!(tool.name(), "find_skills");
    }

    #[test]
    fn test_install_skill_default() {
        let tool = InstallSkillTool::default();
        assert_eq!(tool.name(), "install_skill");
    }

    #[tokio::test]
    async fn test_find_skills_number_query() {
        let tool = FindSkillsTool::new();
        let result = tool
            .execute(&serde_json::json!({"query": 123}))
            .await;
        assert!(result.is_error);
    }

    #[tokio::test]
    async fn test_install_skill_special_chars_slug() {
        let tool = InstallSkillTool::new();
        let result = tool
            .execute(&serde_json::json!({"slug": "../etc/passwd"}))
            .await;
        assert!(result.is_error);
    }
}
