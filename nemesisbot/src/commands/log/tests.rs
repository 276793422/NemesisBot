use super::*;
use tempfile::TempDir;

fn make_config(tmp: &TempDir) -> std::path::PathBuf {
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({
        "logging": default_logging_config()
    });
    std::fs::write(&cfg, serde_json::to_string_pretty(&config).unwrap()).unwrap();
    cfg
}

#[test]
fn test_default_logging_config_structure() {
    let cfg = default_logging_config();
    assert_eq!(cfg["llm"]["enabled"], false);
    assert_eq!(cfg["llm"]["detail_level"], "full");
    assert_eq!(cfg["general"]["enabled"], true);
    assert_eq!(cfg["general"]["level"], "INFO");
    assert_eq!(cfg["general"]["console"], true);
}

#[test]
fn test_expand_tilde_home() {
    let expanded = expand_tilde("~/test/path");
    assert!(!expanded.starts_with('~'));
    assert!(expanded.contains("test") || expanded.contains("path"));
}

#[test]
fn test_expand_tilde_root() {
    let expanded = expand_tilde("~");
    assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
}

#[test]
fn test_expand_tilde_no_tilde() {
    let expanded = expand_tilde("/absolute/path");
    assert_eq!(expanded, "/absolute/path");
}

#[test]
fn test_expand_tilde_backslash() {
    let expanded = expand_tilde("~\\test");
    // Should expand on Windows
    assert!(!expanded.starts_with('~') || !dirs::home_dir().is_some());
}

#[test]
fn test_resolve_path_absolute() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("/absolute/path", tmp.path());
    // On Windows, /absolute/path becomes C:/absolute/path
    assert!(resolved.contains("absolute"));
    assert!(resolved.contains("path"));
}

#[test]
fn test_resolve_path_relative() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("relative/path", tmp.path());
    assert!(resolved.starts_with(&tmp.path().to_string_lossy().to_string()));
    assert!(resolved.contains("relative"));
}

#[test]
fn test_resolve_path_tilde() {
    let tmp = TempDir::new().unwrap();
    let resolved = resolve_path("~/logs", tmp.path());
    assert!(!resolved.starts_with('~') || !dirs::home_dir().is_some());
}

#[test]
fn test_read_logging_config_no_file() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("nonexistent.json");
    let config = read_logging_config(&cfg_path).unwrap();
    assert_eq!(config["llm"]["enabled"], false);
}

#[test]
fn test_read_logging_config_with_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let config = read_logging_config(&cfg).unwrap();
    assert_eq!(config["general"]["level"], "INFO");
}

#[test]
fn test_read_logging_config_file_without_logging() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": "data"}"#).unwrap();
    let config = read_logging_config(&cfg).unwrap();
    // Should return default
    assert_eq!(config["llm"]["enabled"], false);
}

#[test]
fn test_write_logging_config_creates_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let logging = default_logging_config();

    write_logging_config(&cfg, &logging).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data.get("logging").is_some());
}

#[test]
fn test_write_logging_config_preserves_other_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": "data", "version": "1.0"}"#).unwrap();

    let logging = default_logging_config();
    write_logging_config(&cfg, &logging).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["other"], "data");
    assert_eq!(data["version"], "1.0");
    assert!(data.get("logging").is_some());
}

#[test]
fn test_cmd_llm_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
}

#[test]
fn test_cmd_llm_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_llm_disable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], false);
}

#[test]
fn test_cmd_general_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // First disable
    cmd_general_disable(&cfg).unwrap();
    // Then enable
    cmd_general_enable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enabled"], true);
}

#[test]
fn test_cmd_general_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_disable(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enabled"], false);
}

#[test]
fn test_cmd_general_level_valid() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "DEBUG").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "DEBUG");
}

#[test]
fn test_cmd_general_level_case_insensitive() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "warn").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "WARN");
}

#[test]
fn test_cmd_general_level_invalid() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_level(&cfg, "INVALID").unwrap();

    // Level should remain unchanged
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "INFO");
}

#[test]
fn test_cmd_general_file() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    cmd_general_file(&cfg, "/tmp/test.log").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["file"], "/tmp/test.log");
}

#[test]
fn test_cmd_general_console_toggle() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);

    // Default is true, toggle should set to false
    cmd_general_console(&cfg).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], false);

    // Toggle again should set to true
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], true);
}

#[test]
fn test_cmd_llm_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_general_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    cmd_general_status(&cfg).unwrap();
}

#[test]
fn test_cmd_all_status() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_all_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_llm_config_detail_level() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, Some("truncated"), None).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["detail_level"], "truncated");
}

#[test]
fn test_cmd_llm_config_log_dir() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, None, Some("my-logs")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    let log_dir = data["logging"]["llm"]["log_dir"].as_str().unwrap();
    assert!(log_dir.contains("my-logs"));
}

#[test]
fn test_cmd_llm_config_no_changes() {
    let tmp = TempDir::new().unwrap();
    let cfg = make_config(&tmp);
    let workspace = tmp.path().join("workspace");

    cmd_llm_config(&cfg, &workspace, None, None).unwrap();
    // Should succeed with no changes
}

#[test]
fn test_llm_enable_no_existing_section() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, r#"{"other": true}"#).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
}

// -------------------------------------------------------------------------
// Additional log tests for coverage
// -------------------------------------------------------------------------

#[test]
fn test_expand_tilde_no_home_dir() {
    // Just verify it returns the original path for non-tilde paths
    let result = expand_tilde("/absolute/path");
    assert_eq!(result, "/absolute/path");

    let result = expand_tilde("relative/path");
    assert_eq!(result, "relative/path");
}

#[test]
fn test_resolve_path_various() {
    let tmp = TempDir::new().unwrap();

    // Absolute path
    let result = resolve_path("/abs/path", tmp.path());
    assert!(result.contains("abs"));

    // Relative path
    let result = resolve_path("logs/test", tmp.path());
    assert!(result.starts_with(&tmp.path().to_string_lossy().to_string()));

    // Tilde path
    let result = resolve_path("~/my-logs", tmp.path());
    assert!(!result.starts_with('~') || dirs::home_dir().is_none());
}

#[test]
fn test_default_logging_config_completeness() {
    let cfg = default_logging_config();
    // LLM section
    assert!(cfg.get("llm").is_some());
    assert_eq!(cfg["llm"]["enabled"], false);
    assert_eq!(cfg["llm"]["detail_level"], "full");
    assert_eq!(cfg["llm"]["log_dir"], "logs/request_logs");

    // General section
    assert!(cfg.get("general").is_some());
    assert_eq!(cfg["general"]["enabled"], true);
    assert_eq!(cfg["general"]["level"], "INFO");
    assert_eq!(cfg["general"]["console"], true);
    assert_eq!(cfg["general"]["enable_console"], true);
}

#[test]
fn test_write_logging_config_to_new_path() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("subdir").join("config.json");
    let logging = default_logging_config();

    write_logging_config(&cfg, &logging).unwrap();

    // Directory should be created
    assert!(cfg.parent().unwrap().exists());
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert!(data.get("logging").is_some());
}

#[test]
fn test_cmd_llm_enable_with_empty_log_dir() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({
        "logging": {
            "llm": {
                "enabled": false,
                "detail_level": "",
                "log_dir": ""
            }
        }
    });
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_enable(&cfg, &workspace).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["enabled"], true);
    // Empty fields should be filled with defaults
    assert_eq!(data["logging"]["llm"]["log_dir"], "logs/request_logs");
    assert_eq!(data["logging"]["llm"]["detail_level"], "full");
}

#[test]
fn test_cmd_general_level_various_valid_levels() {
    for level in &["DEBUG", "INFO", "WARN", "ERROR", "FATAL", "TRACE"] {
        let tmp = TempDir::new().unwrap();
        let cfg = tmp.path().join("config.json");
        let config = serde_json::json!({"logging": default_logging_config()});
        std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

        cmd_general_level(&cfg, level).unwrap();

        let data: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
        assert_eq!(data["logging"]["general"]["level"], *level);
    }
}

#[test]
fn test_cmd_general_level_lowercase_input() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    cmd_general_level(&cfg, "error").unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["level"], "ERROR");
}

#[test]
fn test_cmd_general_console_multiple_toggles() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    // Toggle false
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], false);

    // Toggle back to true
    cmd_general_console(&cfg).unwrap();
    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["general"]["enable_console"], true);
}

#[test]
fn test_cmd_all_status_no_config() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_all_status(&cfg, &workspace).unwrap();
}

#[test]
fn test_cmd_general_status_no_general_section() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": {}});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();

    cmd_general_status(&cfg).unwrap();
}

#[test]
fn test_read_logging_config_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    std::fs::write(&cfg, "invalid json {{{").unwrap();

    let result = read_logging_config(&cfg);
    assert!(result.is_err());
}

#[test]
fn test_cmd_llm_config_both_options() {
    let tmp = TempDir::new().unwrap();
    let cfg = tmp.path().join("config.json");
    let config = serde_json::json!({"logging": default_logging_config()});
    std::fs::write(&cfg, serde_json::to_string(&config).unwrap()).unwrap();
    let workspace = tmp.path().join("workspace");
    std::fs::create_dir_all(&workspace).unwrap();

    cmd_llm_config(&cfg, &workspace, Some("truncated"), Some("custom-logs")).unwrap();

    let data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg).unwrap()).unwrap();
    assert_eq!(data["logging"]["llm"]["detail_level"], "truncated");
    assert!(data["logging"]["llm"]["log_dir"].as_str().unwrap().contains("custom-logs"));
}
