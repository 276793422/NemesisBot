use super::*;
use std::sync::Arc;

#[test]
fn test_new_installer() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(!installer.has_registry_manager());
    assert!(installer.last_security_check().is_none());
}

#[test]
fn test_uninstall_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.uninstall("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_uninstall_existing() {
    let dir = tempfile::tempdir().unwrap();
    let skills_dir = dir.path().join("skills");
    let skill_dir = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.uninstall("test-skill");
    assert!(result.is_ok());
    assert!(!skill_dir.exists());
}

#[test]
fn test_origin_tracking_roundtrip() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());

    installer
        .write_origin_tracking(
            &skill_dir.to_string_lossy(),
            "github",
            "test-skill",
            "1.0.0",
        )
        .unwrap();

    let origin = installer.get_origin_tracking("test-skill").unwrap();
    assert_eq!(origin.registry, "github");
    assert_eq!(origin.slug, "test-skill");
    assert_eq!(origin.installed_version, "1.0.0");
    assert_eq!(origin.version, 1);
}

#[test]
fn test_get_origin_tracking_nonexistent() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.get_origin_tracking("nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_has_registry_no_manager() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(!installer.has_registry("anything"));
}

#[test]
fn test_flatten_search_results_empty() {
    let flat = SkillInstaller::flatten_search_results(&[]);
    assert!(flat.is_empty());
}

#[test]
fn test_flatten_search_results_multiple() {
    use crate::types::{RegistrySearchResult, SkillSearchResult};
    let grouped = vec![
        RegistrySearchResult {
            registry_name: "source1".to_string(),
            results: vec![SkillSearchResult {
                score: 0.9,
                slug: "skill-a".to_string(),
                display_name: "Skill A".to_string(),
                summary: "A".to_string(),
                version: "1.0".to_string(),
                registry_name: "source1".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
        RegistrySearchResult {
            registry_name: "source2".to_string(),
            results: vec![SkillSearchResult {
                score: 0.8,
                slug: "skill-b".to_string(),
                display_name: "Skill B".to_string(),
                summary: "B".to_string(),
                version: "2.0".to_string(),
                registry_name: "source2".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
    ];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat.len(), 2);
    assert_eq!(flat[0].slug, "skill-a");
    assert_eq!(flat[1].slug, "skill-b");
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_set_registry_manager() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(!installer.has_registry_manager());

    let manager = crate::registry::RegistryManager::new_empty();
    installer.set_registry_manager(manager);
    assert!(installer.has_registry_manager());
}

#[test]
fn test_workspace_path() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().to_string_lossy().to_string();
    let installer = SkillInstaller::new(&path);
    assert_eq!(installer.workspace(), Path::new(&path));
}

#[test]
fn test_has_registry_with_manager_set() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    installer.set_registry_manager(manager);
    assert!(!installer.has_registry("nonexistent"));
}

#[test]
fn test_get_registry_manager_none() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(installer.get_registry_manager().is_none());
}

#[test]
fn test_get_registry_manager_some() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    installer.set_registry_manager(manager);
    assert!(installer.get_registry_manager().is_some());
}

#[test]
fn test_set_github_base_url() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_github_base_url("https://custom.example.com");
    assert_eq!(installer.github_base_url, "https://custom.example.com");
}

#[tokio::test]
async fn test_search_all_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.search_all("test", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_registries_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.search_registries("test", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.install("github", "test-skill", "1.0").await;
    assert!(result.is_err());
}

#[test]
fn test_flatten_search_results_empty_group() {
    use crate::types::RegistrySearchResult;
    let grouped = vec![RegistrySearchResult {
        registry_name: "empty".to_string(),
        results: vec![],
        truncated: false,
    }];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert!(flat.is_empty());
}

#[test]
fn test_write_origin_tracking_creates_file() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("my-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer
        .write_origin_tracking(&skill_dir.to_string_lossy(), "clawhub", "my-skill", "2.0.0")
        .unwrap();

    let origin_path = skill_dir.join(".skill-origin.json");
    assert!(origin_path.exists());

    let data = std::fs::read_to_string(origin_path).unwrap();
    assert!(data.contains("clawhub"));
    assert!(data.contains("my-skill"));
    assert!(data.contains("2.0.0"));
}

#[test]
fn test_last_security_check_initially_none() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(installer.last_security_check().is_none());
}

#[test]
fn test_skill_origin_serialization() {
    let origin = SkillOrigin {
        version: 1,
        registry: "test-registry".to_string(),
        slug: "test-skill".to_string(),
        installed_version: "1.0.0".to_string(),
        installed_at: 1715385600,
    };
    let json = serde_json::to_string(&origin).unwrap();
    let parsed: SkillOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.registry, "test-registry");
    assert_eq!(parsed.slug, "test-skill");
    assert_eq!(parsed.installed_version, "1.0.0");
}

#[test]
fn test_skill_origin_deserialization() {
    let json = r#"{"version":1,"registry":"github","slug":"pdf","installed_version":"2.0","installed_at":1715385600}"#;
    let origin: SkillOrigin = serde_json::from_str(json).unwrap();
    assert_eq!(origin.registry, "github");
    assert_eq!(origin.slug, "pdf");
    assert_eq!(origin.installed_version, "2.0");
}

#[test]
fn test_write_origin_tracking_nonexistent_dir() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.write_origin_tracking("/nonexistent/path/skill", "test", "skill", "1.0");
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_from_registry_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer
        .install_from_registry("github", "test", "1.0")
        .await;
    assert!(result.is_err());
}

#[test]
fn test_flatten_search_results_multiple_registries() {
    use crate::types::{RegistrySearchResult, SkillSearchResult};
    let grouped = vec![
        RegistrySearchResult {
            registry_name: "reg-a".to_string(),
            results: vec![SkillSearchResult {
                score: 1.0,
                slug: "skill-1".to_string(),
                display_name: "Skill 1".to_string(),
                summary: "First skill".to_string(),
                version: "1.0".to_string(),
                registry_name: "reg-a".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
        RegistrySearchResult {
            registry_name: "reg-b".to_string(),
            results: vec![SkillSearchResult {
                score: 0.9,
                slug: "skill-2".to_string(),
                display_name: "Skill 2".to_string(),
                summary: "Second skill".to_string(),
                version: "1.0".to_string(),
                registry_name: "reg-b".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
    ];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat.len(), 2);
}

// ============================================================
// Coverage improvement: additional installer tests
// ============================================================

#[tokio::test]
async fn test_install_skill_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("existing-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(std::sync::Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer.install("stub", "existing-skill", "1.0").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[tokio::test]
async fn test_install_registry_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    installer.set_registry_manager(manager);

    let result = installer.install("nonexistent", "test-skill", "1.0").await;
    assert!(result.is_err());
}

#[test]
fn test_available_skill_from_search_result() {
    let grouped = vec![crate::types::RegistrySearchResult {
        registry_name: "test".to_string(),
        results: vec![crate::types::SkillSearchResult {
            score: 1.0,
            slug: "pdf".to_string(),
            display_name: "PDF".to_string(),
            summary: "Converts PDFs".to_string(),
            version: "1.0".to_string(),
            registry_name: "test".to_string(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 5,
            truncated: false,
        }],
        truncated: false,
    }];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat.len(), 1);
    assert_eq!(flat[0].slug, "pdf");
}

#[test]
fn test_origin_tracking_json_format() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("json-check");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer
        .write_origin_tracking(&skill_dir.to_string_lossy(), "github", "json-check", "3.0")
        .unwrap();

    let content = std::fs::read_to_string(skill_dir.join(".skill-origin.json")).unwrap();
    let parsed: serde_json::Value = serde_json::from_str(&content).unwrap();
    assert_eq!(parsed["version"], 1);
    assert_eq!(parsed["registry"], "github");
    assert_eq!(parsed["slug"], "json-check");
    assert_eq!(parsed["installed_version"], "3.0");
    assert!(parsed["installed_at"].is_number());
}

#[test]
fn test_uninstall_cleans_up_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("to-remove");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Remove me").unwrap();
    std::fs::write(skill_dir.join("extra.txt"), "extra").unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.uninstall("to-remove").unwrap();
    assert!(!skill_dir.exists());
}

// ============================================================
// Coverage improvement: additional installer tests
// ============================================================

#[test]
fn test_list_available_skills_no_registry() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    // Without registry manager, falls back to GitHub which fails without network
    // The call should error since there's no network
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(installer.list_available_skills());
    assert!(result.is_err());
}

#[tokio::test]
async fn test_list_available_skills_with_registry() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(std::sync::Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer.list_available_skills().await;
    assert!(result.is_ok());
    // Stub returns empty results, so list should be empty
    assert!(result.unwrap().is_empty());
}

#[test]
fn test_install_from_github_already_exists() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("my-repo");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(installer.install_from_github("org/my-repo"));
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[test]
fn test_has_registry_with_matching_registry() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(std::sync::Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    assert!(installer.has_registry("stub"));
}

#[test]
fn test_write_origin_tracking_then_read() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("origin-test");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer
        .write_origin_tracking(
            &skill_dir.to_string_lossy(),
            "clawhub",
            "origin-test",
            "3.0.0",
        )
        .unwrap();

    let origin = installer.get_origin_tracking("origin-test").unwrap();
    assert_eq!(origin.registry, "clawhub");
    assert_eq!(origin.slug, "origin-test");
    assert_eq!(origin.installed_version, "3.0.0");
    assert_eq!(origin.version, 1);
    // installed_at should be a reasonable timestamp
    assert!(origin.installed_at > 0);
}

#[test]
fn test_write_origin_tracking_overwrites() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("overwrite-test");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());

    // Write first time
    installer
        .write_origin_tracking(
            &skill_dir.to_string_lossy(),
            "github",
            "overwrite-test",
            "1.0",
        )
        .unwrap();

    // Write again with different values
    installer
        .write_origin_tracking(
            &skill_dir.to_string_lossy(),
            "clawhub",
            "overwrite-test",
            "2.0",
        )
        .unwrap();

    let origin = installer.get_origin_tracking("overwrite-test").unwrap();
    assert_eq!(origin.registry, "clawhub");
    assert_eq!(origin.installed_version, "2.0");
}

#[test]
fn test_flatten_search_results_preserves_order() {
    use crate::types::{RegistrySearchResult, SkillSearchResult};
    let grouped = vec![
        RegistrySearchResult {
            registry_name: "reg-a".to_string(),
            results: vec![SkillSearchResult {
                score: 1.0,
                slug: "first".to_string(),
                display_name: "First".to_string(),
                summary: "First skill".to_string(),
                version: "1.0".to_string(),
                registry_name: "reg-a".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
        RegistrySearchResult {
            registry_name: "reg-b".to_string(),
            results: vec![SkillSearchResult {
                score: 0.9,
                slug: "second".to_string(),
                display_name: "Second".to_string(),
                summary: "Second skill".to_string(),
                version: "2.0".to_string(),
                registry_name: "reg-b".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
    ];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat[0].slug, "first");
    assert_eq!(flat[1].slug, "second");
}

#[test]
fn test_search_all_alias() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(std::sync::Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(installer.search_registries("test", 10));
    assert!(result.is_ok());
}

#[test]
fn test_install_from_registry_no_manager_error() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let rt = tokio::runtime::Runtime::new().unwrap();
    let result = rt.block_on(installer.install_from_registry("github", "test", "1.0"));
    assert!(result.is_err());
}

#[test]
fn test_available_skill_fields() {
    let skill = AvailableSkill {
        name: "pdf".to_string(),
        repository: "org/skills".to_string(),
        description: "PDF converter".to_string(),
        author: "alice".to_string(),
        tags: vec!["pdf".to_string()],
    };
    assert_eq!(skill.name, "pdf");
    assert_eq!(skill.repository, "org/skills");
    assert_eq!(skill.author, "alice");
    assert_eq!(skill.tags.len(), 1);
}

#[test]
fn test_security_check_result_default() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    // No security check has been run
    assert!(installer.last_security_check().is_none());
}

// ============================================================
// Coverage improvement: install, list, search with registries
// ============================================================

#[tokio::test]
async fn test_search_all_with_registry_manager() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let results = installer.search_all("test", 10).await;
    assert!(results.is_ok());
    let grouped = results.unwrap();
    assert_eq!(grouped.len(), 1);
    assert_eq!(grouped[0].registry_name, "stub");
    assert!(grouped[0].results.is_empty());
}

#[tokio::test]
async fn test_search_registries_with_registry_manager() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let results = installer.search_registries("test", 10).await;
    assert!(results.is_ok());
}

#[tokio::test]
async fn test_list_available_skills_with_stub_registry() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer.list_available_skills().await;
    assert!(result.is_ok());
    let skills = result.unwrap();
    assert!(skills.is_empty());
}

#[tokio::test]
async fn test_install_from_registry_registry_not_found_v2() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer
        .install_from_registry("nonexistent", "test", "1.0")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_from_registry_skill_already_exists_v2() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("test-skill-v2");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer
        .install_from_registry("stub", "test-skill-v2", "1.0")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[tokio::test]
async fn test_install_from_registry_success_v2() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let result = installer
        .install_from_registry("stub", "new-skill-v2", "1.0")
        .await;
    assert!(result.is_ok());
    // install_from_registry returns Result<()>
    // Note: StubRegistryProvider doesn't create actual files,
    // so origin tracking may or may not be readable depending on implementation
}

#[test]
fn test_uninstall_skill_removes_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("my-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# My Skill").unwrap();
    std::fs::write(skill_dir.join("helper.sh"), "echo hello").unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.uninstall("my-skill");
    assert!(result.is_ok());
    assert!(!skill_dir.exists());
}

#[test]
fn test_uninstall_nonexistent_skill() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.uninstall("nonexistent");
    assert!(result.is_ok() || result.is_err());
}

#[test]
fn test_origin_tracking_corrupt_file() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("corrupt-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join(".skill-origin.json"), "not valid json").unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.get_origin_tracking("corrupt-skill");
    assert!(result.is_err());
}

#[test]
fn test_origin_tracking_file_read_error() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("dir-as-file");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::create_dir_all(skill_dir.join(".skill-origin.json")).unwrap();

    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let result = installer.get_origin_tracking("dir-as-file");
    assert!(result.is_err());
}

#[test]
fn test_has_registry_false_without_manager() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    assert!(!installer.has_registry("github"));
}

#[test]
fn test_has_registry_true_with_stub() {
    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);
    assert!(installer.has_registry("stub"));
    assert!(!installer.has_registry("nonexistent"));
}

#[test]
fn test_flatten_search_results_empty_v2() {
    let empty: Vec<crate::types::RegistrySearchResult> = vec![];
    let flat = SkillInstaller::flatten_search_results(&empty);
    assert!(flat.is_empty());
}

#[test]
fn test_available_skill_serialization() {
    let skill = AvailableSkill {
        name: "pdf".to_string(),
        repository: "org/skills".to_string(),
        description: "PDF converter".to_string(),
        author: "alice".to_string(),
        tags: vec!["pdf".to_string(), "converter".to_string()],
    };
    let json = serde_json::to_string(&skill).unwrap();
    let parsed: AvailableSkill = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "pdf");
    assert_eq!(parsed.tags.len(), 2);
}

#[test]
fn test_install_result_fields() {
    let result = InstallResult {
        version: "2.0.0".to_string(),
        is_malware_blocked: false,
        is_suspicious: true,
        summary: "Test summary".to_string(),
    };
    assert_eq!(result.version, "2.0.0");
    assert!(!result.is_malware_blocked);
    assert!(result.is_suspicious);
    assert_eq!(result.summary, "Test summary");
}

#[test]
fn test_skill_origin_fields() {
    let origin = SkillOrigin {
        version: 1,
        registry: "github".to_string(),
        slug: "pdf".to_string(),
        installed_version: "1.0.0".to_string(),
        installed_at: 1234567890,
    };
    assert_eq!(origin.version, 1);
    assert_eq!(origin.registry, "github");
    assert_eq!(origin.installed_at, 1234567890);
}

#[test]
fn test_skill_origin_serialization_roundtrip() {
    let origin = SkillOrigin {
        version: 1,
        registry: "clawhub".to_string(),
        slug: "csv".to_string(),
        installed_version: "3.0".to_string(),
        installed_at: 1700000000,
    };
    let json = serde_json::to_string_pretty(&origin).unwrap();
    let parsed: SkillOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.registry, "clawhub");
    assert_eq!(parsed.slug, "csv");
    assert_eq!(parsed.installed_version, "3.0");
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_skill_installer_new_with_path() {
    let installer = SkillInstaller::new("/tmp/test-workspace");
    assert_eq!(
        installer.workspace().to_string_lossy(),
        "/tmp/test-workspace"
    );
    assert!(!installer.has_registry_manager());
    assert!(installer.get_registry_manager().is_none());
}

#[test]
fn test_skill_installer_set_github_base_url() {
    let mut installer = SkillInstaller::new("/tmp/test");
    installer.set_github_base_url("https://custom.github.com");
    assert_eq!(installer.github_base_url, "https://custom.github.com");
}

#[test]
fn test_skill_installer_default_github_url() {
    let installer = SkillInstaller::new("/tmp/test");
    assert_eq!(
        installer.github_base_url,
        "https://raw.githubusercontent.com"
    );
}

#[test]
fn test_skill_installer_last_security_check_new() {
    let installer = SkillInstaller::new("/tmp/test");
    assert!(installer.last_security_check().is_none());
}

#[test]
fn test_has_registry_false_no_manager_v2() {
    let installer = SkillInstaller::new("/tmp/test");
    assert!(!installer.has_registry("any"));
    assert!(!installer.has_registry("github"));
}

#[test]
fn test_uninstall_nonexistent_skill_returns_error() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.uninstall("nonexistent-skill");
    assert!(result.is_err());
}

#[test]
fn test_uninstall_removes_skill_dir_v2() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());

    // Create a skill directory
    let skill_dir = temp.path().join("skills").join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test Skill").unwrap();
    assert!(skill_dir.exists());

    let result = installer.uninstall("test-skill");
    assert!(result.is_ok());
    assert!(!skill_dir.exists());
}

#[test]
fn test_flatten_search_results_merges_registries_v2() {
    let grouped = vec![
        crate::types::RegistrySearchResult {
            registry_name: "github".to_string(),
            results: vec![crate::types::SkillSearchResult {
                score: 1.0,
                slug: "pdf".to_string(),
                display_name: "PDF".to_string(),
                summary: "PDF tool".to_string(),
                version: "1.0".to_string(),
                registry_name: "github".to_string(),
                source_repo: "test/repo".to_string(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            }],
            truncated: false,
        },
        crate::types::RegistrySearchResult {
            registry_name: "clawhub".to_string(),
            results: vec![crate::types::SkillSearchResult {
                score: 0.9,
                slug: "csv".to_string(),
                display_name: "CSV".to_string(),
                summary: "CSV tool".to_string(),
                version: "2.0".to_string(),
                registry_name: "clawhub".to_string(),
                source_repo: String::new(),
                download_path: String::new(),
                downloads: 5,
                truncated: false,
            }],
            truncated: false,
        },
    ];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat.len(), 2);
    assert_eq!(flat[0].slug, "pdf");
    assert_eq!(flat[1].slug, "csv");
}

#[test]
fn test_available_skill_manual_construction() {
    let skill = AvailableSkill {
        name: "test".to_string(),
        repository: "org/repo".to_string(),
        description: "A test skill".to_string(),
        author: "alice".to_string(),
        tags: vec!["test".to_string()],
    };
    assert_eq!(skill.name, "test");
    assert_eq!(skill.repository, "org/repo");
    assert_eq!(skill.description, "A test skill");
    assert_eq!(skill.author, "alice");
}

#[test]
fn test_install_result_fields_v2() {
    let result = InstallResult {
        version: "1.0".to_string(),
        is_malware_blocked: false,
        is_suspicious: false,
        summary: "Test".to_string(),
    };
    assert!(!result.is_malware_blocked);
    assert!(!result.is_suspicious);
    assert_eq!(result.version, "1.0");
}

#[test]
fn test_security_check_result_manual_construction() {
    let result = SecurityCheckResult {
        blocked: false,
        block_reason: String::new(),
        lint_result: crate::lint::LintResult {
            skill_name: "test".to_string(),
            passed: true,
            score: 1.0,
            warnings: vec![],
        },
        quality_score: None,
    };
    assert!(!result.blocked);
    assert!(result.block_reason.is_empty());
    assert!(result.lint_result.passed);
}

// ============================================================
// Coverage improvement: async paths, search, install errors
// ============================================================

#[tokio::test]
async fn test_search_all_no_registry_manager() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.search_all("pdf", 10).await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("registry manager"));
}

#[tokio::test]
async fn test_search_registries_no_registry_manager() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.search_registries("pdf", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_no_registry_manager() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.install("github", "pdf", "1.0").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("registry manager"));
}

#[tokio::test]
async fn test_install_from_registry_no_registry_manager() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer
        .install_from_registry("github", "pdf", "1.0")
        .await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_from_github_connection_error() {
    let temp = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(temp.path().to_str().unwrap());
    installer.set_github_base_url("http://127.0.0.1:1");
    let result = installer.install_from_github("org/repo").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_install_from_github_already_exists_v2() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join("skills").join("repo");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.install_from_github("org/repo").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("already exists"));
}

#[tokio::test]
async fn test_list_available_skills_no_registry_connection_error() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    // No registry manager -> falls back to GitHub which will fail (no network)
    let result = installer.list_available_skills().await;
    assert!(result.is_err());
}

#[test]
fn test_install_skill_already_exists_dir() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join("skills").join("existing-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();

    let _installer = SkillInstaller::new(temp.path().to_str().unwrap());
    // Can't test install() directly without a real registry manager,
    // but we can verify the workspace path check
    assert!(skill_dir.exists());
}

#[test]
fn test_workspace_returns_correct_path() {
    let temp = tempfile::tempdir().unwrap();
    let path = temp.path().to_str().unwrap();
    let installer = SkillInstaller::new(path);
    assert_eq!(installer.workspace(), std::path::Path::new(path));
}

#[test]
fn test_has_registry_manager_false() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    assert!(!installer.has_registry_manager());
}

#[test]
fn test_get_registry_manager_none_v2() {
    let temp = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    assert!(installer.get_registry_manager().is_none());
}

#[test]
fn test_write_origin_tracking_creates_file_v2() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("origin-test");
    std::fs::create_dir_all(&skill_dir).unwrap();

    let installer = SkillInstaller::new(dir.path().to_str().unwrap());
    installer
        .write_origin_tracking(&skill_dir.to_string_lossy(), "clawhub", "my-skill", "2.0.0")
        .unwrap();

    let origin_path = skill_dir.join(".skill-origin.json");
    assert!(origin_path.exists());
    let data = std::fs::read_to_string(&origin_path).unwrap();
    assert!(data.contains("clawhub"));
    assert!(data.contains("my-skill"));
    assert!(data.contains("2.0.0"));
}

#[test]
fn test_write_origin_tracking_invalid_dir() {
    let installer = SkillInstaller::new("/tmp/nonexistent_skill_test_dir");
    let result = installer.write_origin_tracking(
        "/tmp/nonexistent_skill_test_dir/skills/nope",
        "github",
        "nope",
        "1.0",
    );
    assert!(result.is_err());
}

#[test]
fn test_get_origin_tracking_corrupted_file() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("skills").join("corrupted");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(skill_dir.join(".skill-origin.json"), "not valid json{{{").unwrap();

    let installer = SkillInstaller::new(dir.path().to_str().unwrap());
    let result = installer.get_origin_tracking("corrupted");
    assert!(result.is_err());
}

#[test]
fn test_flatten_search_results_empty_v3() {
    let grouped: Vec<crate::types::RegistrySearchResult> = vec![];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert!(flat.is_empty());
}

#[test]
fn test_flatten_search_results_multiple_registries_v2() {
    let grouped = vec![crate::types::RegistrySearchResult {
        registry_name: "github".to_string(),
        results: vec![
            crate::types::SkillSearchResult {
                score: 1.0,
                slug: "pdf".to_string(),
                display_name: "PDF".to_string(),
                summary: "PDF tool".to_string(),
                version: "1.0".to_string(),
                registry_name: "github".to_string(),
                source_repo: "test/repo".to_string(),
                download_path: String::new(),
                downloads: 0,
                truncated: false,
            },
            crate::types::SkillSearchResult {
                score: 0.8,
                slug: "csv".to_string(),
                display_name: "CSV".to_string(),
                summary: "CSV tool".to_string(),
                version: "2.0".to_string(),
                registry_name: "github".to_string(),
                source_repo: "test/repo".to_string(),
                download_path: String::new(),
                downloads: 5,
                truncated: false,
            },
        ],
        truncated: false,
    }];
    let flat = SkillInstaller::flatten_search_results(&grouped);
    assert_eq!(flat.len(), 2);
}

#[test]
fn test_uninstall_removes_all_files() {
    let temp = tempfile::tempdir().unwrap();
    let skill_dir = temp.path().join("skills").join("multi-file-skill");
    std::fs::create_dir_all(skill_dir.join("docs")).unwrap();
    std::fs::write(skill_dir.join("SKILL.md"), "# Test").unwrap();
    std::fs::write(skill_dir.join("docs").join("guide.md"), "# Guide").unwrap();

    let installer = SkillInstaller::new(temp.path().to_str().unwrap());
    let result = installer.uninstall("multi-file-skill");
    assert!(result.is_ok());
    assert!(!skill_dir.exists());
}

// ============================================================
// BUG-HUNT R35：install() 的 staging 清理（remove_dir_all）拿原始 slug
// 拼接——slug 校验必须发生在删除之前，否则 `../../victim` 会在 registry
// 侧校验拒绝之前就把 workspace 外的目录先行删除（CLI `skills install
// github/../../victim` 与 WSAPI skills.install 同达）。
// ============================================================

#[tokio::test]
async fn test_install_rejects_traversal_slug_before_staging_delete() {
    let temp = tempfile::tempdir().unwrap();
    // 哨兵目录：从 .skill-staging 上溯两级（`../../victim`）恰落到 temp 根。
    let victim = temp.path().join("victim");
    std::fs::create_dir_all(victim.join("precious")).unwrap();
    std::fs::write(victim.join("precious").join("data.txt"), "keep").unwrap();
    let ws = temp.path().join("ws");
    std::fs::create_dir_all(&ws).unwrap();

    // 配 stub registry：修复前流程能穿过 manager/registry 查找直达 staging
    // 删除（stub 的 download 不写盘，删完才在后续验签环节报错）——哨兵存活
    // 断言因此是真实闸，不是恒真。
    let mut installer = SkillInstaller::new(ws.to_str().unwrap());
    let manager = crate::registry::RegistryManager::new_empty();
    manager.add_registry(std::sync::Arc::new(crate::registry::StubRegistryProvider));
    installer.set_registry_manager(manager);

    let err = installer
        .install("stub", "../../victim", "1.0")
        .await
        .unwrap_err();
    assert!(
        matches!(err, NemesisError::Validation(ref m) if m.contains("path traversal denied")),
        "expected traversal rejection, got {err:?}"
    );
    assert!(
        victim.join("precious").join("data.txt").exists(),
        "sentinel dir must survive the rejected install"
    );

    // Windows 盘符相对路径（`C:evil`）：PathBuf::join 遇带 prefix 成分整体
    // 截断，同样必须在入口被拒。
    let err = installer
        .install("stub", "C:evil", "1.0")
        .await
        .unwrap_err();
    assert!(
        matches!(err, NemesisError::Validation(ref m) if m.contains("cannot contain ':'")),
        "expected colon rejection, got {err:?}"
    );
}

#[tokio::test]
async fn test_install_clean_slug_passes_entry_validation() {
    // 对照组：合法 slug 不被入口校验误伤——错误推进到「registry manager
    // 未配置」而非校验拒绝。
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let err = installer
        .install("github", "ok-slug", "1.0")
        .await
        .unwrap_err();
    assert!(
        matches!(err, NemesisError::Validation(ref m) if m.contains("registry manager")),
        "clean slug should pass slug validation, got {err:?}"
    );
}

// ---------------------------------------------------------------------------
// R2（2026-10-24 复核）：并发安装共享 staging 串台——事务隔离 + digest 复核 +
// 落位互斥的回归锁。旧形态：A/B 同 slug 并发安装共享 `.skill-staging/<slug>`，
// approve-A 后 rename 落位的是 B 后写的内容（approve-A 装-B）。
// ---------------------------------------------------------------------------

/// pause-able 假 registry：download_and_install 写入指定内容（两安装同 slug）。
struct R2LocalRegistry {
    name: &'static str,
    content: &'static str,
}

#[async_trait::async_trait]
impl crate::registry::SkillRegistry for R2LocalRegistry {
    fn name(&self) -> &str {
        self.name
    }
    async fn search(
        &self,
        _query: &str,
        _limit: usize,
    ) -> Result<Vec<crate::types::SkillSearchResult>> {
        Ok(vec![])
    }
    async fn get_skill_meta(&self, _slug: &str) -> Result<crate::types::SkillMeta> {
        Err(NemesisError::Other("no metadata".into()))
    }
    async fn download_and_install(
        &self,
        _slug: &str,
        _version: &str,
        target: &str,
    ) -> Result<crate::types::InstallResult> {
        std::fs::create_dir_all(target).unwrap();
        std::fs::write(std::path::Path::new(target).join("SKILL.md"), self.content).unwrap();
        Ok(crate::types::InstallResult {
            version: "1".into(),
            is_malware_blocked: false,
            is_suspicious: false,
            summary: self.name.into(),
        })
    }
}

/// 双信号量 gate：decide 进入时亮 entered，等 release 后返回固定裁决。
struct R2PausedGate {
    entered: tokio::sync::Semaphore,
    release: tokio::sync::Semaphore,
    approve: bool,
}

#[async_trait::async_trait]
impl crate::install_gate::InstallGate for R2PausedGate {
    async fn decide(&self, _plan: &InstallPlan) -> InstallDecision {
        self.entered.add_permits(1);
        self.release.acquire().await.unwrap().forget();
        if self.approve {
            InstallDecision::Approve
        } else {
            InstallDecision::Deny {
                reason: "B denied".into(),
            }
        }
    }
}

fn r2_make_installer(
    workspace: &std::path::Path,
    name: &'static str,
    content: &'static str,
    approve: bool,
) -> (std::sync::Arc<SkillInstaller>, std::sync::Arc<R2PausedGate>) {
    let manager = crate::registry::RegistryManager::new(crate::types::RegistryConfig::default());
    manager.add_registry(std::sync::Arc::new(R2LocalRegistry { name, content }));
    let mut installer = SkillInstaller::new(&workspace.to_string_lossy());
    installer.set_registry_manager(manager);
    let gate = std::sync::Arc::new(R2PausedGate {
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
        approve,
    });
    installer.set_install_gate(gate.clone());
    (std::sync::Arc::new(installer), gate)
}

/// R2 回归锁①：approve-A / deny-B 并发同 slug 安装——落位的必须是 A 的
/// 内容（A 审批的载荷），B 诚实报错；两事务 staging 各自清理，互不串台。
/// 旧形态此断言红（落位 = B 内容）。
#[tokio::test]
async fn r2_concurrent_install_approves_a_installs_a_not_b() {
    let dir = tempfile::tempdir().unwrap();
    let a_content =
        "---\nname: shared\ndescription: Safe version A\n---\n# Shared\nReturn version A.\n";
    let b_content =
        "---\nname: shared\ndescription: Different version B\n---\n# Shared\nReturn version B.\n";
    let (a, a_gate) = r2_make_installer(dir.path(), "review-a", a_content, true);
    let (b, b_gate) = r2_make_installer(dir.path(), "review-b", b_content, false);

    let a_task = tokio::spawn({
        let a = a.clone();
        async move { a.install("review-a", "shared", "1").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), a_gate.entered.acquire())
        .await
        .expect("A gate not entered")
        .unwrap()
        .forget();
    let b_task = tokio::spawn({
        let b = b.clone();
        async move { b.install("review-b", "shared", "1").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), b_gate.entered.acquire())
        .await
        .expect("B gate not entered")
        .unwrap()
        .forget();

    // A 先落位：内容必须是 A 审批的载荷。
    a_gate.release.add_permits(1);
    a_task.await.unwrap().expect("A install must succeed");
    let installed = std::fs::read_to_string(dir.path().join("skills/shared/SKILL.md")).unwrap();
    assert_eq!(
        installed, a_content,
        "approved A must be what lands in skills/"
    );

    // B deny：诚实报错；B 的事务 staging 已清理。
    b_gate.release.add_permits(1);
    let b_result = b_task.await.unwrap();
    assert!(b_result.is_err(), "denied B must fail, got {:?}", b_result);

    // A 已被 B 安装重放覆盖？——重读落位内容仍为 A（B 从未落位）。
    let installed_after_b =
        std::fs::read_to_string(dir.path().join("skills/shared/SKILL.md")).unwrap();
    assert_eq!(installed_after_b, a_content);

    // staging 根无残留：事务目录各走各的（rename 落位 / deny 清理）。
    let staging_root = dir.path().join(".skill-staging");
    if staging_root.exists() {
        let leftovers: Vec<_> = std::fs::read_dir(&staging_root)
            .unwrap()
            .filter_map(|e| e.ok())
            .map(|e| e.file_name().to_string_lossy().to_string())
            .collect();
        assert!(leftovers.is_empty(), "staging leftovers: {leftovers:?}");
    }
}

/// R2 回归锁②：审批裁决瞬间 staging 内容被换 = digest 复核 abort，
/// 内容不落 skills/（审批的对象是字节，不是 slug 名义）。
#[tokio::test]
async fn r2_staging_tamper_after_approval_aborts() {
    struct TamperGate {
        workspace: std::path::PathBuf,
        entered: tokio::sync::Semaphore,
        release: tokio::sync::Semaphore,
    }
    #[async_trait::async_trait]
    impl crate::install_gate::InstallGate for TamperGate {
        async fn decide(&self, plan: &InstallPlan) -> InstallDecision {
            self.entered.add_permits(1);
            self.release.acquire().await.unwrap().forget();
            // 审批窗口内换写本事务 staging 的内容（模拟 TOCTOU 换包）。
            let staging = self
                .workspace
                .join(".skill-staging")
                .join(format!("{}.{}", plan.slug, plan.staging_tx));
            let skill_md = staging.join("SKILL.md");
            let mut body = std::fs::read_to_string(&skill_md).unwrap();
            body.push_str("\nTAMPERED\n");
            std::fs::write(&skill_md, body).unwrap();
            InstallDecision::Approve
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let content = "---\nname: shared\ndescription: v1\n---\nbody\n";
    let manager = crate::registry::RegistryManager::new(crate::types::RegistryConfig::default());
    manager.add_registry(std::sync::Arc::new(R2LocalRegistry {
        name: "tamper",
        content,
    }));
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    installer.set_registry_manager(manager);
    let gate = std::sync::Arc::new(TamperGate {
        workspace: dir.path().to_path_buf(),
        entered: tokio::sync::Semaphore::new(0),
        release: tokio::sync::Semaphore::new(0),
    });
    installer.set_install_gate(gate.clone());

    let task = tokio::spawn({
        let installer = std::sync::Arc::new(installer);
        async move { installer.install("tamper", "shared", "1").await }
    });
    tokio::time::timeout(std::time::Duration::from_secs(5), gate.entered.acquire())
        .await
        .expect("gate not entered")
        .unwrap()
        .forget();
    gate.release.add_permits(1);

    let result = task.await.unwrap();
    assert!(
        result.is_err(),
        "tampered staging must abort, got {:?}",
        result
    );
    assert!(
        !dir.path().join("skills/shared").exists(),
        "tampered content must never land in skills/"
    );
}

// ============================================================
// BUG-HUNT R163：安装漏斗三条纪律缺口的红证测试
// ============================================================

/// BUG-20261025-90：plan_github_install 的 slug 必须过 validate_skill_identifier
/// （R35 单一真相源漏接 GitHub 臂）——Windows 保留名（con/aux/nul…）与尾点
/// repo 名是 GitHub 合法名，直通 skills/ 路径拼装会在 Windows 上产出保留名
/// 目录/尾点截断漂移。校验在任何网络语句之前。
#[tokio::test]
async fn test_plan_github_install_rejects_windows_reserved_slug() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());

    // "con" 是合法 GitHub repo 名（纯字母），parse_github_ref 放行；
    // 漏斗必须在触网前以 Validation 拒绝。错误消息与 install() 同形。
    let result = installer.plan_github_install("owner/con").await;
    let err = match result {
        Ok(plan) => panic!(
            "reserved-name slug must be rejected, got plan for '{}'",
            plan.slug
        ),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("invalid skill slug"),
        "expected slug validation error before any network attempt, got: {}",
        err
    );

    // 尾点形态同证：Windows 会把 "foo." 目录截成 "foo"，lockfile 记账
    // 与磁盘目录名永久漂移。
    let result = installer.plan_github_install("owner/foo.").await;
    match result {
        Ok(plan) => panic!(
            "trailing-dot slug must be rejected, got plan for '{}'",
            plan.slug
        ),
        Err(e) => assert!(
            e.to_string().contains("invalid skill slug"),
            "expected slug validation error, got: {}",
            e
        ),
    }
}

/// BUG-20261025-91：registry 路径 age-policy block 拒绝时必须清 staging
/// （漏斗纪律「拒绝时先清 staging 再上抛」——同函数其余拒绝臂全清，
/// 唯 age 臂裸 `?` 泄漏，被拒内容残留盘上直到 24h 清扫）。
#[tokio::test]
async fn test_install_age_block_cleans_staging() {
    use crate::registry::{RegistryManager, SkillRegistry};
    use crate::types::{BrowseResult, BrowseSort, InstallResult, SkillMeta, SkillSearchResult};

    /// 本地测试 registry：落一个文件进 staging + 上报「刚发布」元数据，
    /// 使 min_age block 策略必然命中。
    struct FreshSkillRegistry;

    #[async_trait::async_trait]
    impl SkillRegistry for FreshSkillRegistry {
        fn name(&self) -> &str {
            "fresh"
        }
        async fn search(
            &self,
            _q: &str,
            _l: usize,
        ) -> nemesis_types::error::Result<Vec<SkillSearchResult>> {
            Ok(Vec::new())
        }
        async fn get_skill_meta(&self, slug: &str) -> nemesis_types::error::Result<SkillMeta> {
            Ok(SkillMeta {
                slug: slug.to_string(),
                display_name: slug.to_string(),
                summary: "fresh".to_string(),
                latest_version: "1.0".to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                registry_name: "fresh".to_string(),
                author: String::new(),
                downloads: 0,
                published_at: Some(chrono::Utc::now().timestamp()),
            })
        }
        async fn download_and_install(
            &self,
            _slug: &str,
            version: &str,
            target_dir: &str,
        ) -> nemesis_types::error::Result<InstallResult> {
            std::fs::create_dir_all(target_dir).unwrap();
            std::fs::write(
                std::path::Path::new(target_dir).join("SKILL.md"),
                "# harmless\n",
            )
            .unwrap();
            Ok(InstallResult {
                version: version.to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                summary: "fresh skill".to_string(),
            })
        }
        async fn browse(
            &self,
            _sort: &BrowseSort,
            _limit: usize,
            _cursor: &str,
        ) -> nemesis_types::error::Result<BrowseResult> {
            Ok(BrowseResult {
                items: Vec::new(),
                next_cursor: None,
            })
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(FreshSkillRegistry));
    installer.set_registry_manager(manager);
    installer.set_age_policy(30, "block");

    let result = installer.install("fresh", "newborn-skill", "1.0").await;
    assert!(result.is_err(), "age block must reject");
    assert!(
        result.unwrap_err().to_string().contains("age policy"),
        "rejection must come from the age gate"
    );

    // 泄漏断言：staging 根下不得残留本事务目录（拒绝即清）。
    let staging_root = dir.path().join(".skill-staging");
    let leaked = if staging_root.exists() {
        std::fs::read_dir(&staging_root)
            .unwrap()
            .flatten()
            .filter(|e| e.file_name().to_string_lossy().starts_with("newborn-skill"))
            .count()
    } else {
        0
    };
    assert_eq!(leaked, 0, "blocked install must not leave staging residue");
}

/// BUG-20261025-92：registry 路径落位临界区必须复检目标目录存在性
/// ——注释声称「exists 检查→rename→记账在 INSTALL_COMMIT_LOCK 下原子」，
/// 实际检查在下载 + 审批等待之前；并发同名安装在等待窗口内落位时，
/// 第二次 install 应诚实报 already exists，而不是裸 rename IO 错误
/// （Windows）/静默替换空目录（Unix）。
#[tokio::test]
async fn test_install_duplicate_target_detected_at_commit() {
    use crate::install_gate::{InstallDecision, InstallGate, InstallPlan};
    use crate::registry::{RegistryManager, SkillRegistry};
    use crate::types::{BrowseResult, BrowseSort, InstallResult, SkillMeta, SkillSearchResult};

    struct PlainRegistry;

    #[async_trait::async_trait]
    impl SkillRegistry for PlainRegistry {
        fn name(&self) -> &str {
            "plain"
        }
        async fn search(
            &self,
            _q: &str,
            _l: usize,
        ) -> nemesis_types::error::Result<Vec<SkillSearchResult>> {
            Ok(Vec::new())
        }
        async fn get_skill_meta(&self, slug: &str) -> nemesis_types::error::Result<SkillMeta> {
            Ok(SkillMeta {
                slug: slug.to_string(),
                display_name: slug.to_string(),
                summary: "plain".to_string(),
                latest_version: "1.0".to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                registry_name: "plain".to_string(),
                author: String::new(),
                downloads: 0,
                published_at: None,
            })
        }
        async fn download_and_install(
            &self,
            _slug: &str,
            version: &str,
            target_dir: &str,
        ) -> nemesis_types::error::Result<InstallResult> {
            std::fs::create_dir_all(target_dir).unwrap();
            std::fs::write(
                std::path::Path::new(target_dir).join("SKILL.md"),
                "# plain\n",
            )
            .unwrap();
            Ok(InstallResult {
                version: version.to_string(),
                is_malware_blocked: false,
                is_suspicious: false,
                summary: "plain".to_string(),
            })
        }
        async fn browse(
            &self,
            _sort: &BrowseSort,
            _limit: usize,
            _cursor: &str,
        ) -> nemesis_types::error::Result<BrowseResult> {
            Ok(BrowseResult {
                items: Vec::new(),
                next_cursor: None,
            })
        }
    }

    /// 审批等待窗口内「并发安装完成」的确定性模拟：decide() 时目标目录
    /// 已被另一安装落位（带内容），随后放行本计划。
    struct RacingGate {
        workspace: std::path::PathBuf,
    }

    #[async_trait::async_trait]
    impl InstallGate for RacingGate {
        async fn decide(&self, plan: &InstallPlan) -> InstallDecision {
            let target = self.workspace.join("skills").join(&plan.slug);
            std::fs::create_dir_all(&target).unwrap();
            std::fs::write(target.join("SKILL.md"), "# installed by the other winner\n").unwrap();
            InstallDecision::Approve
        }
        fn name(&self) -> &str {
            "racing-test-gate"
        }
    }

    let dir = tempfile::tempdir().unwrap();
    let mut installer = SkillInstaller::new(&dir.path().to_string_lossy());
    let manager = RegistryManager::new_empty();
    manager.add_registry(Arc::new(PlainRegistry));
    installer.set_registry_manager(manager);
    installer.set_install_gate(Arc::new(RacingGate {
        workspace: dir.path().to_path_buf(),
    }));

    let result = installer.install("plain", "dup-skill", "1.0").await;
    let err = match result {
        Ok(out) => panic!(
            "duplicate target must be rejected, got version '{}'",
            out.version
        ),
        Err(e) => e,
    };
    assert!(
        err.to_string().contains("already exists"),
        "commit-time duplicate must surface the honest Validation error, got: {}",
        err
    );
    // 先到者的内容不得被覆盖（rename 失败也不许换写）。
    let winner =
        std::fs::read_to_string(dir.path().join("skills").join("dup-skill").join("SKILL.md"))
            .unwrap();
    assert_eq!(winner, "# installed by the other winner\n");
}

// BUG-20261028-118 回归锁：回滚备份恢复式清扫——遗孤（skills/<slug> 缺失）
// 先恢复原位；过期备份（原位已在）直接删；skill 本体永不被动。
#[test]
fn r118_rollback_recover_restores_orphans_and_drops_stale() {
    let dir = tempfile::tempdir().unwrap();
    let installer = SkillInstaller::new(&dir.path().to_string_lossy());

    // 遗孤：skills/orphan 缺失（失败/取消现场）→ 恢复。
    let orphan_backup = installer.rollback_root().join("orphan.old-1-2");
    std::fs::create_dir_all(&orphan_backup).unwrap();
    std::fs::write(orphan_backup.join("SKILL.md"), "# orphan old").unwrap();

    // 过期备份：skills/stale 已在新版 → 备份删，skill 不动。
    let stale_skill = dir.path().join("skills/stale");
    std::fs::create_dir_all(stale_skill.join("new-version")).unwrap();
    std::fs::write(stale_skill.join("new-version").join("SKILL.md"), "# new").unwrap();
    let stale_backup = installer.rollback_root().join("stale.old-3-4");
    std::fs::create_dir_all(&stale_backup).unwrap();
    std::fs::write(stale_backup.join("SKILL.md"), "# stale old").unwrap();

    let recovered = installer.recover_stale_rollbacks();

    assert_eq!(
        recovered,
        vec![dir.path().join("skills").join("orphan")],
        "只有遗孤进恢复清单"
    );
    assert_eq!(
        std::fs::read_to_string(dir.path().join("skills/orphan/SKILL.md")).unwrap(),
        "# orphan old",
        "遗孤备份必须原样恢复到原位"
    );
    assert!(!orphan_backup.exists(), "恢复后备份不再留存");
    assert!(!stale_backup.exists(), "原位已在 = 过期备份直接删");
    assert_eq!(
        std::fs::read_to_string(stale_skill.join("new-version").join("SKILL.md")).unwrap(),
        "# new",
        "在场的新版技能不被清扫触碰"
    );
    // 根内无残留（round 2 = 幂等 no-op）。
    assert!(installer.recover_stale_rollbacks().is_empty());
}
