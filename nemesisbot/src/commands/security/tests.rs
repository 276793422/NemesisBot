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
