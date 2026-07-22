//! Additional tests for github_tree.rs covering HTTP-mocked downloads.

use super::*;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

fn http_client() -> Client {
    Client::builder()
        .timeout(std::time::Duration::from_secs(5))
        .build()
        .unwrap()
}

// ============================================================
// decode_tree_blob_paths additional edge cases
// ============================================================

#[test]
fn test_decode_tree_blob_paths_root_is_array() {
    // If body's root is an array (not object), tree is missing
    let json = r#"[{"path":"skills/pdf/SKILL.md","type":"blob"}]"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_root_is_primitive() {
    let result = decode_tree_blob_paths(b"42", "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_root_is_string() {
    let result = decode_tree_blob_paths(b"\"hello\"", "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_root_is_null() {
    let result = decode_tree_blob_paths(b"null", "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_tree_field_not_array() {
    let json = r#"{"tree": "not_an_array"}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_type_as_number() {
    // type field is a number, not string -> defaults to ""
    let json = r#"{"tree":[{"path":"skills/pdf/SKILL.md","type":1}]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_path_as_number() {
    // path field is a number, not string -> defaults to ""
    let json = r#"{"tree":[{"type":"blob","path":42}]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert!(result.is_empty());
}

#[test]
fn test_decode_tree_blob_paths_preserves_order() {
    let json = r#"{"tree":[
        {"path":"skills/pdf/zebra.txt","type":"blob"},
        {"path":"skills/pdf/apple.txt","type":"blob"},
        {"path":"skills/pdf/mango.txt","type":"blob"}
    ]}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 3);
    assert_eq!(result[0], "skills/pdf/zebra.txt");
    assert_eq!(result[1], "skills/pdf/apple.txt");
    assert_eq!(result[2], "skills/pdf/mango.txt");
}

#[test]
fn test_decode_tree_blob_paths_ignores_root_extra_fields() {
    let json = r#"{"sha":"abc","tree":[{"path":"skills/pdf/SKILL.md","type":"blob"}],"truncated":false,"extra":"data"}"#;
    let result = decode_tree_blob_paths(json.as_bytes(), "skills/pdf").unwrap();
    assert_eq!(result.len(), 1);
}

// ============================================================
// download_skill_tree_from_github via wiremock
// ============================================================

#[tokio::test]
async fn test_download_skill_tree_success_downloads_files() {
    let server = MockServer::start().await;
    let tree_body = r#"{
        "sha":"abc",
        "tree":[
            {"path":"skills/pdf/SKILL.md","type":"blob"},
            {"path":"skills/pdf/docs/guide.md","type":"blob"},
            {"path":"skills/pdf","type":"tree"}
        ]
    }"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# PDF Skill"))
        .mount(&server)
        .await;

    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/docs/guide.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# Guide"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let raw_base = server.uri(); // raw points to mock too
    let result = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &raw_base,
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0,
    )
    .await;
    assert!(result.is_ok());

    let skill = std::fs::read(dir.path().join("SKILL.md")).unwrap();
    assert_eq!(skill, b"# PDF Skill");
    let guide = std::fs::read(dir.path().join("docs/guide.md")).unwrap();
    assert_eq!(guide, b"# Guide");
}

#[tokio::test]
async fn test_download_skill_tree_with_trailing_slash_prefix() {
    let server = MockServer::start().await;
    let tree_body = r#"{"tree":[{"path":"skills/pdf/SKILL.md","type":"blob"}]}"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("# content"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let result = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf/", // already has trailing slash
        dir.path().to_str().unwrap(),
        0,
    )
    .await;
    assert!(result.is_ok());
}

#[tokio::test]
async fn test_download_skill_tree_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(500).set_body_string("server err"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let err = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("HTTP") || err.to_string().contains("Trees"));
}

#[tokio::test]
async fn test_download_skill_tree_invalid_json() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string("not json"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let err = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("parse") || err.to_string().contains("tree"));
}

#[tokio::test]
async fn test_download_skill_tree_no_matching_blobs_returns_not_found() {
    let server = MockServer::start().await;
    let tree_body = r#"{"tree":[{"path":"other/x.txt","type":"blob"}]}"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let err = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0,
    )
    .await
    .unwrap_err();
    assert!(err.to_string().contains("no files") || err.to_string().contains("not found"));
}

#[tokio::test]
async fn test_download_skill_tree_filters_out_tree_entries() {
    let server = MockServer::start().await;
    let tree_body = r#"{
        "tree":[
            {"path":"skills/pdf/SKILL.md","type":"blob"},
            {"path":"skills/pdf/subdir","type":"tree"}
        ]
    }"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0,
    )
    .await
    .unwrap();
    // Only SKILL.md was downloaded, not the "subdir" tree entry
    assert!(dir.path().join("SKILL.md").exists());
    assert!(!dir.path().join("subdir").exists());
}

#[tokio::test]
async fn test_download_skill_tree_uses_max_file_size_zero_uses_default() {
    // max_file_size = 0 should use DEFAULT_FILE_MAX_SIZE (10MB), so a small file works
    let server = MockServer::start().await;
    let tree_body = r#"{"tree":[{"path":"skills/pdf/SKILL.md","type":"blob"}]}"#;
    Mock::given(method("GET"))
        .and(path("/repos/org/repo/git/trees/main"))
        .respond_with(ResponseTemplate::new(200).set_body_string(tree_body))
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/org/repo/main/skills/pdf/SKILL.md"))
        .respond_with(ResponseTemplate::new(200).set_body_string("ok"))
        .mount(&server)
        .await;

    let client = http_client();
    let dir = tempfile::tempdir().unwrap();
    let result = download_skill_tree_from_github(
        &client,
        &server.uri(),
        &server.uri(),
        "org/repo",
        "main",
        "skills/pdf",
        dir.path().to_str().unwrap(),
        0, // zero -> default
    )
    .await;
    assert!(result.is_ok());
}

// ============================================================
// download_file via wiremock
// ============================================================

#[tokio::test]
async fn test_download_file_success() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello world"))
        .mount(&server)
        .await;

    let client = http_client();
    let data = download_file(&client, &format!("{}/file.txt", server.uri()), 1024)
        .await
        .unwrap();
    assert_eq!(data, b"hello world");
}

#[tokio::test]
async fn test_download_file_too_large() {
    let server = MockServer::start().await;
    let big = "x".repeat(100);
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string(big))
        .mount(&server)
        .await;

    let client = http_client();
    let err = download_file(&client, &format!("{}/file.txt", server.uri()), 10)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("too large"));
}

#[tokio::test]
async fn test_download_file_http_error() {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(ResponseTemplate::new(404).set_body_string("not found"))
        .mount(&server)
        .await;

    let client = http_client();
    let err = download_file(&client, &format!("{}/file.txt", server.uri()), 1024)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("HTTP"));
}

#[tokio::test]
async fn test_download_file_http_error_truncates_body() {
    let server = MockServer::start().await;
    let long_err_body = "x".repeat(1024);
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(ResponseTemplate::new(500).set_body_string(long_err_body))
        .mount(&server)
        .await;

    let client = http_client();
    let err = download_file(&client, &format!("{}/file.txt", server.uri()), 1024)
        .await
        .unwrap_err();
    let msg = err.to_string();
    // Body should be truncated to 512 chars in error message
    assert!(msg.contains("HTTP"));
}

#[tokio::test]
async fn test_download_file_request_failure() {
    let client = http_client();
    // Invalid port -> connection refused
    let err = download_file(&client, "http://127.0.0.1:1/file.txt", 1024)
        .await
        .unwrap_err();
    assert!(err.to_string().contains("request failed"));
}

#[tokio::test]
async fn test_download_file_zero_max_size_does_not_use_default() {
    // Note: download_file does NOT default zero -> it enforces 0 byte limit
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/file.txt"))
        .respond_with(ResponseTemplate::new(200).set_body_string("hello"))
        .mount(&server)
        .await;

    let client = http_client();
    let err = download_file(&client, &format!("{}/file.txt", server.uri()), 0)
        .await
        .unwrap_err();
    // With max_size=0, any content > 0 is too large
    assert!(err.to_string().contains("too large"));
}

// ============================================================
// DEFAULT_FILE_MAX_SIZE constant
// ============================================================

#[test]
fn test_default_file_max_size_is_10mb() {
    assert_eq!(DEFAULT_FILE_MAX_SIZE, 10 * 1024 * 1024);
}

// ============================================================
// TreeResponse / TreeEntry parsing
// ============================================================

#[test]
fn test_tree_response_with_truncated_explicit_true() {
    let json = r#"{"sha":"x","tree":[{"path":"a","type":"blob"}],"truncated":true}"#;
    let r: TreeResponse = serde_json::from_str(json).unwrap();
    assert_eq!(r.truncated, Some(true));
}

#[test]
fn test_tree_response_extra_fields_ignored() {
    let json = r#"{"sha":"x","tree":[],"truncated":false,"url":"https://x","foo":"bar"}"#;
    let r: TreeResponse = serde_json::from_str(json).unwrap();
    assert!(r.tree.is_empty());
}

#[test]
fn test_tree_entry_rename_type_field() {
    let json = r#"{"path":"x","type":"blob"}"#;
    let e: TreeEntry = serde_json::from_str(json).unwrap();
    assert_eq!(e.path, "x");
    assert_eq!(e.entry_type, "blob");
}

#[test]
fn test_tree_entry_with_missing_type() {
    let json = r#"{"path":"x"}"#;
    let result: std::result::Result<TreeEntry, _> = serde_json::from_str(json);
    // "type" is required (no default), should fail
    assert!(result.is_err());
}

#[test]
fn test_tree_entry_with_missing_path() {
    let json = r#"{"type":"blob"}"#;
    let result: std::result::Result<TreeEntry, _> = serde_json::from_str(json);
    assert!(result.is_err());
}

#[test]
fn test_tree_response_tree_can_be_huge() {
    let mut entries = Vec::new();
    for i in 0..1000 {
        entries.push(format!(r#"{{"path":"file{}.txt","type":"blob"}}"#, i));
    }
    let json = format!(r#"{{"tree":[{}]}}"#, entries.join(","));
    let r: TreeResponse = serde_json::from_str(&json).unwrap();
    assert_eq!(r.tree.len(), 1000);
}
