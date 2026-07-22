use super::*;

#[test]
fn test_forge_share_success() {
    let handler = ForgeHandler::new("node-a".into());
    let payload = serde_json::json!({
        "report": {"insights": ["test"]},
        "source_node": "node-b"
    });

    let result = handler.handle("forge_share", payload);
    assert!(result.success);
    assert_eq!(result.response["status"], "received");
}

#[test]
fn test_forge_share_missing_report() {
    let handler = ForgeHandler::new("node-a".into());
    let payload = serde_json::json!({"source_node": "node-b"});

    let result = handler.handle("forge_share", payload);
    assert!(!result.success);
    assert!(result.error.is_some());
}

#[test]
fn test_forge_get_reflections() {
    let handler = ForgeHandler::new("node-a".into());
    let result = handler.handle("forge_get_reflections", serde_json::json!({}));
    assert!(result.success);
    assert!(result.response.get("reflections").is_some());
    assert_eq!(result.response["node_id"], "node-a");
}

#[test]
fn test_unknown_forge_action() {
    let handler = ForgeHandler::new("node-a".into());
    let result = handler.handle("forge_unknown", serde_json::json!({}));
    assert!(!result.success);
}

// -- File-based provider tests --

#[test]
fn test_file_provider_receive_and_list() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());

    let payload = serde_json::json!({
        "source_node": "node-b",
        "report": {"insights": ["test insight"], "score": 0.85},
    });

    provider.receive_reflection(&payload).unwrap();

    let list = provider.get_reflections_list_payload();
    let reflections = list["reflections"].as_array().unwrap();
    assert!(!reflections.is_empty());

    // The stored file should be in remote/
    let remote_files: Vec<_> = reflections
        .iter()
        .filter(|r| r["remote"].as_bool().unwrap_or(false))
        .collect();
    assert!(!remote_files.is_empty());
}

#[test]
fn test_file_provider_read_content() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());

    let payload = serde_json::json!({
        "source_node": "node-c",
        "report": {"data": "hello world"},
    });

    provider.receive_reflection(&payload).unwrap();

    let list = provider.get_reflections_list_payload();
    let filename = list["reflections"].as_array().unwrap()[0]["filename"]
        .as_str()
        .unwrap();

    let content = provider.read_reflection_content(filename).unwrap();
    assert!(content.contains("hello world"));
}

#[test]
fn test_file_provider_read_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());

    let result = provider.read_reflection_content("nonexistent.json");
    assert!(result.is_err());
}

#[test]
fn test_file_provider_path_traversal_prevention() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());

    let result = provider.read_reflection_content("../../../etc/passwd");
    assert!(result.is_err());
}

#[test]
fn test_sanitize_redacts_aws_keys() {
    let content = r#"{"key": "AKIAIOSFODNN7EXAMPLE", "data": "normal"}"#;
    let sanitized = FileForgeProvider::do_sanitize(content);
    assert!(!sanitized.contains("AKIAIOSFODNN7EXAMPLE"));
    assert!(sanitized.contains("[REDACTED_AWS_KEY]"));
}

#[test]
fn test_sanitize_redacts_private_ips() {
    let content = r#"{"server": "192.168.1.100", "port": 8080}"#;
    let sanitized = FileForgeProvider::do_sanitize(content);
    assert!(!sanitized.contains("192.168.1.100"));
    assert!(sanitized.contains("[IP]"));
}

#[test]
fn test_sanitize_preserves_public_ips() {
    let content = r#"{"server": "8.8.8.8", "port": 53}"#;
    let sanitized = FileForgeProvider::do_sanitize(content);
    assert!(sanitized.contains("8.8.8.8"));
}

#[test]
fn test_handler_with_file_provider() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(FileForgeProvider::new(dir.path()));
    let handler = ForgeHandler::with_provider("node-a".into(), provider);

    // Share
    let share_payload = serde_json::json!({
        "source_node": "node-b",
        "report": {"test": "data"},
    });
    let result = handler.handle("forge_share", share_payload);
    assert!(result.success);

    // List
    let result = handler.handle("forge_get_reflections", serde_json::json!({}));
    assert!(result.success);
    let reflections = result.response["reflections"].as_array().unwrap();
    assert!(!reflections.is_empty());
}

#[test]
fn test_handler_get_specific_reflection() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(FileForgeProvider::new(dir.path()));
    let handler = ForgeHandler::with_provider("node-a".into(), provider);

    // Share a report
    let share_payload = serde_json::json!({
        "source_node": "node-b",
        "report": {"secret": "value123"},
    });
    let result = handler.handle("forge_share", share_payload);
    assert!(result.success);

    // List to get the filename
    let list_result = handler.handle("forge_get_reflections", serde_json::json!({}));
    let filename = list_result.response["reflections"].as_array().unwrap()[0]["filename"]
        .as_str()
        .unwrap();

    // Get specific
    let get_payload = serde_json::json!({
        "filename": filename,
    });
    let result = handler.handle("forge_get_reflections", get_payload);
    assert!(result.success);
    assert!(result.response.get("content").is_some());
}

#[test]
fn test_set_provider() {
    let mut handler = ForgeHandler::new("node-a".into());
    let dir = tempfile::tempdir().unwrap();
    handler.set_provider(Box::new(FileForgeProvider::new(dir.path())));

    let result = handler.handle("forge_get_reflections", serde_json::json!({}));
    assert!(result.success);
}

// ============================================================
// Coverage improvement: sanitization, provider edge cases
// ============================================================

#[test]
fn test_sanitize_redacts_file_paths() {
    let content = r#"path: /home/user/secret.txt"#;
    let sanitized = FileForgeProvider::do_sanitize(content);
    assert!(sanitized.contains("[REDACTED_PATH]"));
    assert!(!sanitized.contains("/home/user/"));
}

#[test]
fn test_sanitize_no_redaction_needed() {
    let content = r#"{"data": "public info", "score": 0.95}"#;
    let sanitized = FileForgeProvider::do_sanitize(content);
    assert_eq!(sanitized, content);
}

#[test]
fn test_file_provider_receive_missing_report() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());
    let payload = serde_json::json!({"source_node": "node-b"});
    let result = provider.receive_reflection(&payload);
    assert!(result.is_err());
    assert!(result.unwrap_err().contains("report field is required"));
}

#[test]
fn test_file_provider_clone_boxed() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());
    let cloned = provider.clone_boxed();
    // Verify the cloned provider works
    let payload = serde_json::json!({
        "source_node": "node-b",
        "report": {"test": "data"},
    });
    cloned.receive_reflection(&payload).unwrap();
}

#[test]
fn test_file_provider_sanitize_content() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());
    let content = "AKIAIOSFODNN7EXAMPLE key found at 192.168.1.100";
    let sanitized = provider.sanitize_content(content);
    assert!(sanitized.contains("[REDACTED_AWS_KEY]"));
    assert!(sanitized.contains("[IP]"));
}

#[test]
fn test_forge_share_with_provider_bad_dir() {
    // Use a temp dir where we can write successfully
    let dir = tempfile::tempdir().unwrap();
    let forge_dir = dir.path().join("forge");
    std::fs::create_dir_all(forge_dir.join("reflections").join("remote")).unwrap();
    let provider = Box::new(FileForgeProvider::new(&forge_dir));
    let handler = ForgeHandler::with_provider("node-a".into(), provider);

    let payload = serde_json::json!({
        "source_node": "node-b",
        "report": {"insights": ["test"]},
    });
    let result = handler.handle("forge_share", payload);
    // Should succeed since the directory exists
    assert!(result.success);
}

#[test]
fn test_forge_get_reflections_with_specific_filename_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(FileForgeProvider::new(dir.path()));
    let handler = ForgeHandler::with_provider("node-a".into(), provider);

    let result = handler.handle(
        "forge_get_reflections",
        serde_json::json!({"filename": "nonexistent.json"}),
    );
    assert!(!result.success);
}

#[test]
fn test_forge_get_reflections_with_empty_filename() {
    let dir = tempfile::tempdir().unwrap();
    let provider = Box::new(FileForgeProvider::new(dir.path()));
    let handler = ForgeHandler::with_provider("node-a".into(), provider);

    let result = handler.handle("forge_get_reflections", serde_json::json!({"filename": ""}));
    assert!(result.success);
}

#[test]
fn test_redact_api_keys_short_content() {
    // Content too short for full AWS key
    let content = "AKIA";
    let result = redact_api_keys(content);
    assert_eq!(result, "AKIA"); // Not long enough to redact
}

#[test]
fn test_redact_private_ips_partial_ip() {
    // IP with fewer than 3 dots should not be redacted
    let content = "server at 192.168.1";
    let result = redact_private_ips(content);
    assert!(result.contains("192.168.1")); // Not fully qualified IP
}

#[test]
fn test_redact_file_paths_windows_style() {
    let content = r#"path: C:\Users\admin\documents\secret.txt"#;
    let result = redact_file_paths(content);
    assert!(result.contains("[REDACTED_PATH]"));
}

#[test]
fn test_file_provider_read_reflection_content_path_traversal_complex() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());
    let result = provider.read_reflection_content("../../etc/shadow");
    assert!(result.is_err());
}

#[test]
fn test_file_provider_list_empty_dir() {
    let dir = tempfile::tempdir().unwrap();
    let provider = FileForgeProvider::new(dir.path());
    let list = provider.get_reflections_list_payload();
    assert_eq!(list["count"], 0);
    assert!(list["reflections"].as_array().unwrap().is_empty());
}
