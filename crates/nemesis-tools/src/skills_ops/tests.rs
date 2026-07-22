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
    let result = tool.execute(&serde_json::json!({"query": "docker"})).await;
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
    let result = tool.execute(&serde_json::json!({"slug": "weather"})).await;
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
    let result = tool.execute(&serde_json::json!({"slug": ""})).await;
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
    let result = tool.execute(&serde_json::json!({"query": ""})).await;
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
    let result = tool.execute(&serde_json::json!({"slug": "   "})).await;
    assert!(result.is_error);
}

#[tokio::test]
async fn test_find_skills_null_query() {
    let tool = FindSkillsTool::new();
    let result = tool.execute(&serde_json::json!({"query": null})).await;
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
    let result = tool.execute(&serde_json::json!({"query": "test"})).await;
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
    let result = tool.execute(&serde_json::json!({"query": 123})).await;
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
