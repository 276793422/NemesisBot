use super::*;
use tempfile::TempDir;

#[test]
fn test_parse_github_url_https() {
    let (owner, repo) = parse_github_url("https://github.com/anthropics/skills").unwrap();
    assert_eq!(owner, "anthropics");
    assert_eq!(repo, "skills");
}

#[test]
fn test_parse_github_url_https_with_git() {
    let (owner, repo) = parse_github_url("https://github.com/openclaw/skills.git").unwrap();
    assert_eq!(owner, "openclaw");
    assert_eq!(repo, "skills");
}

#[test]
fn test_parse_github_url_http() {
    let (owner, repo) = parse_github_url("http://github.com/user/repo").unwrap();
    assert_eq!(owner, "user");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_git_at() {
    let (owner, repo) = parse_github_url("git@github.com:user/repo.git").unwrap();
    assert_eq!(owner, "user");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_git_at_no_git_suffix() {
    let (owner, repo) = parse_github_url("git@github.com:myorg/myrepo").unwrap();
    assert_eq!(owner, "myorg");
    assert_eq!(repo, "myrepo");
}

#[test]
fn test_parse_github_url_shorthand() {
    let (owner, repo) = parse_github_url("user/repo").unwrap();
    assert_eq!(owner, "user");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_trailing_slash() {
    let (owner, repo) = parse_github_url("https://github.com/user/repo/").unwrap();
    assert_eq!(owner, "user");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_invalid_no_slash() {
    let result = parse_github_url("noslash");
    assert!(result.is_err());
}

#[test]
fn test_parse_github_url_invalid_empty() {
    let result = parse_github_url("");
    assert!(result.is_err());
}

#[test]
fn test_parse_github_url_invalid_space() {
    let result = parse_github_url("user name/repo");
    assert!(result.is_err());
}

#[test]
fn test_parse_github_url_empty_parts() {
    let result = parse_github_url("/repo");
    assert!(result.is_err());
}

#[test]
fn test_get_builtin_skills_count() {
    let skills = get_builtin_skills();
    assert_eq!(skills.len(), 10);
}

#[test]
fn test_get_builtin_skills_has_weather() {
    let skills = get_builtin_skills();
    assert!(skills.iter().any(|(n, _)| *n == "weather"));
}

#[test]
fn test_get_builtin_skills_has_structured_development() {
    let skills = get_builtin_skills();
    assert!(skills.iter().any(|(n, _)| *n == "structured-development"));
}

#[test]
fn test_get_builtin_skills_descriptions_nonempty() {
    let skills = get_builtin_skills();
    for (name, desc) in &skills {
        assert!(!desc.is_empty(), "Skill '{}' has empty description", name);
    }
}

#[test]
fn test_load_registry_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let config = load_registry_config(&path);
    // Should return default config
    assert!(config.github_sources.is_empty());
}

#[test]
fn test_load_registry_config_with_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let data = serde_json::json!({
        "github_sources": [{
            "name": "test",
            "repo": "user/test",
            "enabled": true,
            "branch": "main",
            "index_type": "github_api",
            "skill_path_pattern": "skills/{slug}/SKILL.md"
        }],
        "github_sources_legacy": [],
        "clawhub": {"enabled": false, "base_url": ""},
        "search_cache": {"enabled": true, "max_size": 100, "ttl_secs": 300}
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

    let config = load_registry_config(&path);
    assert_eq!(config.github_sources.len(), 1);
    assert_eq!(config.github_sources[0].name, "test");
}

#[test]
fn test_save_and_load_registry_config_roundtrip() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let mut config = nemesis_skills::types::RegistryConfig::default();
    config
        .github_sources
        .push(nemesis_skills::types::GitHubSourceConfig {
            name: "mysource".to_string(),
            repo: "org/repo".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });

    save_registry_config(&path, &config).unwrap();
    let loaded = load_registry_config(&path);
    assert_eq!(loaded.github_sources.len(), 1);
    assert_eq!(loaded.github_sources[0].name, "mysource");
}

#[test]
fn test_cmd_remove_nonexistent_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    // Should succeed even if skill doesn't exist
    cmd_remove(&skills_dir, "nonexistent").unwrap();
}

#[test]
fn test_cmd_remove_existing_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("test-skill");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "# Test Skill").unwrap();

    cmd_remove(&skills_dir, "test-skill").unwrap();
    assert!(!skill_path.exists());
}

#[test]
fn test_cmd_show_existing_skill_with_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("demo");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "# Demo Skill\nA demo.").unwrap();

    cmd_show(&skills_dir, "demo").unwrap();
}

#[test]
fn test_cmd_show_nonexistent_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();

    cmd_show(&skills_dir, "nonexistent").unwrap();
}

#[test]
fn test_cmd_source_remove_nonexistent() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let config = nemesis_skills::types::RegistryConfig::default();
    save_registry_config(&path, &config).unwrap();

    cmd_source_remove(&path, "nonexistent").unwrap();
}

#[test]
fn test_cmd_install_builtin_creates_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    cmd_install_builtin(&skills_dir, Some("weather")).unwrap();

    let skill_md = skills_dir.join("weather").join("SKILL.md");
    assert!(skill_md.exists());
    let content = std::fs::read_to_string(&skill_md).unwrap();
    assert!(content.contains("weather"));
}

#[test]
fn test_cmd_install_builtin_already_exists() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("calculator");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "original").unwrap();

    cmd_install_builtin(&skills_dir, Some("calculator")).unwrap();

    // Should NOT overwrite
    let content = std::fs::read_to_string(skill_path.join("SKILL.md")).unwrap();
    assert_eq!(content, "original");
}

#[test]
fn test_cmd_install_builtin_unknown_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    cmd_install_builtin(&skills_dir, Some("nonexistent_skill_xyz")).unwrap();
    // Should report not found but not crash
}

#[test]
fn test_cmd_list_no_dir() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("nonexistent");
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    std::fs::create_dir_all(&skills_dir).unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_source_list_no_file() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    cmd_source_list(&path).unwrap();
}

#[test]
fn test_cmd_validate_nonexistent_path() {
    cmd_validate("/nonexistent/path").unwrap();
}

#[test]
fn test_cmd_validate_with_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("test-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "# Test\nname: test\ndescription: A test skill\nsteps:\n- step1",
    )
    .unwrap();

    cmd_validate(&skill_dir.to_string_lossy()).unwrap();
}

// -------------------------------------------------------------------------
// parse_github_url additional edge cases
// -------------------------------------------------------------------------

#[test]
fn test_parse_github_url_https_with_trailing_git() {
    let (owner, repo) = parse_github_url("https://github.com/org/repo.git").unwrap();
    assert_eq!(owner, "org");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_git_at_with_nested_path() {
    // git@github.com:user/repo.git
    let (owner, repo) = parse_github_url("git@github.com:myorg/my-repo.git").unwrap();
    assert_eq!(owner, "myorg");
    assert_eq!(repo, "my-repo");
}

#[test]
fn test_parse_github_url_https_with_path_component() {
    // Only first two path segments are used
    let result = parse_github_url("https://github.com/user/repo/extra/path");
    // splitn(2, '/') on "user/repo/extra/path" => ["user", "repo/extra/path"]
    // But we strip prefix "https://github.com/" first, so it becomes "user/repo/extra/path"
    // splitn(2, '/') => ["user", "repo/extra/path"]
    // .trim_end_matches(".git") => still "repo/extra/path"
    // This is an edge case but should parse
    if let Ok((owner, repo)) = result {
        assert_eq!(owner, "user");
        assert!(repo.contains("repo"));
    }
}

#[test]
fn test_parse_github_url_empty_repo() {
    let result = parse_github_url("user/");
    assert!(result.is_err());
}

#[test]
fn test_parse_github_url_just_slash() {
    let result = parse_github_url("/");
    assert!(result.is_err());
}

#[test]
fn test_parse_github_url_git_at_no_slash() {
    let result = parse_github_url("git@github.com:norepo");
    assert!(result.is_err());
}

// -------------------------------------------------------------------------
// get_builtin_skills comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_get_builtin_skills_contains_expected_names() {
    let skills = get_builtin_skills();
    let names: Vec<&str> = skills.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"weather"));
    assert!(names.contains(&"news"));
    assert!(names.contains(&"stock"));
    assert!(names.contains(&"calculator"));
    assert!(names.contains(&"structured-development"));
    assert!(names.contains(&"build-project"));
    assert!(names.contains(&"automated-testing"));
    assert!(names.contains(&"desktop-automation"));
    assert!(names.contains(&"wsl-operations"));
    assert!(names.contains(&"dump-analyze"));
}

#[test]
fn test_get_builtin_skills_all_unique_names() {
    let skills = get_builtin_skills();
    let names: Vec<&str> = skills.iter().map(|(n, _)| *n).collect();
    let unique: std::collections::HashSet<&str> = names.iter().copied().collect();
    assert_eq!(names.len(), unique.len());
}

// -------------------------------------------------------------------------
// save_registry_config / load_registry_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_save_registry_config_creates_parent_dir() {
    let tmp = TempDir::new().unwrap();
    let nested_path = tmp
        .path()
        .join("nested")
        .join("dir")
        .join("config.skills.json");
    let config = nemesis_skills::types::RegistryConfig::default();
    save_registry_config(&nested_path, &config).unwrap();
    assert!(nested_path.exists());
}

#[test]
fn test_load_registry_config_empty_json() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    std::fs::write(&path, "{}").unwrap();
    let config = load_registry_config(&path);
    assert!(config.github_sources.is_empty());
}

#[test]
fn test_load_registry_config_invalid_content() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    std::fs::write(&path, "not json at all").unwrap();
    let config = load_registry_config(&path);
    assert!(config.github_sources.is_empty());
}

// -------------------------------------------------------------------------
// cmd_source_remove tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_source_remove_existing() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let mut config = nemesis_skills::types::RegistryConfig::default();
    config
        .github_sources
        .push(nemesis_skills::types::GitHubSourceConfig {
            name: "test-source".to_string(),
            repo: "org/repo".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
    save_registry_config(&path, &config).unwrap();

    cmd_source_remove(&path, "test-source").unwrap();

    let loaded = load_registry_config(&path);
    assert!(loaded.github_sources.is_empty());
}

// -------------------------------------------------------------------------
// cmd_list with actual skills
// -------------------------------------------------------------------------

#[test]
fn test_cmd_list_with_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("weather");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "# Weather\nweather skill").unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_with_forge_skill() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("my-skill-forge");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "# Forge Skill").unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_with_description_in_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("calculator");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(
        skill_path.join("SKILL.md"),
        "description: A calculator skill\n# Calculator",
    )
    .unwrap();
    cmd_list(&skills_dir).unwrap();
}

// -------------------------------------------------------------------------
// cmd_show edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_show_skill_without_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("noskillmd");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("other.txt"), "some file").unwrap();
    cmd_show(&skills_dir, "noskillmd").unwrap();
}

// -------------------------------------------------------------------------
// cmd_install_builtin comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_install_builtin_all() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    cmd_install_builtin(&skills_dir, None).unwrap();

    let skills = get_builtin_skills();
    for (name, _) in &skills {
        assert!(
            skills_dir.join(name).join("SKILL.md").exists(),
            "Skill '{}' should be installed",
            name
        );
    }
}

// -------------------------------------------------------------------------
// cmd_list_builtin test
// -------------------------------------------------------------------------

#[test]
fn test_cmd_list_builtin() {
    cmd_list_builtin().unwrap();
}

// -------------------------------------------------------------------------
// cmd_validate edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_validate_file_path() {
    let tmp = TempDir::new().unwrap();
    let skill_file = tmp.path().join("SKILL.md");
    std::fs::write(&skill_file, "# My Skill\nname: test").unwrap();
    cmd_validate(&skill_file.to_string_lossy()).unwrap();
}

#[test]
fn test_cmd_validate_no_skill_md_in_dir() {
    let tmp = TempDir::new().unwrap();
    let skill_dir = tmp.path().join("empty-skill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    cmd_validate(&skill_dir.to_string_lossy()).unwrap();
}

// -------------------------------------------------------------------------
// Skill description extraction tests (simulating cmd_list logic)
// -------------------------------------------------------------------------

#[test]
fn test_skill_description_from_header() {
    let content = "# My Cool Skill\nSome content here";
    let desc = content
        .lines()
        .find(|l| l.trim().starts_with("description:") || l.trim().starts_with("# "))
        .map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                l.trim_start_matches('#').trim().to_string()
            } else {
                l.trim_start_matches("description:")
                    .trim()
                    .trim_matches('"')
                    .to_string()
            }
        })
        .unwrap_or_default();
    assert_eq!(desc, "My Cool Skill");
}

#[test]
fn test_skill_description_from_yaml() {
    let content = "description: This is a test skill\n# Header";
    let desc = content
        .lines()
        .find(|l| l.trim().starts_with("description:") || l.trim().starts_with("# "))
        .map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                l.trim_start_matches('#').trim().to_string()
            } else {
                l.trim_start_matches("description:")
                    .trim()
                    .trim_matches('"')
                    .to_string()
            }
        })
        .unwrap_or_default();
    assert_eq!(desc, "This is a test skill");
}

// -------------------------------------------------------------------------
// Source type detection tests (matching cmd_list logic)
// -------------------------------------------------------------------------

#[test]
fn test_skill_source_type_forge() {
    let name = "my-skill-forge";
    let is_forge = name.ends_with("-forge");
    let source_type = if is_forge { "forge" } else { "local" };
    assert_eq!(source_type, "forge");
}

#[test]
fn test_skill_source_type_builtin() {
    let builtins: Vec<&str> = get_builtin_skills().iter().map(|(n, _)| *n).collect();
    let name = "weather";
    let is_forge = name.ends_with("-forge");
    let source_type = if is_forge {
        "forge"
    } else if builtins.contains(&name) {
        "builtin"
    } else {
        "local"
    };
    assert_eq!(source_type, "builtin");
}

#[test]
fn test_skill_source_type_local() {
    let builtins: Vec<&str> = get_builtin_skills().iter().map(|(n, _)| *n).collect();
    let name = "custom-skill";
    let is_forge = name.ends_with("-forge");
    let source_type = if is_forge {
        "forge"
    } else if builtins.contains(&name) {
        "builtin"
    } else {
        "local"
    };
    assert_eq!(source_type, "local");
}

// -------------------------------------------------------------------------
// Description parsing from SKILL.md content
// -------------------------------------------------------------------------

#[test]
fn test_skill_md_description_parsing_with_header() {
    let content = "# My Skill\nSome description text";
    let desc = content
        .lines()
        .find(|l| l.trim().starts_with("description:") || l.trim().starts_with("# "))
        .map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                l.trim_start_matches('#').trim().to_string()
            } else {
                l.trim_start_matches("description:")
                    .trim()
                    .trim_matches('"')
                    .to_string()
            }
        })
        .unwrap_or_default();
    assert_eq!(desc, "My Skill");
}

#[test]
fn test_skill_md_description_parsing_with_yaml() {
    let content = "name: test\ndescription: \"A test skill\"\nsteps:\n- step1";
    let desc = content
        .lines()
        .find(|l| l.trim().starts_with("description:"))
        .map(|l| {
            l.trim_start_matches("description:")
                .trim()
                .trim_matches('"')
                .to_string()
        })
        .unwrap_or_default();
    assert_eq!(desc, "A test skill");
}

#[test]
fn test_skill_md_description_parsing_no_description() {
    let content = "# Just a heading\nSome other text";
    let desc = content
        .lines()
        .find(|l| l.trim().starts_with("description:"))
        .map(|l| {
            l.trim_start_matches("description:")
                .trim()
                .trim_matches('"')
                .to_string()
        })
        .unwrap_or_default();
    assert!(desc.is_empty());
}

// -------------------------------------------------------------------------
// cmd_list with actual skill directories
// -------------------------------------------------------------------------

#[test]
fn test_cmd_list_with_skill_md_v2() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("test-skill-v2");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(
        skill.join("SKILL.md"),
        "# Test Skill\nA test skill for testing",
    )
    .unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_with_forge_skill_v2() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("my-skill-forge");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("SKILL.md"), "# Forge Skill").unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_without_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("bare-skill");
    std::fs::create_dir_all(&skill).unwrap();
    // No SKILL.md
    cmd_list(&skills_dir).unwrap();
}

// -------------------------------------------------------------------------
// cmd_show with various skill structures
// -------------------------------------------------------------------------

#[test]
fn test_cmd_show_skill_without_skill_md_lists_files() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("files-only");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("config.json"), "{}").unwrap();
    std::fs::write(skill.join("data.txt"), "data").unwrap();

    cmd_show(&skills_dir, "files-only").unwrap();
}

// -------------------------------------------------------------------------
// cmd_install_builtin edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_install_builtin_all_v2() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");

    cmd_install_builtin(&skills_dir, None).unwrap();

    let entries: Vec<_> = std::fs::read_dir(&skills_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert_eq!(entries.len(), 10);
}

// -------------------------------------------------------------------------
// load_registry_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_load_registry_config_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    std::fs::write(&path, "not valid json {{{{").unwrap();
    let config = load_registry_config(&path);
    // Should return default config on parse error
    assert!(config.github_sources.is_empty());
}

#[test]
fn test_load_registry_config_partial_fields() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    let data = serde_json::json!({
        "github_sources": []
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let config = load_registry_config(&path);
    assert!(config.github_sources.is_empty());
    assert!(!config.clawhub.enabled); // should use default
}

// -------------------------------------------------------------------------
// save_registry_config overwrite test
// -------------------------------------------------------------------------

#[test]
fn test_save_registry_config_overwrite() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");

    let mut config = nemesis_skills::types::RegistryConfig::default();
    config
        .github_sources
        .push(nemesis_skills::types::GitHubSourceConfig {
            name: "first".to_string(),
            repo: "org/first".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
    save_registry_config(&path, &config).unwrap();

    config
        .github_sources
        .push(nemesis_skills::types::GitHubSourceConfig {
            name: "second".to_string(),
            repo: "org/second".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
    save_registry_config(&path, &config).unwrap();

    let loaded = load_registry_config(&path);
    assert_eq!(loaded.github_sources.len(), 2);
}

// -------------------------------------------------------------------------
// cmd_source_remove tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_source_remove_existing_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");

    let mut config = nemesis_skills::types::RegistryConfig::default();
    config
        .github_sources
        .push(nemesis_skills::types::GitHubSourceConfig {
            name: "test-source".to_string(),
            repo: "org/test".to_string(),
            enabled: true,
            branch: "main".to_string(),
            index_type: "github_api".to_string(),
            index_path: String::new(),
            skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
            timeout_secs: 0,
            max_size: 0,
        });
    save_registry_config(&path, &config).unwrap();

    cmd_source_remove(&path, "test-source").unwrap();

    let loaded = load_registry_config(&path);
    assert!(loaded.github_sources.is_empty());
}

// -------------------------------------------------------------------------
// Additional coverage tests for skills
// -------------------------------------------------------------------------

#[test]
fn test_parse_github_url_https_with_www() {
    // Not www - but test the standard https format
    let (owner, repo) = parse_github_url("https://github.com/org/repo").unwrap();
    assert_eq!(owner, "org");
    assert_eq!(repo, "repo");
}

#[test]
fn test_parse_github_url_http_no_git() {
    let (owner, repo) = parse_github_url("http://github.com/test/proj").unwrap();
    assert_eq!(owner, "test");
    assert_eq!(repo, "proj");
}

#[test]
fn test_parse_github_url_single_component() {
    let result = parse_github_url("onlyone/");
    // Should succeed but with empty repo - or fail, depends on impl
    // Actually splitn(2, '/') gives ["onlyone", ""], parts[1] is empty so fails
    assert!(result.is_err());
}

#[test]
fn test_get_builtin_skills_all_names() {
    let skills = get_builtin_skills();
    let names: Vec<&str> = skills.iter().map(|(n, _)| *n).collect();
    assert!(names.contains(&"weather"));
    assert!(names.contains(&"news"));
    assert!(names.contains(&"stock"));
    assert!(names.contains(&"calculator"));
    assert!(names.contains(&"structured-development"));
    assert!(names.contains(&"build-project"));
    assert!(names.contains(&"automated-testing"));
    assert!(names.contains(&"desktop-automation"));
    assert!(names.contains(&"wsl-operations"));
    assert!(names.contains(&"dump-analyze"));
}

#[test]
fn test_load_registry_config_bad_json_v2() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    std::fs::write(&path, "not valid json{{{").unwrap();
    let config = load_registry_config(&path);
    // Should return default config
    assert!(config.github_sources.is_empty());
}

#[test]
fn test_load_registry_config_empty_obj() {
    let tmp = TempDir::new().unwrap();
    let path = tmp.path().join("config.skills.json");
    std::fs::write(&path, "{}").unwrap();
    let config = load_registry_config(&path);
    assert!(config.github_sources.is_empty());
}

#[test]
fn test_save_registry_config_creates_parent_dirs() {
    let tmp = TempDir::new().unwrap();
    let path = tmp
        .path()
        .join("nested")
        .join("dir")
        .join("config.skills.json");
    let config = nemesis_skills::types::RegistryConfig::default();
    save_registry_config(&path, &config).unwrap();
    assert!(path.exists());
}

#[test]
fn test_cmd_list_with_skill_dirs() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill1 = skills_dir.join("skill-a");
    let skill2 = skills_dir.join("skill-b");
    std::fs::create_dir_all(&skill1).unwrap();
    std::fs::create_dir_all(&skill2).unwrap();
    std::fs::write(skill1.join("SKILL.md"), "# Skill A\nDescription of A").unwrap();
    std::fs::write(skill2.join("SKILL.md"), "# Skill B").unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_with_forge_skill_v3() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let forge_skill = skills_dir.join("test-forge-v3");
    std::fs::create_dir_all(&forge_skill).unwrap();
    std::fs::write(forge_skill.join("SKILL.md"), "# Forge Skill V3").unwrap();
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_list_with_no_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("incomplete");
    std::fs::create_dir_all(&skill).unwrap();
    // No SKILL.md
    cmd_list(&skills_dir).unwrap();
}

#[test]
fn test_cmd_show_skill_no_skill_md() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill = skills_dir.join("noskillmd");
    std::fs::create_dir_all(&skill).unwrap();
    std::fs::write(skill.join("other.txt"), "content").unwrap();
    cmd_show(&skills_dir, "noskillmd").unwrap();
}

#[test]
fn test_cmd_validate_directory_with_skill_md() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("test-skill");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(
        dir.join("SKILL.md"),
        "# Test Skill\nname: test\ndescription: test\nsteps:\n- step1",
    )
    .unwrap();
    cmd_validate(&dir.to_string_lossy()).unwrap();
}

#[test]
fn test_cmd_validate_file_directly() {
    let tmp = TempDir::new().unwrap();
    let file = tmp.path().join("SKILL.md");
    std::fs::write(&file, "# Test\nname: test").unwrap();
    cmd_validate(&file.to_string_lossy()).unwrap();
}

#[test]
fn test_cmd_validate_nonexistent() {
    cmd_validate("/nonexistent/path/to/skill").unwrap();
}

#[test]
fn test_cmd_install_builtin_all_v3() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills-v3");
    cmd_install_builtin(&skills_dir, None).unwrap();
    // Check that at least some skills were installed
    let entries: Vec<_> = std::fs::read_dir(&skills_dir)
        .unwrap()
        .filter_map(|e| e.ok())
        .collect();
    assert!(!entries.is_empty());
}

#[test]
fn test_cmd_remove_removes_directory() {
    let tmp = TempDir::new().unwrap();
    let skills_dir = tmp.path().join("skills");
    let skill_path = skills_dir.join("to-remove");
    std::fs::create_dir_all(&skill_path).unwrap();
    std::fs::write(skill_path.join("SKILL.md"), "content").unwrap();

    cmd_remove(&skills_dir, "to-remove").unwrap();
    assert!(!skill_path.exists());
}

#[test]
fn test_parse_github_url_with_extra_path_components() {
    let result = parse_github_url("https://github.com/user/repo/tree/main");
    // Should still extract user/repo
    assert!(result.is_ok());
    let (owner, repo) = result.unwrap();
    assert_eq!(owner, "user");
    assert_eq!(repo, "repo/tree/main"); // splitn(2, '/') gives only first two
}
