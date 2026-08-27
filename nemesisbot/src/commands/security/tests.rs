use super::*;

#[test]
fn test_match_pattern_exact() {
    assert!(match_pattern("test.exe", "test.exe"));
    assert!(!match_pattern("test.exe", "other.exe"));
}

#[test]
fn test_match_pattern_single_star() {
    assert!(match_pattern("*.exe", "test.exe"));
    assert!(!match_pattern("*.exe", "dir/test.exe"));
    assert!(match_pattern("test*", "testFile"));
}

#[test]
fn test_match_pattern_double_star() {
    assert!(match_pattern("**/*.exe", "test.exe"));
    assert!(match_pattern("**/*.exe", "dir/test.exe"));
    assert!(match_pattern("**/*.exe", "a/b/c/test.exe"));
    assert!(match_pattern("**", "anything"));
}

#[test]
fn test_match_pattern_mixed() {
    assert!(match_pattern("dir/*.log", "dir/test.log"));
    assert!(!match_pattern("dir/*.log", "dir/sub/test.log"));
    assert!(match_pattern("dir/**/*.log", "dir/sub/test.log"));
}

// -------------------------------------------------------------------------
// match_pattern additional comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_match_pattern_question_mark() {
    assert!(match_pattern("test?.exe", "test1.exe"));
    assert!(match_pattern("?est.exe", "test.exe"));
    assert!(!match_pattern("test?.exe", "test.exe")); // ? requires exactly one char
}

#[test]
fn test_match_pattern_empty_pattern() {
    assert!(match_pattern("", ""));
    assert!(!match_pattern("", "something"));
}

#[test]
fn test_match_pattern_only_stars() {
    assert!(match_pattern("*", "test"));
    assert!(match_pattern("**", "test"));
    // Single star does not cross path separators
    assert!(!match_pattern("*", "a/b/c"));
    assert!(match_pattern("**", "a/b/c")); // double star does
    assert!(match_pattern("***", "a/b/c"));
}

#[test]
fn test_match_pattern_backslash_normalization() {
    assert!(match_pattern("dir\\*.log", "dir/test.log"));
    assert!(match_pattern("dir/*.log", "dir\\test.log"));
    assert!(match_pattern("dir\\*.log", "dir\\test.log"));
}

#[test]
fn test_match_pattern_double_star_prefix() {
    assert!(match_pattern("**/test.log", "test.log"));
    assert!(match_pattern("**/test.log", "a/test.log"));
    assert!(match_pattern("**/test.log", "a/b/c/test.log"));
}

#[test]
fn test_match_pattern_double_star_suffix() {
    assert!(match_pattern("dir/**", "dir/file.txt"));
    assert!(match_pattern("dir/**", "dir/sub/file.txt"));
    assert!(match_pattern("dir/**", "dir/"));
}

#[test]
fn test_match_pattern_single_star_no_cross_separator() {
    assert!(!match_pattern("dir/*", "dir/sub/file")); // single * should not cross /
    assert!(match_pattern("dir/*", "dir/file"));
}

#[test]
fn test_match_pattern_exact_no_match() {
    assert!(!match_pattern("hello", "world"));
    assert!(!match_pattern("test.exe", "test.txt"));
}

#[test]
fn test_match_pattern_case_sensitive() {
    assert!(!match_pattern("TEST.exe", "test.exe"));
    assert!(match_pattern("test.exe", "test.exe"));
}

#[test]
fn test_match_pattern_partial_star() {
    assert!(match_pattern("test*", "testing123"));
    assert!(match_pattern("*test", "mytest"));
    assert!(match_pattern("*test*", "mytesting123"));
}

// -------------------------------------------------------------------------
// valid_operations_for_type tests
// -------------------------------------------------------------------------

#[test]
fn test_valid_operations_file() {
    let ops = valid_operations_for_type("file");
    assert!(ops.contains(&"read"));
    assert!(ops.contains(&"write"));
    assert!(ops.contains(&"delete"));
    assert_eq!(ops.len(), 3);
}

#[test]
fn test_valid_operations_directory() {
    let ops = valid_operations_for_type("directory");
    assert!(ops.contains(&"read"));
    assert!(ops.contains(&"create"));
    assert!(ops.contains(&"delete"));
}

#[test]
fn test_valid_operations_process() {
    let ops = valid_operations_for_type("process");
    assert!(ops.contains(&"exec"));
    assert!(ops.contains(&"spawn"));
    assert!(ops.contains(&"kill"));
    assert!(ops.contains(&"suspend"));
}

#[test]
fn test_valid_operations_network() {
    let ops = valid_operations_for_type("network");
    assert!(ops.contains(&"request"));
    assert!(ops.contains(&"download"));
    assert!(ops.contains(&"upload"));
}

#[test]
fn test_valid_operations_hardware() {
    let ops = valid_operations_for_type("hardware");
    assert!(ops.contains(&"i2c"));
    assert!(ops.contains(&"spi"));
    assert!(ops.contains(&"gpio"));
}

#[test]
fn test_valid_operations_registry() {
    let ops = valid_operations_for_type("registry");
    assert!(ops.contains(&"read"));
    assert!(ops.contains(&"write"));
    assert!(ops.contains(&"delete"));
}

#[test]
fn test_valid_operations_unknown() {
    let ops = valid_operations_for_type("unknown");
    assert!(ops.is_empty());
}

// -------------------------------------------------------------------------
// VALID_RULE_TYPES tests
// -------------------------------------------------------------------------

#[test]
fn test_valid_rule_types() {
    assert!(VALID_RULE_TYPES.contains(&"file"));
    assert!(VALID_RULE_TYPES.contains(&"directory"));
    assert!(VALID_RULE_TYPES.contains(&"process"));
    assert!(VALID_RULE_TYPES.contains(&"network"));
    assert!(VALID_RULE_TYPES.contains(&"hardware"));
    assert!(VALID_RULE_TYPES.contains(&"registry"));
    assert_eq!(VALID_RULE_TYPES.len(), 6);
}

// -------------------------------------------------------------------------
// default_security_config tests
// -------------------------------------------------------------------------

#[test]
fn test_default_security_config_structure() {
    let cfg = default_security_config();
    assert_eq!(cfg["default_action"], "ask");
    assert_eq!(cfg["log_all_operations"], false);
    assert_eq!(cfg["log_denials_only"], true);
    assert_eq!(cfg["approval_timeout"], 300);
    assert_eq!(cfg["max_pending_requests"], 10);
    assert_eq!(cfg["audit_retention_days"], 30);
    assert_eq!(cfg["audit_log_file_enabled"], true);
    assert_eq!(cfg["synchronous_mode"], false);
    assert!(cfg["pending"].is_array());
    assert!(cfg["rules"].is_object());
}

#[test]
fn test_default_rules_structure() {
    let rules = default_rules();
    assert!(rules["file"].is_array());
    assert!(rules["directory"].is_array());
    assert!(rules["process"].is_array());
    assert!(rules["network"].is_array());
    assert!(rules["hardware"].is_array());
    assert!(rules["registry"].is_array());
    // All should be empty arrays
    for key in &[
        "file",
        "directory",
        "process",
        "network",
        "hardware",
        "registry",
    ] {
        assert!(rules[*key].as_array().unwrap().is_empty());
    }
}

// -------------------------------------------------------------------------
// read_rules_config / write_rules_config tests
// -------------------------------------------------------------------------

#[test]
fn test_read_rules_config_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let cfg = read_rules_config(&path).unwrap();
    assert_eq!(cfg["default_action"], "ask");
}

#[test]
fn test_read_rules_config_existing_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "default_action": "deny",
        "rules": {
            "file": [{"pattern": "*.exe", "operation": "write", "action": "deny"}]
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    assert_eq!(cfg["default_action"], "deny");
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 1);
}

#[test]
fn test_read_rules_config_adds_missing_rules() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({"default_action": "allow"});
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    // Should have added rules section
    assert!(cfg["rules"].is_object());
}

#[test]
fn test_write_and_read_rules_config_roundtrip() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    let cfg = default_security_config();
    write_rules_config(&path, &cfg).unwrap();
    assert!(path.exists());
    let loaded = read_rules_config(&path).unwrap();
    assert_eq!(loaded["default_action"], "ask");
}

// -------------------------------------------------------------------------
// cmd_rules_list tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_list_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    cmd_rules_list(&path, None).unwrap();
}

#[test]
fn test_cmd_rules_list_with_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let cfg = default_security_config();
    write_rules_config(&path, &cfg).unwrap();
    cmd_rules_list(&path, None).unwrap();
}

#[test]
fn test_cmd_rules_list_invalid_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    cmd_rules_list(&path, Some("invalid_type")).unwrap();
}

#[test]
fn test_cmd_rules_list_specific_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let cfg = default_security_config();
    write_rules_config(&path, &cfg).unwrap();
    cmd_rules_list(&path, Some("file")).unwrap();
}

// -------------------------------------------------------------------------
// cmd_rules_add tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_add_valid() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "write", Some("*.exe"), Some("deny")).unwrap();

    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 1);
    assert_eq!(file_rules[0]["pattern"], "*.exe");
    assert_eq!(file_rules[0]["operation"], "write");
    assert_eq!(file_rules[0]["action"], "deny");
}

#[test]
fn test_cmd_rules_add_invalid_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "invalid", "read", None, None).unwrap();
    // Should succeed (prints error) but not add a rule
    let cfg = read_rules_config(&path).unwrap();
    assert!(cfg["rules"]["invalid"].is_null() || cfg["rules"].get("invalid").is_none());
}

#[test]
fn test_cmd_rules_add_invalid_operation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "launch", None, None).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert!(file_rules.is_empty());
}

#[test]
fn test_cmd_rules_add_invalid_action() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "read", None, Some("destroy")).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert!(file_rules.is_empty());
}

#[test]
fn test_cmd_rules_add_allow_action() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "read", Some("*.txt"), Some("allow")).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules[0]["action"], "allow");
}

#[test]
fn test_cmd_rules_add_ask_action() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "process", "exec", Some("rm"), Some("ask")).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    let rules = cfg["rules"]["process"].as_array().unwrap();
    assert_eq!(rules[0]["action"], "ask");
}

#[test]
fn test_cmd_rules_add_default_pattern() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "network", "request", None, None).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    let rules = cfg["rules"]["network"].as_array().unwrap();
    assert_eq!(rules[0]["pattern"], "*"); // default pattern
}

// -------------------------------------------------------------------------
// cmd_rules_remove tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_remove_valid() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.exe", "operation": "write", "action": "deny", "comment": ""},
        {"pattern": "*.txt", "operation": "read", "action": "allow", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_remove(&path, "file", "write", 0).unwrap();

    let loaded = read_rules_config(&path).unwrap();
    let file_rules = loaded["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 1);
    assert_eq!(file_rules[0]["operation"], "read");
}

#[test]
fn test_cmd_rules_remove_invalid_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_remove(&path, "invalid_type", "read", 0).unwrap();
}

#[test]
fn test_cmd_rules_remove_out_of_range() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.exe", "operation": "write", "action": "deny", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_remove(&path, "file", "write", 5).unwrap();
    // No crash, no change
    let loaded = read_rules_config(&path).unwrap();
    let file_rules = loaded["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 1);
}

// -------------------------------------------------------------------------
// cmd_rules_test tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_test_matching_rule() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.exe", "operation": "write", "action": "deny", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_test(&path, "file", "write", "test.exe").unwrap();
}

#[test]
fn test_cmd_rules_test_no_matching_rule() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.exe", "operation": "write", "action": "deny", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_test(&path, "file", "write", "test.txt").unwrap();
}

#[test]
fn test_cmd_rules_test_invalid_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();
    cmd_rules_test(&path, "invalid", "read", "test").unwrap();
}

#[test]
fn test_cmd_rules_test_wildcard_operation() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.log", "operation": "*", "action": "allow", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    // Should match any operation
    cmd_rules_test(&path, "file", "write", "test.log").unwrap();
    cmd_rules_test(&path, "file", "read", "test.log").unwrap();
}

// -------------------------------------------------------------------------
// cmd_pending / cmd_approve / cmd_deny tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_pending_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let security_cfg = tmp
        .path()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_pending(&security_cfg).unwrap();
}

#[test]
fn test_cmd_pending_empty_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    std::fs::write(&pending_path, "[]").unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_pending(&security_cfg).unwrap();
}

#[test]
fn test_cmd_approve_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let security_cfg = tmp
        .path()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_approve(&security_cfg, "test-id").unwrap();
}

#[test]
fn test_cmd_approve_existing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    // cmd_approve resolves: security_cfg.parent().parent().join("workspace").join("security").join("pending.json")
    // security_cfg = home/workspace/config/config.security.json
    // So pending.json is at: home/workspace/workspace/security/pending.json
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    let pending = serde_json::json!([
        {"id": "op-1", "operation": "file_write", "tool_name": "test"},
        {"id": "op-2", "operation": "process_exec", "tool_name": "test"}
    ]);
    std::fs::write(&pending_path, serde_json::to_string(&pending).unwrap()).unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_approve(&security_cfg, "op-1").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    let remaining = data.as_array().unwrap();
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0]["id"], "op-2");
}

#[test]
fn test_cmd_approve_not_found() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    std::fs::write(&pending_path, r#"[{"id": "op-1"}]"#).unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_approve(&security_cfg, "nonexistent-id").unwrap();
}

#[test]
fn test_cmd_deny_existing() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    let pending = serde_json::json!([{"id": "op-1"}, {"id": "op-2"}]);
    std::fs::write(&pending_path, serde_json::to_string(&pending).unwrap()).unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_deny(&security_cfg, "op-1", Some("dangerous")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert_eq!(data.as_array().unwrap().len(), 1);
}

#[test]
fn test_cmd_deny_no_file() {
    let tmp = tempfile::TempDir::new().unwrap();
    let security_cfg = tmp
        .path()
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_deny(&security_cfg, "test-id", None).unwrap();
}

// -------------------------------------------------------------------------
// Additional coverage tests for security
// -------------------------------------------------------------------------

#[test]
fn test_match_pattern_edge_cases() {
    // Empty inputs
    assert!(match_pattern("", ""));
    assert!(!match_pattern("", "x"));

    // Pattern longer than value
    assert!(!match_pattern("test.exe.bak", "test.exe"));

    // Multiple wildcards
    assert!(match_pattern("*.*", "test.exe"));
    assert!(match_pattern("*.*", "a.b"));
    assert!(!match_pattern("*.*", "noext"));

    // Consecutive stars
    assert!(match_pattern("**/**", "a/b"));
    assert!(match_pattern("**/**", "a/b/c/d"));
}

#[test]
fn test_valid_operations_all_types() {
    let types = vec![
        "file",
        "directory",
        "process",
        "network",
        "hardware",
        "registry",
    ];
    for t in &types {
        let ops = valid_operations_for_type(t);
        assert!(!ops.is_empty(), "Type '{}' should have operations", t);
    }
    assert!(valid_operations_for_type("unknown_type").is_empty());
    assert!(valid_operations_for_type("").is_empty());
}

#[test]
fn test_default_security_config_all_fields() {
    let cfg = default_security_config();
    assert_eq!(cfg["default_action"], "ask");
    assert_eq!(cfg["log_all_operations"], false);
    assert_eq!(cfg["log_denials_only"], true);
    assert_eq!(cfg["approval_timeout"], 300);
    assert_eq!(cfg["max_pending_requests"], 10);
    assert_eq!(cfg["audit_retention_days"], 30);
    assert_eq!(cfg["audit_log_file_enabled"], true);
    assert_eq!(cfg["synchronous_mode"], false);
    assert!(cfg["pending"].is_array());
    assert!(cfg["pending"].as_array().unwrap().is_empty());
}

#[test]
fn test_default_rules_all_types() {
    let rules = default_rules();
    for t in &[
        "file",
        "directory",
        "process",
        "network",
        "hardware",
        "registry",
    ] {
        assert!(rules[*t].is_array());
        assert!(rules[*t].as_array().unwrap().is_empty());
    }
}

#[test]
fn test_read_rules_config_with_rules_present() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "default_action": "deny",
        "rules": {
            "file": [{"pattern": "*.exe", "operation": "write", "action": "deny"}],
            "process": []
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    assert_eq!(cfg["default_action"], "deny");
    assert!(cfg["rules"]["file"].as_array().unwrap().len() == 1);
}

#[test]
fn test_cmd_rules_list_all_types() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    for t in &[
        "file",
        "directory",
        "process",
        "network",
        "hardware",
        "registry",
    ] {
        cmd_rules_list(&path, Some(t)).unwrap();
    }
}

#[test]
fn test_cmd_rules_add_multiple_rules_same_type() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "write", Some("*.exe"), Some("deny")).unwrap();
    cmd_rules_add(&path, "file", "write", Some("*.dll"), Some("deny")).unwrap();
    cmd_rules_add(&path, "file", "read", Some("*.txt"), Some("allow")).unwrap();

    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 3);
}

#[test]
fn test_cmd_rules_add_all_action_types() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_add(&path, "file", "read", Some("*.log"), Some("allow")).unwrap();
    cmd_rules_add(&path, "file", "write", Some("*.sys"), Some("deny")).unwrap();
    cmd_rules_add(&path, "file", "delete", Some("*.tmp"), Some("ask")).unwrap();

    let cfg = read_rules_config(&path).unwrap();
    let file_rules = cfg["rules"]["file"].as_array().unwrap();
    assert_eq!(file_rules.len(), 3);
}

#[test]
fn test_cmd_rules_remove_from_empty() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_remove(&path, "file", "read", 0).unwrap();
}

#[test]
fn test_cmd_rules_test_all_match_types() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_test(&path, "file", "read", "test.txt").unwrap();
    cmd_rules_test(&path, "directory", "create", "/tmp/test").unwrap();
    cmd_rules_test(&path, "process", "exec", "ls").unwrap();
    cmd_rules_test(&path, "network", "request", "example.com").unwrap();
}

#[test]
fn test_cmd_deny_with_reason() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    let pending = serde_json::json!([{"id": "op-1"}, {"id": "op-2"}]);
    std::fs::write(&pending_path, serde_json::to_string(&pending).unwrap()).unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_deny(&security_cfg, "op-2", Some("too dangerous")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert_eq!(data.as_array().unwrap().len(), 1);
}

#[test]
fn test_cmd_deny_without_reason() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending_path = dir.join("pending.json");
    let pending = serde_json::json!([{"id": "op-x"}]);
    std::fs::write(&pending_path, serde_json::to_string(&pending).unwrap()).unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_deny(&security_cfg, "op-x", None).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert!(data.as_array().unwrap().is_empty());
}

// -------------------------------------------------------------------------
// Additional match_pattern coverage
// -------------------------------------------------------------------------

#[test]
fn test_match_pattern_double_star_middle() {
    // Pattern with ** in the middle
    assert!(match_pattern("a/**/b", "a/x/y/z/b"));
    assert!(match_pattern("a/**/b", "a/b"));
}

#[test]
fn test_match_pattern_complex_patterns() {
    assert!(match_pattern("**/src/**/*.rs", "src/main.rs"));
    assert!(match_pattern("**/src/**/*.rs", "src/lib/module.rs"));
    assert!(match_pattern("*.exe", "malware.exe"));
    assert!(!match_pattern("*.exe", "malware.txt"));
    assert!(match_pattern("/tmp/**", "/tmp/session_123/output.log"));
}

#[test]
fn test_match_pattern_unicode() {
    assert!(match_pattern("*", "test"));
    assert!(match_pattern("test", "test"));
}

// -------------------------------------------------------------------------
// valid_operations_for_type comprehensive
// -------------------------------------------------------------------------

#[test]
fn test_valid_operations_for_file() {
    let ops = valid_operations_for_type("file");
    assert_eq!(ops, &["read", "write", "delete"]);
}

#[test]
fn test_valid_operations_for_directory() {
    let ops = valid_operations_for_type("directory");
    assert_eq!(ops, &["read", "create", "delete"]);
}

#[test]
fn test_valid_operations_for_process() {
    let ops = valid_operations_for_type("process");
    assert!(ops.contains(&"exec"));
    assert!(ops.contains(&"spawn"));
    assert!(ops.contains(&"kill"));
    assert!(ops.contains(&"suspend"));
    assert_eq!(ops.len(), 4);
}

#[test]
fn test_valid_operations_for_network() {
    let ops = valid_operations_for_type("network");
    assert_eq!(ops, &["request", "download", "upload"]);
}

#[test]
fn test_valid_operations_for_hardware() {
    let ops = valid_operations_for_type("hardware");
    assert_eq!(ops, &["i2c", "spi", "gpio"]);
}

#[test]
fn test_valid_operations_for_registry() {
    let ops = valid_operations_for_type("registry");
    assert_eq!(ops, &["read", "write", "delete"]);
}

// -------------------------------------------------------------------------
// default_security_config comprehensive tests
// -------------------------------------------------------------------------

#[test]
fn test_default_security_config_pending_is_empty() {
    let cfg = default_security_config();
    let pending = cfg["pending"].as_array().unwrap();
    assert!(pending.is_empty());
}

#[test]
fn test_default_security_config_rules_all_empty() {
    let cfg = default_security_config();
    let rules = &cfg["rules"];
    for key in &[
        "file",
        "directory",
        "process",
        "network",
        "hardware",
        "registry",
    ] {
        assert!(
            rules[key].as_array().unwrap().is_empty(),
            "Rule type '{}' should be empty",
            key
        );
    }
}

// -------------------------------------------------------------------------
// read_rules_config edge cases
// -------------------------------------------------------------------------

#[test]
fn test_read_rules_config_with_partial_rules() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("config.security.json");
    let data = serde_json::json!({
        "default_action": "deny",
        "rules": {
            "file": [{"pattern": "*.exe", "operation": "write", "action": "deny"}]
            // Missing other types
        }
    });
    std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();
    let cfg = read_rules_config(&path).unwrap();
    assert_eq!(cfg["rules"]["file"].as_array().unwrap().len(), 1);
}

// -------------------------------------------------------------------------
// cmd_rules_add with all rule types
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_add_for_each_type() {
    let types_and_ops = vec![
        ("file", "read"),
        ("directory", "create"),
        ("process", "exec"),
        ("network", "request"),
        ("hardware", "i2c"),
        ("registry", "read"),
    ];

    for (rule_type, operation) in &types_and_ops {
        let tmp = tempfile::TempDir::new().unwrap();
        let dir = tmp.path().join("config");
        let path = dir.join("config.security.json");
        write_rules_config(&path, &default_security_config()).unwrap();

        cmd_rules_add(&path, rule_type, operation, Some("*.test"), Some("deny")).unwrap();

        let cfg = read_rules_config(&path).unwrap();
        let rules = cfg["rules"][*rule_type].as_array().unwrap();
        assert_eq!(rules.len(), 1, "Failed for type: {}", rule_type);
        assert_eq!(rules[0]["pattern"], "*.test");
        assert_eq!(rules[0]["operation"], *operation);
        assert_eq!(rules[0]["action"], "deny");
    }
}

// -------------------------------------------------------------------------
// cmd_rules_remove edge cases
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_remove_middle_index() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["network"] = serde_json::json!([
        {"pattern": "*.com", "operation": "request", "action": "allow", "comment": ""},
        {"pattern": "*.evil", "operation": "request", "action": "deny", "comment": "bad"},
        {"pattern": "*.local", "operation": "request", "action": "allow", "comment": ""}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_remove(&path, "network", "request", 1).unwrap();

    let loaded = read_rules_config(&path).unwrap();
    let rules = loaded["rules"]["network"].as_array().unwrap();
    assert_eq!(rules.len(), 2);
    assert_eq!(rules[0]["pattern"], "*.com");
    assert_eq!(rules[1]["pattern"], "*.local");
}

// -------------------------------------------------------------------------
// cmd_pending tests
// -------------------------------------------------------------------------

#[test]
fn test_cmd_pending_no_dir() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_pending(&security_cfg).unwrap();
}

#[test]
fn test_cmd_pending_with_entries() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending = serde_json::json!([
        {"id": "op-1", "operation": "file_write", "target": "/etc/test", "risk": "HIGH"},
        {"id": "op-2", "operation": "process_exec", "target": "rm -rf /", "risk": "CRITICAL"}
    ]);
    std::fs::write(
        dir.join("pending.json"),
        serde_json::to_string(&pending).unwrap(),
    )
    .unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");
    cmd_pending(&security_cfg).unwrap();
}

// -------------------------------------------------------------------------
// cmd_approve with actual entries
// -------------------------------------------------------------------------

#[test]
fn test_cmd_approve_removes_entry() {
    let tmp = tempfile::TempDir::new().unwrap();
    let home = tmp.path();
    let dir = home.join("workspace").join("workspace").join("security");
    std::fs::create_dir_all(&dir).unwrap();
    let pending = serde_json::json!([{"id": "op-approve-test"}, {"id": "op-other"}]);
    std::fs::write(
        dir.join("pending.json"),
        serde_json::to_string(&pending).unwrap(),
    )
    .unwrap();
    let security_cfg = home
        .join("workspace")
        .join("config")
        .join("config.security.json");

    cmd_approve(&security_cfg, "op-approve-test").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(dir.join("pending.json")).unwrap()).unwrap();
    let arr = data.as_array().unwrap();
    assert_eq!(arr.len(), 1);
    assert_eq!(arr[0]["id"], "op-other");
}

// -------------------------------------------------------------------------
// VALID_RULE_TYPES constant test
// -------------------------------------------------------------------------

#[test]
fn test_valid_rule_types_has_six_entries() {
    assert_eq!(VALID_RULE_TYPES.len(), 6);
    assert!(VALID_RULE_TYPES.contains(&"file"));
    assert!(VALID_RULE_TYPES.contains(&"directory"));
    assert!(VALID_RULE_TYPES.contains(&"process"));
    assert!(VALID_RULE_TYPES.contains(&"network"));
    assert!(VALID_RULE_TYPES.contains(&"hardware"));
    assert!(VALID_RULE_TYPES.contains(&"registry"));
}

// -------------------------------------------------------------------------
// cmd_rules_test with matching rules
// -------------------------------------------------------------------------

#[test]
fn test_cmd_rules_test_with_matching_deny_rule() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");

    let mut cfg = default_security_config();
    cfg["rules"]["file"] = serde_json::json!([
        {"pattern": "*.exe", "operation": "write", "action": "deny", "comment": "block exe"}
    ]);
    write_rules_config(&path, &cfg).unwrap();

    cmd_rules_test(&path, "file", "write", "malware.exe").unwrap();
}

#[test]
fn test_cmd_rules_test_no_match() {
    let tmp = tempfile::TempDir::new().unwrap();
    let dir = tmp.path().join("config");
    let path = dir.join("config.security.json");
    write_rules_config(&path, &default_security_config()).unwrap();

    cmd_rules_test(&path, "file", "read", "safe.txt").unwrap();
}

// -------------------------------------------------------------------------
// write_rules_config with nested path
// -------------------------------------------------------------------------

#[test]
fn test_write_rules_config_creates_dirs() {
    let tmp = tempfile::TempDir::new().unwrap();
    let path = tmp.path().join("a").join("b").join("c").join("config.json");
    let cfg = default_security_config();
    write_rules_config(&path, &cfg).unwrap();
    assert!(path.exists());
}

// =========================================================================
// S11b 覆盖率冲刺：security run() 全 arm 扫描（Status/Enable/Disable/
// Config Show+Edit/Audit Show+Export+Denied/Test/Rules 分发/Approve/Deny/
// Pending/Edit）+ cmd_edit 三分支（EDITOR 成功/非零/不存在）。
// ConfigReset 走 stdin 交互（豁免，不测）。
//
// 隔离：NEMESISBOT_HOME → 临时根（home = {tmp}/.nemesisbot），全程持
// crate::GLOBAL_STATE_LOCK；EDITOR 覆盖测试同锁内 set/restore。
// =========================================================================

struct S11bTempHomeEnv {
    _tmp: tempfile::TempDir,
    home: std::path::PathBuf,
}

impl Drop for S11bTempHomeEnv {
    fn drop(&mut self) {
        unsafe { std::env::remove_var("NEMESISBOT_HOME") };
    }
}

fn s11b_temp_home_env() -> S11bTempHomeEnv {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("config")).unwrap();
    unsafe { std::env::set_var("NEMESISBOT_HOME", tmp.path()) };
    S11bTempHomeEnv { _tmp: tmp, home }
}

/// EDITOR 环境变量 RAII：set → drop 恢复原值（或移除）。
struct S11bEditorEnv(Option<String>);

impl S11bEditorEnv {
    fn set(val: &str) -> Self {
        let old = std::env::var("EDITOR").ok();
        unsafe { std::env::set_var("EDITOR", val) };
        S11bEditorEnv(old)
    }
}

impl Drop for S11bEditorEnv {
    fn drop(&mut self) {
        match self.0.take() {
            Some(v) => unsafe { std::env::set_var("EDITOR", v) },
            None => unsafe { std::env::remove_var("EDITOR") },
        }
    }
}

// ------------------------------- Status -----------------------------------

#[tokio::test]
async fn test_s11b_run_status_default_no_files() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    // 无 config.json / 无 security cfg → 全默认 + "Scanner: not configured"
    run(SecurityAction::Status, false).await.unwrap();
    assert!(!crate::common::config_path(&th.home).exists());
}

#[tokio::test]
async fn test_s11b_run_status_with_configs() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    // 主配置：security.enabled=false + restrict_to_workspace=true
    let cfg_path = crate::common::config_path(&th.home);
    std::fs::write(
        &cfg_path,
        r#"{"security": {"enabled": false}, "agents": {"defaults": {"restrict_to_workspace": true}}}"#,
    )
    .unwrap();
    // security cfg：scanner 段 + 带 op 计数与不带 op 计数的 rules
    let sec_cfg = crate::common::security_config_path(&th.home);
    std::fs::write(
        &sec_cfg,
        r#"{
            "default_action": "deny",
            "log_all_operations": true,
            "audit_log_file_enabled": true,
            "approval_timeout": 60,
            "audit_retention_days": 7,
            "enabled": ["clamav"],
            "restrict_to_workspace": false,
            "rules": {
                "file": [{"operation": "read", "pattern": "*.txt", "action": "deny"},
                          {"operation": "read", "pattern": "*.md", "action": "deny"}],
                "network": [{"operation": "weird_op", "pattern": "*", "action": "deny"}]
            }
        }"#,
    )
    .unwrap();
    run(SecurityAction::Status, false).await.unwrap();
}

// --------------------------- Enable / Disable -----------------------------

#[tokio::test]
async fn test_s11b_run_enable_disable_flips() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let cfg_path = crate::common::config_path(&th.home);
    std::fs::write(
        &cfg_path,
        r#"{"security": {"enabled": false}, "agents": {"defaults": {"restrict_to_workspace": true}}}"#,
    )
    .unwrap();
    let sec_cfg = crate::common::security_config_path(&th.home);

    run(SecurityAction::Enable, false).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["security"]["enabled"], true);
    assert_eq!(v["agents"]["defaults"]["restrict_to_workspace"], false);
    assert!(sec_cfg.exists(), "Enable 会落默认 security 配置");

    run(SecurityAction::Disable, false).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["security"]["enabled"], false);
    assert_eq!(v["agents"]["defaults"]["restrict_to_workspace"], true);
}

#[tokio::test]
async fn test_s11b_run_enable_disable_insert_missing_sections() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let cfg_path = crate::common::config_path(&th.home);
    // 实测行为：security 段缺失会补插；agents 段整体缺失时【不】补插
    // （外层 if let 无 else），restrict_to_workspace 维持未写。
    std::fs::write(&cfg_path, "{}").unwrap();
    run(SecurityAction::Enable, false).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["security"]["enabled"], true);
    assert!(v.get("agents").is_none(), "agents 段缺失时不补插（现状）");
    // agents 段存在但无 defaults → 走 defaults 补插分支
    std::fs::write(&cfg_path, r#"{"agents": {}}"#).unwrap();
    run(SecurityAction::Enable, false).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["agents"]["defaults"]["restrict_to_workspace"], false);
    std::fs::write(&cfg_path, r#"{"agents": {}}"#).unwrap();
    run(SecurityAction::Disable, false).await.unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["agents"]["defaults"]["restrict_to_workspace"], true);
    // 无 config.json 时 Enable/Disable 只落 security cfg（不崩）
    std::fs::remove_file(&cfg_path).unwrap();
    run(SecurityAction::Disable, false).await.unwrap();
    assert!(!cfg_path.exists());
}

// ------------------------------ Config ------------------------------------

#[tokio::test]
async fn test_s11b_run_config_show_missing_and_present() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let sec_cfg = crate::common::security_config_path(&th.home);
    // 无文件：None 与 Show 都走 default 分支
    run(SecurityAction::Config { action: None }, false).await.unwrap();
    run(
        SecurityAction::Config {
            action: Some(SecurityConfigAction::Show),
        },
        false,
    )
    .await
    .unwrap();
    // 有文件：打印文件内容
    std::fs::create_dir_all(sec_cfg.parent().unwrap()).unwrap();
    std::fs::write(&sec_cfg, r#"{"default_action": "marker-s11b"}"#).unwrap();
    run(
        SecurityAction::Config {
            action: Some(SecurityConfigAction::Show),
        },
        false,
    )
    .await
    .unwrap();
}

/// cmd_edit / SecurityAction::Edit 三分支：EDITOR 成功退出 / 非零退出 / 不存在。
#[tokio::test]
async fn test_s11b_run_edit_via_editor_env() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();

    // 1) EDITOR=hostname（存在、退出 0）→ Configuration saved 分支；
    //    同时覆盖「配置不存在时先落默认」的分支
    {
        let _ed = S11bEditorEnv::set("hostname");
        run(
            SecurityAction::Config {
                action: Some(SecurityConfigAction::Edit),
            },
            false,
        )
        .await
        .unwrap();
        let sec_cfg = crate::common::security_config_path(&_th.home);
        assert!(sec_cfg.exists(), "cmd_edit 先写默认配置再开编辑器");
    }

    // 2) EDITOR=不存在的命令 → Failed to open editor 分支
    {
        let _ed = S11bEditorEnv::set("definitely-missing-editor-s11b");
        run(
            SecurityAction::Config {
                action: Some(SecurityConfigAction::Edit),
            },
            false,
        )
        .await
        .unwrap();
    }

    // 3) EDITOR=脚本退出 3 → "Editor exited with status" 分支；顺带覆盖顶层 Edit arm
    let tmp = tempfile::tempdir().unwrap();
    let bat = tmp.path().join("fail3.bat");
    std::fs::write(&bat, "@exit /b 3\r\n").unwrap();
    {
        let _ed = S11bEditorEnv::set(bat.to_str().unwrap());
        run(SecurityAction::Edit, false).await.unwrap();
    }
}

// ------------------------------- Audit ------------------------------------

#[tokio::test]
async fn test_s11b_run_audit_show_export_denied() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let audit_path = crate::common::workspace_path(&th.home).join("audit_chain.jsonl");
    std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();
    let line = |ts: &str, op: &str, tool: &str, dec: &str, reason: &str| {
        serde_json::json!({
            "timestamp": ts, "operation": op, "tool_name": tool,
            "decision": dec, "reason": reason
        })
        .to_string()
    };
    std::fs::write(
        &audit_path,
        [
            line("t1", "file_read", "read_file", "allowed", ""),
            line("t2", "process_exec", "exec", "denied", "dangerous command"),
            line("t3", "file_write", "write_file", "denied", "outside workspace"),
            "not-json-line".to_string(),
        ]
        .join("\n"),
    )
    .unwrap();

    // Show：默认 limit=20（None）与 Some(Show{limit:1})
    run(SecurityAction::Audit { action: None }, false).await.unwrap();
    run(
        SecurityAction::Audit {
            action: Some(AuditAction::Show { limit: 1 }),
        },
        false,
    )
    .await
    .unwrap();
    // Denied：2 条 denied + 总数行
    run(
        SecurityAction::Audit {
            action: Some(AuditAction::Denied),
        },
        false,
    )
    .await
    .unwrap();
    // Export：写出 JSON（跳过坏行）
    let out = th.home.join("export.json");
    run(
        SecurityAction::Audit {
            action: Some(AuditAction::Export {
                output: out.to_string_lossy().into_owned(),
            }),
        },
        false,
    )
    .await
    .unwrap();
    let v: serde_json::Value = serde_json::from_str(&std::fs::read_to_string(&out).unwrap()).unwrap();
    assert_eq!(v["total_entries"], 3, "坏行被 filter_map 跳过");
    assert_eq!(v["entries"].as_array().unwrap().len(), 3);

    // 无审计文件的三条分支
    std::fs::remove_file(&audit_path).unwrap();
    run(SecurityAction::Audit { action: None }, false).await.unwrap();
    run(
        SecurityAction::Audit {
            action: Some(AuditAction::Denied),
        },
        false,
    )
    .await
    .unwrap();
    let out2 = th.home.join("export2.json");
    run(
        SecurityAction::Audit {
            action: Some(AuditAction::Export {
                output: out2.to_string_lossy().into_owned(),
            }),
        },
        false,
    )
    .await
    .unwrap();
    assert!(!out2.exists(), "无审计文件不落 export");
}

// -------------------------------- Test ------------------------------------

#[tokio::test(flavor = "multi_thread")]
async fn test_s11b_run_security_test_allowed_blocked_invalid_json() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    // ALLOWED：低风险读文件
    run(
        SecurityAction::Test {
            tool: "read_file".into(),
            args: r#"{"path": "a.txt"}"#.into(),
        },
        false,
    )
    .await
    .unwrap();
    // BLOCKED：命令守卫确定性拦截 rm -rf /
    run(
        SecurityAction::Test {
            tool: "exec".into(),
            args: r#"{"command": "rm -rf /"}"#.into(),
        },
        false,
    )
    .await
    .unwrap();
    // 非法 JSON args → 错误打印
    run(
        SecurityAction::Test {
            tool: "read_file".into(),
            args: "not json".into(),
        },
        false,
    )
    .await
    .unwrap();
}

// ------------------------------- Rules ------------------------------------

#[tokio::test]
async fn test_s11b_run_rules_dispatch_arms() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    // List：无类型 + 指定类型（无配置文件 → 默认规则）
    run(
        SecurityAction::Rules {
            action: RulesAction::List { rule_type: None },
        },
        false,
    )
    .await
    .unwrap();
    run(
        SecurityAction::Rules {
            action: RulesAction::List {
                rule_type: Some("file".into()),
            },
        },
        false,
    )
    .await
    .unwrap();
    // Test：对默认（空）规则集测试目标
    run(
        SecurityAction::Rules {
            action: RulesAction::Test {
                rule_type: "file".into(),
                operation: "read".into(),
                target: "x.txt".into(),
            },
        },
        false,
    )
    .await
    .unwrap();
}

// --------------------- Approve / Deny / Pending ---------------------------
// 注意（挂账 S11b-1，quality-hardening goal 冲刺 S11）：cmd_approve/cmd_deny/
// cmd_pending 经 security_cfg（<home>/workspace/config/config.security.json）
// parent().parent() 再 join("workspace")/join("security") → 解析到
// <home>/workspace/workspace/security/pending.json（双 workspace）。
// 且全代码库零写入者（ApprovalManager 挂内存）→ 该文件永远不存在，
// CLI 审批三命令实际永远走 "No pending operations found"。
// 本测试把夹具放在当前（错误的）解析路径以钉住现状；修复路径时此测试需同步改。

fn s11b_write_dead_pending(home: &std::path::Path, ids: &[&str]) -> std::path::PathBuf {
    let pending_path = home
        .join("workspace")
        .join("workspace")
        .join("security")
        .join("pending.json");
    std::fs::create_dir_all(pending_path.parent().unwrap()).unwrap();
    let arr: Vec<serde_json::Value> = ids
        .iter()
        .map(|id| serde_json::json!({"id": id, "operation": "file_write", "args": {}}))
        .collect();
    std::fs::write(&pending_path, serde_json::to_string_pretty(&arr).unwrap()).unwrap();
    pending_path
}

#[tokio::test]
async fn test_s11b_run_approve_deny_pending() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let th = s11b_temp_home_env();
    let pending_path = s11b_write_dead_pending(&th.home, &["op-1", "op-2"]);

    // Pending：列出条目
    run(SecurityAction::Pending, false).await.unwrap();
    // Approve：op-1 移除
    run(SecurityAction::Approve { id: "op-1".into() }, false)
        .await
        .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert_eq!(v.as_array().unwrap().len(), 1);
    assert_eq!(v[0]["id"], "op-2");
    // Approve 不存在的 id
    run(SecurityAction::Approve { id: "nope".into() }, false)
        .await
        .unwrap();
    // Deny：带 reason（Vec join 分支）与不带（None 分支）
    run(
        SecurityAction::Deny {
            id: "op-2".into(),
            reason: vec!["too".into(), "risky".into()],
        },
        false,
    )
    .await
    .unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&pending_path).unwrap()).unwrap();
    assert!(v.as_array().unwrap().is_empty());
    run(
        SecurityAction::Deny {
            id: "op-2".into(),
            reason: vec![],
        },
        false,
    )
    .await
    .unwrap();
    // 文件移除后 → "No pending operations found."
    std::fs::remove_file(&pending_path).unwrap();
    run(SecurityAction::Pending, false).await.unwrap();
    run(SecurityAction::Approve { id: "x".into() }, false)
        .await
        .unwrap();
}

// ---------------------------- Scanner 委派 --------------------------------

#[tokio::test]
async fn test_s11b_run_scanner_delegate_list() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let _th = s11b_temp_home_env();
    // security scanner 子命令直接委派给 scanner 模块（无配置 → 空表）
    run(
        SecurityAction::Scanner {
            action: ScannerAction::List,
        },
        false,
    )
    .await
    .unwrap();
}

// =========================================================================
// wave_b（覆盖率补测 2026-08-27）：规则引擎剩余可测臂 + run() 分发缺口。
//   - 列表三态：有条目分组打印 / rules 空对象 "No rules defined."；
//   - add 对缺类型键的 rules 对象 insert 空数组臂；
//   - test 的 op 不匹配 continue 臂 + ask→deny 审批 reason 臂；
//   - pending 存在但为空数组的臂（走真实解析路径 pending.json="[]"）；
//   - write_rules_config 落盘失败上抛（父目录段被普通文件阻断）；
//   - run() 分发 Rules::Add / Rules::Remove 两臂（既有 dispatch 测试只走了
//     List/Test，Add/Remove 的解构传参臂从未到过）。
// 豁免：EDITOR 未设时的 notepad/vi GUI 回退（会 spawn 编辑器）、ConfigReset
// 全链路（真 stdin 交互，无法在测试里喂输入）。
// =========================================================================
mod wave_b {
    use super::*;

    /// 直接写一份 config.security.json（不经 write_rules_config，控制原始形状）。
    fn wb_write_cfg(path: &std::path::Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn wave_b_rules_list_prints_indexed_entries_per_operation() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        // file: read x2（索引 [0]/[1] 连续出现在同一 op 组），write x1，
        // network: request x1；其余 op 打 (none)。
        wb_write_cfg(
            &path,
            r#"{"rules": {"file": [
                {"pattern":"*.txt","operation":"read","action":"allow"},
                {"pattern":"secret*","operation":"read","action":"deny"},
                {"pattern":"*.log","operation":"write","action":"ask"}
              ],
              "network": [{"pattern":"**","operation":"request","action":"deny"}]}}"#,
        );
        cmd_rules_list(&path, None).unwrap();
        // 指定类型也过一遍过滤分支
        cmd_rules_list(&path, Some("file")).unwrap();
    }

    #[test]
    fn wave_b_rules_list_empty_rules_object_says_no_rules_defined() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        // rules 是空对象：所有 get(rt) 都 None → found_any=false + 无 rule_type
        // → "No rules defined."（区别于缺 rules 键——那走 default_rules 全空数组）。
        wb_write_cfg(&path, r#"{"rules": {}}"#);
        cmd_rules_list(&path, None).unwrap();
    }

    #[test]
    fn wave_b_rules_add_creates_missing_type_key_array_then_pushes_entry() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        wb_write_cfg(&path, r#"{"rules": {}}"#);

        cmd_rules_add(&path, "registry", "read", Some("**HKLM**"), Some("deny")).unwrap();

        let cfg = read_rules_config(&path).unwrap();
        let arr = cfg["rules"]["registry"].as_array().expect("type key created");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["pattern"], "**HKLM**");
        assert_eq!(arr[0]["operation"], "read");
        assert_eq!(arr[0]["action"], "deny");
    }

    #[test]
    fn wave_b_rules_test_operation_mismatch_continues_to_later_rules() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        // 规则 0：pattern `*` 会匹配任意 target，但 operation=delete ≠ write
        // → 必须被 continue 跳过；规则 1 才是命中者。若 continue 缺失/错位，
        // 规则 0 会把 final_action 抢成 deny（本例断言只能靠行为顺序无副作用，
        // 所以再放一个 allow 收尾规则验证「跳过后仍能继续扫」）。
        wb_write_cfg(
            &path,
            r#"{"rules": {"file": [
                {"pattern":"*","operation":"delete","action":"deny"},
                {"pattern":"*.txt","operation":"write","action":"allow"}
              ]}}"#,
        );
        cmd_rules_test(&path, "file", "write", "a.txt").unwrap();
    }

    #[test]
    fn wave_b_rules_test_ask_action_reports_approval_reason_and_deny() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        wb_write_cfg(
            &path,
            r#"{"rules": {"network": [
                {"pattern":"*.evil.example","operation":"request","action":"ask"}
              ]}}"#,
        );
        // ask 命中 → final_action 映射为 deny + reason 走「requires approval」臂。
        cmd_rules_test(&path, "network", "request", "c2.evil.example").unwrap();
    }

    #[tokio::test]
    async fn wave_b_pending_existing_file_with_empty_array_prints_no_ops() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        // 双 workspace 解析路径（S11b-1 挂账形状）：pending.json 存在且为 []，
        // 走 exists→读取→空数组提示臂（非不存在早退、也非条目列表）。
        let pending_path = s11b_write_dead_pending(&th.home, &[]);
        assert_eq!(
            std::fs::read_to_string(&pending_path).unwrap().trim(),
            "[]",
            "空 ids 应写出空数组"
        );
        run(SecurityAction::Pending, false).await.unwrap();
    }

    #[test]
    fn wave_b_write_rules_config_errors_when_parent_component_is_regular_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        // 中间路径段是普通文件 → create_dir_all 被 `let _` 吞掉后
        // fs::write 的 `?` 上抛（security.rs:208-216 的 Err 路径）。
        let blocker = tmp.path().join("blocker");
        std::fs::write(&blocker, b"occupies dir slot").unwrap();
        let cfg_path = blocker.join("sub").join("config.security.json");
        let res = write_rules_config(&cfg_path, &default_security_config());
        assert!(res.is_err(), "fs::write 到文件下级路径必须失败");
        assert!(!cfg_path.exists());
    }

    #[tokio::test]
    async fn wave_b_run_rules_add_dispatch_destructures_and_persists_rule() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = th.home.join("workspace").join("config").join("config.security.json");

        run(
            SecurityAction::Rules {
                action: RulesAction::Add {
                    rule_type: "process".into(),
                    operation: "exec".into(),
                    pattern: Some("**taskkill**".into()),
                    action: Some("deny".into()),
                },
            },
            false,
        )
        .await
        .unwrap();

        let cfg = read_rules_config(&sec_cfg).unwrap();
        let arr = cfg["rules"]["process"].as_array().expect("run-level Add 写盘");
        assert_eq!(arr.len(), 1);
        assert_eq!(arr[0]["operation"], "exec");
        assert_eq!(arr[0]["action"], "deny");
    }

    #[tokio::test]
    async fn wave_b_run_rules_remove_dispatch_removes_matched_operation_entry() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = th.home.join("workspace").join("config").join("config.security.json");

        // 预置两条 file.read 规则；Remove #0 应摘除第一条（matching[0]=实际 idx 0）
        cmd_rules_add(&sec_cfg, "file", "read", Some("*.tmp"), Some("allow")).unwrap();
        cmd_rules_add(&sec_cfg, "file", "read", Some("keep.txt"), Some("deny")).unwrap();

        run(
            SecurityAction::Rules {
                action: RulesAction::Remove {
                    rule_type: "file".into(),
                    operation: "read".into(),
                    index: 0,
                },
            },
            false,
        )
        .await
        .unwrap();

        let cfg = read_rules_config(&sec_cfg).unwrap();
        let arr = cfg["rules"]["file"].as_array().unwrap();
        assert_eq!(arr.len(), 1, "#0 已摘除");
        assert_eq!(arr[0]["pattern"], "keep.txt", "留下的是第二条");
    }

    #[tokio::test]
    async fn wave_b_audit_denied_with_only_allowed_entries_prints_no_ops() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let audit_path = crate::common::workspace_path(&th.home).join("audit_chain.jsonl");
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();
        // 审计文件存在但全是 allowed（区别于无文件早退、也区别于有 denied 条目
        // 的列表臂）→ 过滤后空集 → "No denied operations found." 臂。
        std::fs::write(
            &audit_path,
            [
                r#"{"timestamp":"t1","operation":"file_read","tool_name":"read_file","decision":"allowed","reason":""}"#,
                r#"{"timestamp":"t2","operation":"dir_list","tool_name":"list_dir","decision":"allowed","reason":""}"#,
            ]
            .join("\n"),
        )
        .unwrap();

        run(
            SecurityAction::Audit {
                action: Some(AuditAction::Denied),
            },
            false,
        )
        .await
        .unwrap();
    }
}

// =========================================================================
// wave_c（覆盖率补测 C 波）：损坏配置夹具臂 + EOF 零输入交互臂。
//   - security 配置文件存在但 JSON 解析失败：read_rules_config 的 `?`
//     上抛 + run() 各 caller（Status / Rules Add）的报错传播与
//     「错误时不得改盘」不变量；
//   - 主配置 config.json 损坏：Status / Enable / Disable 的早退传播，
//     并钉住 Enable 的副作用顺序——主配置解析失败必须先于默认
//     security 配置的播种（否则会留下半初始化现场）；
//   - 主配置是合法 JSON 但非对象（数组）：Enable 对主文件静默无操作、
//     默认 security 配置照常播种（现状行为钉住）；
//   - Config Show 对任意字节原样打印、从不解析（宽容行为钉住）；
//   - ConfigReset 在 EOF stdin（cargo test 常态）下走 Aborted 臂，
//     文件不被触碰；直接函数层 + run() 分发层各过一遍。
//     （确认=y 的真重置链路仍是豁免项——需要喂真实 stdin。）
//   - Audit Export 输出路径被普通文件阻断时的写盘 Err 上抛
//     （成功臂已由 S11b 的 export 断言覆盖，此处补 Err 臂）。
// =========================================================================
mod wave_c {
    use super::*;

    /// 原样写一份（可能损坏的）security 配置，不经 write_rules_config，
    /// 以控制磁盘上的原始字节。
    fn wc_write_raw(path: &std::path::Path, body: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, body).unwrap();
    }

    #[test]
    fn wc_read_rules_config_corrupt_json_is_err() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.security.json");
        // 截断的 JSON：exists()==true 但 from_str 失败 → Err 上抛（167-168）。
        wc_write_raw(&path, r#"{"default_action": "deny""#);
        assert!(read_rules_config(&path).is_err());
    }

    #[tokio::test]
    async fn wc_run_status_corrupt_security_config_propagates_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = crate::common::security_config_path(&th.home);
        wc_write_raw(&sec_cfg, "{{{ definitely-not-json");
        // Status 在打印完头部后 read_rules_config 上抛 → run 返回 Err（不 panic）
        assert!(run(SecurityAction::Status, false).await.is_err());
    }

    #[tokio::test]
    async fn wc_run_status_corrupt_main_config_propagates_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        std::fs::write(crate::common::config_path(&th.home), "{ broken").unwrap();
        // Status 第一段读主配置处 from_str 失败即上抛
        assert!(run(SecurityAction::Status, false).await.is_err());
    }

    #[tokio::test]
    async fn wc_run_enable_corrupt_main_config_fails_before_seeding_security_defaults() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        std::fs::write(
            crate::common::config_path(&th.home),
            "{ broken-main-cfg",
        )
        .unwrap();
        let sec_cfg = crate::common::security_config_path(&th.home);

        assert!(run(SecurityAction::Enable, false).await.is_err());
        // 副作用顺序不变量：Enable 先处理主配置再播种默认 security 配置，
        // 所以主配置坏了时后者绝不能被创建（半初始化现场防护）。
        assert!(
            !sec_cfg.exists(),
            "主配置解析失败时不得播种默认 security 配置"
        );
    }

    #[tokio::test]
    async fn wc_run_disable_corrupt_main_config_propagates_error() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        std::fs::write(
            crate::common::config_path(&th.home),
            r#"{"security": "#,
        )
        .unwrap();
        assert!(run(SecurityAction::Disable, false).await.is_err());
    }

    #[tokio::test]
    async fn wc_run_enable_non_object_main_config_is_silent_noop_for_main_file() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let cfg_path = crate::common::config_path(&th.home);
        // 合法 JSON 但顶层是数组：as_object_mut() 为 None → security 段插入被
        // 静默跳过且不回写主文件；默认 security 配置照常播种（现状钉住）。
        std::fs::write(&cfg_path, "[1, 2]").unwrap();
        let sec_cfg = crate::common::security_config_path(&th.home);

        run(SecurityAction::Enable, false).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap().trim(),
            "[1, 2]",
            "非对象主配置不被改写"
        );
        assert!(sec_cfg.exists(), "默认 security 配置照常播种");
    }

    #[tokio::test]
    async fn wc_run_rules_add_on_corrupt_policy_errors_and_preserves_garbage() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = crate::common::security_config_path(&th.home);
        const GARBAGE: &str = r#"{"rules": {"file": ["#;
        wc_write_raw(&sec_cfg, GARBAGE);

        let res = run(
            SecurityAction::Rules {
                action: RulesAction::Add {
                    rule_type: "file".into(),
                    operation: "read".into(),
                    pattern: Some("**blocked**".into()),
                    action: Some("deny".into()),
                },
            },
            false,
        )
        .await;

        // 解析失败上抛；磁盘上的坏内容原封不动（错误路径不改盘）
        assert!(res.is_err());
        assert_eq!(
            std::fs::read_to_string(&sec_cfg).unwrap(),
            GARBAGE,
            "报错路径不得半写"
        );
    }

    #[tokio::test]
    async fn wc_run_config_show_prints_raw_bytes_without_parsing() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = crate::common::security_config_path(&th.home);
        // Show 不做任何 JSON 校验，任意字节原样输出且命令成功（宽容行为现状）。
        wc_write_raw(&sec_cfg, "<html>not-json & raw bytes 0x01\x02");
        run(
            SecurityAction::Config {
                action: Some(SecurityConfigAction::Show),
            },
            false,
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn wc_run_config_reset_stdin_eof_aborts_without_touching_file() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = crate::common::security_config_path(&th.home);
        wc_write_raw(&sec_cfg, r#"{"sentinel": true}"#);

        // cargo test 下 stdin 为管道 EOF → read_line 得空串 → 非 y → Aborted。
        // 函数层与 run() 分发层各走一遍；确认=y 的重置链路仍是豁免项。
        cmd_config_reset(&sec_cfg).unwrap();
        run(SecurityAction::ConfigReset, false).await.unwrap();

        assert_eq!(
            std::fs::read_to_string(&sec_cfg).unwrap().trim(),
            r#"{"sentinel": true}"#,
            "Aborted 臂不得触碰配置文件"
        );
    }

    /// Edit 收尾分支收口：EDITOR 脚本退出码 0 且吃掉路径参数 → 命中
    /// match status 的 success 分支（"Configuration saved." 打印区）。
    /// （既有 S11b 用 EDITOR=hostname 尝试过此分支，但 hostname 带参
    /// 实际退非零 → 只到过 "exited with status" 分支。）
    #[tokio::test]
    async fn wc_run_edit_editor_exit_zero_reaches_saved_branch() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let _th = s11b_temp_home_env();
        let tmp = tempfile::tempdir().unwrap();
        let bat = tmp.path().join("ok0.bat");
        std::fs::write(&bat, "@rem noop\r\n@exit /b 0\r\n").unwrap();
        let _ed = S11bEditorEnv::set(bat.to_str().unwrap());
        run(SecurityAction::Edit, false).await.unwrap();
    }

    #[tokio::test]
    async fn wc_run_audit_export_write_failure_propagates_when_output_blocked() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let audit_path = crate::common::workspace_path(&th.home).join("audit_chain.jsonl");
        std::fs::create_dir_all(audit_path.parent().unwrap()).unwrap();
        std::fs::write(
            &audit_path,
            r#"{"timestamp":"t","operation":"file_read","tool_name":"read_file","decision":"allowed","reason":""}"#,
        )
        .unwrap();

        // 输出路径的父目录段被普通文件占用 → fs::write 失败 → Export `?` 上抛
        // （Export 成功写入臂由 S11b 覆盖；此处补写盘 Err 臂）。
        let blocker = th.home.join("outblock");
        std::fs::write(&blocker, b"occupies dir slot").unwrap();
        let out = blocker.join("export.json");
        let res = run(
            SecurityAction::Audit {
                action: Some(AuditAction::Export {
                    output: out.to_string_lossy().into_owned(),
                }),
            },
            false,
        )
        .await;
        assert!(res.is_err(), "被阻断的输出路径必须上抛而非静默成功");
        assert!(!out.exists());
    }
}

// ===========================================================================
// r10（覆盖率 goal R10 批）：config reset 确认=y 链路（此前 S11b/wave_c 的
// 明示豁免项）+ 嵌套 Config 分发线 + corrupt 夹具直调四连。
//   • 子进程 "y\n"：test_harness 起真二进制喂 stdin，覆盖 cmd_config_reset
//     的确认臂（写默认配置 + 回执打印）与嵌套派发行（Nested Reset → fn）。
//   • 进程内嵌套分发：stdin 管道 EOF → Aborted 臂（不触盘），同锁纪律。
//   • corrupt 直调族：read_rules_config 合法但结构错位（标量根 / 标量槽 /
//     对象槽）时 add/remove/test 的 if-let 失败跳过路径——纯函数调用，
//     显式路径无 env 依赖。
// ===========================================================================

mod r10_arcs {
    use super::*;
    use test_harness::{resolve_nemesisbot_bin, TestWorkspace};

    /// 确认=y：真二进制 `security config reset` 吃到 "y" → 重置为默认并落盘。
    #[tokio::test]
    async fn r10_config_reset_answered_y_resets_file_to_defaults_subprocess() {
        let ws = TestWorkspace::new().unwrap();
        std::fs::create_dir_all(ws.home()).unwrap();
        let sec_cfg = crate::common::security_config_path(&ws.home());
        std::fs::create_dir_all(sec_cfg.parent().unwrap()).unwrap();
        std::fs::write(&sec_cfg, r#"{"default_action":"zz-r10-sentinel"}"#).unwrap();

        let Ok(bin) = resolve_nemesisbot_bin() else {
            return;
        };
        let out = ws
            .run_cli_with_stdin(
                &bin,
                &["security", "config", "reset"],
                "y\n",
                60,
            )
            .await;

        assert!(
            out.success(),
            "确认=y 是正常收尾 rc 0\nstdout={} stderr={}",
            out.stdout,
            out.stderr
        );
        assert!(
            out.stdout_contains("Security configuration reset to defaults."),
            "必须走重置回执而非 Aborted：{}",
            out.stdout
        );

        // 文件被 default_security_config 整体替换：sentinel 消失，
        // default_action 回到默认值。
        let raw = std::fs::read_to_string(&sec_cfg).unwrap();
        assert!(!raw.contains("zz-r10-sentinel"), "旧内容被整体替换: {raw}");
        let v: serde_json::Value = serde_json::from_str(&raw).unwrap();
        assert_eq!(v["default_action"], "ask", "reset 后默认动作是 ask: {raw}");
    }

    /// 嵌套派发：run(Config{Some(Reset)}) → 分发行 → cmd_config_reset；
    /// cargo test 下 stdin 为管道 EOF → "非 y" → Aborted 且不动盘。
    #[tokio::test]
    async fn r10_nested_config_reset_dispatch_eof_aborts_without_touching_file() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let sec_cfg = crate::common::security_config_path(&th.home);
        std::fs::write(&sec_cfg, r#"{"r10":"untouched"}"#).unwrap();

        run(
            SecurityAction::Config {
                action: Some(SecurityConfigAction::Reset),
            },
            false,
        )
        .await
        .expect("EOF → Aborted → Ok");

        assert_eq!(
            std::fs::read_to_string(&sec_cfg).unwrap(),
            r#"{"r10":"untouched"}"#,
            "未确认的重置不得触碰配置"
        );
    }

    // --------------------- corrupt 结构直调族 ---------------------

    /// 标量根 `{"rules":42}`：add 的规则表 if-let 整体跳过，但收尾仍整包重写
    /// （cfg 值原样序列化）；Ok 不 panic、rules 仍为标量。
    #[test]
    fn r10_rules_add_scalar_rules_root_skips_merge_but_keeps_value() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.security.json");
        std::fs::write(&path, r#"{"rules":42}"#).unwrap();

        cmd_rules_add(&path, "file", "read", Some("*.txt"), None)
            .expect("标量根不算解析错误，静默跳过 merge");

        let v: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert_eq!(
            v["rules"],
            42,
            "merge 被跳过 → rules 必须仍是原标量"
        );
    }

    /// 类型槽标量 `{"rules":{"file":7}}`：remove 找不到数组 → not-found 臂
    /// 且**不回写**（字节级原样）。
    #[test]
    fn r10_rules_remove_scalar_type_slot_reports_not_found_without_write() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.security.json");
        std::fs::write(&path, r#"{"rules":{"file":7}}"#).unwrap();

        cmd_rules_remove(&path, "file", "read", 0).expect("槽标量 → found=false → Ok");

        assert_eq!(
            std::fs::read_to_string(&path).unwrap(),
            r#"{"rules":{"file":7}}"#,
            "not-found 不得回写"
        );
    }

    /// 类型槽对象 `{"rules":{"file":{"a":1}}}`：test 的 as_array 失败 →
    /// 规则遍历跳过 → 默认 deny 结论臂。
    #[test]
    fn r10_rules_test_object_type_slot_skips_iteration_defaults_deny() {
        let tmp = tempfile::tempdir().unwrap();
        let path = tmp.path().join("config.security.json");
        std::fs::write(&path, r#"{"rules":{"file":{"a":1}}}"#).unwrap();

        cmd_rules_test(&path, "file", "read", "*.txt")
            .expect("槽对象 → 无数组可比对 → Ok");
    }

    /// config.json 是顶层数组时 Disable：as_object_mut 失败 → 整段编辑跳过、
    /// 不回写主配置、也不创建 security 配置（Disable 与 Enable 不同，后者
    /// 会补默认 security cfg）。
    #[tokio::test]
    async fn r10_disable_top_level_array_config_skips_edit_without_writes() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let th = s11b_temp_home_env();
        let cfg_path = crate::common::config_path(&th.home);
        std::fs::write(&cfg_path, "[1,2]").unwrap();
        let sec_cfg = crate::common::security_config_path(&th.home);

        run(SecurityAction::Disable, false).await.expect("非对象主配置 → 跳过编辑 → Ok");

        assert_eq!(
            std::fs::read_to_string(&cfg_path).unwrap(),
            "[1,2]",
            "非对象主配置必须原样保留"
        );
        assert!(
            !sec_cfg.exists(),
            "Disable 不负责补建 security 配置（那是 Enable 的职责）"
        );
    }
}
