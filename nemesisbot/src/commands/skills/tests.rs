// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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

/// 与 cmd_cache_stats/clear 内部一致的缓存目录推导（经 nemesis-path 唯一
/// 真相源）：`{workspace}/skills/.cache`（2026-08-28 修复双重 workspace 拼接）。
fn cache_dir_of(home: &std::path::Path) -> std::path::PathBuf {
    nemesis_path::resolve_skills_cache_dir_in_workspace(
        skills_cfg_of(home).parent().unwrap().parent().unwrap(),
    )
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

// ===========================================================================
// wave_b（覆盖率补测 2026-08-27）——只补「进程内可构造」的 miss 行。
//
// 不触网纪律的三条实现路径：
// 1. nemesis-skills 的 ClawHub registry 的 base_url / convex_url 来自
//    config.skills.json（ClawHubConfig），可指到 127.0.0.1 临时端口上的
//    本地 mock——这是 cmd_search 渲染链路唯一的离线可达入口。
// 2. CLI build_agent_loop 的 api_base 来自 config.json（OpenAI 兼容
//    HttpCompat provider，POST {base}/chat/completions），同样可注入本地
//    mock——cmd_learn 成功链路（build Ok 臂 + process_direct Ok 臂）由此覆盖。
// 3. GitHub 硬编码端点（api.github.com / raw.githubusercontent.com）无任何
//    重定向钩子 → 相关代码一律 EXEMPT。cmd_install 的 GitHub 回退是雷区：
//    install 失败必然带着合法 owner/repo 进 cmd_install_github 的下载循环，
//    所以本块用**含空格的 slug**毒化 full_ref（parse_github_url 的 shorthand
//    分支要求不含空格 → 在任何 socket 之前的第 580 行 parse 处即死）；
//    且 validate_skill_identifier（types.rs:480）允许空格，slug 能一路活着
//    到达 install 失败点——两条事实合起来才构成离线安全证明。
// 涉及环境变量的测试持 crate::GLOBAL_STATE_LOCK，prev-value 按 Option 恢复。
// ===========================================================================

mod wave_b {
    use super::*;

    // -----------------------------------------------------------------------
    // 本地 mock HTTP 服务（一次性路由表 + 计数自停）
    // -----------------------------------------------------------------------

    struct WaveBMock {
        addr: std::net::SocketAddr,
    }

    impl WaveBMock {
        /// `routes`: (请求头子串匹配模式, 响应 body)；按序首个命中生效。
        /// 未命中一律回 `{"results":[]}`。服务满 expected_requests 个请求后
        /// 线程自然退出。
        fn start(routes: Vec<(&'static str, String)>, expected_requests: usize) -> Self {
            let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
            let addr = listener.local_addr().unwrap();
            let rem = std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(
                expected_requests,
            ));
            std::thread::spawn(move || {
                use std::io::Read;
                use std::sync::atomic::Ordering;
                while rem.load(Ordering::SeqCst) > 0 {
                    let Ok((mut stream, _)) = listener.accept() else {
                        break;
                    };
                    // ── 读请求头（直到 \r\n\r\n）──
                    let mut buf: Vec<u8> = Vec::new();
                    let mut chunk = [0u8; 2048];
                    loop {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                if buf.windows(4).any(|w| w == b"\r\n\r\n") {
                                    break;
                                }
                            }
                        }
                    }
                    let head_end = buf
                        .windows(4)
                        .position(|w| w == b"\r\n\r\n")
                        .map(|p| p + 4)
                        .unwrap_or(buf.len());
                    let head = String::from_utf8_lossy(&buf[..head_end]).to_ascii_lowercase();
                    // ── 有 body 就补读完（避免下一连接读到脏字节）──
                    let content_len: usize = head
                        .lines()
                        .find_map(|l| l.strip_prefix("content-length:"))
                        .and_then(|v| v.trim().parse().ok())
                        .unwrap_or(0);
                    let mut have = buf.len() - head_end;
                    while have < content_len {
                        match stream.read(&mut chunk) {
                            Ok(0) | Err(_) => break,
                            Ok(n) => {
                                buf.extend_from_slice(&chunk[..n]);
                                have += n;
                            }
                        }
                    }
                    // ── 选路由并应答（Connection: close → 客户端每次新连接）──
                    let req_head = String::from_utf8_lossy(&buf[..head_end]).to_string();
                    let body = routes
                        .iter()
                        .find(|(pat, _)| req_head.contains(pat))
                        .map(|(_, b)| b.clone())
                        .unwrap_or_else(|| r#"{"results":[]}"#.to_string());
                    let resp = format!(
                        "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {l}\r\nConnection: close\r\n\r\n{b}",
                        l = body.len(),
                        b = body
                    );
                    let _ = std::io::Write::write_all(&mut stream, resp.as_bytes());
                    rem.fetch_sub(1, Ordering::SeqCst);
                }
            });
            WaveBMock { addr }
        }

        fn base_url(&self) -> String {
            format!("http://{}", self.addr)
        }
    }

    /// 写一份启用 ClawHub（base 指向 mock）的 config.skills.json。
    /// convex_url 可与 base 不同源（列表端点走 convex、搜索走 base）。
    fn wave_b_write_clawhub_cfg(
        home: &std::path::Path,
        base_url: &str,
        convex_url: &str,
    ) {
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            home,
            &serde_json::json!({
                "search_cache": { "enabled": false },
                "clawhub": {
                    "enabled": true,
                    "base_url": base_url,
                    "convex_url": convex_url
                }
            }),
        );
    }

    /// NEMESISBOT_HOME 守卫：纪律要求 prev-value 按 Option 恢复
    /// （既有 temp_home_env 不恢复环境变量，本块不沿用）。调用方必须持
    /// crate::GLOBAL_STATE_LOCK。
    struct WaveBHomeGuard {
        prev: Option<std::ffi::OsString>,
    }

    impl WaveBHomeGuard {
        fn set(root: &std::path::Path) -> Self {
            let prev = std::env::var_os("NEMESISBOT_HOME");
            unsafe { std::env::set_var("NEMESISBOT_HOME", root) };
            Self { prev }
        }
    }

    impl Drop for WaveBHomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v) },
                None => unsafe { std::env::remove_var("NEMESISBOT_HOME") },
            }
        }
    }

    // -----------------------------------------------------------------------
    // load_registry_config / save_registry_config 缺口（169 / 184）
    // -----------------------------------------------------------------------

    /// config 路径指向一个【目录】：exists()==true 但 read_to_string 失败 →
    /// 走读取失败回落分支（169 一带区域）。产物必须是干净默认值。
    #[test]
    fn wave_b_load_registry_config_unreadable_path_returns_default() {
        let tmp = TempDir::new().unwrap();
        // 目标本身就是已存在的目录
        let cfg_dir = tmp.path().join("config.skills.json");
        std::fs::create_dir_all(&cfg_dir).unwrap();

        let config = load_registry_config(&cfg_dir);
        assert!(config.github_sources.is_empty(), "读取失败必须回落默认");
        assert!(config.github_sources_legacy.is_empty());
        assert!(!config.clawhub.enabled);
    }

    /// save_registry_config 的写盘目标本身是目录 → fs::write Err 经 184 行的
    /// `?` 向上传播（此前测试只走过成功路径）。
    #[test]
    fn wave_b_save_registry_config_write_error_propagates() {
        let tmp = TempDir::new().unwrap();
        let target = tmp.path().join("occupied");
        std::fs::create_dir_all(&target).unwrap();

        let config = nemesis_skills::types::RegistryConfig::default();
        let err = save_registry_config(&target, &config)
            .expect_err("对目录写文件必须失败传播（184 行 ?）");
        assert!(
            err.to_string().to_lowercase().contains("denied")
                || err.to_string().to_lowercase().contains("os error"),
            "期望 OS 写失败，got: {err}"
        );
    }

    // -----------------------------------------------------------------------
    // cmd_search 缺口（466-523）：经 ClawHub 本地 mock 全离线驱动
    // -----------------------------------------------------------------------

    /// 所有 registry 搜索失败（此处 base 指向已被释放的本地端口，连接秒拒）
    /// → Err 臂的 Fallback 提示三连打印（515-520）+ 正常收尾（522）。
    /// 同时覆盖非空 registry 时才出现的 "Searching N registry/ies" 打印（466/468-469）。
    #[tokio::test]
    async fn wave_b_cmd_search_all_registries_fail_prints_fallback_hint() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        // 抢一个端口后立刻释放 → 后续连接被本机拒绝（纯回环，不出机器）
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let dead_port = listener.local_addr().unwrap().port();
        drop(listener);
        wave_b_write_clawhub_cfg(
            &home,
            &format!("http://127.0.0.1:{dead_port}"),
            &format!("http://127.0.0.1:{dead_port}"),
        );

        cmd_search(&skills_cfg_of(&home), "wifi", 5).await.unwrap();

        // 再来一次空 query 路线（convex 列表端点同样被拒）确保两条搜索路线都进 Err 臂
        cmd_search(&skills_cfg_of(&home), "", 5).await.unwrap();
    }

    /// 非 query 搜索命中 mock：结果组渲染主链路——长摘要截断（>100 字符取
    /// 前 97 加省略号）、display_name 空/非空双臂、短摘要直显、总数统计。
    #[tokio::test]
    async fn wave_b_cmd_search_renders_groups_names_summaries_and_totals() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let long_summary = format!("very long summary padding {}", "x".repeat(120));
        let payload = serde_json::json!({
            "results": [
                { "score": 0.92, "slug": "demo-one", "displayName": "Demo One",
                  "summary": long_summary },
                { "score": 0.51, "slug": "demo-two", "displayName": "", "summary": "" }
            ]
        })
        .to_string();
        let _mock = WaveBMock::start(vec![("/api/search", payload)], 1);
        wave_b_write_clawhub_cfg(&home, &_mock.base_url(), &_mock.base_url());

        cmd_search(&skills_cfg_of(&home), "demo", 5).await.unwrap();
    }

    /// 空 query 走 Convex 列表端点且恰好返回 limit 条 → 组级 truncated=true
    /// → cmd_search 的 ", truncated" 标签臂（483-484）。
    #[tokio::test]
    async fn wave_b_cmd_search_truncated_group_label_via_local_list_endpoint() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let list_payload = serde_json::json!({
            "status": "success",
            "value": [
                { "slug": "list-one", "displayName": "List One",
                  "summary": "short summary", "stats": { "downloads": 42 } }
            ]
        })
        .to_string();
        // 请求头区分路由：GET /api/search?... vs POST /api/query
        let _mock = WaveBMock::start(vec![("POST /api/query", list_payload)], 1);
        wave_b_write_clawhub_cfg(&home, &_mock.base_url(), &_mock.base_url());

        cmd_search(&skills_cfg_of(&home), "", 1).await.unwrap();
    }

    // -----------------------------------------------------------------------
    // cmd_install 缺口（525-576 一带的离线可达行）
    // -----------------------------------------------------------------------

    /// registry/slug 直拆分支 + install 失败后的 GitHub 回退【毒化】终局：
    /// slug 含空格 → full_ref=`clawhub/bad slug` → parse_github_url 在任何
    /// 网络语句之前报错返回（shorthand 要求不含空格）。零出网。
    #[tokio::test]
    async fn wave_b_cmd_install_direct_ref_poisons_github_fallback() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let skills_dir = crate::common::workspace_path(&home).join("skills");

        let err = cmd_install(&skills_dir, &skills_cfg_of(&home), "clawhub/bad slug")
            .await
            .expect_err("registry 未注册 → install 失败 → 毒化回退必以 Invalid GitHub URL 终结");
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    }

    /// 无斜杠 skill_ref：搜遍空 registry 不中 → "Trying GitHub fallback"
    /// 分支（546-550）→ 回退以裸名进 parse → 同样在任何 socket 前终结。
    #[tokio::test]
    async fn wave_b_cmd_install_no_slash_search_miss_falls_back_to_invalid_github() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let skills_dir = crate::common::workspace_path(&home).join("skills");

        let err = cmd_install(&skills_dir, &skills_cfg_of(&home), "zz-noslash-anywhere")
            .await
            .expect_err("裸名找不到 → fallback parse 必失败");
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    }

    /// 无斜杠 + mock 搜索命中（slug 故意含空格）→ Found 分支（540-544 拿到
    /// registry/slug 元组）→ install 链走到 convex（指 ：9 回环死端口，连接
    /// 即拒）失败 → 567-568 → 毒化 full_ref 回退在 parse 处安全终结。
    #[tokio::test]
    async fn wave_b_cmd_install_found_via_mock_registry_then_poisoned_full_ref() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace")).unwrap();
        let skills_dir = crate::common::workspace_path(&home).join("skills");

        let search_payload = serde_json::json!({
            "results": [
                { "score": 1.0, "slug": "bad slug", "displayName": "Poison Slug",
                  "summary": "poisoned slug full_ref dies at parse" }
            ]
        })
        .to_string();
        let _mock = WaveBMock::start(vec![("/api/search", search_payload)], 1);
        // convex 走死端口：identifier 校验放行空格 slug 后，第一步 convex 调用即败
        wave_b_write_clawhub_cfg(&home, &_mock.base_url(), "http://127.0.0.1:9");

        let err = cmd_install(&skills_dir, &skills_cfg_of(&home), "poison-demo")
            .await
            .expect_err("install 失败 → 毒化回退终结");
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    }

    // -----------------------------------------------------------------------
    // run() 分发臂缺口（Install / Source::Add 的 block_in_place 桥）
    // -----------------------------------------------------------------------

    /// Install 臂的桥接体（1153-1158 + 1160-1161）：走上面同款离线终局。
    #[tokio::test(flavor = "multi_thread")]
    async fn wave_b_run_skills_install_arm_bridge_dispatch() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home_root = TempDir::new().unwrap();
        std::fs::create_dir_all(home_root.path().join(".nemesisbot")).unwrap();
        let _env = WaveBHomeGuard::set(home_root.path());
        let err = run(
            SkillsAction::Install {
                skill: "zz-noslash-anywhere".into(),
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    }

    /// Source::Add 子臂（1168-1172）：非法 URL 在任何网络语句之前被 parse 拒绝。
    #[tokio::test(flavor = "multi_thread")]
    async fn wave_b_run_skills_source_add_arm_bridge_invalid_url() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let home_root = TempDir::new().unwrap();
        std::fs::create_dir_all(home_root.path().join(".nemesisbot")).unwrap();
        let _env = WaveBHomeGuard::set(home_root.path());
        let err = run(
            SkillsAction::Source {
                action: SourceAction::Add {
                    url: "not-a-github-url!".into(),
                },
            },
            false,
        )
        .unwrap_err();
        assert!(err.to_string().contains("Invalid GitHub URL"), "err: {err}");
    }

    // -----------------------------------------------------------------------
    // cmd_cache_stats：.stats.json 存在但不可读（目录伪装成文件）→ (0,0) 臂
    // -----------------------------------------------------------------------

    #[tokio::test]
    async fn wave_b_cmd_cache_stats_stats_file_directory_degrades_zero() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        write_skills_cfg(
            &home,
            &serde_json::json!({ "search_cache": { "enabled": true, "max_size": 10, "ttl_secs": 60 } }),
        );
        let dir = make_cache_dir(&home);
        put_entries(&dir, 2);
        // 目录让 exists()==true 成立但 read_to_string 必败 → 719 行 (0,0) 臂
        std::fs::create_dir_all(dir.join(".stats.json")).unwrap();

        cmd_cache_stats(&skills_cfg_of(&home)).await.unwrap();
    }

    // -----------------------------------------------------------------------
    // cmd_validate：security_check 的 BLOCKED / warnings 两臂（1012 / 1014-1017）
    // 触发内容依据 nemesis-skills lint 模式表：
    //   DEST-001 `rm\s+-rf\s+/` Critical·Destructive → blocked（has_critical）
    //   RECN-001 `(?i)(?:^|\W)nmap(?:\s|$)` → 仅告警，单条 Recon 扣分远不到 0.3
    // -----------------------------------------------------------------------

    #[test]
    fn wave_b_cmd_validate_blocked_skill_reports_security_block() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("evil-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "# evil\nname: evil\ndescription: bad\nsteps:\n- run rm -rf /important",
        )
        .unwrap();

        cmd_validate(&dir.to_string_lossy()).unwrap();
    }

    #[test]
    fn wave_b_cmd_validate_warned_skill_lists_warning_count() {
        let tmp = TempDir::new().unwrap();
        let dir = tmp.path().join("nosy-skill");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("SKILL.md"),
            "# nosy\nname: nosy\ndescription: scans network\nsteps:\n- probe with nmap -sV",
        )
        .unwrap();

        cmd_validate(&dir.to_string_lossy()).unwrap();
    }

    // -----------------------------------------------------------------------
    // cmd_source_list：legacy 来源循环体（818-823）+ 配置存在但零来源（832）
    // -----------------------------------------------------------------------

    #[test]
    fn wave_b_cmd_source_list_legacy_rows_and_empty_registry_notice() {
        let tmp = TempDir::new().unwrap();

        // legacy 条目渲染（github_sources 空、仅 legacy 命中 817-823 循环体）
        let path = tmp.path().join("legacy.json");
        let data = serde_json::json!({
            "github_sources": [],
            "github_sources_legacy": [
                { "name": "old-src", "url": "https://github.com/org/repo", "branch": "main" }
            ],
            "clawhub": { "enabled": false, "base_url": "" }
        });
        std::fs::write(&path, serde_json::to_string_pretty(&data).unwrap()).unwrap();
        cmd_source_list(&path).unwrap();

        // 配置文件存在但全部为空 → found_any=false → "No registries configured."（832）
        let empty_path = tmp.path().join("empty.json");
        std::fs::write(&empty_path, "{}").unwrap();
        cmd_source_list(&empty_path).unwrap();
    }

    // -----------------------------------------------------------------------
    // cmd_learn：成功链路（1246 Ok 臂 / 1255-1258 提示拼装 / 1267-1272 输出 /
    // 1277 收尾）—— 经 CLI build_agent_loop 的 OpenAI 兼容 api_base 打到本地
    // mock；以及 provider 连不通时 process_direct Err → "Agent error"（1273-1274，
    // api_base 用 127.0.0.1:9 回环拒绝，agent_factory 测试同款手法）。
    // -----------------------------------------------------------------------

    fn wave_b_completions_payload() -> String {
        serde_json::json!({
            "id": "wave-b-1",
            "object": "chat.completion",
            "choices": [{
                "index": 0,
                "message": { "role": "assistant", "content": "wave-b learn mock reply" },
                "finish_reason": "stop"
            }],
            "usage": { "prompt_tokens": 11, "completion_tokens": 5, "total_tokens": 16 }
        })
        .to_string()
    }

    fn wave_b_write_llm_home(home: &std::path::Path, api_base: &str) {
        std::fs::create_dir_all(home).unwrap();
        let cfg = serde_json::json!({
            "agents": { "defaults": { "llm": "wave-b-model", "max_tool_iterations": 5 } },
            "model_list": [{
                "model_name": "wave-b-model",
                "model": "testai/wave-b-model",
                "api_key": "wave-b-key",
                "api_base": api_base,
                "model_tier": "mini"
            }]
        });
        std::fs::write(home.join("config.json"), cfg.to_string()).unwrap();
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wave_b_cmd_learn_success_round_trip_local_llm_mock() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        let _mock = WaveBMock::start(
            vec![("/chat/completions", wave_b_completions_payload())],
            1,
        );
        wave_b_write_llm_home(&home, &format!("{}/v1", _mock.base_url()));

        cmd_learn(&home, "some learning source", Some("wave-b-name"))
            .await
            .expect("本地 mock 应答 → cmd_learn 全链路 Ok（1246/1255-1258/1267-1272/1277）");
    }

    #[tokio::test(flavor = "multi_thread")]
    async fn wave_b_cmd_learn_provider_failure_surfaces_agent_error() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        // provider 构造离线可成功（:9 是保留丢弃端口，仅回环连接拒绝）
        wave_b_write_llm_home(&home, "http://127.0.0.1:9");

        let err = cmd_learn(&home, "src", None)
            .await
            .expect_err("provider 不可达 → process_direct Err → Agent error bail");
        assert!(err.to_string().contains("Agent error"), "err: {err}");
    }
}

// ===========================================================================
// wave_c（覆盖率补测 2026-08-27）——cmd_install 的 ClawHub ZIP 安装成功链。
//
// 分类定性轮认定的安装成功链残行（563-566 `Ok(version)` 的 ✅ Installed +
// Location 打印臂、575 收尾 `Ok(())`）：wave_b 只把 install 的【失败】终局
// 钉死（毒化回退），成功臂此前只能走真网。真正的 seam 在 config.skills.json
// 的 ClawHubConfig：base_url / convex_url / convex_site_url 三者皆可注入，
// RegistryManager::from_config（registry.rs「ClawHub support」段）原样喂给
// with_urls → getBySlug 明细 + ZIP 下载两条网络腿全部落在 127.0.0.1 随机
// 高位端口的本地 mock 上，全程零外联。
//
// mock 返回的 ZIP 由本模块手搓 STORE（method 0）格式拼装：nemesisbot 的
// dev-deps 只有 tempfile，不为测试改 Cargo.toml；CRC32 用无表位旋算法，
// zip crate 解压时实校验 CRC，算错即红——保留的是真实解压+落盘路径。
// ZIP 成功后 download_and_install 提前返回，绝不会坠入硬编码
// api.github.com 的 GitHub Trees 回退（那条分支保持豁免口径不变）。
// ===========================================================================

mod wave_c {
    use super::*;

    /// 无表 CRC32（IEEE 反射多项式 0xEDB88320）。
    fn wave_c_crc32(data: &[u8]) -> u32 {
        let mut crc: u32 = 0xFFFF_FFFF;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                let mask = (crc & 1).wrapping_neg();
                crc = (crc >> 1) ^ (0xEDB8_8320 & mask);
            }
        }
        !crc
    }

    /// 手搓最小 STORE-ZIP（每条目 local header + 中央目录 + EOCD）。
    fn wave_c_store_zip(entries: &[(&str, &[u8])]) -> Vec<u8> {
        struct CentralEntry {
            name: Vec<u8>,
            crc: u32,
            size: u32,
            offset: u32,
        }

        let mut out: Vec<u8> = Vec::new();
        let mut centrals: Vec<CentralEntry> = Vec::new();

        for (name, data) in entries {
            let name = name.as_bytes().to_vec();
            let offset = out.len() as u32;
            out.extend_from_slice(&[0x50, 0x4B, 0x03, 0x04]); // local header sig
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method = STORE
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0x5821u16.to_le_bytes()); // mod date 2024-01-01
            let crc = wave_c_crc32(data);
            out.extend_from_slice(&crc.to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(&name);
            out.extend_from_slice(data);
            centrals.push(CentralEntry {
                name,
                crc,
                size: data.len() as u32,
                offset,
            });
        }

        let cd_offset = out.len() as u32;
        for c in &centrals {
            out.extend_from_slice(&[0x50, 0x4B, 0x01, 0x02]); // central dir sig
            out.extend_from_slice(&20u16.to_le_bytes()); // version made by
            out.extend_from_slice(&20u16.to_le_bytes()); // version needed
            out.extend_from_slice(&0u16.to_le_bytes()); // flags
            out.extend_from_slice(&0u16.to_le_bytes()); // method
            out.extend_from_slice(&0u16.to_le_bytes()); // mod time
            out.extend_from_slice(&0x5821u16.to_le_bytes()); // mod date
            out.extend_from_slice(&c.crc.to_le_bytes());
            out.extend_from_slice(&c.size.to_le_bytes());
            out.extend_from_slice(&c.size.to_le_bytes());
            out.extend_from_slice(&(c.name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes()); // extra len
            out.extend_from_slice(&0u16.to_le_bytes()); // comment len
            out.extend_from_slice(&0u16.to_le_bytes()); // disk start
            out.extend_from_slice(&0u16.to_le_bytes()); // internal attrs
            out.extend_from_slice(&0u32.to_le_bytes()); // external attrs
            out.extend_from_slice(&c.offset.to_le_bytes());
            out.extend_from_slice(&c.name);
        }
        let cd_size = out.len() as u32 - cd_offset;

        out.extend_from_slice(&[0x50, 0x4B, 0x05, 0x06]); // EOCD sig
        out.extend_from_slice(&0u16.to_le_bytes()); // this disk
        out.extend_from_slice(&0u16.to_le_bytes()); // cd disk
        out.extend_from_slice(&(centrals.len() as u16).to_le_bytes());
        out.extend_from_slice(&(centrals.len() as u16).to_le_bytes());
        out.extend_from_slice(&cd_size.to_le_bytes());
        out.extend_from_slice(&cd_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes()); // comment len
        out
    }

    /// ClawHub 双端点本地 mock（绑 127.0.0.1 随机高位端口）：
    /// POST /api/query（skills:getBySlug）→ Convex envelope 明细；
    /// GET /api/v1/download?slug=… → application/zip 字节流。
    /// 每应答往通道发一个路由标签（"convex"/"zip"）作调用凭证；
    /// 请求按声明 Content-Length 读满再应答，防大请求体半读。
    fn wave_c_start_clawhub_mock(
        convex_json: String,
        zip: Vec<u8>,
    ) -> (
        String,
        std::sync::mpsc::Receiver<&'static str>,
    ) {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("bind ephemeral port");
        let addr = listener.local_addr().unwrap();
        let (tx, rx) = std::sync::mpsc::channel::<&'static str>();
        std::thread::spawn(move || {
            use std::io::{Read, Write};
            for _ in 0..2 {
                let Ok((mut stream, _)) = listener.accept() else {
                    break;
                };
                let mut buf: Vec<u8> = Vec::new();
                let mut chunk = [0u8; 4096];
                loop {
                    match stream.read(&mut chunk) {
                        Ok(0) | Err(_) => break,
                        Ok(n) => {
                            buf.extend_from_slice(&chunk[..n]);
                            if let Some(h) = buf
                                .windows(4)
                                .position(|w| w == b"\r\n\r\n")
                                .map(|p| p + 4)
                            {
                                let clen: usize = String::from_utf8_lossy(&buf[..h])
                                    .to_ascii_lowercase()
                                    .lines()
                                    .find_map(|l| {
                                        l.strip_prefix("content-length:")
                                            .and_then(|v| v.trim().parse().ok())
                                    })
                                    .unwrap_or(0);
                                if buf.len() >= h + clen {
                                    break;
                                }
                            }
                        }
                    }
                }
                let req_head = String::from_utf8_lossy(&buf).to_ascii_lowercase();
                let (served, ctype, body): (&'static str, &'static str, Vec<u8>) =
                    if req_head.contains("/api/v1/download") {
                        ("zip", "application/zip", zip.clone())
                    } else {
                        ("convex", "application/json", convex_json.clone().into_bytes())
                    };
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {cl}\r\nConnection: close\r\n\r\n",
                    ct = ctype,
                    cl = body.len()
                );
                let _ = stream.write_all(resp.as_bytes());
                let _ = stream.write_all(&body);
                let _ = stream.flush();
                let _ = tx.send(served);
            }
        });
        (format!("http://{addr}"), rx)
    }

    const WAVE_C_SKILL_MD: &str = "# wavec-zip-skill\nname: wavec-zip-skill\ndescription: offline ClawHub ZIP install regression\nsteps:\n- say hi from the local mock\n";

    /// 主回归：config 注入本地 ClawHub 三 URL → cmd_install 全成功链。
    /// 覆盖意图：555 📥 打印、562 manager.install Ok、563-566 ✅ Installed +
    /// Location、575 收尾 Ok(())；下游 ClawHubRegistry 的 getBySlug 解析、
    /// ZIP content-type 校验、STORE 解压、单顶层文件非平铺直写、目标落盘。
    #[tokio::test]
    async fn wave_c_cmd_install_clawhub_zip_success_round_trip() {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        let skills_dir = crate::common::workspace_path(&home).join("skills");

        let zip = wave_c_store_zip(&[("SKILL.md", WAVE_C_SKILL_MD.as_bytes())]);
        let detail = serde_json::json!({
            "status": "success",
            "value": {
                "owner": { "handle": "wavec-author" },
                "skill": {
                    "slug": "wavec-zip-skill",
                    "displayName": "WaveC Zip Skill",
                    "summary": "offline ClawHub ZIP install round trip",
                    "stats": { "downloads": 7 }
                },
                "latestVersion": { "version": "1.2.3" },
                "resolvedSlug": "wavec-zip-skill"
            }
        })
        .to_string();

        let (base, rx) = wave_c_start_clawhub_mock(detail, zip);
        write_skills_cfg(
            &home,
            &serde_json::json!({
                "search_cache": { "enabled": false },
                "clawhub": {
                    "enabled": true,
                    "base_url": base,
                    "convex_url": base,
                    "convex_site_url": base
                }
            }),
        );

        cmd_install(&skills_dir, &skills_cfg_of(&home), "clawhub/wavec-zip-skill")
            .await
            .expect("ClawHub ZIP 成功链必须 Ok（✅ Installed 臂 + 收尾 Ok）");

        // 断言点①：技能文件确实落到 tempdir 工作区 skills/<slug>/SKILL.md 且内容匹配。
        let landed = skills_dir.join("wavec-zip-skill").join("SKILL.md");
        assert!(landed.exists(), "SKILL.md 必须落在 workspace skills/<slug>/ 下");
        assert_eq!(
            std::fs::read_to_string(&landed).unwrap(),
            WAVE_C_SKILL_MD,
            "落盘内容须与 ZIP 内条目逐字节一致"
        );

        // 断言点②：真实网络链路确实走过两条腿（convex 明细 → site ZIP 下载），
        // 而不是某处短路提前返回 Ok。
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok("convex"),
            "第一腿：POST /api/query（skills:getBySlug）"
        );
        assert_eq!(
            rx.recv_timeout(std::time::Duration::from_secs(10)),
            Ok("zip"),
            "第二腿：GET /api/v1/download（application/zip）"
        );
    }
}

// ===========================================================================
// r10 wave（覆盖率 95% goal 第七波）——cmd_source_add 主干 + 安装离线终点臂。
//
// GitHub 端点（api.github.com / raw.githubusercontent.com）在 cmd_* 内部硬编码、
// 无 URL 注入 seam，无法指到本地 mock；改用 reqwest 的系统代理语义：
// HTTPS_PROXY 指向回环死端口（127.0.0.1:9）后所有出网连接立刻被本机拒绝——
// 验证重试循环走满 3 次（~4s 睡眠）、detect_skill_structure 两分支全部
// if-let Err 落穿到默认兜底元组。为兼容"机器配了系统级代理导致真连上
// GitHub"的奇异环境，探测目标固定用真实存在的冻结仓库 octocat/Hello-World：
// 无论离线拒绝 / 在线验证成功 / 限流，主干最终都把同名源写进
// config.skills.json（网络世界的差异只影响写回之前是否早退，不影响写回断言；
// 重名拒绝臂在所有世界里都会执行到，因为它不依赖 verified 标志）。
// env 是进程全局 → 全部相关测试持 crate::GLOBAL_STATE_LOCK；prev 值按
// Option 恢复（wave_b HomeGuard 同款纪律）。
// ===========================================================================

mod r10_wave {
    use super::*;
    use std::ffi::OsString;

    /// 真实存在的冻结仓库：任何网络世界下 trunk 终局一致。
    const R10_URL: &str = "octocat/Hello-World";

    struct R10OfflineNet {
        prev: Option<OsString>,
    }

    impl R10OfflineNet {
        fn engage() -> Self {
            let prev = std::env::var_os("HTTPS_PROXY");
            unsafe { std::env::set_var("HTTPS_PROXY", "http://127.0.0.1:9") };
            Self { prev }
        }
    }

    impl Drop for R10OfflineNet {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("HTTPS_PROXY", v) },
                None => unsafe { std::env::remove_var("HTTPS_PROXY") },
            }
        }
    }

    struct R10HomeGuard {
        prev: Option<OsString>,
    }

    impl R10HomeGuard {
        fn set(root: &std::path::Path) -> Self {
            let prev = std::env::var_os("NEMESISBOT_HOME");
            unsafe { std::env::set_var("NEMESISBOT_HOME", root) };
            Self { prev }
        }
    }

    impl Drop for R10HomeGuard {
        fn drop(&mut self) {
            match self.prev.take() {
                Some(v) => unsafe { std::env::set_var("NEMESISBOT_HOME", v) },
                None => unsafe { std::env::remove_var("NEMESISBOT_HOME") },
            }
        }
    }

    fn r10_fresh_home() -> (TempDir, std::path::PathBuf) {
        let tmp = TempDir::new().unwrap();
        let home = tmp.path().join(".nemesisbot");
        std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
        (tmp, home)
    }

    /// 主干全链：parse → 验证（重试耗尽或成功皆可）→ 结构探测兜底 →
    /// 写回 github_sources + github_sources_legacy 双份登记。
    #[tokio::test]
    async fn r10_skills_source_add_trunk_persists_new_registry_source() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = R10OfflineNet::engage();
        let (_tmp, home) = r10_fresh_home();
        let cfg = skills_cfg_of(&home);

        cmd_source_add(&cfg, R10_URL)
            .await
            .expect("无论验证成功与否主干都应写回配置并返回 Ok");

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap())
                .expect("config.skills.json 必须被写回且是合法 JSON");
        let srcs = saved["github_sources"]
            .as_array()
            .expect("sources 数组存在");
        assert_eq!(srcs.len(), 1, "恰好登记一个新源");
        assert_eq!(srcs[0]["name"], "Hello-World");
        assert_eq!(srcs[0]["repo"], "octocat/Hello-World");
        assert_eq!(srcs[0]["enabled"], true);
        let legacy = saved["github_sources_legacy"].as_array().unwrap();
        assert_eq!(legacy.len(), 1);
        assert_eq!(legacy[0]["name"], "Hello-World");
        assert_eq!(legacy[0]["url"], R10_URL);
    }

    /// 重名拒绝臂：检测阶段照跑（含网络重试/探测），但走到重名检查时
    /// 提前 Ok 返回——既不加新条目也不写 legacy。
    #[tokio::test]
    async fn r10_skills_source_add_duplicate_name_rejected_before_second_write() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = R10OfflineNet::engage();
        let (_tmp, home) = r10_fresh_home();

        write_skills_cfg(
            &home,
            &serde_json::json!({
                "github_sources": [
                    { "name": "Hello-World", "repo": "original/holder",
                      "branch": "main", "index_type": "flat", "enabled": true,
                      "skill_path_pattern": "skills/{slug}/SKILL.md" }
                ]
            }),
        );
        let cfg = skills_cfg_of(&home);

        cmd_source_add(&cfg, R10_URL)
            .await
            .expect("重名时提前返回 Ok（打印 Error 后结束）");

        let saved: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        let srcs = saved["github_sources"].as_array().unwrap();
        assert_eq!(srcs.len(), 1, "不得出现第二个同名源");
        assert_eq!(srcs[0]["repo"], "original/holder", "原条目原样保留");
        // 提前返回发生在 legacy push 之前 → 空的 github_sources_legacy 会被
        // 序列化器整个省略（实测：缺省键而非空数组）。键缺省 / 非数组 / 空数组
        // 都视为「未追加」；断言面不用会毒化全局锁的 unwrap。
        let legacy_added = saved
            .get("github_sources_legacy")
            .and_then(|v| v.as_array())
            .map(|a| !a.is_empty())
            .unwrap_or(false);
        assert!(!legacy_added, "提前返回前不得追加 legacy 登记");
    }

    /// run() 的 AddSource 分发桥（合法 URL 变体）：此前只有非法 URL 早退版本，
    /// 这里补上「桥接体真正驱动完整个 async 主干」的分发路径。
    #[tokio::test(flavor = "multi_thread")]
    async fn r10_run_add_source_arm_drives_full_trunk_via_bridge() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp_root = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp_root.path().join(".nemesisbot")).unwrap();
        let _home = R10HomeGuard::set(tmp_root.path());
        let _net = R10OfflineNet::engage();

        run(
            SkillsAction::AddSource {
                url: R10_URL.to_string(),
            },
            false,
        )
        .expect("分发桥 + 主干全链应 Ok");

        let cfg = skills_cfg_of(&tmp_root.path().join(".nemesisbot"));
        assert!(cfg.exists(), "run() 链路同样必须落盘配置");
    }

    /// cmd_install_github 的下载穷尽终局：owner/repo 合法解析后 4 路径 × 2
    /// 分支共 8 次 raw 请求全部失败（离线被拒 / 在线 404 / 限流），落在
    /// "Failed to download skill from GitHub" 打印后干净收尾。
    #[tokio::test]
    async fn r10_cmd_install_github_download_exhaustion_ends_cleanly() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = R10OfflineNet::engage();
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");

        cmd_install_github(&skills_dir, R10_URL)
            .await
            .expect("8 连 miss 必须以 Ok 收尾而非报错");

        // 失败穷尽路径不创建任何安装目录（只有下载成功的写入分支才建目录）。
        assert!(!skills_dir.join("Hello-World").exists());
    }

    /// cmd_install_clawhub 不可达终点臂：openclaw/skills 的 raw URL 打不通时
    /// 打印排障提示并 Ok 收尾；output_name None / Some 两变体都过一遍
    /// （覆盖 out_name 解析两个臂）。
    #[tokio::test]
    async fn r10_cmd_install_clawhub_unreachable_reports_and_returns_ok() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _net = R10OfflineNet::engage();
        let tmp = TempDir::new().unwrap();
        let skills_dir = tmp.path().join("skills");

        cmd_install_clawhub(&skills_dir, "r10-none-author", "r10-none-skill", None)
            .await
            .expect("下载失败也必须收在 Ok");
        cmd_install_clawhub(
            &skills_dir,
            "r10-none-author",
            "r10-none-skill",
            Some("r10-renamed-output"),
        )
        .await
        .expect("output_name 变体同样 Ok");

        assert!(!skills_dir.join("r10-renamed-output").exists());
    }

    /// run() 的 InstallClawhub 分发桥：既有测试从未驱动过的 async 命令包装臂
    /// （block_in_place + Handle::block_on 承载内部异步 client）。
    #[tokio::test(flavor = "multi_thread")]
    async fn r10_run_install_clawhub_bridge_dispatches_offline_error_arm() {
        let _lock = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let tmp_root = TempDir::new().unwrap();
        std::fs::create_dir_all(tmp_root.path().join(".nemesisbot")).unwrap();
        let _home = R10HomeGuard::set(tmp_root.path());
        let _net = R10OfflineNet::engage();

        run(
            SkillsAction::InstallClawhub {
                author: "r10-none-author".to_string(),
                skill_name: "r10-none-skill".to_string(),
                output_name: None,
            },
            false,
        )
        .expect("分发桥驱动下载失败臂后收在 Ok");
    }
}
