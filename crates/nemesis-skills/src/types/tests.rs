use super::*;

#[test]
fn test_skill_info_serialization_roundtrip() {
    let info = SkillInfo {
        name: "test-skill".to_string(),
        path: "/skills/test-skill".to_string(),
        source: "local".to_string(),
        description: "A test skill".to_string(),
        lint_score: Some(0.95),
        has_warnings: false,
    };
    let json = serde_json::to_string(&info).unwrap();
    let deserialized: SkillInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(deserialized.name, "test-skill");
    assert_eq!(deserialized.lint_score, Some(0.95));
    assert!(!deserialized.has_warnings);
}

#[test]
fn test_skill_search_result_defaults() {
    let result = SkillSearchResult {
        score: 0.85,
        slug: "my-skill".to_string(),
        display_name: "My Skill".to_string(),
        summary: "Does things".to_string(),
        version: String::new(),
        registry_name: "official".to_string(),
        source_repo: String::new(),
        download_path: String::new(),
        downloads: 0,
        truncated: false,
    };
    assert_eq!(result.version, "");
    assert_eq!(result.score, 0.85);
}

#[test]
fn test_registry_config_default() {
    let config = RegistryConfig::default();
    assert!(!config.search_cache.enabled);
    assert!(config.github_sources.is_empty());
    assert_eq!(config.max_concurrent_searches, 2);
}

#[test]
fn test_validate_skill_identifier_valid() {
    assert!(validate_skill_identifier("my-skill").is_ok());
    assert!(validate_skill_identifier("pdf").is_ok());
}

#[test]
fn test_validate_skill_identifier_empty() {
    assert!(validate_skill_identifier("").is_err());
    assert!(validate_skill_identifier("  ").is_err());
}

#[test]
fn test_validate_skill_identifier_path_traversal() {
    assert!(validate_skill_identifier("../etc/passwd").is_err());
    assert!(validate_skill_identifier("foo/bar").is_err());
    assert!(validate_skill_identifier("foo\\bar").is_err());
}

#[test]
fn test_validate_skill_identifier_too_long() {
    let long_slug = "a".repeat(65);
    assert!(validate_skill_identifier(&long_slug).is_err());
}

#[test]
fn test_contains_ci() {
    assert!(contains_ci("Hello World", "hello"));
    assert!(contains_ci("Hello World", "WORLD"));
    assert!(contains_ci("Hello World", ""));
    assert!(!contains_ci("Hello", "xyz"));
}

#[test]
fn test_skill_info_validate_valid() {
    let info = SkillInfo {
        name: "my-skill".to_string(),
        path: "/skills/my-skill".to_string(),
        source: "local".to_string(),
        description: "A valid skill".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    assert!(info.validate().is_empty());
}

#[test]
fn test_skill_info_validate_empty_name() {
    let info = SkillInfo {
        name: String::new(),
        path: "/skills/test".to_string(),
        source: "local".to_string(),
        description: "desc".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("name is required")));
}

#[test]
fn test_skill_info_validate_name_too_long() {
    let info = SkillInfo {
        name: "a".repeat(65),
        path: "/skills/test".to_string(),
        source: "local".to_string(),
        description: "desc".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("exceeds")));
}

#[test]
fn test_skill_info_validate_invalid_name_chars() {
    let info = SkillInfo {
        name: "my skill!".to_string(),
        path: "/skills/test".to_string(),
        source: "local".to_string(),
        description: "desc".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("alphanumeric")));
}

#[test]
fn test_skill_info_validate_empty_description() {
    let info = SkillInfo {
        name: "my-skill".to_string(),
        path: "/skills/test".to_string(),
        source: "local".to_string(),
        description: String::new(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("description is required")));
}

#[test]
fn test_skill_info_validate_description_too_long() {
    let info = SkillInfo {
        name: "my-skill".to_string(),
        path: "/skills/test".to_string(),
        source: "local".to_string(),
        description: "x".repeat(1025),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("description exceeds")));
}

#[test]
fn test_to_lower() {
    assert_eq!(to_lower("Hello WORLD"), "hello world");
    assert_eq!(to_lower("already-lower"), "already-lower");
    assert_eq!(to_lower("123_ABC"), "123_abc");
    assert_eq!(to_lower(""), "");
}

#[test]
fn test_contains_ci_unicode() {
    assert!(contains_ci("HELLO", "hello"));
    assert!(contains_ci("hElLo WoRlD", "HELLO WORLD"));
}

#[test]
fn test_contains_ci_empty_needle() {
    assert!(contains_ci("anything", ""));
    assert!(contains_ci("", ""));
}

#[test]
fn test_contains_ci_no_match() {
    assert!(!contains_ci("hello", "xyz"));
    assert!(!contains_ci("", "a"));
}

#[test]
fn test_skill_search_result_serialization() {
    let result = SkillSearchResult {
        score: 0.92,
        slug: "pdf-generator".to_string(),
        display_name: "PDF Generator".to_string(),
        summary: "Generate PDFs".to_string(),
        version: "1.0.0".to_string(),
        registry_name: "official".to_string(),
        source_repo: "org/skills".to_string(),
        download_path: "skills/pdf/SKILL.md".to_string(),
        downloads: 500,
        truncated: false,
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: SkillSearchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.slug, "pdf-generator");
    assert_eq!(parsed.score, 0.92);
    assert_eq!(parsed.downloads, 500);
}

#[test]
fn test_registry_search_result_serialization() {
    let result = RegistrySearchResult {
        registry_name: "github".to_string(),
        results: vec![SkillSearchResult {
            score: 1.0,
            slug: "test".to_string(),
            display_name: "Test".to_string(),
            summary: "Test skill".to_string(),
            version: String::new(),
            registry_name: "github".to_string(),
            source_repo: String::new(),
            download_path: String::new(),
            downloads: 0,
            truncated: false,
        }],
        truncated: true,
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: RegistrySearchResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.results.len(), 1);
    assert!(parsed.truncated);
}

#[test]
fn test_skill_meta_serialization() {
    let meta = SkillMeta {
        slug: "test-skill".to_string(),
        display_name: "Test Skill".to_string(),
        summary: "A test".to_string(),
        latest_version: "2.0.0".to_string(),
        is_malware_blocked: false,
        is_suspicious: true,
        registry_name: "clawhub".to_string(),
        author: "testauthor".to_string(),
        downloads: 42,
    };
    let json = serde_json::to_string(&meta).unwrap();
    let parsed: SkillMeta = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.slug, "test-skill");
    assert!(parsed.is_suspicious);
    assert!(!parsed.is_malware_blocked);
}

#[test]
fn test_install_result_serialization() {
    let result = InstallResult {
        version: "1.5.0".to_string(),
        is_malware_blocked: false,
        is_suspicious: false,
        summary: "Installed successfully".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: InstallResult = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, "1.5.0");
}

#[test]
fn test_skill_origin_serialization() {
    let origin = SkillOrigin {
        version: 1,
        registry: "github".to_string(),
        slug: "my-skill".to_string(),
        installed_version: "1.0.0".to_string(),
        installed_at: 1700000000,
    };
    let json = serde_json::to_string(&origin).unwrap();
    let parsed: SkillOrigin = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.version, 1);
    assert_eq!(parsed.installed_at, 1700000000);
}

#[test]
fn test_available_skill_serialization() {
    let skill = AvailableSkill {
        name: "test".to_string(),
        repository: "org/repo".to_string(),
        description: "A test skill".to_string(),
        author: "test-author".to_string(),
        tags: vec!["utility".to_string(), "pdf".to_string()],
    };
    let json = serde_json::to_string(&skill).unwrap();
    let parsed: AvailableSkill = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.tags.len(), 2);
}

#[test]
fn test_search_cache_config_default() {
    let config = SearchCacheConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.max_size, 50);
    assert_eq!(config.ttl_secs, 300);
}

#[test]
fn test_clawhub_config_default() {
    let config = ClawHubConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.base_url, "https://clawhub.ai");
    assert!(config.convex_url.contains("convex.cloud"));
    assert!(config.convex_site_url.is_empty());
    assert_eq!(config.timeout_secs, 0);
}

#[test]
fn test_github_config_default() {
    let config = GitHubConfig::default();
    assert!(!config.enabled);
    assert_eq!(config.base_url, "https://raw.githubusercontent.com");
    assert_eq!(config.timeout_secs, 0);
}

#[test]
fn test_github_source_config_serialization() {
    let config = GitHubSourceConfig {
        name: "test-source".to_string(),
        repo: "org/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_tree".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 30,
        max_size: 1048576,
    };
    let json = serde_json::to_string(&config).unwrap();
    let parsed: GitHubSourceConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.name, "test-source");
    assert_eq!(parsed.branch, "main");
}

#[test]
fn test_github_source_legacy_serialization() {
    let source = GithubSource {
        name: "legacy-source".to_string(),
        url: "https://github.com/org/skills".to_string(),
        branch: "develop".to_string(),
    };
    let json = serde_json::to_string(&source).unwrap();
    let parsed: GithubSource = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed.branch, "develop");
}

#[test]
fn test_validate_skill_identifier_max_length() {
    let slug_64 = "a".repeat(64);
    assert!(validate_skill_identifier(&slug_64).is_ok());
    let slug_65 = "a".repeat(65);
    assert!(validate_skill_identifier(&slug_65).is_err());
}

#[test]
fn test_validate_skill_identifier_double_dot() {
    assert!(validate_skill_identifier("skill..name").is_err());
}

#[test]
fn test_validate_skill_identifier_valid_hyphens() {
    assert!(validate_skill_identifier("my-awesome-skill-123").is_ok());
}

#[test]
fn test_registry_config_roundtrip() {
    let config = RegistryConfig {
        search_cache: SearchCacheConfig {
            enabled: true,
            max_size: 100,
            ttl_secs: 600,
        },
        clawhub: ClawHubConfig {
            enabled: true,
            base_url: "https://custom.clawhub.ai".to_string(),
            convex_url: "https://custom.convex.cloud".to_string(),
            convex_site_url: String::new(),
            timeout_secs: 60,
        },
        github: GitHubConfig {
            enabled: true,
            base_url: "https://custom.github.com".to_string(),
            timeout_secs: 30,
            max_size: 2048000,
        },
        github_sources: vec![],
        max_concurrent_searches: 4,
        github_sources_legacy: vec![],
        modelscope: ModelScopeConfig {
            enabled: false,
            timeout_secs: 0,
        },
    };
    let json = serde_json::to_string_pretty(&config).unwrap();
    let parsed: RegistryConfig = serde_json::from_str(&json).unwrap();
    assert!(parsed.search_cache.enabled);
    assert_eq!(parsed.search_cache.max_size, 100);
    assert!(parsed.clawhub.enabled);
    assert_eq!(parsed.max_concurrent_searches, 4);
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_registry_config_parse_from_json() {
    let json = r#"{
        "search_cache": {"enabled": true, "max_size": 25, "ttl_secs": 120},
        "clawhub": {"enabled": false},
        "github": {"enabled": true, "base_url": "https://example.com", "timeout_secs": 10, "max_size": 500000},
        "github_sources": [],
        "max_concurrent_searches": 3,
        "github_sources_legacy": []
    }"#;
    let config: RegistryConfig = serde_json::from_str(json).unwrap();
    assert!(config.search_cache.enabled);
    assert_eq!(config.search_cache.max_size, 25);
    assert_eq!(config.search_cache.ttl_secs, 120);
    assert!(config.github.enabled);
    assert_eq!(config.github.base_url, "https://example.com");
    assert_eq!(config.github.timeout_secs, 10);
    assert_eq!(config.github.max_size, 500000);
    assert_eq!(config.max_concurrent_searches, 3);
}

#[test]
fn test_registry_config_parse_with_github_sources() {
    let json = r#"{
        "search_cache": {"enabled": false},
        "clawhub": {"enabled": false},
        "github": {"enabled": false},
        "github_sources": [
            {
                "name": "anthropics",
                "repo": "anthropics/skills",
                "enabled": true,
                "branch": "main",
                "index_type": "github_api",
                "index_path": "",
                "skill_path_pattern": "skills/{slug}/SKILL.md",
                "timeout_secs": 30,
                "max_size": 1048576
            }
        ],
        "max_concurrent_searches": 2
    }"#;
    let config: RegistryConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.github_sources.len(), 1);
    assert_eq!(config.github_sources[0].name, "anthropics");
    assert_eq!(config.github_sources[0].repo, "anthropics/skills");
    assert!(config.github_sources[0].enabled);
    assert_eq!(config.github_sources[0].branch, "main");
}

#[test]
fn test_security_check_result_serialization() {
    let result = SecurityCheckResult {
        lint_result: crate::lint::LintResult {
            skill_name: "test".to_string(),
            passed: true,
            score: 0.95,
            warnings: vec![],
        },
        quality_score: Some(crate::quality::QualityScore {
            overall: 85.0,
            security: crate::quality::DimensionScore {
                score: 100.0,
                max: 100.0,
                details: "safe".to_string(),
            },
            completeness: crate::quality::DimensionScore {
                score: 80.0,
                max: 100.0,
                details: "good".to_string(),
            },
            clarity: crate::quality::DimensionScore {
                score: 75.0,
                max: 100.0,
                details: "ok".to_string(),
            },
            testing: crate::quality::DimensionScore {
                score: 85.0,
                max: 100.0,
                details: "good tests".to_string(),
            },
        }),
        blocked: false,
        block_reason: String::new(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: SecurityCheckResult = serde_json::from_str(&json).unwrap();
    assert!(!parsed.blocked);
    assert!(parsed.block_reason.is_empty());
    assert!(parsed.lint_result.passed);
    assert!(parsed.quality_score.is_some());
    assert!((parsed.quality_score.unwrap().overall - 85.0).abs() < 0.01);
}

#[test]
fn test_security_check_result_blocked() {
    let result = SecurityCheckResult {
        lint_result: crate::lint::LintResult {
            skill_name: "malware".to_string(),
            passed: false,
            score: 0.1,
            warnings: vec![],
        },
        quality_score: None,
        blocked: true,
        block_reason: "critical severity issue detected".to_string(),
    };
    let json = serde_json::to_string(&result).unwrap();
    let parsed: SecurityCheckResult = serde_json::from_str(&json).unwrap();
    assert!(parsed.blocked);
    assert!(parsed.block_reason.contains("critical"));
}

#[test]
fn test_skill_info_validate_boundary_name_length() {
    let info = SkillInfo {
        name: "a".repeat(64),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    assert!(info.validate().is_empty());

    let info_over = SkillInfo {
        name: "a".repeat(65),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info_over.validate();
    assert!(errors.iter().any(|e| e.contains("exceeds")));
}

#[test]
fn test_skill_info_validate_valid_hyphenated_names() {
    let info = SkillInfo {
        name: "my-awesome-skill-123".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    assert!(info.validate().is_empty());
}

#[test]
fn test_skill_info_validate_name_with_underscores_fails() {
    let info = SkillInfo {
        name: "my_skill".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("alphanumeric")));
}

#[test]
fn test_skill_info_validate_name_with_spaces_fails() {
    let info = SkillInfo {
        name: "my skill".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("alphanumeric")));
}

#[test]
fn test_skill_info_validate_name_starts_with_hyphen_fails() {
    let info = SkillInfo {
        name: "-skill".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("alphanumeric")));
}

#[test]
fn test_skill_info_validate_name_ends_with_hyphen_fails() {
    let info = SkillInfo {
        name: "skill-".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "valid".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.iter().any(|e| e.contains("alphanumeric")));
}

#[test]
fn test_skill_info_validate_description_boundary_length() {
    let info = SkillInfo {
        name: "valid-name".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "x".repeat(1024),
        lint_score: None,
        has_warnings: false,
    };
    assert!(info.validate().is_empty());

    let info_over = SkillInfo {
        name: "valid-name".to_string(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: "x".repeat(1025),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info_over.validate();
    assert!(errors.iter().any(|e| e.contains("description exceeds")));
}

#[test]
fn test_skill_info_validate_multiple_errors() {
    let info = SkillInfo {
        name: String::new(),
        path: "/test".to_string(),
        source: "local".to_string(),
        description: String::new(),
        lint_score: None,
        has_warnings: false,
    };
    let errors = info.validate();
    assert!(errors.len() >= 2);
    assert!(errors.iter().any(|e| e.contains("name is required")));
    assert!(errors.iter().any(|e| e.contains("description is required")));
}

#[test]
fn test_clawhub_config_parse_from_json() {
    let json = r#"{
        "enabled": true,
        "base_url": "https://custom.clawhub.ai",
        "convex_url": "https://custom.convex.cloud",
        "convex_site_url": "https://custom.convex.site",
        "timeout_secs": 45
    }"#;
    let config: ClawHubConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    assert_eq!(config.base_url, "https://custom.clawhub.ai");
    assert_eq!(config.convex_url, "https://custom.convex.cloud");
    assert_eq!(config.convex_site_url, "https://custom.convex.site");
    assert_eq!(config.timeout_secs, 45);
}

#[test]
fn test_github_config_parse_from_json() {
    let json = r#"{"enabled": true, "base_url": "https://cdn.example.com", "timeout_secs": 20, "max_size": 2000000}"#;
    let config: GitHubConfig = serde_json::from_str(json).unwrap();
    assert!(config.enabled);
    assert_eq!(config.base_url, "https://cdn.example.com");
    assert_eq!(config.timeout_secs, 20);
    assert_eq!(config.max_size, 2000000);
}

#[test]
fn test_github_source_config_default_fields() {
    let json = r#"{"name": "test", "repo": "org/repo"}"#;
    let config: GitHubSourceConfig = serde_json::from_str(json).unwrap();
    assert_eq!(config.name, "test");
    assert_eq!(config.repo, "org/repo");
    assert!(config.enabled); // default true
    assert_eq!(config.branch, "main"); // default main
    assert_eq!(config.index_type, "skills_json"); // default
}

#[test]
fn test_github_source_legacy_parse() {
    let json = r#"{"name": "mysource", "url": "https://github.com/org/skills", "branch": "develop"}"#;
    let source: GithubSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.name, "mysource");
    assert_eq!(source.url, "https://github.com/org/skills");
    assert_eq!(source.branch, "develop");
}

#[test]
fn test_github_source_legacy_default_branch() {
    let json = r#"{"name": "mysource", "url": "https://github.com/org/skills"}"#;
    let source: GithubSource = serde_json::from_str(json).unwrap();
    assert_eq!(source.branch, "main");
}

#[test]
fn test_skill_origin_default_version() {
    let json = r#"{"registry": "github", "slug": "test", "installed_version": "1.0", "installed_at": 12345}"#;
    let origin: SkillOrigin = serde_json::from_str(json).unwrap();
    assert_eq!(origin.version, 1); // default
}

#[test]
fn test_available_skill_minimal_json() {
    let json = r#"{"name": "test"}"#;
    let skill: AvailableSkill = serde_json::from_str(json).unwrap();
    assert_eq!(skill.name, "test");
    assert!(skill.repository.is_empty());
    assert!(skill.description.is_empty());
    assert!(skill.author.is_empty());
    assert!(skill.tags.is_empty());
}

#[test]
fn test_skill_search_result_all_defaults() {
    let json = r#"{"score": 0.5, "slug": "test", "display_name": "Test", "summary": "A test", "registry_name": "reg"}"#;
    let result: SkillSearchResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.score, 0.5);
    assert!(result.version.is_empty());
    assert!(result.source_repo.is_empty());
    assert!(result.download_path.is_empty());
    assert_eq!(result.downloads, 0);
    assert!(!result.truncated);
}

#[test]
fn test_registry_search_result_minimal_json() {
    let json = r#"{"registry_name": "reg", "results": []}"#;
    let result: RegistrySearchResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.registry_name, "reg");
    assert!(result.results.is_empty());
    assert!(!result.truncated);
}

#[test]
fn test_skill_meta_minimal_json() {
    let json = r#"{"slug": "test"}"#;
    let meta: SkillMeta = serde_json::from_str(json).unwrap();
    assert_eq!(meta.slug, "test");
    assert!(meta.display_name.is_empty());
    assert!(meta.summary.is_empty());
    assert!(meta.latest_version.is_empty());
    assert!(!meta.is_malware_blocked);
    assert!(!meta.is_suspicious);
    assert!(meta.registry_name.is_empty());
}

#[test]
fn test_install_result_minimal_json() {
    let json = r#"{"version": "1.0"}"#;
    let result: InstallResult = serde_json::from_str(json).unwrap();
    assert_eq!(result.version, "1.0");
    assert!(!result.is_malware_blocked);
    assert!(!result.is_suspicious);
    assert!(result.summary.is_empty());
}

#[test]
fn test_validate_skill_identifier_with_dot() {
    // Dots are allowed (no rule against them)
    assert!(validate_skill_identifier("skill.name").is_ok());
}

#[test]
fn test_validate_skill_identifier_whitespace_only() {
    assert!(validate_skill_identifier("  ").is_err());
}

#[test]
fn test_validate_skill_identifier_with_double_dot() {
    assert!(validate_skill_identifier("skill..name").is_err()); // ".." anywhere is blocked
}

#[test]
fn test_contains_ci_unicode_extended() {
    assert!(contains_ci("UBUNTU", "ubuntu"));
    assert!(!contains_ci("hello", "HELLO_WORLD"));
}

#[test]
fn test_to_lower_mixed_ascii_unicode() {
    let result = to_lower("Hello123");
    assert_eq!(result, "hello123");
}
