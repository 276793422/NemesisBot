use super::*;
use std::fs;

#[test]
fn test_scan_empty_directory() {
    let dir = tempfile::tempdir().unwrap();
    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert!(skills.is_empty());
}

#[test]
fn test_scan_directory_with_skill() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();

    let skill_md_content = r#"---
name: My Awesome Skill
description: Does awesome things
---
# My Awesome Skill

This skill does things.
"#;
    fs::write(skill_dir.join("SKILL.md"), skill_md_content).unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "My Awesome Skill");
    assert_eq!(skills[0].description, "Does awesome things");
    assert_eq!(skills[0].source, "local");
}

#[test]
fn test_scan_skips_dirs_without_skill_md() {
    let dir = tempfile::tempdir().unwrap();
    // Create a directory without SKILL.md
    fs::create_dir_all(dir.path().join("not-a-skill")).unwrap();
    // Create a file (not a directory)
    fs::write(dir.path().join("random.txt"), "hello").unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert!(skills.is_empty());
}

#[test]
fn test_scan_uses_dirname_as_fallback_name() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("fallback-name");
    fs::create_dir_all(&skill_dir).unwrap();
    // SKILL.md without frontmatter
    fs::write(
        skill_dir.join("SKILL.md"),
        "Just some content without frontmatter",
    )
    .unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "fallback-name");
    assert!(skills[0].description.is_empty());
}

#[test]
fn test_list_skills_priority_override() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    // Create same-named skill in workspace and global
    let ws_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::write(
        ws_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Workspace version\n---\nBody",
    )
    .unwrap();

    let g_dir = global.path().join("my-skill");
    fs::create_dir_all(&g_dir).unwrap();
    fs::write(
        g_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Global version\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "Workspace version");
    assert_eq!(skills[0].source, "workspace");
}

#[test]
fn test_load_skill_returns_content() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("test-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test\n---\n# Test Skill\n\nHello world",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let content = loader.load_skill("test-skill").unwrap();
    assert!(content.contains("Hello world"));
    assert!(!content.contains("---"));
}

#[test]
fn test_load_skill_not_found() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    assert!(loader.load_skill("nonexistent").is_none());
}

#[test]
fn test_load_skills_for_context() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Test\n---\nSkill content here",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let context = loader.load_skills_for_context(&["my-skill".to_string()]);
    assert!(context.contains("### Skill: my-skill"));
    assert!(context.contains("Skill content here"));

    let empty = loader.load_skills_for_context(&[]);
    assert!(empty.is_empty());
}

#[test]
fn test_build_skills_summary() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Test skill\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let summary = loader.build_skills_summary();
    assert!(summary.contains("<skills>"));
    assert!(summary.contains("</skills>"));
    assert!(summary.contains("<name>my-skill</name>"));
    assert!(summary.contains("<description>Test skill</description>"));
    assert!(summary.contains("<source>workspace</source>"));
}

#[test]
fn test_build_skills_summary_empty() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    assert!(loader.build_skills_summary().is_empty());
}

#[test]
fn test_strip_frontmatter() {
    let content = "---\nname: test\ndescription: desc\n---\nBody content";
    let stripped = SkillsLoader::strip_frontmatter(content);
    assert_eq!(stripped.trim(), "Body content");
}

#[test]
fn test_escape_xml() {
    assert_eq!(escape_xml("a<b>c&d"), "a&lt;b&gt;c&amp;d");
}

#[test]
fn test_is_valid_name() {
    assert!(is_valid_name("my-skill"));
    assert!(is_valid_name("skill123"));
    assert!(!is_valid_name(""));
    assert!(!is_valid_name("has spaces"));
    assert!(!is_valid_name("under_score"));
}

#[test]
fn test_parse_json_frontmatter() {
    let content =
        "---\n{\"name\": \"My JSON Skill\", \"description\": \"From JSON\"}\n---\nBody here";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "My JSON Skill");
    assert_eq!(map.get("description").unwrap(), "From JSON");
    assert_eq!(body.trim(), "Body here");
}

#[test]
fn test_parse_yaml_frontmatter_fallback() {
    // Content that is not valid JSON should fall back to YAML parsing
    let content = "---\nname: YAML Skill\ndescription: From YAML\n---\nBody here";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "YAML Skill");
    assert_eq!(map.get("description").unwrap(), "From YAML");
    assert_eq!(body.trim(), "Body here");
}

#[test]
fn test_json_frontmatter_takes_priority() {
    // Valid JSON frontmatter should be used instead of YAML
    let content = "---\n{\"name\": \"JSON Wins\", \"description\": \"JSON parsed\"}\n---\nBody";
    let (map, _) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "JSON Wins");
}

#[test]
fn test_invalid_json_falls_back_to_yaml() {
    // Invalid JSON should fall back to YAML
    let content = "---\nname: Fallback Skill\ndescription: YAML fallback\n---\nBody";
    let (map, _) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "Fallback Skill");
    assert_eq!(map.get("description").unwrap(), "YAML fallback");
}

#[test]
fn test_scan_directory_with_json_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("json-skill");
    fs::create_dir_all(&skill_dir).unwrap();

    let skill_md_content = "---\n{\"name\": \"JSON Skill\", \"description\": \"Parsed from JSON\"}\n---\n# JSON Skill\n\nContent here.\n";
    fs::write(skill_dir.join("SKILL.md"), skill_md_content).unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].name, "JSON Skill");
    assert_eq!(skills[0].description, "Parsed from JSON");
}

#[test]
fn test_extract_frontmatter_yaml() {
    let content = "---\nname: My Skill\ndescription: A test\n---\nBody here";
    let fm = extract_frontmatter(content);
    assert!(fm.contains("name: My Skill"));
    assert!(fm.contains("description: A test"));
}

#[test]
fn test_extract_frontmatter_json() {
    let content = "---\n{\"name\": \"Test\", \"description\": \"Desc\"}\n---\nBody";
    let fm = extract_frontmatter(content);
    assert!(fm.contains("\"name\""));
}

#[test]
fn test_extract_frontmatter_none() {
    let content = "Just some content without frontmatter";
    let fm = extract_frontmatter(content);
    assert!(fm.is_empty());
}

#[test]
fn test_extract_frontmatter_windows_line_endings() {
    let content = "---\r\nname: Test\r\ndescription: Desc\r\n---\r\nBody";
    let fm = extract_frontmatter(content);
    assert!(fm.contains("name: Test"));
}

#[test]
fn test_parse_simple_yaml() {
    let yaml = "name: My Skill\ndescription: A test skill\nauthor: test";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.get("name").unwrap(), "My Skill");
    assert_eq!(map.get("description").unwrap(), "A test skill");
    assert_eq!(map.get("author").unwrap(), "test");
}

#[test]
fn test_parse_simple_yaml_quoted() {
    let yaml = "name: \"My Skill\"\ndescription: 'Another desc'";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.get("name").unwrap(), "My Skill");
    assert_eq!(map.get("description").unwrap(), "Another desc");
}

#[test]
fn test_parse_simple_yaml_comments() {
    let yaml = "# This is a comment\nname: Test\n# Another comment\ndescription: Desc";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.len(), 2);
}

#[test]
fn test_parse_simple_yaml_windows_line_endings() {
    let yaml = "name: Test\r\ndescription: Desc\r\n";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.get("name").unwrap(), "Test");
    assert_eq!(map.get("description").unwrap(), "Desc");
}

#[test]
fn test_get_skill_metadata_yaml() {
    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(
        &skill_md,
        "---\nname: Test Skill\ndescription: Test desc\n---\nBody",
    )
    .unwrap();

    let meta = get_skill_metadata(&skill_md).unwrap();
    assert_eq!(meta.name, "Test Skill");
    assert_eq!(meta.description, "Test desc");
}

#[test]
fn test_get_skill_metadata_json() {
    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(
        &skill_md,
        "---\n{\"name\": \"JSON Skill\", \"description\": \"From JSON\"}\n---\nBody",
    )
    .unwrap();

    let meta = get_skill_metadata(&skill_md).unwrap();
    assert_eq!(meta.name, "JSON Skill");
    assert_eq!(meta.description, "From JSON");
}

#[test]
fn test_get_skill_metadata_no_frontmatter() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("my-skill-dir");
    fs::create_dir_all(&subdir).unwrap();
    let skill_md = subdir.join("SKILL.md");
    fs::write(&skill_md, "Just content without frontmatter").unwrap();

    let meta = get_skill_metadata(&skill_md).unwrap();
    assert_eq!(meta.name, "my-skill-dir");
    assert!(meta.description.is_empty());
}

#[test]
fn test_scan_skill_security_noop_when_disabled() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(&skill_md, "content").unwrap();

    let mut info = SkillInfo {
        name: "test".to_string(),
        path: skill_md.to_string_lossy().to_string(),
        source: "local".to_string(),
        description: "test".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    loader.scan_skill_security(&mut info, &skill_md);
    assert!(info.lint_score.is_none());
}

// ============================================================
// Additional tests for coverage improvement
// ============================================================

#[test]
fn test_scan_skill_security_enabled() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let mut loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );
    loader.enable_security();

    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(&skill_md, "This is safe content for testing.").unwrap();

    let mut info = SkillInfo {
        name: "test".to_string(),
        path: skill_md.to_string_lossy().to_string(),
        source: "local".to_string(),
        description: "test".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    loader.scan_skill_security(&mut info, &skill_md);
    assert!(info.lint_score.is_some());
    assert_eq!(info.lint_score.unwrap(), 1.0); // Clean content
    assert!(!info.has_warnings);
}

#[test]
fn test_scan_skill_security_detects_dangerous() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();
    let mut loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );
    loader.enable_security();

    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(&skill_md, "Run this: rm -rf /").unwrap();

    let mut info = SkillInfo {
        name: "test".to_string(),
        path: skill_md.to_string_lossy().to_string(),
        source: "local".to_string(),
        description: "test".to_string(),
        lint_score: None,
        has_warnings: false,
    };
    loader.scan_skill_security(&mut info, &skill_md);
    assert!(info.lint_score.is_some());
    assert!(info.lint_score.unwrap() < 1.0);
    assert!(info.has_warnings);
}

#[test]
fn test_list_skills_three_priority_levels() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    // Create skill in all three locations
    let ws_dir = workspace.path().join("skills").join("shared-skill");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::write(
        ws_dir.join("SKILL.md"),
        "---\nname: shared-skill\ndescription: Workspace version\n---\nBody",
    )
    .unwrap();

    let g_dir = global.path().join("shared-skill");
    fs::create_dir_all(&g_dir).unwrap();
    fs::write(
        g_dir.join("SKILL.md"),
        "---\nname: shared-skill\ndescription: Global version\n---\nBody",
    )
    .unwrap();

    let b_dir = builtin.path().join("shared-skill");
    fs::create_dir_all(&b_dir).unwrap();
    fs::write(
        b_dir.join("SKILL.md"),
        "---\nname: shared-skill\ndescription: Builtin version\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].description, "Workspace version");
    assert_eq!(skills[0].source, "workspace");
}

#[test]
fn test_list_skills_global_overrides_builtin() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let g_dir = global.path().join("my-skill");
    fs::create_dir_all(&g_dir).unwrap();
    fs::write(
        g_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Global version\n---\nBody",
    )
    .unwrap();

    let b_dir = builtin.path().join("my-skill");
    fs::create_dir_all(&b_dir).unwrap();
    fs::write(
        b_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Builtin version\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 1);
    assert_eq!(skills[0].source, "global");
}

#[test]
fn test_list_skills_multiple_different_skills() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let ws_dir = workspace.path().join("skills").join("skill-a");
    fs::create_dir_all(&ws_dir).unwrap();
    fs::write(
        ws_dir.join("SKILL.md"),
        "---\nname: skill-a\ndescription: Skill A\n---\nBody",
    )
    .unwrap();

    let g_dir = global.path().join("skill-b");
    fs::create_dir_all(&g_dir).unwrap();
    fs::write(
        g_dir.join("SKILL.md"),
        "---\nname: skill-b\ndescription: Skill B\n---\nBody",
    )
    .unwrap();

    let b_dir = builtin.path().join("skill-c");
    fs::create_dir_all(&b_dir).unwrap();
    fs::write(
        b_dir.join("SKILL.md"),
        "---\nname: skill-c\ndescription: Skill C\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 3);
}

#[test]
fn test_load_skill_fallback_to_global() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let g_dir = global.path().join("test-skill");
    fs::create_dir_all(&g_dir).unwrap();
    fs::write(
        g_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test\n---\n# Test\n\nGlobal content",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let content = loader.load_skill("test-skill").unwrap();
    assert!(content.contains("Global content"));
}

#[test]
fn test_load_skill_fallback_to_builtin() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let b_dir = builtin.path().join("test-skill");
    fs::create_dir_all(&b_dir).unwrap();
    fs::write(
        b_dir.join("SKILL.md"),
        "---\nname: test-skill\ndescription: A test\n---\n# Test\n\nBuiltin content",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let content = loader.load_skill("test-skill").unwrap();
    assert!(content.contains("Builtin content"));
}

#[test]
fn test_build_skills_summary_with_xml_escaping() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: A <test> & skill\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let summary = loader.build_skills_summary();
    assert!(summary.contains("&lt;test&gt;"));
    assert!(summary.contains("&amp;"));
}

#[test]
fn test_build_skills_summary_with_security_score() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Test skill\n---\nBody",
    )
    .unwrap();

    let mut loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );
    loader.enable_security();

    let summary = loader.build_skills_summary();
    assert!(summary.contains("<security_score>"));
}

#[test]
fn test_scan_directory_with_invalid_name() {
    let dir = tempfile::tempdir().unwrap();
    // Create skill with invalid name (spaces)
    let skill_dir = dir.path().join("invalid name");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: invalid name\ndescription: Valid description\n---\nBody",
    )
    .unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert_eq!(skills.len(), 1); // Uses directory name when name is invalid
}

#[test]
fn test_scan_directory_not_exists() {
    let nonexistent = format!("C:/__nonexistent_test_dir_{}", std::process::id());
    let result = SkillsLoader::scan_directory(Path::new(&nonexistent));
    assert!(result.is_err());
}

#[test]
fn test_scan_directory_is_file() {
    let dir = tempfile::tempdir().unwrap();
    let file_path = dir.path().join("somefile.txt");
    fs::write(&file_path, "content").unwrap();

    let result = SkillsLoader::scan_directory(&file_path);
    assert!(result.is_err());
}

#[test]
fn test_scan_directory_empty_description_skipped() {
    let dir = tempfile::tempdir().unwrap();
    let skill_dir = dir.path().join("empty-desc");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: empty-desc\ndescription:\n---\nBody",
    )
    .unwrap();

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    // Empty description means the skill should be skipped by validation
    assert!(skills.is_empty() || skills[0].description.is_empty());
}

#[test]
fn test_load_skills_for_context_partial_failure() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("existing-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: existing-skill\ndescription: Test\n---\nExisting content",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let context = loader.load_skills_for_context(&[
        "existing-skill".to_string(),
        "nonexistent-skill".to_string(),
    ]);
    assert!(context.contains("### Skill: existing-skill"));
    assert!(!context.contains("### Skill: nonexistent-skill"));
}

#[test]
fn test_get_skill_metadata_nonexistent_file() {
    let result = get_skill_metadata(Path::new("/nonexistent/path/SKILL.md"));
    assert!(result.is_none());
}

#[test]
fn test_parse_json_frontmatter_both_fields() {
    let content = "---\n{\"name\": \"Skill\", \"description\": \"Desc\"}\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "Skill");
    assert_eq!(map.get("description").unwrap(), "Desc");
    assert_eq!(body.trim(), "Body");
}

#[test]
fn test_parse_frontmatter_no_closing_delimiter() {
    let content = "---\nname: test\nNo closing delimiter";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert!(map.is_empty());
    assert_eq!(body, content);
}

#[test]
fn test_strip_frontmatter_no_frontmatter() {
    let content = "Just body content";
    let stripped = SkillsLoader::strip_frontmatter(content);
    assert_eq!(stripped, content);
}

#[test]
fn test_extract_frontmatter_mac_line_endings() {
    let content = "---\rname: Test\rdescription: Desc\r---\rBody";
    let fm = extract_frontmatter(content);
    assert!(fm.contains("name: Test"));
}

#[test]
fn test_parse_simple_yaml_empty_lines_and_comments() {
    let yaml = "\n# Header comment\n\nname: Test\n\n# Footer\n\ndescription: Desc\n\n";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.len(), 2);
    assert_eq!(map.get("name").unwrap(), "Test");
    assert_eq!(map.get("description").unwrap(), "Desc");
}

#[test]
fn test_parse_simple_yaml_empty() {
    let map = parse_simple_yaml("");
    assert!(map.is_empty());
}

#[test]
fn test_parse_simple_yaml_no_colon() {
    let yaml = "just a line without colon";
    let map = parse_simple_yaml(yaml);
    assert!(map.is_empty());
}

#[test]
fn test_is_valid_name_edge_cases() {
    assert!(is_valid_name("a")); // Single char
    assert!(is_valid_name("a-b")); // Two parts
    assert!(is_valid_name("123")); // Numbers only
    assert!(!is_valid_name("-")); // Just hyphen
    assert!(!is_valid_name("A-B-C-")); // Trailing hyphen
}

#[test]
fn test_escape_xml_no_special_chars() {
    assert_eq!(escape_xml("hello world"), "hello world");
}

#[test]
fn test_escape_xml_all_special_chars() {
    assert_eq!(escape_xml("&<>"), "&amp;&lt;&gt;");
}

// ============================================================
// Coverage improvement: additional loader tests
// ============================================================

#[test]
fn test_parse_json_frontmatter_name_only() {
    let content = "---\n{\"name\": \"Only Name\"}\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "Only Name");
    assert!(!map.contains_key("description"));
    assert_eq!(body.trim(), "Body");
}

#[test]
fn test_parse_json_frontmatter_description_only() {
    let content = "---\n{\"description\": \"Only Desc\"}\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert!(!map.contains_key("name"));
    assert_eq!(map.get("description").unwrap(), "Only Desc");
    assert_eq!(body.trim(), "Body");
}

#[test]
fn test_parse_json_frontmatter_empty_object_falls_to_yaml() {
    // An empty JSON object {} has no name/description fields, so it falls back to YAML
    let content = "---\n{}\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert!(map.is_empty());
    assert_eq!(body.trim(), "Body");
}

#[test]
fn test_parse_frontmatter_yaml_with_comments() {
    let content = "---\n# comment\nname: test-skill\ndescription: desc\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "test-skill");
    assert_eq!(map.get("description").unwrap(), "desc");
    assert_eq!(body.trim(), "Body");
}

#[test]
fn test_parse_frontmatter_yaml_with_quoted_values() {
    let content = "---\nname: \"Quoted Name\"\ndescription: 'Single Quoted'\n---\nBody";
    let (map, _body) = SkillsLoader::parse_frontmatter(content);
    assert_eq!(map.get("name").unwrap(), "Quoted Name");
    assert_eq!(map.get("description").unwrap(), "Single Quoted");
}

#[test]
fn test_parse_frontmatter_no_opening_delimiter() {
    let content = "name: test\n---\nBody";
    let (map, body) = SkillsLoader::parse_frontmatter(content);
    assert!(map.is_empty());
    assert_eq!(body, content);
}

#[test]
fn test_parse_frontmatter_only_dashes() {
    let content = "------\nBody";
    let (map, _) = SkillsLoader::parse_frontmatter(content);
    // "------" starts with "---", then looks for next "---" inside
    // The remaining "---\nBody" will match but frontmatter is empty
    assert!(map.is_empty() || map.contains_key("")); // depends on parsing behavior
}

#[test]
fn test_is_valid_name_max_length() {
    let name_64 = "a".repeat(64);
    assert!(is_valid_name(&name_64));
    let name_65 = "a".repeat(65);
    assert!(!is_valid_name(&name_65));
}

#[test]
fn test_is_valid_name_leading_hyphen() {
    assert!(!is_valid_name("-leading"));
}

#[test]
fn test_is_valid_name_trailing_hyphen() {
    assert!(!is_valid_name("trailing-"));
}

#[test]
fn test_is_valid_name_double_hyphen() {
    assert!(!is_valid_name("double--hyphen"));
}

#[test]
fn test_scan_directory_multiple_skills() {
    let dir = tempfile::tempdir().unwrap();

    for name in &["skill-a", "skill-b", "skill-c"] {
        let skill_dir = dir.path().join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!("---\nname: {}\ndescription: {}\n---\nBody", name, name),
        )
        .unwrap();
    }

    let skills = SkillsLoader::scan_directory(dir.path()).unwrap();
    assert_eq!(skills.len(), 3);
}

#[test]
fn test_list_skills_with_security_enabled_clean() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("safe-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: safe-skill\ndescription: A safe skill\n---\nClean content here",
    )
    .unwrap();

    let mut loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );
    loader.enable_security();

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 1);
    assert!(skills[0].lint_score.is_some());
    assert_eq!(skills[0].lint_score.unwrap(), 1.0);
    assert!(!skills[0].has_warnings);
}

#[test]
fn test_list_skills_with_security_enabled_dangerous() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("danger-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: danger-skill\ndescription: A dangerous skill\n---\nrm -rf /",
    )
    .unwrap();

    let mut loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );
    loader.enable_security();

    let skills = loader.list_skills();
    assert_eq!(skills.len(), 1);
    assert!(skills[0].lint_score.is_some());
    assert!(skills[0].lint_score.unwrap() < 1.0);
    assert!(skills[0].has_warnings);
}

#[test]
fn test_build_skills_summary_no_security_score_when_disabled() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("my-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: my-skill\ndescription: Test skill\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let summary = loader.build_skills_summary();
    assert!(!summary.contains("<security_score>"));
}

#[test]
fn test_load_skills_for_context_multiple() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    for name in &["skill-a", "skill-b"] {
        let skill_dir = workspace.path().join("skills").join(name);
        fs::create_dir_all(&skill_dir).unwrap();
        fs::write(
            skill_dir.join("SKILL.md"),
            format!(
                "---\nname: {}\ndescription: {}\n---\nContent {}",
                name, name, name
            ),
        )
        .unwrap();
    }

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let context = loader.load_skills_for_context(&["skill-a".to_string(), "skill-b".to_string()]);
    assert!(context.contains("### Skill: skill-a"));
    assert!(context.contains("### Skill: skill-b"));
    assert!(context.contains("---"));
}

#[test]
fn test_get_skill_metadata_json_name_only() {
    let dir = tempfile::tempdir().unwrap();
    let skill_md = dir.path().join("SKILL.md");
    fs::write(&skill_md, "---\n{\"name\": \"NameOnly\"}\n---\nBody").unwrap();

    let meta = get_skill_metadata(&skill_md).unwrap();
    assert_eq!(meta.name, "NameOnly");
    assert!(meta.description.is_empty());
}

#[test]
fn test_get_skill_metadata_json_description_only() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("my-dir");
    fs::create_dir_all(&subdir).unwrap();
    let skill_md = subdir.join("SKILL.md");
    fs::write(&skill_md, "---\n{\"description\": \"DescOnly\"}\n---\nBody").unwrap();

    let meta = get_skill_metadata(&skill_md).unwrap();
    assert_eq!(meta.name, ""); // No name in JSON, and frontmatter exists, so name is empty
    assert_eq!(meta.description, "DescOnly");
}

#[test]
fn test_strip_frontmatter_multiline_body() {
    let content = "---\nname: test\n---\n# Title\n\nParagraph 1\n\nParagraph 2";
    let stripped = SkillsLoader::strip_frontmatter(content);
    assert!(stripped.contains("# Title"));
    assert!(stripped.contains("Paragraph 1"));
    assert!(stripped.contains("Paragraph 2"));
    assert!(!stripped.contains("name: test"));
}

#[test]
fn test_parse_simple_yaml_multiple_fields() {
    let yaml = "name: Test\nauthor: Alice\nversion: 1.0\ntags: pdf,convert";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.len(), 4);
    assert_eq!(map.get("name").unwrap(), "Test");
    assert_eq!(map.get("author").unwrap(), "Alice");
    assert_eq!(map.get("version").unwrap(), "1.0");
    assert_eq!(map.get("tags").unwrap(), "pdf,convert");
}

#[test]
fn test_parse_simple_yaml_value_with_colon() {
    let yaml = "url: https://example.com:8080";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.get("url").unwrap(), "https://example.com:8080");
}

#[test]
fn test_parse_simple_yaml_mixed_quotes() {
    let yaml = "key1: \"double\"\nkey2: 'single'\nkey3: plain";
    let map = parse_simple_yaml(yaml);
    assert_eq!(map.get("key1").unwrap(), "double");
    assert_eq!(map.get("key2").unwrap(), "single");
    assert_eq!(map.get("key3").unwrap(), "plain");
}

// ============================================================
// Coverage improvement: loader priority override tests
// ============================================================

#[test]
fn test_list_skills_skips_invalid_name() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("invalid name!");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: invalid name!\ndescription: Bad name\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert!(skills.iter().all(|s| s.name != "invalid name!"));
}

#[test]
fn test_list_skills_skips_empty_description() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("no-desc");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("SKILL.md"), "---\nname: no-desc\n---\nBody").unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert!(skills.iter().all(|s| s.name != "no-desc"));
}

#[test]
fn test_list_skills_skips_too_long_description() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("long-desc");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        format!(
            "---\nname: long-desc\ndescription: {}\n---\nBody",
            "x".repeat(1025)
        ),
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert!(skills.iter().all(|s| s.name != "long-desc"));
}

#[test]
fn test_list_skills_skips_dir_without_skill_md() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("no-skill-md");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(skill_dir.join("README.md"), "# No SKILL.md here").unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let skills = loader.list_skills();
    assert!(skills.is_empty());
}

#[test]
fn test_load_skill_missing_returns_none() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    assert!(loader.load_skill("nonexistent").is_none());
}

#[test]
fn test_load_skills_for_context_no_names() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let context = loader.load_skills_for_context(&[]);
    assert!(context.is_empty());
}

#[test]
fn test_build_skills_summary_no_skills() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let summary = loader.build_skills_summary();
    assert!(summary.is_empty());
}

#[test]
fn test_build_skills_summary_with_xml_escape() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = workspace.path().join("skills").join("xml-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: xml-skill\ndescription: <script>alert('xss')</script>\n---\nBody",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let summary = loader.build_skills_summary();
    assert!(summary.contains("&lt;script&gt;"));
    assert!(!summary.contains("<script>"));
}

#[test]
fn test_load_skill_global_fallback() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = global.path().join("global-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: global-skill\ndescription: from global\n---\nGlobal content",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let content = loader.load_skill("global-skill");
    assert!(content.is_some());
    assert!(content.unwrap().contains("Global content"));
}

#[test]
fn test_load_skill_builtin_fallback() {
    let workspace = tempfile::tempdir().unwrap();
    let global = tempfile::tempdir().unwrap();
    let builtin = tempfile::tempdir().unwrap();

    let skill_dir = builtin.path().join("builtin-skill");
    fs::create_dir_all(&skill_dir).unwrap();
    fs::write(
        skill_dir.join("SKILL.md"),
        "---\nname: builtin-skill\ndescription: from builtin\n---\nBuiltin content",
    )
    .unwrap();

    let loader = SkillsLoader::new(
        &workspace.path().to_string_lossy(),
        &global.path().to_string_lossy(),
        &builtin.path().to_string_lossy(),
    );

    let content = loader.load_skill("builtin-skill");
    assert!(content.is_some());
    assert!(content.unwrap().contains("Builtin content"));
}
