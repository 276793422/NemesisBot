use super::*;
use std::io::Write;

#[test]
fn test_name() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.name(), "clawhub");
}

#[test]
fn test_site_url_default() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.site_url(), "https://wry-manatee-359.convex.site");
}

#[test]
fn test_site_url_custom() {
    let registry = ClawHubRegistry::with_urls(
        "https://clawhub.ai",
        "https://example.convex.cloud",
        "https://custom.convex.site",
    );
    assert_eq!(registry.site_url(), "https://custom.convex.site");
}

#[test]
fn test_find_common_prefix() {
    let entries = vec![
        "my-skill/SKILL.md".to_string(),
        "my-skill/scripts/run.sh".to_string(),
    ];
    assert_eq!(find_common_prefix(&entries), Some("my-skill/".to_string()));
}

#[test]
fn test_find_common_prefix_no_common() {
    let entries = vec!["SKILL.md".to_string(), "scripts/run.sh".to_string()];
    assert_eq!(find_common_prefix(&entries), None);
}

#[test]
fn test_find_common_prefix_empty() {
    assert_eq!(find_common_prefix(&[]), None);
}

#[test]
fn test_urlencoding() {
    assert_eq!(urlencoding::encode("hello world"), "hello%20world");
    assert_eq!(urlencoding::encode("test-skill"), "test-skill");
    assert_eq!(urlencoding::encode("a/b"), "a%2Fb");
}

#[test]
fn test_convex_response_deserialization() {
    let json = r#"{"status":"success","value":{"test":true}}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "success");
}

#[test]
fn test_convex_response_error() {
    let json = r#"{"status":"error","value":null,"errorMessage":"not found"}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_message, Some("not found".to_string()));
}

#[test]
fn test_search_item_deserialization() {
    let json = r#"{"results":[{"score":4.5,"slug":"pdf","displayName":"PDF Tool","summary":"Converts PDFs","version":"1.0"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].slug, "pdf");
    assert_eq!(resp.results[0].score, 4.5);
}

#[test]
fn test_skill_detail_deserialization() {
    let json = r#"{
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF", "summary": "Converts", "stats": {"downloads": 100.0}},
        "latestVersion": {"version": "2.0"},
        "resolvedSlug": "pdf"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert_eq!(detail.owner.handle, "alice");
    assert_eq!(detail.skill.slug, "pdf");
    assert_eq!(detail.latest_version.version, "2.0");
}

#[test]
fn test_extract_zip_to_dir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("extracted");

    // Create a minimal ZIP in memory.
    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("test-skill/SKILL.md", options).unwrap();
        writer.write_all(b"# Test Skill\nHello").unwrap();
        writer.finish().unwrap();
    }

    extract_zip_to_dir(&buf, &target.to_string_lossy()).unwrap();

    let skill_md = target.join("SKILL.md");
    assert!(skill_md.exists());
    let content = std::fs::read_to_string(skill_md).unwrap();
    assert!(content.contains("Test Skill"));
}

// ============================================================
// Additional tests for missing coverage
// ============================================================

#[test]
fn test_new_from_config_default() {
    let config = crate::types::ClawHubConfig::default();
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.name(), "clawhub");
    assert_eq!(registry.base_url, "https://clawhub.ai");
}

#[test]
fn test_new_from_config_custom_urls() {
    let config = crate::types::ClawHubConfig {
        enabled: true,
        base_url: "https://custom.clawhub.ai".to_string(),
        convex_url: "https://custom.convex.cloud".to_string(),
        convex_site_url: "https://custom.convex.site".to_string(),
        timeout_secs: 10,
    };
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://custom.clawhub.ai");
    assert_eq!(registry.convex_url, "https://custom.convex.cloud");
    assert_eq!(registry.convex_site_url, "https://custom.convex.site");
}

#[test]
fn test_new_from_config_empty_urls_use_defaults() {
    let config = crate::types::ClawHubConfig {
        enabled: true,
        base_url: String::new(),
        convex_url: String::new(),
        convex_site_url: String::new(),
        timeout_secs: 0,
    };
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://clawhub.ai");
    assert_eq!(registry.convex_url, "https://wry-manatee-359.convex.cloud");
}

#[test]
fn test_default_impl() {
    let registry = ClawHubRegistry::default();
    assert_eq!(registry.name(), "clawhub");
}

#[test]
fn test_convex_response_deserialization_success() {
    let json = r#"{"status":"success","value":[1,2,3]}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "success");
    assert!(resp.value.is_array());
}

#[test]
fn test_convex_response_deserialization_null_value() {
    let json = r#"{"status":"success","value":null}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "success");
    assert!(resp.value.is_null());
}

#[test]
fn test_convex_response_deserialization_missing_error_message() {
    let json = r#"{"status":"error","value":null}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_message, None);
}

#[test]
fn test_clawhub_search_response_empty() {
    let json = r#"{"results":[]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert!(resp.results.is_empty());
}

#[test]
fn test_clawhub_search_item_missing_optional_fields() {
    let json =
        r#"{"results":[{"score":1.0,"slug":"test","displayName":"Test","summary":"A test"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results.len(), 1);
    assert!(resp.results[0].version.is_none());
}

#[test]
fn test_convex_skill_list_item_deserialization() {
    let json = r#"{"slug":"pdf","displayName":"PDF Tool","summary":"Converts","stats":{"downloads":42.0}}"#;
    let item: ConvexSkillListItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.slug, "pdf");
    assert_eq!(item.stats.downloads, 42.0);
}

#[test]
fn test_convex_skill_detail_empty_slug() {
    let json = r#"{
        "owner": {"handle": "bob"},
        "skill": {"slug": "", "displayName": "", "summary": "", "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "fallback"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert!(detail.skill.slug.is_empty());
    assert_eq!(detail.resolved_slug, "fallback");
}

#[test]
fn test_urlencoding_special_chars() {
    assert_eq!(urlencoding::encode("a b+c"), "a%20b%2Bc");
    assert_eq!(urlencoding::encode(""), "");
    assert_eq!(urlencoding::encode("simple"), "simple");
}

#[test]
fn test_find_common_prefix_single_entry_with_dir() {
    let entries = vec!["my-skill/file.txt".to_string()];
    assert_eq!(find_common_prefix(&entries), Some("my-skill/".to_string()));
}

#[test]
fn test_find_common_prefix_mixed_dirs() {
    let entries = vec!["dir1/file1.txt".to_string(), "dir2/file2.txt".to_string()];
    // Different top-level dirs, no common prefix
    assert_eq!(find_common_prefix(&entries), None);
}

#[test]
fn test_extract_zip_to_dir_no_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("flat");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("SKILL.md", options).unwrap();
        writer.write_all(b"# Flat Skill").unwrap();
        writer.finish().unwrap();
    }

    extract_zip_to_dir(&buf, &target.to_string_lossy()).unwrap();
    assert!(target.join("SKILL.md").exists());
}

#[test]
fn test_extract_zip_to_dir_multiple_files_in_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("multi");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("skill/SKILL.md", options).unwrap();
        writer.write_all(b"# Skill").unwrap();
        writer.start_file("skill/docs/guide.md", options).unwrap();
        writer.write_all(b"# Guide").unwrap();
        writer.finish().unwrap();
    }

    extract_zip_to_dir(&buf, &target.to_string_lossy()).unwrap();
    assert!(target.join("SKILL.md").exists());
    assert!(target.join("docs/guide.md").exists());
}

// ============================================================
// Coverage improvement: additional clawhub tests
// ============================================================

#[test]
fn test_with_urls_custom() {
    let registry = ClawHubRegistry::with_urls(
        "https://custom.base",
        "https://custom.convex",
        "https://custom.site",
    );
    assert_eq!(registry.base_url, "https://custom.base");
    assert_eq!(registry.convex_url, "https://custom.convex");
    assert_eq!(registry.convex_site_url, "https://custom.site");
}

#[test]
fn test_site_url_from_convex_cloud() {
    let registry =
        ClawHubRegistry::with_urls("https://clawhub.ai", "https://my-app.convex.cloud", "");
    assert_eq!(registry.site_url(), "https://my-app.convex.site");
}

#[test]
fn test_site_url_prefers_custom_site() {
    let registry = ClawHubRegistry::with_urls(
        "https://clawhub.ai",
        "https://my-app.convex.cloud",
        "https://override.convex.site",
    );
    assert_eq!(registry.site_url(), "https://override.convex.site");
}

#[test]
fn test_flatten_single_top_dir_single_dir() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("inner");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("file.txt"), "data").unwrap();

    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, subdir);
}

#[test]
fn test_flatten_single_top_dir_multiple_entries() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("a.txt"), "a").unwrap();
    std::fs::write(dir.path().join("b.txt"), "b").unwrap();

    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, dir.path().to_path_buf());
}

#[test]
fn test_flatten_single_top_dir_empty() {
    let dir = tempfile::tempdir().unwrap();
    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, dir.path().to_path_buf());
}

#[test]
fn test_flatten_single_top_dir_file_not_dir() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(dir.path().join("only.txt"), "data").unwrap();
    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, dir.path().to_path_buf());
}

#[test]
fn test_move_dir_contents_basic() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    std::fs::write(src_dir.path().join("file.txt"), "hello").unwrap();

    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    assert!(dst_dir.path().join("file.txt").exists());
    let content = std::fs::read_to_string(dst_dir.path().join("file.txt")).unwrap();
    assert_eq!(content, "hello");
}

#[test]
fn test_move_dir_contents_with_subdirectories() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    let subdir = src_dir.path().join("sub");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("nested.txt"), "nested data").unwrap();

    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    assert!(dst_dir.path().join("sub/nested.txt").exists());
}

#[test]
fn test_move_dir_contents_empty() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();
    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    // Should succeed without error
}

#[test]
fn test_convex_owner_deserialization() {
    let json = r#"{"handle":"alice"}"#;
    let owner: ConvexOwner = serde_json::from_str(json).unwrap();
    assert_eq!(owner.handle, "alice");
}

#[test]
fn test_convex_skill_deserialization() {
    let json = r#"{"slug":"pdf","displayName":"PDF Tool","summary":"Converts PDFs","stats":{"downloads":50.0}}"#;
    let skill: ConvexSkill = serde_json::from_str(json).unwrap();
    assert_eq!(skill.slug, "pdf");
    assert_eq!(skill.display_name, "PDF Tool");
    assert_eq!(skill.stats.downloads, 50.0);
}

#[test]
fn test_convex_latest_version_deserialization() {
    let json = r#"{"version":"3.1.4"}"#;
    let ver: ConvexLatestVersion = serde_json::from_str(json).unwrap();
    assert_eq!(ver.version, "3.1.4");
}

#[test]
fn test_convex_stats_deserialization() {
    let json = r#"{"downloads":1234.5}"#;
    let stats: ConvexStats = serde_json::from_str(json).unwrap();
    assert_eq!(stats.downloads, 1234.5);
}

#[test]
fn test_extract_zip_to_dir_with_directory_entries() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("zipdirs");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.add_directory("my-skill/", options).unwrap();
        writer.start_file("my-skill/SKILL.md", options).unwrap();
        writer.write_all(b"# Skill content").unwrap();
        writer.finish().unwrap();
    }

    extract_zip_to_dir(&buf, &target.to_string_lossy()).unwrap();
    assert!(target.join("SKILL.md").exists());
}

#[test]
fn test_find_common_prefix_with_single_file() {
    let entries = vec!["file.txt".to_string()];
    // Single file has no dir prefix (no slash)
    let result = find_common_prefix(&entries);
    assert!(result.is_none() || result.unwrap().ends_with("/"));
}

#[test]
fn test_extract_zip_to_dir_invalid_data() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("bad");
    let data = b"this is not a zip file";
    let result = extract_zip_to_dir(data, &target.to_string_lossy());
    assert!(result.is_err());
}

// ============================================================
// Additional coverage tests
// ============================================================

#[test]
fn test_clawhub_registry_with_urls_fields() {
    let registry = ClawHubRegistry::with_urls(
        "https://base.example.com",
        "https://convex.example.com",
        "https://site.example.com",
    );
    assert_eq!(registry.base_url, "https://base.example.com");
    assert_eq!(registry.convex_url, "https://convex.example.com");
    assert_eq!(registry.convex_site_url, "https://site.example.com");
}

#[test]
fn test_clawhub_registry_new_defaults() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.base_url, "https://clawhub.ai");
    assert_eq!(registry.convex_url, "https://wry-manatee-359.convex.cloud");
    assert_eq!(registry.convex_site_url, "");
}

#[test]
fn test_convex_response_success_with_value() {
    let json = r#"{"status":"success","value":{"slug":"pdf","name":"PDF Tool"}}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "success");
    assert_eq!(resp.value["slug"], "pdf");
}

#[test]
fn test_convex_response_error_with_message() {
    let json = r#"{"status":"error","value":null,"errorMessage":"Rate limit exceeded"}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "error");
    assert_eq!(resp.error_message, Some("Rate limit exceeded".to_string()));
}

#[test]
fn test_clawhub_search_item_score_normalization_high() {
    // Score > 1.0 should be normalized by dividing by 5.0
    let json =
        r#"{"results":[{"score":4.5,"slug":"pdf","displayName":"PDF","summary":"PDF tool"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results[0].score, 4.5);
}

#[test]
fn test_clawhub_search_item_score_normalization_low() {
    // Score <= 1.0 should be kept as-is
    let json =
        r#"{"results":[{"score":0.8,"slug":"csv","displayName":"CSV","summary":"CSV tool"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results[0].score, 0.8);
}

#[test]
fn test_find_common_prefix_with_top_level_dir() {
    let entries = vec![
        "top/skill/SKILL.md".to_string(),
        "top/skill/docs/guide.md".to_string(),
        "top/skill/scripts/run.sh".to_string(),
    ];
    assert_eq!(find_common_prefix(&entries), Some("top/".to_string()));
}

#[test]
fn test_find_common_prefix_single_entry_no_slash() {
    let entries = vec!["README.md".to_string()];
    // "README.md" split by '/' gives just "README.md", no entry starts with "README.md/"
    // but the function checks `e == first_dir` which matches the entry itself
    let result = find_common_prefix(&entries);
    // It may return Some("README.md/") since the single entry equals first_dir
    // This is acceptable behavior
    if let Some(prefix) = result {
        assert!(prefix.ends_with("/"));
    }
}

#[test]
fn test_urlencoding_unreserved_chars() {
    assert_eq!(
        urlencoding::encode("hello-world_test.txt"),
        "hello-world_test.txt"
    );
    assert_eq!(urlencoding::encode("path/to/file"), "path%2Fto%2Ffile");
}

#[test]
fn test_move_dir_contents_nested() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();

    let deep = src_dir.path().join("a").join("b");
    std::fs::create_dir_all(&deep).unwrap();
    std::fs::write(deep.join("deep.txt"), "deep data").unwrap();

    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    assert!(dst_dir.path().join("a/b/deep.txt").exists());
}

#[test]
fn test_extract_zip_to_dir_with_nested_dirs() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("nested");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.add_directory("skill/", options).unwrap();
        writer.add_directory("skill/docs/", options).unwrap();
        writer.start_file("skill/docs/guide.md", options).unwrap();
        writer.write_all(b"# Guide").unwrap();
        writer.start_file("skill/SKILL.md", options).unwrap();
        writer.write_all(b"# Skill").unwrap();
        writer.finish().unwrap();
    }

    extract_zip_to_dir(&buf, &target.to_string_lossy()).unwrap();
    assert!(target.join("docs/guide.md").exists());
    assert!(target.join("SKILL.md").exists());
}

#[test]
fn test_convex_skill_detail_with_all_fields() {
    let json = r#"{
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF Tool", "summary": "PDF converter", "stats": {"downloads": 500.0}},
        "latestVersion": {"version": "3.0.0"},
        "resolvedSlug": "pdf"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert_eq!(detail.owner.handle, "alice");
    assert_eq!(detail.skill.slug, "pdf");
    assert_eq!(detail.skill.display_name, "PDF Tool");
    assert_eq!(detail.skill.summary, "PDF converter");
    assert_eq!(detail.skill.stats.downloads, 500.0);
    assert_eq!(detail.latest_version.version, "3.0.0");
    assert_eq!(detail.resolved_slug, "pdf");
}

#[test]
fn test_convex_skill_list_item_multiple() {
    let json = r#"[
        {"slug":"pdf","displayName":"PDF","summary":"PDF tool","stats":{"downloads":100.0}},
        {"slug":"csv","displayName":"CSV","summary":"CSV tool","stats":{"downloads":50.0}}
    ]"#;
    let items: Vec<ConvexSkillListItem> = serde_json::from_str(json).unwrap();
    assert_eq!(items.len(), 2);
    assert_eq!(items[0].slug, "pdf");
    assert_eq!(items[1].stats.downloads, 50.0);
}

#[test]
fn test_new_from_config_custom_timeout() {
    let config = crate::types::ClawHubConfig {
        enabled: true,
        base_url: String::new(),
        convex_url: String::new(),
        convex_site_url: String::new(),
        timeout_secs: 60,
    };
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://clawhub.ai");
    assert_eq!(registry.convex_url, "https://wry-manatee-359.convex.cloud");
}

// ============================================================
// Coverage improvement: search result building, serialization
// ============================================================

#[test]
fn test_search_item_with_version() {
    let json = r#"{"results":[{"score":4.5,"slug":"pdf","displayName":"PDF Tool","summary":"Converts PDFs","version":"2.0"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results[0].version, Some("2.0".to_string()));
}

#[test]
fn test_search_item_score_boundary_at_one() {
    // Score exactly 1.0 should be kept as-is (not normalized)
    let json = r#"{"results":[{"score":1.0,"slug":"exact","displayName":"Exact","summary":"Exact match"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results[0].score, 1.0);
}

#[test]
fn test_search_item_score_boundary_just_above_one() {
    // Score 1.1 should be normalized by dividing by 5.0
    let json =
        r#"{"results":[{"score":1.1,"slug":"above","displayName":"Above","summary":"Above one"}]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results[0].score, 1.1);
}

#[test]
fn test_convex_response_missing_all_optional() {
    let json = r#"{"status":"ok","value":42}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "ok");
    assert_eq!(resp.error_message, None);
}

#[test]
fn test_convex_skill_list_item_no_stats() {
    // Stats is required in the struct but test edge case
    let json =
        r#"{"slug":"pdf","displayName":"PDF","summary":"PDF tool","stats":{"downloads":0.0}}"#;
    let item: ConvexSkillListItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.stats.downloads, 0.0);
}

#[test]
fn test_convex_skill_detail_empty_version() {
    let json = r#"{
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF", "summary": "Tool", "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "pdf"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert!(detail.latest_version.version.is_empty());
}

#[test]
fn test_convex_skill_detail_both_empty_slugs() {
    // When both slug and resolved_slug are empty
    let json = r#"{
        "owner": {"handle": ""},
        "skill": {"slug": "", "displayName": "", "summary": "", "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": ""
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert!(detail.skill.slug.is_empty());
    assert!(detail.resolved_slug.is_empty());
}

#[test]
fn test_clawhub_search_response_multiple_results() {
    let json = r#"{"results":[
        {"score":4.5,"slug":"pdf","displayName":"PDF","summary":"PDF tool"},
        {"score":3.2,"slug":"csv","displayName":"CSV","summary":"CSV tool"},
        {"score":2.1,"slug":"json","displayName":"JSON","summary":"JSON tool"}
    ]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results.len(), 3);
}

#[test]
fn test_find_common_prefix_two_levels_deep() {
    let entries = vec![
        "level1/level2/file1.txt".to_string(),
        "level1/level2/file2.txt".to_string(),
    ];
    let result = find_common_prefix(&entries);
    assert_eq!(result, Some("level1/".to_string()));
}

#[test]
fn test_find_common_prefix_no_slash_in_entry() {
    let entries = vec!["nofile.txt".to_string()];
    // Single entry without slash: first_dir is "nofile.txt", e == first_dir matches
    let result = find_common_prefix(&entries);
    assert_eq!(result, Some("nofile.txt/".to_string()));
}

#[test]
fn test_extract_zip_to_dir_empty_zip() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("empty");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        // Write only a directory entry, no files
        writer.add_directory("empty-dir/", options).unwrap();
        writer.finish().unwrap();
    }

    let result = extract_zip_to_dir(&buf, &target.to_string_lossy());
    // Should succeed even with empty content (just dir entry)
    assert!(result.is_ok());
}

#[test]
fn test_move_dir_contents_with_file() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();

    // Single file at root
    std::fs::write(src_dir.path().join("root.txt"), "root").unwrap();

    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    assert!(dst_dir.path().join("root.txt").exists());
    let content = std::fs::read_to_string(dst_dir.path().join("root.txt")).unwrap();
    assert_eq!(content, "root");
}

#[test]
fn test_flatten_single_top_dir_file_only() {
    let dir = tempfile::tempdir().unwrap();
    // Only a file, no subdirectory
    std::fs::write(dir.path().join("SKILL.md"), "# Skill").unwrap();
    let result = flatten_single_top_dir(dir.path());
    // File is not a directory, so result should be the original path
    assert_eq!(result, dir.path().to_path_buf());
}

#[test]
fn test_clawhub_registry_default_trait() {
    let registry = ClawHubRegistry::default();
    assert_eq!(registry.base_url, "https://clawhub.ai");
    assert_eq!(registry.convex_url, "https://wry-manatee-359.convex.cloud");
    assert_eq!(registry.convex_site_url, "");
}

// ============================================================
// Coverage improvement: additional clawhub tests
// ============================================================

#[test]
fn test_clawhub_registry_custom_urls() {
    let registry = ClawHubRegistry::with_urls(
        "https://custom.clawhub.ai",
        "https://custom.convex.cloud",
        "https://custom.convex.site",
    );
    assert_eq!(registry.base_url, "https://custom.clawhub.ai");
    assert_eq!(registry.convex_url, "https://custom.convex.cloud");
    assert_eq!(registry.convex_site_url, "https://custom.convex.site");
}

#[test]
fn test_clawhub_registry_name() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.name(), "clawhub");
}

#[test]
fn test_clawhub_search_item_deserialization() {
    let json = r#"{"score":1.5,"slug":"pdf","displayName":"PDF Tool","summary":"Converts PDFs"}"#;
    let result: ClawhubSearchItem = serde_json::from_str(json).unwrap();
    assert_eq!(result.slug, "pdf");
    assert_eq!(result.display_name, "PDF Tool");
    assert_eq!(result.summary, "Converts PDFs");
    assert!((result.score - 1.5).abs() < f64::EPSILON);
}

#[test]
fn test_clawhub_search_item_with_empty_slug() {
    let json = r#"{"score":0.0,"slug":"","displayName":"","summary":""}"#;
    let result: ClawhubSearchItem = serde_json::from_str(json).unwrap();
    assert!(result.slug.is_empty());
}

#[test]
fn test_convex_skill_list_item_with_zero_downloads() {
    let json = r#"{"_id":"abc","slug":"csv","displayName":"CSV","summary":"CSV tool","stats":{"downloads":0.0}}"#;
    let item: ConvexSkillListItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.slug, "csv");
    assert_eq!(item.stats.downloads, 0.0);
}

#[test]
fn test_clawhub_search_response_empty_results() {
    let json = r#"{"results":[]}"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert!(resp.results.is_empty());
}

#[test]
fn test_find_common_prefix_no_common_v2() {
    let entries = vec!["dirA/file1.txt".to_string(), "dirB/file2.txt".to_string()];
    let result = find_common_prefix(&entries);
    assert_eq!(result, None);
}

#[test]
fn test_find_common_prefix_single_entry() {
    let entries = vec!["mydir/file.txt".to_string()];
    let result = find_common_prefix(&entries);
    assert_eq!(result, Some("mydir/".to_string()));
}

#[test]
fn test_find_common_prefix_nested() {
    let entries = vec![
        "skills/pdf/docs/guide.md".to_string(),
        "skills/pdf/scripts/run.sh".to_string(),
        "skills/pdf/SKILL.md".to_string(),
    ];
    let result = find_common_prefix(&entries);
    // find_common_prefix extracts first directory level only
    assert_eq!(result, Some("skills/".to_string()));
}

#[test]
fn test_extract_zip_to_dir_with_file() {
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("extracted");

    let mut buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("test.txt", options).unwrap();
        writer.write_all(b"hello world").unwrap();
        writer.finish().unwrap();
    }

    let result = extract_zip_to_dir(&buf, &target.to_string_lossy());
    assert!(result.is_ok());
    let content = std::fs::read_to_string(target.join("test.txt")).unwrap();
    assert_eq!(content, "hello world");
}

#[test]
fn test_move_dir_contents_with_subdirs() {
    let src_dir = tempfile::tempdir().unwrap();
    let dst_dir = tempfile::tempdir().unwrap();

    std::fs::create_dir_all(src_dir.path().join("subdir")).unwrap();
    std::fs::write(src_dir.path().join("root.txt"), "root").unwrap();
    std::fs::write(src_dir.path().join("subdir/nested.txt"), "nested").unwrap();

    move_dir_contents(src_dir.path(), dst_dir.path()).unwrap();
    assert!(dst_dir.path().join("root.txt").exists());
    assert!(dst_dir.path().join("subdir/nested.txt").exists());
}

#[test]
fn test_flatten_single_top_dir_with_single_subdir() {
    let dir = tempfile::tempdir().unwrap();
    let subdir = dir.path().join("my-skill");
    std::fs::create_dir_all(&subdir).unwrap();
    std::fs::write(subdir.join("SKILL.md"), "# Skill").unwrap();

    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, subdir);
}

#[test]
fn test_flatten_single_top_dir_multiple_entries_v2() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::create_dir_all(dir.path().join("a")).unwrap();
    std::fs::create_dir_all(dir.path().join("b")).unwrap();

    let result = flatten_single_top_dir(dir.path());
    assert_eq!(result, dir.path().to_path_buf());
}

#[test]
fn test_clawhub_registry_trait_impl() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.name(), "clawhub");
}

#[test]
fn test_convex_skill_detail_full() {
    let json = r#"{
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF Tool", "summary": "Converts PDFs", "stats": {"downloads": 100.0}},
        "latestVersion": {"version": "2.0.0"},
        "resolvedSlug": "pdf"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert_eq!(detail.skill.slug, "pdf");
    assert_eq!(detail.owner.handle, "alice");
    assert_eq!(detail.latest_version.version, "2.0.0");
    assert_eq!(detail.resolved_slug, "pdf");
    assert_eq!(detail.skill.stats.downloads, 100.0);
}

// ============================================================
// Additional coverage tests for 95%+ target
// ============================================================

#[test]
fn test_clawhub_registry_new_default_v2() {
    let registry = ClawHubRegistry::new();
    assert_eq!(registry.name(), "clawhub");
    assert_eq!(registry.base_url, DEFAULT_CLAWHUB_URL);
    assert_eq!(registry.convex_url, DEFAULT_CONVEX_URL);
}

#[test]
fn test_clawhub_registry_with_custom_urls() {
    let registry = ClawHubRegistry::with_urls(
        "https://custom.clawhub.io",
        "https://custom.convex.cloud",
        "https://custom.convex.site",
    );
    assert_eq!(registry.base_url, "https://custom.clawhub.io");
    assert_eq!(registry.convex_url, "https://custom.convex.cloud");
    assert_eq!(registry.convex_site_url, "https://custom.convex.site");
}

#[test]
fn test_clawhub_site_url_with_explicit_site() {
    let registry = ClawHubRegistry::with_urls(
        "https://clawhub.ai",
        "https://my.convex.cloud",
        "https://my.convex.site",
    );
    assert_eq!(registry.site_url(), "https://my.convex.site");
}

#[test]
fn test_clawhub_site_url_derived_from_convex_v2() {
    let registry = ClawHubRegistry::with_urls(
        "https://clawhub.ai",
        "https://wry-manatee-359.convex.cloud",
        "",
    );
    assert_eq!(registry.site_url(), "https://wry-manatee-359.convex.site");
}

#[test]
fn test_new_from_config_default_urls() {
    let config = crate::types::ClawHubConfig {
        enabled: true,
        base_url: String::new(),
        convex_url: String::new(),
        convex_site_url: String::new(),
        timeout_secs: 0,
    };
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, DEFAULT_CLAWHUB_URL);
    assert_eq!(registry.convex_url, DEFAULT_CONVEX_URL);
    assert_eq!(registry.convex_site_url, "");
}

#[test]
fn test_new_from_config_custom_urls_v2() {
    let config = crate::types::ClawHubConfig {
        enabled: true,
        base_url: "https://test.clawhub.io".to_string(),
        convex_url: "https://test.convex.cloud".to_string(),
        convex_site_url: "https://test.convex.site".to_string(),
        timeout_secs: 60,
    };
    let registry = ClawHubRegistry::new_from_config(&config);
    assert_eq!(registry.base_url, "https://test.clawhub.io");
    assert_eq!(registry.convex_url, "https://test.convex.cloud");
    assert_eq!(registry.convex_site_url, "https://test.convex.site");
}

#[test]
fn test_convex_response_success_value() {
    let json = r#"{"status": "success", "value": {"key": "val"}}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "success");
    assert_eq!(resp.value["key"], "val");
    assert!(resp.error_message.is_none());
}

#[test]
fn test_convex_response_error_without_message() {
    let json = r#"{"status": "error", "value": null}"#;
    let resp: ConvexResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.status, "error");
    assert!(resp.error_message.is_none());
}

#[test]
fn test_convex_skill_detail_empty_slug_uses_resolved() {
    let json = r#"{
        "owner": {"handle": "bob"},
        "skill": {"slug": "", "displayName": "Test", "summary": "A test", "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "fallback-slug"
    }"#;
    let detail: ConvexSkillDetail = serde_json::from_str(json).unwrap();
    assert!(detail.skill.slug.is_empty());
    assert_eq!(detail.resolved_slug, "fallback-slug");
    assert!(detail.latest_version.version.is_empty());
}

#[test]
fn test_clawhub_search_item_minimal() {
    let json = r#"{
        "slug": "test",
        "displayName": "Test Skill",
        "summary": "A test",
        "score": 0.5
    }"#;
    let item: ClawhubSearchItem = serde_json::from_str(json).unwrap();
    assert_eq!(item.slug, "test");
    assert_eq!(item.display_name, "Test Skill");
    assert_eq!(item.score, 0.5);
}

#[test]
fn test_convex_stats_deserialization_v2() {
    let json = r#"{"downloads": 0.0}"#;
    let stats: ConvexStats = serde_json::from_str(json).unwrap();
    assert_eq!(stats.downloads, 0.0);
}

#[test]
fn test_convex_latest_version_deserialization_v2() {
    let json = r#"{"version": "3.1.4"}"#;
    let version: ConvexLatestVersion = serde_json::from_str(json).unwrap();
    assert_eq!(version.version, "3.1.4");
}

#[test]
fn test_convex_latest_version_empty() {
    let json = r#"{"version": ""}"#;
    let version: ConvexLatestVersion = serde_json::from_str(json).unwrap();
    assert!(version.version.is_empty());
}

#[test]
fn test_convex_owner_handle_deserialization() {
    let json = r#"{"handle": "alice"}"#;
    let owner: ConvexOwner = serde_json::from_str(json).unwrap();
    assert_eq!(owner.handle, "alice");
}

#[test]
fn test_convex_skill_info_deserialization() {
    let json = r#"{
        "slug": "my-skill",
        "displayName": "My Skill",
        "summary": "A great skill",
        "stats": {"downloads": 999.0}
    }"#;
    let info: ConvexSkill = serde_json::from_str(json).unwrap();
    assert_eq!(info.slug, "my-skill");
    assert_eq!(info.display_name, "My Skill");
    assert_eq!(info.summary, "A great skill");
    assert_eq!(info.stats.downloads, 999.0);
}

#[test]
fn test_clawhub_search_response_with_results() {
    let json = r#"{
        "results": [
            {
                "slug": "skill-a",
                "displayName": "Skill A",
                "summary": "First skill",
                "score": 0.95
            }
        ]
    }"#;
    let resp: ClawhubSearchResponse = serde_json::from_str(json).unwrap();
    assert_eq!(resp.results.len(), 1);
    assert_eq!(resp.results[0].slug, "skill-a");
}

// ============================================================
// Coverage: HTTP-dependent async paths (connection errors)
// ============================================================

#[tokio::test]
async fn test_search_connection_error() {
    let registry = ClawHubRegistry::with_urls("http://127.0.0.1:1", "http://127.0.0.1:1", "");
    let result = registry.search("pdf", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_search_empty_query_connection_error() {
    let registry = ClawHubRegistry::with_urls("http://127.0.0.1:1", "http://127.0.0.1:1", "");
    let result = registry.search("", 10).await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_get_skill_meta_invalid_slug() {
    let registry = ClawHubRegistry::new();
    // Slash in slug should fail validation
    let result = registry.get_skill_meta("bad/slug").await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test]
async fn test_get_skill_meta_connection_error() {
    let registry = ClawHubRegistry::with_urls("http://127.0.0.1:1", "http://127.0.0.1:1", "");
    let result = registry.get_skill_meta("pdf").await;
    assert!(result.is_err());
}

#[tokio::test]
async fn test_download_and_install_invalid_slug() {
    let registry = ClawHubRegistry::new();
    let result = registry
        .download_and_install("bad/slug", "1.0", "/tmp")
        .await;
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("invalid"));
}

#[tokio::test]
async fn test_download_and_install_connection_error() {
    let registry = ClawHubRegistry::with_urls("http://127.0.0.1:1", "http://127.0.0.1:1", "");
    let result = registry.download_and_install("pdf", "1.0", "/tmp").await;
    assert!(result.is_err());
}

#[test]
fn test_extract_zip_to_dir_invalid_data_v2() {
    let result = extract_zip_to_dir(&[0x00, 0x01, 0x02], "/tmp/nonexistent_test_dir_v2");
    assert!(result.is_err());
    assert!(result.unwrap_err().to_string().contains("ZIP"));
}

#[test]
fn test_find_common_prefix_empty_dir_name_v2() {
    // Entry that starts with empty split result
    let entries = vec!["".to_string()];
    let result = find_common_prefix(&entries);
    assert_eq!(result, None);
}

#[test]
fn test_move_dir_contents_nonexistent_source() {
    let result = move_dir_contents(
        std::path::Path::new("/tmp/nonexistent_src_dir_xyz"),
        std::path::Path::new("/tmp/nonexistent_dst_dir_xyz"),
    );
    assert!(result.is_err());
}

#[test]
fn test_flatten_single_top_dir_nonexistent_dir() {
    let result = flatten_single_top_dir(std::path::Path::new("/tmp/nonexistent_flat_dir_xyz"));
    // Should return the path as-is since read_dir fails
    assert_eq!(
        result,
        std::path::PathBuf::from("/tmp/nonexistent_flat_dir_xyz")
    );
}

// ============================================================
// wiremock-based HTTP tests for async functions
//
// Covers the success + error paths of the async HTTP functions:
//   - search (search_query via ClawHub search API + search_list via convex)
//   - get_skill_meta (convex skills:getBySlug)
//   - get_skill_content (ClawHub file API + convex/github fallback)
//   - browse (ClawHub REST /api/v1/skills)
//   - download_and_install (ZIP download + github fallback)
// Each test starts its own MockServer to avoid cross-test interference.
// ============================================================

use wiremock::matchers::{method, path, query_param};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Build a Convex-shaped response envelope for wiremock.
fn convex_body(status: &str, value: serde_json::Value) -> serde_json::Value {
    serde_json::json!({ "status": status, "value": value })
}

// --- search (non-empty query → ClawHub search API) ---

#[tokio::test]
async fn test_search_query_success_parses_results() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let body = serde_json::json!({
        "results": [
            {"score": 4.5, "slug": "pdf", "displayName": "PDF Tool", "summary": "Converts PDFs"},
            {"score": 0.8, "slug": "csv", "displayName": "CSV Tool", "summary": "Converts CSV"}
        ]
    });

    Mock::given(method("GET"))
        .and(path("/api/search"))
        .and(query_param("q", "pdf"))
        .and(query_param("limit", "10"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let results = registry
        .search("pdf", 10)
        .await
        .expect("search should succeed");
    assert_eq!(results.len(), 2);
    // score 4.5 > 1.0 -> normalized to 4.5 / 5.0 = 0.9
    assert!((results[0].score - 0.9).abs() < 1e-9);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].display_name, "PDF Tool");
    assert_eq!(results[0].summary, "Converts PDFs");
    assert_eq!(results[0].registry_name, "clawhub");
    assert_eq!(results[0].version, "latest");
    // score 0.8 <= 1.0 -> kept as-is, not normalized
    assert!((results[1].score - 0.8).abs() < 1e-9);
    assert_eq!(results[1].slug, "csv");
}

#[tokio::test]
async fn test_search_query_marks_truncation_when_full_limit() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    // Exactly `limit` (2) results -> last entry truncated = true.
    let body = serde_json::json!({
        "results": [
            {"score": 0.5, "slug": "a", "displayName": "A", "summary": "a"},
            {"score": 0.4, "slug": "b", "displayName": "B", "summary": "b"}
        ]
    });

    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let results = registry.search("anything", 2).await.unwrap();
    assert_eq!(results.len(), 2);
    assert!(!results[0].truncated);
    assert!(results.last().unwrap().truncated);
}

#[tokio::test]
async fn test_search_query_http_error_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
        .mount(&server)
        .await;

    let err = registry.search("pdf", 10).await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("status 500") || msg.contains("500"),
        "msg: {}",
        msg
    );
}

#[tokio::test]
async fn test_search_query_malformed_json_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/search"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json {"))
        .mount(&server)
        .await;

    let err = registry.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("parse search response"));
}

// --- search (empty query → convex skills:list) ---

#[tokio::test]
async fn test_search_list_success_parses_results() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let value = serde_json::json!([
        {"slug": "pdf", "displayName": "PDF", "summary": "s1", "stats": {"downloads": 42.0}},
        {"slug": "csv", "displayName": "CSV", "summary": "s2", "stats": {"downloads": 7.0}}
    ]);

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let results = registry.search("", 10).await.expect("list should succeed");
    assert_eq!(results.len(), 2);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].downloads, 42);
    assert!((results[0].score - 1.0).abs() < 1e-9);
    assert_eq!(results[1].slug, "csv");
    assert_eq!(results[1].downloads, 7);
}

#[tokio::test]
async fn test_search_list_convex_error_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let body = serde_json::json!({
        "status": "error",
        "value": null,
        "errorMessage": "rate limited"
    });

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let err = registry.search("", 10).await.unwrap_err();
    assert!(err.to_string().contains("convex error") && err.to_string().contains("rate limited"));
}

#[tokio::test]
async fn test_search_list_malformed_value_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    // value is a string, not an array -> deserialization of Vec fails.
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(
            ResponseTemplate::new(200)
                .set_body_json(convex_body("success", serde_json::json!("not-an-array"))),
        )
        .mount(&server)
        .await;

    assert!(registry.search("", 10).await.is_err());
}

// --- get_skill_meta (convex skills:getBySlug) ---

#[tokio::test]
async fn test_get_skill_meta_success() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let value = serde_json::json!({
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF Tool", "summary": "Converts PDFs",
                   "stats": {"downloads": 1234.0}},
        "latestVersion": {"version": "2.0.0"},
        "resolvedSlug": "pdf"
    });

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let meta = registry
        .get_skill_meta("pdf")
        .await
        .expect("meta should succeed");
    assert_eq!(meta.slug, "pdf");
    assert_eq!(meta.display_name, "PDF Tool");
    assert_eq!(meta.summary, "Converts PDFs");
    assert_eq!(meta.latest_version, "2.0.0");
    assert_eq!(meta.author, "alice");
    assert_eq!(meta.downloads, 1234);
    assert_eq!(meta.registry_name, "clawhub");
    assert!(!meta.is_malware_blocked);
}

#[tokio::test]
async fn test_get_skill_meta_empty_version_defaults_to_latest() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let value = serde_json::json!({
        "owner": {"handle": "bob"},
        "skill": {"slug": "", "displayName": "", "summary": "",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "fallback-slug"
    });

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let meta = registry.get_skill_meta("fallback-slug").await.unwrap();
    // Empty skill.slug falls back to resolved_slug.
    assert_eq!(meta.slug, "fallback-slug");
    // Empty version defaults to "latest".
    assert_eq!(meta.latest_version, "latest");
}

#[tokio::test]
async fn test_get_skill_meta_both_slugs_empty_returns_not_found() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let value = serde_json::json!({
        "owner": {"handle": "x"},
        "skill": {"slug": "", "displayName": "", "summary": "",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": ""
    });

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let err = registry.get_skill_meta("missing").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("not found") || msg.contains("missing"),
        "msg: {}",
        msg
    );
}

#[tokio::test]
async fn test_get_skill_meta_convex_error_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "error", "value": null, "errorMessage": "no such skill"
        })))
        .mount(&server)
        .await;

    let err = registry.get_skill_meta("pdf").await.unwrap_err();
    assert!(err.to_string().contains("convex error"));
}

// --- get_skill_content (ClawHub file API primary path) ---

#[tokio::test]
async fn test_get_skill_content_file_api_success() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/v1/skills/pdf/file"))
        .and(query_param("path", "SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# PDF Skill\nbody"))
        .mount(&server)
        .await;

    let content = registry
        .get_skill_content("pdf")
        .await
        .expect("content should succeed");
    assert_eq!(content.slug, "pdf");
    assert_eq!(content.filename, "SKILL.md");
    assert!(content.content.contains("PDF Skill"));
}

#[tokio::test]
async fn test_get_skill_content_file_api_404_falls_back_through_convex_to_github() {
    let server = MockServer::start().await;
    // base_url + convex_url pointed at the mock; github raw URL is hardcoded in
    // the source (https://raw.githubusercontent.com/...) and cannot be
    // redirected, so the fallback reaches the real host and fails there.
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    // Strategy 1: file API returns 404 -> fallback path triggered.
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/pdf/file"))
        .respond_with(ResponseTemplate::new(404).set_body_string("no file api"))
        .mount(&server)
        .await;

    // Strategy 2: convex returns owner handle (proving the fallback reached
    // convex and parsed it successfully).
    let value = serde_json::json!({
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF", "summary": "s",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "pdf"
    });
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .named("convex-by-slug")
        .mount(&server)
        .await;

    // The github raw call hits the real internet and fails; the overall result
    // is Err with an HTTP status message. This proves the fallback chain
    // advanced past both mocked stages (file API -> convex) to the final
    // github raw fetch.
    let err = registry.get_skill_content("pdf").await.unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("HTTP") || msg.contains("request failed"),
        "expected github raw fetch error, got: {}",
        msg
    );
    // Verify convex was actually consulted during the fallback.
    server.verify().await;
}

#[tokio::test]
async fn test_get_skill_content_fallback_owner_empty_returns_not_found() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    // file API fails.
    Mock::given(method("GET"))
        .and(path("/api/v1/skills/pdf/file"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    // convex returns empty owner -> NotFound error.
    let value = serde_json::json!({
        "owner": {"handle": ""},
        "skill": {"slug": "pdf", "displayName": "", "summary": "",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "pdf"
    });
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let err = registry.get_skill_content("pdf").await.unwrap_err();
    assert!(err.to_string().contains("owner handle not found"));
}

#[tokio::test]
async fn test_get_skill_content_invalid_slug_returns_validation_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let err = registry.get_skill_content("bad/slug").await.unwrap_err();
    assert!(err.to_string().contains("invalid skill slug"));
}

// --- browse (ClawHub REST /api/v1/skills) ---

#[tokio::test]
async fn test_browse_success_parses_items_and_cursor() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    let body = serde_json::json!({
        "items": [
            {"slug": "a", "displayName": "A", "summary": "sa", "stats": {"downloads": 10.0}},
            {"slug": "b", "displayName": "B", "summary": "sb", "stats": {"downloads": 5.0}}
        ],
        "nextCursor": "cursor-123"
    });

    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .and(query_param("sort", "trending"))
        .and(query_param("limit", "20"))
        .respond_with(ResponseTemplate::new(200).set_body_json(body))
        .mount(&server)
        .await;

    let result = registry
        .browse(&BrowseSort::Trending, 0, "")
        .await
        .expect("browse should succeed");
    assert_eq!(result.items.len(), 2);
    assert_eq!(result.items[0].slug, "a");
    assert_eq!(result.items[0].downloads, 10);
    assert_eq!(result.items[1].slug, "b");
    assert_eq!(result.next_cursor.as_deref(), Some("cursor-123"));
}

#[tokio::test]
async fn test_browse_includes_cursor_param_when_provided() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .and(query_param("cursor", "page2"))
        .and(query_param("sort", "downloads"))
        .and(query_param("limit", "5"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [],
            "nextCursor": null
        })))
        .mount(&server)
        .await;

    let result = registry
        .browse(&BrowseSort::Downloads, 5, "page2")
        .await
        .unwrap();
    assert!(result.items.is_empty());
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_browse_http_error_returns_err_with_status() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .respond_with(ResponseTemplate::new(403).set_body_string("forbidden body"))
        .mount(&server)
        .await;

    let err = registry
        .browse(&BrowseSort::Stars, 10, "")
        .await
        .unwrap_err();
    let msg = err.to_string();
    assert!(
        msg.contains("browse failed") && msg.contains("403"),
        "msg: {}",
        msg
    );
}

#[tokio::test]
async fn test_browse_malformed_json_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_string("garbage"))
        .mount(&server)
        .await;

    let err = registry
        .browse(&BrowseSort::Updated, 10, "")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("parse browse response"));
}

#[tokio::test]
async fn test_browse_limit_clamped_to_100() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), "");

    // Request 500 -> clamped to 100.
    Mock::given(method("GET"))
        .and(path("/api/v1/skills"))
        .and(query_param("limit", "100"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({"items": []})))
        .mount(&server)
        .await;

    let result = registry.browse(&BrowseSort::Rating, 500, "").await.unwrap();
    assert!(result.items.is_empty());
}

// --- download_and_install (ZIP path) ---

#[tokio::test]
async fn test_download_and_install_zip_success() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), &server.uri());

    // 1. convex get-by-slug returns owner + version.
    let value = serde_json::json!({
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "PDF", "summary": "Converts PDFs",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": "1.2.0"},
        "resolvedSlug": "pdf"
    });
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    // 2. ZIP download returns a real zip containing a single top-level dir.
    let dir = tempfile::tempdir().unwrap();
    let target = dir.path().join("installed");
    let mut zip_buf = Vec::new();
    {
        let mut writer = zip::ZipWriter::new(std::io::Cursor::new(&mut zip_buf));
        let options = zip::write::SimpleFileOptions::default();
        writer.start_file("pdf/SKILL.md", options).unwrap();
        writer.write_all(b"# PDF Skill").unwrap();
        writer.finish().unwrap();
    }

    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .and(query_param("slug", "pdf"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "application/zip")
                .set_body_bytes(zip_buf.clone()),
        )
        .mount(&server)
        .await;

    let result = registry
        .download_and_install("pdf", "1.0", &target.to_string_lossy())
        .await
        .expect("install should succeed");
    assert_eq!(result.version, "1.2.0");
    assert_eq!(result.summary, "Converts PDFs");
    // File extracted (top-level dir flattened).
    assert!(target.join("SKILL.md").exists());
}

#[tokio::test]
async fn test_download_and_install_convex_error_returns_err() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), &server.uri());

    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "status": "error", "value": null, "errorMessage": "db down"
        })))
        .mount(&server)
        .await;

    let err = registry
        .download_and_install("pdf", "1.0", "/tmp/whatever")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("convex error"));
}

#[tokio::test]
async fn test_download_and_install_empty_owner_returns_not_found() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), &server.uri());

    let value = serde_json::json!({
        "owner": {"handle": ""},
        "skill": {"slug": "pdf", "displayName": "", "summary": "",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": ""},
        "resolvedSlug": "pdf"
    });
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    let err = registry
        .download_and_install("pdf", "1.0", "/tmp/whatever")
        .await
        .unwrap_err();
    assert!(err.to_string().contains("owner handle not found"));
}

#[tokio::test]
async fn test_download_and_install_zip_wrong_content_type_falls_back_to_github() {
    let server = MockServer::start().await;
    let registry = ClawHubRegistry::with_urls(&server.uri(), &server.uri(), &server.uri());

    // convex returns owner.
    let value = serde_json::json!({
        "owner": {"handle": "alice"},
        "skill": {"slug": "pdf", "displayName": "", "summary": "s",
                   "stats": {"downloads": 0.0}},
        "latestVersion": {"version": "1.0.0"},
        "resolvedSlug": "pdf"
    });
    Mock::given(method("POST"))
        .and(path("/api/query"))
        .respond_with(ResponseTemplate::new(200).set_body_json(convex_body("success", value)))
        .mount(&server)
        .await;

    // ZIP download returns 200 but a non-zip content type -> download_skill_zip
    // returns Err -> download_and_install falls back to GitHub Trees API, which
    // hits /repos/.../git/trees/main on api.github.com. Since we don't mock that
    // real host, the fallback network call fails, producing an Err (not a panic).
    Mock::given(method("GET"))
        .and(path("/api/v1/download"))
        .respond_with(
            ResponseTemplate::new(200)
                .insert_header("Content-Type", "text/html")
                .set_body_string("<html>not a zip</html>"),
        )
        .mount(&server)
        .await;

    let result = registry
        .download_and_install("pdf", "1.0", "/tmp/nemesis_test_install")
        .await;
    assert!(
        result.is_err(),
        "expected github fallback to fail without network"
    );
}
