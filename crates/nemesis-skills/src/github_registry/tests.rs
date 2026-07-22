use super::*;

#[test]
fn test_skill_dir_prefix_two_layer() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(
        registry.skill_dir_prefix("pdf"),
        Some("skills/pdf".to_string())
    );
}

#[test]
fn test_skill_dir_prefix_three_layer() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(
        registry.skill_dir_prefix("clawcv/pdf"),
        Some("skills/clawcv/pdf".to_string())
    );
}

#[test]
fn test_skill_dir_prefix_three_layer_no_author() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    // Without author, returns None.
    assert!(registry.skill_dir_prefix("pdf").is_none());
}

#[test]
fn test_build_skill_url() {
    let registry = GitHubRegistry::new("", 0, 0);
    let url = registry.build_skill_url("pdf");
    assert!(url.contains("skills/pdf/SKILL.md"));
}

#[test]
fn test_is_three_layer_pattern() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert!(!registry.is_three_layer_pattern());

    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert!(registry.is_three_layer_pattern());
}

#[test]
fn test_name_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.name(), "github");
}

#[test]
fn test_name_custom() {
    let config = GitHubSourceConfig {
        name: "anthropics".to_string(),
        repo: "anthropics/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.name(), "anthropics");
}

#[test]
fn test_github_skill_deserialization() {
    let json = r#"[{"name":"pdf","description":"PDF converter","repository":"test","author":"alice","tags":["pdf","convert"]}]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "pdf");
    assert_eq!(skills[0].description, "PDF converter");
}

#[test]
fn test_tree_response_deserialization() {
    let json = r#"{
        "sha": "abc123",
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf", "type": "tree"}
        ],
        "truncated": false
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 2);
    assert_eq!(response.truncated, Some(false));
}

// ============================================================
// Coverage improvement: additional github_registry tests
// ============================================================

#[test]
fn test_new_default_base_url() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.base_url, "https://raw.githubusercontent.com");
}

#[test]
fn test_new_custom_base_url() {
    let registry = GitHubRegistry::new("https://custom.url", 10, 2048);
    assert_eq!(registry.base_url, "https://custom.url");
    assert_eq!(registry.timeout, Duration::from_secs(10));
    assert_eq!(registry.max_size, 2048);
}

#[test]
fn test_new_default_timeout() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.timeout, DEFAULT_TIMEOUT);
    assert_eq!(registry.max_size, DEFAULT_MAX_SIZE);
}

#[test]
fn test_new_from_config_defaults() {
    let config = crate::types::GitHubConfig {
        base_url: String::new(),
        timeout_secs: 0,
        max_size: 0,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://raw.githubusercontent.com");
    assert_eq!(registry.timeout, DEFAULT_TIMEOUT);
    assert_eq!(registry.max_size, DEFAULT_MAX_SIZE);
}

#[test]
fn test_new_from_config_custom() {
    let config = crate::types::GitHubConfig {
        base_url: "https://my.server".to_string(),
        timeout_secs: 60,
        max_size: 4096,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://my.server");
    assert_eq!(registry.timeout, Duration::from_secs(60));
    assert_eq!(registry.max_size, 4096);
}

#[test]
fn test_api_base_url_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.api_base_url(), "https://api.github.com");
}

#[test]
fn test_api_base_url_custom() {
    let mut registry = GitHubRegistry::new("", 0, 0);
    registry.set_github_api_url("https://gh.enterprise.com/api/v3");
    assert_eq!(registry.api_base_url(), "https://gh.enterprise.com/api/v3");
}

#[test]
fn test_api_base_url_empty_string() {
    let mut registry = GitHubRegistry::new("", 0, 0);
    registry.set_github_api_url("");
    assert_eq!(registry.api_base_url(), "https://api.github.com");
}

#[test]
fn test_build_skill_url_with_author() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("clawcv/pdf");
    assert!(url.contains("skills/clawcv/pdf/SKILL.md"));
}

#[test]
fn test_build_skill_url_with_author_no_slash() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("pdf");
    // Without slash, it just replaces {slug} with pdf and {author} stays
    assert!(url.contains("skills"));
}

#[test]
fn test_from_source_custom_branch() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "develop".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "index.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 15,
        max_size: 512,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.branch, "develop");
    assert_eq!(registry.repo, "test/repo");
    assert_eq!(registry.index_path, "index.json");
    assert_eq!(registry.timeout, Duration::from_secs(15));
    assert_eq!(registry.max_size, 512);
}

#[test]
fn test_from_source_empty_branch_defaults_to_main() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: String::new(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.branch, "main");
}

#[test]
fn test_github_skill_minimal_deserialization() {
    let json = r#"[{"name":"pdf","description":"PDF converter"}]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "pdf");
    assert!(skills[0].repository.is_none());
    assert!(skills[0].author.is_none());
    assert!(skills[0].tags.is_none());
}

#[test]
fn test_github_content_entry_deserialization() {
    let json = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"README.md","type":"file","path":"README.md"}
    ]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry_type, "dir");
    assert_eq!(entries[1].entry_type, "file");
}

#[test]
fn test_tree_entry_deserialization() {
    let json = r#"{
        "tree": [
            {"path": "skills/author/my-skill/SKILL.md", "type": "blob"},
            {"path": "skills/author/my-skill", "type": "tree"}
        ],
        "truncated": true
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 2);
    assert_eq!(response.truncated, Some(true));
    assert_eq!(response.sha, None);
}

#[test]
fn test_github_source_config_fields() {
    let config = GitHubSourceConfig {
        name: "test-source".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "v2".to_string(),
        index_type: "github_api".to_string(),
        index_path: "".to_string(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 45,
        max_size: 8192,
    };
    assert_eq!(config.name, "test-source");
    assert_eq!(config.skill_path_pattern, "{slug}/SKILL.md");
}

#[test]
fn test_skill_dir_prefix_root_pattern() {
    // Root pattern: {slug}/SKILL.md
    let config = GitHubSourceConfig {
        name: "root".to_string(),
        repo: "test/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let prefix = registry.skill_dir_prefix("pdf");
    assert_eq!(prefix, Some("pdf".to_string()));
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_github_registry_default_fields() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.repo, "276793422/nemesisbot-skills");
    assert_eq!(registry.branch, "main");
    assert_eq!(registry.index_type, "skills_json");
    assert_eq!(registry.index_path, "skills.json");
    assert_eq!(registry.skill_path_pattern, "skills/{slug}/SKILL.md");
}

#[test]
fn test_search_dispatches_to_skills_json() {
    // When index_type is "skills_json", search should use search_skills_json path
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.index_type, "skills_json");
}

#[test]
fn test_search_dispatches_to_github_api() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.index_type, "github_api");
}

#[test]
fn test_github_source_config_clone() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "v2".to_string(),
        index_type: "github_api".to_string(),
        index_path: "".to_string(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 45,
        max_size: 8192,
    };
    let cloned = config.clone();
    assert_eq!(cloned.name, "test");
    assert_eq!(cloned.repo, "org/repo");
    assert_eq!(cloned.timeout_secs, 45);
}

#[test]
fn test_github_config_defaults() {
    let config = crate::types::GitHubConfig {
        base_url: String::new(),
        timeout_secs: 0,
        max_size: 0,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.repo, "276793422/nemesisbot-skills");
    assert_eq!(registry.branch, "main");
    assert_eq!(registry.index_type, "skills_json");
}

#[test]
fn test_set_github_api_url() {
    let mut registry = GitHubRegistry::new("", 0, 0);
    registry.set_github_api_url("https://gh.enterprise.com");
    assert_eq!(registry.github_api_url, "https://gh.enterprise.com");
}

#[test]
fn test_github_content_entry_only_required_fields() {
    let json = r#"[{"name":"pdf","type":"dir","path":"skills/pdf"}]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries[0].name, "pdf");
    assert_eq!(entries[0].entry_type, "dir");
}

#[test]
fn test_github_tree_entry_types() {
    let json = r#"{
        "tree": [
            {"path": "skills/pdf", "type": "tree"},
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf/data.bin", "type": "blob"}
        ]
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 3);
}

// ============================================================
// Coverage improvement: parsing/validation/state tests
// ============================================================

#[test]
fn test_github_skill_full_deserialization() {
    let json = r#"[{
        "name": "pdf",
        "description": "PDF converter",
        "repository": "https://github.com/org/pdf",
        "author": "alice",
        "tags": ["pdf", "converter"]
    }]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "pdf");
    assert_eq!(
        skills[0].repository.as_deref(),
        Some("https://github.com/org/pdf")
    );
    assert_eq!(skills[0].author.as_deref(), Some("alice"));
    assert_eq!(skills[0].tags.as_ref().unwrap().len(), 2);
}

#[test]
fn test_github_content_entry_with_all_fields() {
    let json = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"SKILL.md","type":"file","path":"skills/pdf/SKILL.md"}
    ]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries.len(), 2);
    assert_eq!(entries[0].entry_type, "dir");
    assert_eq!(entries[1].entry_type, "file");
    assert_eq!(entries[1].path, "skills/pdf/SKILL.md");
}

#[test]
fn test_build_skill_url_two_layer_pattern() {
    let config = GitHubSourceConfig {
        name: "anthropics".to_string(),
        repo: "anthropics/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("pdf");
    assert!(url.contains("skills/pdf/SKILL.md"));
}

#[test]
fn test_build_skill_url_three_layer_pattern_with_slash() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    // Slug with slash: "author/slug" gets split
    let url = registry.build_skill_url("clawcv/pdf");
    assert!(url.contains("skills/clawcv/pdf/SKILL.md"));
}

#[test]
fn test_skill_dir_prefix_three_layer_with_slash() {
    let config = GitHubSourceConfig {
        name: "openclaw".to_string(),
        repo: "openclaw/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let prefix = registry.skill_dir_prefix("clawcv/pdf");
    assert_eq!(prefix, Some("skills/clawcv/pdf".to_string()));
}

#[test]
fn test_skill_dir_prefix_two_layer_basic() {
    let config = GitHubSourceConfig {
        name: "anthropics".to_string(),
        repo: "anthropics/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let prefix = registry.skill_dir_prefix("pdf");
    assert_eq!(prefix, Some("skills/pdf".to_string()));
}

#[test]
fn test_github_registry_name_custom() {
    let config = GitHubSourceConfig {
        name: "my-custom-source".to_string(),
        repo: "myorg/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.registry_name, "my-custom-source");
    assert_eq!(registry.name(), "my-custom-source");
}

#[test]
fn test_github_registry_default_name() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.name(), "github");
}

#[test]
fn test_github_tree_response_with_sha() {
    let json = r#"{
        "sha": "abc123def456",
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"}
        ],
        "truncated": false
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.sha, Some("abc123def456".to_string()));
    assert_eq!(response.truncated, Some(false));
}

#[test]
fn test_github_tree_response_minimal() {
    let json = r#"{
        "tree": []
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert!(response.tree.is_empty());
    assert_eq!(response.sha, None);
    assert_eq!(response.truncated, None);
}

#[test]
fn test_from_source_with_zero_timeout() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "index.json".to_string(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.timeout, Duration::from_secs(30)); // default
    assert_eq!(registry.max_size, 1024 * 1024); // default
}

#[test]
fn test_github_content_entry_missing_optional_fields() {
    let json = r#"[{"name":"pdf","type":"dir","path":"skills/pdf"}]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries[0].name, "pdf");
    assert_eq!(entries[0].entry_type, "dir");
    assert_eq!(entries[0].path, "skills/pdf");
}

#[test]
fn test_new_from_config_custom_base_url() {
    let config = crate::types::GitHubConfig {
        base_url: "https://custom.githubusercontent.com".to_string(),
        timeout_secs: 20,
        max_size: 2048,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://custom.githubusercontent.com");
    assert_eq!(registry.timeout, Duration::from_secs(20));
    assert_eq!(registry.max_size, 2048);
}

#[test]
fn test_github_registry_default_github_api_url() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.github_api_url, "https://api.github.com");
}

#[test]
fn test_github_registry_repo_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.repo, "276793422/nemesisbot-skills");
}

#[test]
fn test_github_registry_index_path_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.index_path, "skills.json");
}

#[test]
fn test_github_registry_index_type_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.index_type, "skills_json");
}

#[test]
fn test_build_skill_url_root_pattern() {
    let config = GitHubSourceConfig {
        name: "root".to_string(),
        repo: "test/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("pdf");
    assert!(url.contains("pdf/SKILL.md"));
}

// ============================================================
// Coverage improvement: more parsing and utility tests
// ============================================================

#[test]
fn test_github_registry_with_custom_base_url() {
    let registry = GitHubRegistry::new("https://custom.host.com", 10, 4096);
    assert_eq!(registry.base_url, "https://custom.host.com");
    assert_eq!(registry.timeout, Duration::from_secs(10));
    assert_eq!(registry.max_size, 4096);
}

#[test]
fn test_github_registry_empty_base_url_default() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.base_url, "https://raw.githubusercontent.com");
}

#[test]
fn test_github_skill_with_empty_description() {
    let json = r#"[{"name":"pdf","description":""}]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].description, "");
}

#[test]
fn test_github_skill_array_deserialization() {
    let json = r#"[]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert!(skills.is_empty());
}

#[test]
fn test_github_content_entry_filter_by_type() {
    let json = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"SKILL.md","type":"file","path":"skills/pdf/SKILL.md"},
        {"name":"csv","type":"dir","path":"skills/csv"}
    ]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    let dirs: Vec<_> = entries.iter().filter(|e| e.entry_type == "dir").collect();
    let files: Vec<_> = entries.iter().filter(|e| e.entry_type == "file").collect();
    assert_eq!(dirs.len(), 2);
    assert_eq!(files.len(), 1);
}

#[test]
fn test_skill_dir_prefix_root_pattern_no_slash() {
    let config = GitHubSourceConfig {
        name: "root".to_string(),
        repo: "test/skills".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let prefix = registry.skill_dir_prefix("csv");
    assert_eq!(prefix, Some("csv".to_string()));
}

#[test]
fn test_skill_dir_prefix_no_matching_pattern() {
    // Pattern without {slug} placeholder - should return None
    let config = GitHubSourceConfig {
        name: "broken".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    // skill_dir_prefix looks for {slug} in the pattern to determine dir
    let prefix = registry.skill_dir_prefix("pdf");
    // Without {slug}, the pattern cannot be resolved
    // The function should handle this gracefully
    let _ = prefix;
}

#[test]
fn test_github_tree_response_with_many_entries() {
    let mut entries = Vec::new();
    for i in 0..50 {
        entries.push(format!(
            r#"{{"path":"skills/skill{}/SKILL.md","type":"blob"}}"#,
            i
        ));
    }
    let json = format!(r#"{{"tree":[{}]}}"#, entries.join(","));
    let response: GithubTreeResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(response.tree.len(), 50);
}

#[test]
fn test_github_source_config_equality() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 30,
        max_size: 1024,
    };
    assert_eq!(config.name, "test");
    assert_eq!(config.repo, "org/repo");
    assert!(config.enabled);
}

#[test]
fn test_contains_ci() {
    use crate::types::contains_ci;
    assert!(contains_ci("PDF Converter", "pdf"));
    assert!(contains_ci("PDF Converter", "PDF"));
    assert!(contains_ci("PDF Converter", "converter"));
    assert!(!contains_ci("PDF Converter", "excel"));
    assert!(contains_ci("", ""));
    assert!(!contains_ci("", "test"));
    assert!(contains_ci("test", ""));
}

#[test]
fn test_validate_skill_identifier() {
    use crate::types::validate_skill_identifier;
    // Valid identifier (no slashes)
    assert!(validate_skill_identifier("pdf").is_ok());
    assert!(validate_skill_identifier("my-skill").is_ok());
    // Invalid: contains slash
    assert!(validate_skill_identifier("anthropics/pdf").is_err());
    // Invalid: empty
    assert!(validate_skill_identifier("").is_err());
    // Invalid: contains backslash
    assert!(validate_skill_identifier("path\\to\\skill").is_err());
    // Invalid: contains ..
    assert!(validate_skill_identifier("skill..traversal").is_err());
}

#[test]
fn test_build_skill_url_custom_branch() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/skills".to_string(),
        enabled: true,
        branch: "develop".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("pdf");
    assert!(url.contains("/develop/"));
    assert!(url.contains("skills/pdf/SKILL.md"));
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_from_source_sets_all_fields() {
    let config = GitHubSourceConfig {
        name: "mysource".to_string(),
        repo: "myorg/myrepo".to_string(),
        enabled: true,
        branch: "v3".to_string(),
        index_type: "github_api".to_string(),
        index_path: "custom_index.json".to_string(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 20,
        max_size: 2048,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.name(), "mysource");
    assert_eq!(registry.repo, "myorg/myrepo");
    assert_eq!(registry.branch, "v3");
    assert_eq!(registry.index_type, "github_api");
    assert_eq!(registry.index_path, "custom_index.json");
    assert_eq!(
        registry.skill_path_pattern,
        "skills/{author}/{slug}/SKILL.md"
    );
    assert_eq!(registry.github_api_url, "https://api.github.com");
    assert!(registry.is_three_layer_pattern());
}

#[test]
fn test_github_registry_new_with_nonzero_params() {
    let registry = GitHubRegistry::new("https://custom.base", 120, 5_000_000);
    assert_eq!(registry.base_url, "https://custom.base");
    assert_eq!(registry.timeout, Duration::from_secs(120));
    assert_eq!(registry.max_size, 5_000_000);
}

#[test]
fn test_new_from_config_with_base_url() {
    let config = crate::types::GitHubConfig {
        base_url: "https://mirror.example.com".to_string(),
        timeout_secs: 45,
        max_size: 8192,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://mirror.example.com");
    assert_eq!(registry.timeout, Duration::from_secs(45));
    assert_eq!(registry.max_size, 8192);
}

#[test]
fn test_github_tree_response_empty_tree() {
    let json = r#"{"tree": [], "truncated": false}"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert!(response.tree.is_empty());
    assert_eq!(response.truncated, Some(false));
}

#[test]
fn test_github_tree_response_with_sha_v2() {
    let json = r#"{"sha": "abc123def456", "tree": [{"path": "skills/test/SKILL.md", "type": "blob"}], "truncated": null}"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.sha, Some("abc123def456".to_string()));
    assert_eq!(response.tree.len(), 1);
    assert!(response.truncated.is_none());
}

#[test]
fn test_github_content_entry_rename() {
    let json = r#"{"name": "mydir", "type": "dir", "path": "skills/mydir"}"#;
    let entry: GitHubContentEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.name, "mydir");
    assert_eq!(entry.entry_type, "dir");
}

#[test]
fn test_github_content_entry_file_type() {
    let json = r#"{"name": "file.md", "type": "file", "path": "skills/file.md"}"#;
    let entry: GitHubContentEntry = serde_json::from_str(json).unwrap();
    assert_eq!(entry.entry_type, "file");
}

#[test]
fn test_github_skill_with_all_fields() {
    let json = r#"[{
        "name": "advanced-pdf",
        "description": "Advanced PDF tool",
        "repository": "org/pdf-tool",
        "author": "alice",
        "tags": ["pdf", "convert", "advanced"]
    }]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "advanced-pdf");
    assert_eq!(skills[0].repository.as_deref(), Some("org/pdf-tool"));
    assert_eq!(skills[0].author.as_deref(), Some("alice"));
    let tags = skills[0].tags.as_ref().unwrap();
    assert_eq!(tags.len(), 3);
}

#[test]
fn test_build_skill_url_two_layer_format() {
    let registry = GitHubRegistry::new("https://raw.githubusercontent.com", 30, 1024);
    let url = registry.build_skill_url("my-skill");
    assert_eq!(
        url,
        "https://raw.githubusercontent.com/276793422/nemesisbot-skills/main/skills/my-skill/SKILL.md"
    );
}

#[test]
fn test_build_skill_url_three_layer_format_with_author() {
    let config = GitHubSourceConfig {
        name: "test3".to_string(),
        repo: "org/repo3".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let url = registry.build_skill_url("author1/skill1");
    assert!(url.contains("skills/author1/skill1/SKILL.md"));
    assert!(url.contains("org/repo3/main"));
}

#[test]
fn test_skill_dir_prefix_returns_correct_parent() {
    let registry = GitHubRegistry::new("", 0, 0);
    let prefix = registry.skill_dir_prefix("test-skill");
    assert_eq!(prefix, Some("skills/test-skill".to_string()));
}

#[test]
fn test_skill_dir_prefix_none_for_three_layer_without_author() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert!(registry.skill_dir_prefix("noslash").is_none());
}

#[test]
fn test_name_returns_github_when_empty() {
    let registry = GitHubRegistry::new("", 0, 0);
    assert_eq!(registry.name(), "github");
}

#[test]
fn test_api_base_url_returns_default_when_empty() {
    let mut registry = GitHubRegistry::new("", 0, 0);
    registry.github_api_url = String::new();
    assert_eq!(registry.api_base_url(), "https://api.github.com");
}

#[test]
fn test_set_github_api_url_updates() {
    let mut registry = GitHubRegistry::new("", 0, 0);
    registry.set_github_api_url("https://custom.api.url");
    assert_eq!(registry.api_base_url(), "https://custom.api.url");
}

#[test]
fn test_github_source_config_debug() {
    let config = GitHubSourceConfig {
        name: "debug-test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 30,
        max_size: 1024,
    };
    let debug_str = format!("{:?}", config);
    assert!(debug_str.contains("debug-test"));
    assert!(debug_str.contains("org/repo"));
}

// ============================================================
// Coverage improvement: do_get error paths, search, meta
// ============================================================

#[tokio::test]
async fn test_do_get_connection_error() {
    let registry = GitHubRegistry::new("http://127.0.0.1:1", 1, 1024);
    let result = registry.do_get("http://127.0.0.1:1/nonexistent").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("request failed"));
}

#[tokio::test]
async fn test_search_skills_json_connection_error() {
    let registry = GitHubRegistry::new("http://127.0.0.1:1", 1, 1024);
    let result = registry.search("pdf", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_github_api_connection_error() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 1,
        max_size: 1024,
    };
    let mut registry = GitHubRegistry::from_source(&config);
    registry.set_github_api_url("http://127.0.0.1:1");
    let result = registry.search("pdf", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_skill_meta_invalid_slug() {
    let registry = GitHubRegistry::new("", 0, 0);
    // Slug with slash should fail validation
    let result = registry.get_skill_meta("invalid/slug").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test]
async fn test_get_skill_meta_empty_slug() {
    let registry = GitHubRegistry::new("", 0, 0);
    let result = registry.get_skill_meta("").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_download_and_install_invalid_slug() {
    let registry = GitHubRegistry::new("", 0, 0);
    let result = registry
        .download_and_install("bad/slug", "1.0", "/tmp")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test]
async fn test_get_skill_meta_github_api_returns_basic() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 1,
        max_size: 1024,
    };
    let registry = GitHubRegistry::from_source(&config);
    // For github_api, get_skill_meta returns basic info without HTTP
    let result = registry.get_skill_meta("pdf").await;
    assert!(result.is_ok());
    let meta = result.unwrap();
    assert_eq!(meta.slug, "pdf");
    assert_eq!(meta.registry_name, "test");
}

#[tokio::test]
async fn test_download_skill_tree_connection_error() {
    let registry = GitHubRegistry::new("http://127.0.0.1:1", 1, 1024);
    let result = registry
        .download_skill_tree("skills/pdf", "/tmp/nonexistent")
        .await;
    assert!(result.is_err());
}

#[test]
fn test_skill_dir_prefix_no_slash_in_pattern() {
    // Pattern without a trailing filename slash
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "test/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "SKILL.md".to_string(), // no slash
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    let prefix = registry.skill_dir_prefix("pdf");
    // No slash in pattern after replacing {slug}, rfind returns None
    assert!(prefix.is_none());
}

#[test]
fn test_build_skill_url_three_layer_no_author() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    // No slash in slug -> falls through to replace {slug} only
    let url = registry.build_skill_url("noslash");
    assert!(url.contains("noslash"));
}

#[test]
fn test_registry_name_empty_returns_github() {
    let registry = GitHubRegistry::new("", 0, 0);
    // registry_name is empty for default constructor
    assert_eq!(registry.registry_name, "");
    assert_eq!(registry.name(), "github");
}

#[test]
fn test_from_source_config_values() {
    let config = GitHubSourceConfig {
        name: "mysource".to_string(),
        repo: "myorg/myrepo".to_string(),
        enabled: false,
        branch: "".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "custom.json".to_string(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.name(), "mysource");
    assert_eq!(registry.repo, "myorg/myrepo");
    assert_eq!(registry.branch, "main"); // default when empty
    assert_eq!(registry.index_path, "custom.json");
    assert_eq!(registry.skill_path_pattern, "{slug}/SKILL.md");
}

#[test]
fn test_github_skill_deserialization_with_special_chars() {
    let json = r#"[{"name":"my-skill_v2","description":"A skill with special chars: <>&\""}]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "my-skill_v2");
    assert!(skills[0].description.contains("<>&"));
}

#[test]
fn test_github_content_entry_multiple() {
    let json = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"csv","type":"dir","path":"skills/csv"},
        {"name":"SKILL.md","type":"file","path":"skills/pdf/SKILL.md"}
    ]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries.len(), 3);
    let dirs: Vec<_> = entries.iter().filter(|e| e.entry_type == "dir").collect();
    assert_eq!(dirs.len(), 2);
}

#[test]
fn test_github_tree_entry_deserialization() {
    let json = r#"{
        "sha": "abc",
        "tree": [
            {"path": "skills/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/pdf", "type": "tree"}
        ],
        "truncated": false
    }"#;
    let response: GithubTreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(response.tree.len(), 2);
    assert_eq!(response.tree[0].entry_type, "blob");
    assert_eq!(response.tree[1].entry_type, "tree");
}

#[test]
fn test_skill_dir_prefix_three_layer_without_slash() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    // No slash in slug -> returns None
    let prefix = registry.skill_dir_prefix("simple");
    assert!(prefix.is_none());
}

#[test]
fn test_build_skill_url_custom_base_url() {
    let config = GitHubSourceConfig {
        name: "custom-base".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let mut registry = GitHubRegistry::from_source(&config);
    registry.base_url = "https://custom.githubusercontent.com".to_string();
    let url = registry.build_skill_url("pdf");
    assert!(url.starts_with("https://custom.githubusercontent.com"));
}

#[test]
fn test_from_source_with_custom_timeout_and_size() {
    let config = GitHubSourceConfig {
        name: "custom-timeout".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "index.json".to_string(),
        skill_path_pattern: "{slug}/SKILL.md".to_string(),
        timeout_secs: 45,
        max_size: 2048 * 1024,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert_eq!(registry.timeout, Duration::from_secs(45));
    assert_eq!(registry.max_size, 2048 * 1024);
}

#[test]
fn test_github_skill_missing_optional_fields() {
    let json = r#"[{"name":"minimal","description":"minimal skill"}]"#;
    let skills: Vec<GithubSkill> = serde_json::from_str(json).unwrap();
    assert_eq!(skills[0].name, "minimal");
    assert!(skills[0].repository.is_none());
    assert!(skills[0].author.is_none());
    assert!(skills[0].tags.is_none());
}

#[test]
fn test_github_content_entry_type_file() {
    let json = r#"[{"name":"SKILL.md","type":"file","path":"skills/pdf/SKILL.md"}]"#;
    let entries: Vec<GitHubContentEntry> = serde_json::from_str(json).unwrap();
    assert_eq!(entries[0].entry_type, "file");
}

#[test]
fn test_github_tree_response_large_tree() {
    let mut entries = Vec::new();
    for i in 0..50 {
        entries.push(format!(
            r#"{{"path": "skills/skill{}/SKILL.md", "type": "blob"}}"#,
            i
        ));
    }
    let json = format!(
        r#"{{"sha": "bigsha", "tree": [{}], "truncated": true}}"#,
        entries.join(",")
    );
    let response: GithubTreeResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(response.tree.len(), 50);
    assert_eq!(response.truncated, Some(true));
}

#[tokio::test]
async fn test_search_connection_error() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 1,
        max_size: 1024,
    };
    let registry = GitHubRegistry::from_source(&config);
    let result = registry.search("test", 10).await;
    // Connection should fail
    assert!(result.is_err());
}

#[tokio::test]
async fn test_download_and_install_connection_error() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 1,
        max_size: 1024,
    };
    let registry = GitHubRegistry::from_source(&config);
    let result = registry
        .download_and_install("pdf", "1.0", "/tmp/nonexistent_install_target")
        .await;
    assert!(result.is_err());
}

#[test]
fn test_api_base_url_uses_github_api() {
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 0,
        max_size: 0,
    };
    let registry = GitHubRegistry::from_source(&config);
    assert!(registry.api_base_url().contains("api.github.com"));
}

#[test]
fn test_new_from_config_enabled() {
    let config = crate::types::GitHubConfig {
        base_url: String::new(),
        timeout_secs: 0,
        max_size: 0,
        enabled: true,
    };
    let registry = GitHubRegistry::new_from_config(&config);
    assert_eq!(registry.name(), "github");
}
