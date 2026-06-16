//! Additional tests for github_registry.rs targeting HTTP-mocked flows
//! and uncovered branches.

use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn two_layer_config(repo: &str) -> GitHubSourceConfig {
    GitHubSourceConfig {
        name: "test".to_string(),
        repo: repo.to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "skills_json".to_string(),
        index_path: "skills.json".to_string(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 5,
        max_size: 1024 * 1024,
    }
}

fn three_layer_config(repo: &str) -> GitHubSourceConfig {
    GitHubSourceConfig {
        name: "test".to_string(),
        repo: repo.to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{author}/{slug}/SKILL.md".to_string(),
        timeout_secs: 5,
        max_size: 1024 * 1024,
    }
}

fn two_layer_api_config(repo: &str) -> GitHubSourceConfig {
    GitHubSourceConfig {
        name: "test".to_string(),
        repo: repo.to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "skills/{slug}/SKILL.md".to_string(),
        timeout_secs: 5,
        max_size: 1024 * 1024,
    }
}

// ============================================================
// search_skills_json: success, no match, http error, large body
// ============================================================

#[tokio::test]
async fn test_search_skills_json_success_via_mock() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","description":"PDF tool"},{"name":"csv","description":"CSV tool"}]"#;
    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills.json"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].summary, "PDF tool");
}

#[tokio::test]
async fn test_search_skills_json_no_match() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","description":"PDF tool"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let results = reg.search("excel", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_search_skills_json_matches_description() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"tool","description":"converts PDF files"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "tool");
}

#[tokio::test]
async fn test_search_skills_json_respects_limit() {
    let server = MockServer::start().await;
    let body = r#"[
        {"name":"pdf1","description":"pdf"},
        {"name":"pdf2","description":"pdf"},
        {"name":"pdf3","description":"pdf"}
    ]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let results = reg.search("pdf", 2).await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_search_skills_json_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let err = reg.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_search_skills_json_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let err = reg.search("pdf", 10).await.unwrap_err();
    // Serialization error or "parse"
    assert!(err.to_string().len() > 0);
}

#[tokio::test]
async fn test_search_skills_json_empty_array() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let results = reg.search("anything", 10).await.unwrap();
    assert!(results.is_empty());
}

#[tokio::test]
async fn test_do_get_too_large_response() {
    let server = MockServer::start().await;
    let big_body = "x".repeat(2048);
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big_body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    reg.max_size = 100; // tiny limit
    let err = reg.do_get(&format!("{}/org/repo/main/skills.json", server.uri())).await.unwrap_err();
    assert!(err.to_string().contains("too large"));
}

#[tokio::test]
async fn test_do_get_success_under_limit() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;
    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let body = reg.do_get(&format!("{}/x", server.uri())).await.unwrap();
    assert_eq!(body, b"hello");
}

// ============================================================
// search_two_layer via Contents API
// ============================================================

#[tokio::test]
async fn test_search_two_layer_via_contents_api() {
    let server = MockServer::start().await;
    let body = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"csv","type":"dir","path":"skills/csv"},
        {"name":"README.md","type":"file","path":"README.md"}
    ]"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/contents/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].download_path, "skills/pdf/SKILL.md");
}

#[tokio::test]
async fn test_search_two_layer_filters_non_dir() {
    let server = MockServer::start().await;
    let body = r#"[
        {"name":"pdf","type":"dir","path":"skills/pdf"},
        {"name":"SKILL.md","type":"file","path":"skills/SKILL.md"},
        {"name":"submodule","type":"submodule","path":"submodule"}
    ]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    // Empty query = match everything
    let results = reg.search("", 10).await.unwrap();
    // Only dirs match
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf");
}

#[tokio::test]
async fn test_search_two_layer_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(403).set_body_string("rate limit"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let err = reg.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_search_two_layer_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("garbage"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let err = reg.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("parse"));
}

#[tokio::test]
async fn test_search_two_layer_empty_response() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("[]"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("anything", 10).await.unwrap();
    assert!(results.is_empty());
}

// ============================================================
// search_three_layer via Trees API
// ============================================================

#[tokio::test]
async fn test_search_three_layer_via_trees_api() {
    let server = MockServer::start().await;
    let body = r#"{
        "sha": "abc",
        "tree": [
            {"path": "skills/author1/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/author1/csv/SKILL.md", "type": "blob"},
            {"path": "skills/author2/excel/SKILL.md", "type": "blob"},
            {"path": "skills/author1/pdf", "type": "tree"}
        ],
        "truncated": false
    }"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "pdf");
    assert_eq!(results[0].download_path, "skills/author1/pdf/SKILL.md");
}

#[tokio::test]
async fn test_search_three_layer_empty_query_returns_all() {
    let server = MockServer::start().await;
    let body = r#"{
        "tree": [
            {"path": "skills/a/x/SKILL.md", "type": "blob"},
            {"path": "skills/b/y/SKILL.md", "type": "blob"}
        ]
    }"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("", 10).await.unwrap();
    assert_eq!(results.len(), 2);
}

#[tokio::test]
async fn test_search_three_layer_deduplicates_slug() {
    let server = MockServer::start().await;
    let body = r#"{
        "tree": [
            {"path": "skills/author1/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/author2/pdf/SKILL.md", "type": "blob"}
        ]
    }"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    // Dedup by slug: only one "pdf"
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_three_layer_skips_non_blob() {
    let server = MockServer::start().await;
    let body = r#"{
        "tree": [
            {"path": "skills/author1/pdf", "type": "tree"},
            {"path": "skills/author1/pdf/SKILL.md", "type": "blob"}
        ]
    }"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_three_layer_skips_non_skills_prefix() {
    let server = MockServer::start().await;
    let body = r#"{
        "tree": [
            {"path": "docs/author1/pdf/SKILL.md", "type": "blob"},
            {"path": "skills/author1/csv/SKILL.md", "type": "blob"}
        ]
    }"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("", 10).await.unwrap();
    // Only "skills/" prefix matches
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].slug, "csv");
}

#[tokio::test]
async fn test_search_three_layer_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500).set_body_string("err"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let err = reg.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_search_three_layer_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let err = reg.search("pdf", 10).await.unwrap_err();
    assert!(err.to_string().contains("parse"));
}

// ============================================================
// get_skill_meta via skills.json
// ============================================================

#[tokio::test]
async fn test_get_skill_meta_skills_json_found() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","description":"PDF tool","author":"alice"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let meta = reg.get_skill_meta("pdf").await.unwrap();
    assert_eq!(meta.slug, "pdf");
    assert_eq!(meta.summary, "PDF tool");
    assert_eq!(meta.author, "alice");
}

#[tokio::test]
async fn test_get_skill_meta_skills_json_not_found() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","description":"PDF tool"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let err = reg.get_skill_meta("missing").await.unwrap_err();
    assert!(err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_get_skill_meta_skills_json_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    assert!(reg.get_skill_meta("pdf").await.is_err());
}

// ============================================================
// get_skill_content
// ============================================================

#[tokio::test]
async fn test_get_skill_content_success() {
    let server = MockServer::start().await;
    let body = "# PDF Skill\nDescription here";
    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let content = reg.get_skill_content("pdf").await.unwrap();
    assert_eq!(content.slug, "pdf");
    assert_eq!(content.filename, "SKILL.md");
    assert!(content.content.contains("PDF Skill"));
}

#[tokio::test]
async fn test_get_skill_content_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(404))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    assert!(reg.get_skill_content("pdf").await.is_err());
}

// ============================================================
// browse: pagination logic
// ============================================================

#[tokio::test]
async fn test_browse_default_limit() {
    let server = MockServer::start().await;
    // Return one item
    let body = r#"[{"name":"pdf","description":"x"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    // limit=0 should default to 20
    let result = reg.browse(&BrowseSort::Trending, 0, "").await.unwrap();
    assert_eq!(result.items.len(), 1);
    // Only 1 item, less than limit (20) -> no next cursor
    assert!(result.next_cursor.is_none());
}

#[tokio::test]
async fn test_browse_pagination_offset_cursor() {
    let server = MockServer::start().await;
    // Return 30 items to test pagination
    let mut items = Vec::new();
    for i in 0..30 {
        items.push(format!(r#"{{"name":"skill{}","description":"d"}}"#, i));
    }
    let body = format!("[{}]", items.join(","));
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    let result = reg.browse(&BrowseSort::Trending, 10, "").await.unwrap();
    assert_eq!(result.items.len(), 10);
    assert_eq!(result.next_cursor.unwrap(), "offset:10");

    // Second page
    let result2 = reg.browse(&BrowseSort::Trending, 10, "offset:10").await.unwrap();
    assert_eq!(result2.items.len(), 10);
    assert_eq!(result2.next_cursor.unwrap(), "offset:20");

    // Third page (only 10 left)
    let result3 = reg.browse(&BrowseSort::Trending, 10, "offset:20").await.unwrap();
    assert_eq!(result3.items.len(), 10);
    // After 30 items exactly, items.len() == limit, so cursor is Some
    // Actually: offset=20, take 10 -> 10 items. items.len() == limit(10) -> next = offset:30
    assert_eq!(result3.next_cursor.unwrap(), "offset:30");

    // Fourth page - empty
    let result4 = reg.browse(&BrowseSort::Trending, 10, "offset:30").await.unwrap();
    assert_eq!(result4.items.len(), 0);
    assert!(result4.next_cursor.is_none());
}

#[tokio::test]
async fn test_browse_invalid_cursor_defaults_to_zero() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","description":"x"}]"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    // Invalid cursor format -> defaults to offset 0
    let result = reg.browse(&BrowseSort::Trending, 10, "garbage").await.unwrap();
    assert_eq!(result.items.len(), 1);
}

#[tokio::test]
async fn test_browse_search_error_propagates() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.base_url = server.uri();
    assert!(reg.browse(&BrowseSort::Trending, 10, "").await.is_err());
}

// ============================================================
// download_and_install: error / fallback paths
// ============================================================

#[tokio::test]
async fn test_download_and_install_github_api_no_dir_prefix_returns_legacy() {
    let server = MockServer::start().await;
    let skill_body = r#"# PDF Skill"#;
    // The legacy fallback URL: skills/pdf/SKILL.md on raw (server.uri() is base_url)
    Mock::given(method("GET"))
        .and(path("/org/repo/main/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string(skill_body))
        .mount(&server)
        .await;

    // Pattern without {slug} -> skill_dir_prefix returns None -> legacy path
    let config = GitHubSourceConfig {
        name: "test".to_string(),
        repo: "org/repo".to_string(),
        enabled: true,
        branch: "main".to_string(),
        index_type: "github_api".to_string(),
        index_path: String::new(),
        skill_path_pattern: "SKILL.md".to_string(), // root, no {slug}
        timeout_secs: 5,
        max_size: 1024 * 1024,
    };
    let mut reg = GitHubRegistry::from_source(&config);
    reg.base_url = server.uri();
    let dir = tempfile::tempdir().unwrap();
    let result = reg.download_and_install("pdf", "1.0", dir.path().to_str().unwrap()).await;
    assert!(result.is_ok());
    let installed = std::fs::read(dir.path().join("SKILL.md")).unwrap();
    assert_eq!(installed, b"# PDF Skill");
}

#[tokio::test]
async fn test_download_and_install_skills_json_index_with_tree_download() {
    // skills_json index_type but skill_path_pattern has {slug} -> tree download path
    let server = MockServer::start().await;
    // Tree API call - return no files under prefix
    let tree_body = r#"{"tree":[]}"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let dir = tempfile::tempdir().unwrap();
    let err = reg.download_and_install("pdf", "1.0", dir.path().to_str().unwrap()).await.unwrap_err();
    // No files under prefix -> NotFound
    assert!(err.to_string().contains("no files") || err.to_string().contains("not found"));
}

// ============================================================
// download_skill_tree direct
// ============================================================

#[tokio::test]
async fn test_download_skill_tree_no_files_under_prefix() {
    let server = MockServer::start().await;
    let body = r#"{"tree":[{"path":"other/file.txt","type":"blob"}]}"#;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let dir = tempfile::tempdir().unwrap();
    let err = reg.download_skill_tree("skills/pdf", dir.path().to_str().unwrap()).await.unwrap_err();
    assert!(err.to_string().contains("no files") || err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_download_skill_tree_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .respond_with(ResponseTemplate::new(500))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let dir = tempfile::tempdir().unwrap();
    let err = reg.download_skill_tree("skills/pdf", dir.path().to_str().unwrap()).await.unwrap_err();
    assert!(err.to_string().contains("HTTP") || err.to_string().contains("tree"));
}

// ============================================================
// search dispatch
// ============================================================

#[tokio::test]
async fn test_search_dispatches_to_three_layer_when_pattern_has_author() {
    let server = MockServer::start().await;
    let body = r#"{
        "tree": [
            {"path":"skills/auth/pdf/SKILL.md","type":"blob"}
        ]
    }"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&three_layer_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}

#[tokio::test]
async fn test_search_dispatches_to_two_layer_when_no_author() {
    let server = MockServer::start().await;
    let body = r#"[{"name":"pdf","type":"dir","path":"skills/pdf"}]"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/contents/skills"))
        .respond_with(ResponseTemplate::new(200).set_body_string(body))
        .mount(&server)
        .await;

    let mut reg = GitHubRegistry::from_source(&two_layer_api_config("org/repo"));
    reg.set_github_api_url(&server.uri());
    let results = reg.search("pdf", 10).await.unwrap();
    assert_eq!(results.len(), 1);
}
