use super::*;
use tempfile::TempDir;

#[test]
fn test_parse_github_url_https() {
    let (owner, repo) = parse_github_url("https://github.com/anthropics/skills").unwrap();
    assert_eq!(owner, "anthropics");
    assert_eq!(repo, "skills");
}

// (BUG #25, quality-hardening goal 冲刺 S10) 前缀命中但只有 owner 段：
// 原实现掉进 shorthand 分支产出 ("https:", "/github.com/onlyowner") 垃圾
// 解析，现在直接报错。
#[test]
fn test_parse_github_url_https_owner_only_errors() {
    let err = parse_github_url("https://github.com/onlyowner").unwrap_err();
    assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    let err = parse_github_url("http://github.com/onlyowner").unwrap_err();
    assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
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

// =========================================================================
// run() / cmd_* 分支覆盖（S11 覆盖率冲刺）
//
// 策略：NEMESISBOT_HOME 指向临时目录（resolve_home 优先级 2），
// 全程只读写临时 home；env set_var 进程级 → 持 crate::GLOBAL_STATE_LOCK。
// run() 的 Search/Cache/Learn 分支用 tokio::task::block_in_place →
// 必须 multi_thread flavor；直测 async cmd_* 用普通 #[tokio::test]。
// 不触网：cmd_search 走"无 registry 提前返回"分支（默认配置
// search_cache.enabled=false、clawhub.enabled=false、github_sources 空）；
// cmd_learn 用 config 缺失 / 不可解析模型两条纯本地 bail 路径。
// =========================================================================

struct TempHomeEnv {
    _tmp: TempDir,
    home: std::path::PathBuf,
}

impl Drop for TempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn temp_home_env() -> TempHomeEnv {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    TempHomeEnv { _tmp: tmp, home }
}

fn skills_cfg_of(home: &std::path::Path) -> std::path::PathBuf {
    crate::common::skills_config_path(home)
}

fn write_skills_cfg(home: &std::path::Path, cfg: &serde_json::Value) {
    std::fs::write(
        skills_cfg_of(home),
        serde_json::to_string_pretty(cfg).unwrap(),
    )
    .unwrap();
}

/// 与 cmd_cache_stats/clear 内部一致的缓存目录推导：
/// `{workspace}/workspace/skills/.cache`（skills_cfg 上两级再拼 workspace）。
fn cache_dir_of(home: &std::path::Path) -> std::path::PathBuf {
    skills_cfg_of(home)
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("workspace")
        .join("skills")
        .join(".cache")
}

fn make_cache_dir(home: &std::path::Path) -> std::path::PathBuf {
    let dir = cache_dir_of(home);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

fn write_stats(dir: &std::path::Path, json: &str) {
    std::fs::write(dir.join(".stats.json"), json).unwrap();
}

fn put_entries(dir: &std::path::Path, n: usize) {
    for i in 0..n {
        std::fs::write(dir.join(format!("entry_{}.json", i)), "{}").unwrap();
    }
}

// -------------------------------------------------------------------------
// cmd_cache_stats（纯文件逻辑）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_cache_stats_disabled() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    write_skills_cfg(&home, &serde_json::json!({ "search_cache": { "enabled": false } }));
    cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    // 配置文件缺失 → 默认 enabled=false 同样走 disabled 分支
    let tmp2 = TempDir::new().unwrap();
    let home2 = tmp2.path().join(".nemesisbot");
    std::fs::create_dir_all(home2.join("workspace").join("config")).unwrap();
    cmd_cache_stats(&skills_cfg_of(&home2)).await.unwrap();
}

#[tokio::test]
async fn test_cmd_cache_stats_no_dir() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    write_skills_cfg(
        &home,
        &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10, "ttl_secs": 60 } }),
    );
    // 缓存目录不存在 → 全 0 / N/A 分支
    cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
}

#[tokio::test]
async fn test_cmd_cache_stats_empty_dir_with_stats() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    write_skills_cfg(
        &home,
        &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10, "ttl_secs": 60 } }),
    );
    let dir = make_cache_dir(&home);
    write_stats(&dir, r#"{"hits": 5, "misses": 5}"#);
    // 只有 .stats.json → count=0 → "No entries yet"
    cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
}

#[tokio::test]
async fn test_cmd_cache_stats_rating_branches() {
    // Excellent（命中率 ≥80%）
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 2);
        write_stats(&dir, r#"{"hits": 9, "misses": 1}"#);
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
    // Good（50%-80%）
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 2);
        write_stats(&dir, r#"{"hits": 6, "misses": 4}"#);
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
    // Excellent (by fill)：无请求 + count ≥ 80% max_size
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 8);
        write_stats(&dir, r#"{"hits": 0, "misses": 0}"#);
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
    // Good (by fill)：无请求 + count ≥ 50% max_size
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 5);
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
    // Low：无请求 + 低填充
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 100 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 1);
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
    // .stats.json 损坏 → (0,0) 兜底
    {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 1);
        write_stats(&dir, "not-json{{{");
        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }
}

// -------------------------------------------------------------------------
// cmd_cache_clear（纯文件逻辑）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_cache_clear_removes_dir() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    let dir = make_cache_dir(&home);
    put_entries(&dir, 3);
    cmd_cache_clear(&skills_cfg_of(&home)).await.unwrap();
    assert!(!dir.exists());
}

#[tokio::test]
async fn test_cmd_cache_clear_missing_dir() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    cmd_cache_clear(&skills_cfg_of(&home)).await.unwrap();
}

// -------------------------------------------------------------------------
// cmd_search：空 registry 提前返回（不触网）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_search_no_registries_early_return() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    // 配置缺失 → 默认 RegistryConfig（clawhub.enabled=false，无 github_sources）
    // → registries() 为空 → 打印提示后返回，绝不发网络请求
    cmd_search(&skills_cfg_of(&home), "query", 10).await.unwrap();
}

// -------------------------------------------------------------------------
// cmd_learn：两条纯本地 bail 路径（不构造真 agent / 不发 LLM 请求）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_learn_missing_config_bails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let err = cmd_learn(&home, "some-source", None).await.unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

#[tokio::test]
async fn test_cmd_learn_unresolvable_model_bails() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    // 模型不可解析（未知 ref 且无关键词可推断 provider）→ agent 初始化失败
    std::fs::write(
        crate::common::config_path(&home),
        serde_json::to_string(&serde_json::json!({
            "agents": { "defaults": { "llm": "zz-unresolvable-model" } },
            "model_list": []
        }))
        .unwrap(),
    )
    .unwrap();
    let err = cmd_learn(&home, "some-source", None).await.unwrap_err();
    assert!(err.to_string().contains("Failed to initialize agent"), "err: {err}");
}

// -------------------------------------------------------------------------
// cmd_source_add：URL 解析错误（不触网，parse 先行）
// -------------------------------------------------------------------------

#[tokio::test]
async fn test_cmd_source_add_invalid_url_errors_before_network() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    // (BUG #27 同类横向, quality-hardening goal 冲刺 S11e) cmd_source_add 已
    // async 化（原同步实现内部 reqwest::blocking，在 run_command 的 runtime
    // 上下文 drop 嵌套 runtime 必 panic）；URL 解析仍在任何网络 IO 之前。
    let err = cmd_source_add(&skills_cfg_of(&home), "not a github url!")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
}

// -------------------------------------------------------------------------
// run() 同步分支
// -------------------------------------------------------------------------

#[test]
fn test_run_skills_list_dir_missing_and_present() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(SkillsAction::List, false).unwrap();
    // 有 skill 目录（带/不带 SKILL.md、forge 目录）
    let skills = crate::common::workspace_path(&th.home).join("skills");
    std::fs::create_dir_all(skills.join("alpha")).unwrap();
    std::fs::write(skills.join("alpha").join("SKILL.md"), "# Alpha\nname: alpha\ndescription: d\n").unwrap();
    std::fs::create_dir_all(skills.join("beta")).unwrap();
    std::fs::create_dir_all(skills.join(".forge").join("gamma")).unwrap();
    run(SkillsAction::List, false).unwrap();
}

#[test]
fn test_run_skills_remove_found_and_not_found() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let skills = crate::common::workspace_path(&th.home).join("skills");
    std::fs::create_dir_all(skills.join("gone")).unwrap();
    run(
        SkillsAction::Remove { name: "gone".into() },
        false,
    )
    .unwrap();
    assert!(!skills.join("gone").exists());
    run(
        SkillsAction::Remove { name: "ghost".into() },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_skills_show_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    let skills = crate::common::workspace_path(&th.home).join("skills");
    // 不存在
    run(SkillsAction::Show { name: "nope".into() }, false).unwrap();
    // 带 SKILL.md
    std::fs::create_dir_all(skills.join("withmd")).unwrap();
    std::fs::write(skills.join("withmd").join("SKILL.md"), "# content").unwrap();
    run(SkillsAction::Show { name: "withmd".into() }, false).unwrap();
    // 不带 SKILL.md → 列目录文件
    std::fs::create_dir_all(skills.join("nomd")).unwrap();
    std::fs::write(skills.join("nomd").join("helper.py"), "x").unwrap();
    run(SkillsAction::Show { name: "nomd".into() }, false).unwrap();
}

#[test]
fn test_run_skills_source_list_and_remove() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 配置缺失
    run(
        SkillsAction::Source {
            action: SourceAction::List,
        },
        false,
    )
    .unwrap();
    // 有 github_sources + legacy + clawhub
    write_skills_cfg(
        &th.home,
        &serde_json::json!({
            "github_sources": [
                { "name": "s1", "repo": "o/r", "branch": "main", "index_type": "flat", "enabled": true }
            ],
            "clawhub": { "enabled": true, "base_url": "https://clawhub.ai" }
        }),
    );
    run(
        SkillsAction::Source {
            action: SourceAction::List,
        },
        false,
    )
    .unwrap();
    // 移除存在的
    run(
        SkillsAction::Source {
            action: SourceAction::Remove { name: "s1".into() },
        },
        false,
    )
    .unwrap();
    let cfg: serde_json::Value = serde_json::from_str(
        &std::fs::read_to_string(skills_cfg_of(&th.home)).unwrap(),
    )
    .unwrap();
    assert_eq!(cfg["github_sources"].as_array().unwrap().len(), 0);
    // 移除不存在的
    run(
        SkillsAction::Source {
            action: SourceAction::Remove { name: "ghost".into() },
        },
        false,
    )
    .unwrap();
}

// (BUG #27 同类横向, quality-hardening goal 冲刺 S11e) AddSource 臂已 async 化
// （block_in_place + Handle::block_on 桥承载异步 client，与 Search/Install 臂同
// 模式），要求 tokio multi-thread runtime 上下文——因此本测试改为生产同款拓扑：
// 自建 runtime + block_on 驱动 run()。旧「裸线程直调」契约随之作废（原实现内嵌
// reqwest::blocking，在生产 runtime 上下文里 drop 必 panic，才是真正要改的 bug）。
#[test]
fn test_run_skills_add_source_invalid_url() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    std::thread::scope(|s| {
        let handle = s.spawn(|| {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = rt.block_on(async {
                // 非 github URL → parse 先失败，不触网
                run(
                    SkillsAction::AddSource { url: "not-a-url".into() },
                    false,
                )
            });
            drop(rt);
            result
        });
        let err = handle.join().expect("run() 在 runtime 上下文不得 panic").unwrap_err();
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    });
}

#[test]
fn test_run_skills_validate_variants() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // 路径不存在
    run(
        SkillsAction::Validate {
            path: "Z:/no/such/path".into(),
        },
        false,
    )
    .unwrap();
    // 目录带 SKILL.md
    let skill_dir = th.home.join("vskill");
    std::fs::create_dir_all(&skill_dir).unwrap();
    std::fs::write(
        skill_dir.join("SKILL.md"),
        "name: v\ndescription: test\nsteps:\n- a\n",
    )
    .unwrap();
    run(
        SkillsAction::Validate {
            path: skill_dir.to_string_lossy().to_string(),
        },
        false,
    )
    .unwrap();
    // 目录不带 SKILL.md
    let empty_dir = th.home.join("vempty");
    std::fs::create_dir_all(&empty_dir).unwrap();
    run(
        SkillsAction::Validate {
            path: empty_dir.to_string_lossy().to_string(),
        },
        false,
    )
    .unwrap();
    // 直接指向单个文件
    run(
        SkillsAction::Validate {
            path: skill_dir
                .join("SKILL.md")
                .to_string_lossy()
                .to_string(),
        },
        false,
    )
    .unwrap();
}

#[test]
fn test_run_skills_builtin_install_and_list() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    run(SkillsAction::ListBuiltin, false).unwrap();
    // 指定名字安装
    run(
        SkillsAction::InstallBuiltin { name: Some("weather".into()) },
        false,
    )
    .unwrap();
    let skills = crate::common::workspace_path(&th.home).join("skills");
    assert!(skills.join("weather").exists());
    // 重复安装 → already exists 分支
    run(
        SkillsAction::InstallBuiltin { name: Some("weather".into()) },
        false,
    )
    .unwrap();
    // 未知名字
    run(
        SkillsAction::InstallBuiltin { name: Some("no-such-builtin".into()) },
        false,
    )
    .unwrap();
}

// -------------------------------------------------------------------------
// run() 异步分支（block_in_place → multi_thread flavor）
// -------------------------------------------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_run_skills_search_empty_registries_dispatch() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    run(
        SkillsAction::Search {
            query: Some("q".into()),
            limit: 5,
        },
        false,
    )
    .unwrap();
    // query=None → 空串
    run(SkillsAction::Search { query: None, limit: 5 }, false).unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_run_skills_cache_dispatch() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = temp_home_env();
    // Stats：disabled 分支
    run(
        SkillsAction::Cache {
            action: CacheAction::Stats,
        },
        false,
    )
    .unwrap();
    // Clear：目录存在分支
    let dir = make_cache_dir(&th.home);
    put_entries(&dir, 2);
    run(
        SkillsAction::Cache {
            action: CacheAction::Clear,
        },
        false,
    )
    .unwrap();
    assert!(!dir.exists());
    // Clear：目录不存在分支
    run(
        SkillsAction::Cache {
            action: CacheAction::Clear,
        },
        false,
    )
    .unwrap();
}

#[tokio::test(flavor = "multi_thread")]
async fn test_run_skills_learn_missing_config_dispatch() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = temp_home_env();
    let err = run(
        SkillsAction::Learn {
            source: "src".into(),
            name: None,
        },
        false,
    )
    .unwrap_err();
    assert!(err.to_string().contains("Configuration not found"));
}

// ===========================================================================
// panic 探针（BUG #27 同类横向, quality-hardening goal 冲刺 S11e）
//
// run() 的 SourceAction::Add / AddSource / InstallClawhub 臂使用「block_in_place
// + Handle::block_on 承载 async fn（内部异步 reqwest client）」桥接模式（本批
// 把这三臂从直接调用同步 fn 内嵌 blocking client 改成此模式；Search/Install/
// Learn/Cache 臂是既有同款）。按纪律：首次引入的模式必须有 catch_unwind 探针
// 实证不 panic——探针拓扑忠实复刻 main.rs 非 gateway 命令路径：
//   新建 multi_thread runtime → rt.block_on(run_command 未来体)
//   → 命令内部 block_in_place(|| Handle::block_on(async { 异步 client }))
#[test]
fn probe_block_in_place_bridge_with_async_client_is_panic_free() {
    // 本地一次性 mock（127.0.0.1:0，绝占不端口黑名单上的固定口）
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..3 {
            let Ok((mut stream, _)) = listener.accept() else { break };
            let mut buf = [0u8; 2048];
            let _ = stream.read(&mut buf);
            let body = r#"{"ok":true}"#;
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {l}\r\n\r\n{b}",
                l = body.len(),
                b = body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });

    std::thread::scope(|s| {
        // 忠实复刻 main.rs：block_on 驱动「命令未来体」，体内走
        // block_in_place + Handle::block_on 桥，桥内异步 client 打本地 mock。
        let handle = s.spawn(move || {
            let rt = tokio::runtime::Builder::new_multi_thread()
                .enable_all()
                .build()
                .unwrap();
            let result = rt.block_on(async move {
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async move {
                        // 生产同款：async client（无嵌套 runtime），不是 blocking client。
                        let client = reqwest::Client::builder()
                            .timeout(std::time::Duration::from_secs(5))
                            .build()
                            .unwrap();
                        let url = format!("http://127.0.0.1:{}/", port);
                        let resp = client.get(&url).send().await.expect("mock 可达");
                        resp.status().as_u16()
                    })
                })
            });
            drop(rt);
            result
        });
        // 若 bridge 模式 panic，join 的 expect 会如实把测试打红。
        let status = handle
            .join()
            .expect("bridge 模式绝不允许 panic 外溢（这是 async 化的全部意义）");
        assert_eq!(status, 200, "HTTP 往返应成功");
    })
}
