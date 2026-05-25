use super::*;
use tempfile::TempDir;

// ===========================================================================
// Helper: create a test home directory with config.json
// ===========================================================================

fn setup_home(config: &serde_json::Value) -> (TempDir, std::path::PathBuf) {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(&home).unwrap();
    let cfg_path = home.join("config.json");
    std::fs::write(&cfg_path, serde_json::to_string_pretty(config).unwrap()).unwrap();
    (tmp, home)
}

fn setup_paths(home: &std::path::Path) -> common::Paths {
    common::Paths::from_home(home)
}

// ===========================================================================
// read_main_switch tests
// ===========================================================================

#[test]
fn test_read_main_switch_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    let cfg_path = home.join("config.json");
    // No config file exists
    assert!(!cfg_path.exists());
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_enabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(read_main_switch(&cfg_path), true);
}

#[test]
fn test_read_main_switch_disabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({
        "memory": { "enabled": false }
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_missing_field() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    // config.json exists but no memory section
    let cfg = serde_json::json!({ "agents": {} });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_memory_object_without_enabled() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({ "memory": { "some_other_key": "value" } });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

#[test]
fn test_read_main_switch_invalid_json() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    std::fs::write(&cfg_path, "not valid json{{{").unwrap();
    assert_eq!(read_main_switch(&cfg_path), false);
}

// ===========================================================================
// set_main_switch tests
// ===========================================================================

#[test]
fn test_set_main_switch_enable() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({ "agents": {} });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
    // Other fields should be preserved
    assert!(updated.get("agents").is_some());
}

#[test]
fn test_set_main_switch_disable() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({ "memory": { "enabled": true } });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    set_main_switch(&cfg_path, false).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], false);
}

#[test]
fn test_set_main_switch_creates_memory_object() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({});
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
}

#[test]
fn test_set_main_switch_preserves_existing_memory_fields() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({
        "memory": {
            "enabled": false,
            "some_setting": "preserve_me"
        }
    });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    set_main_switch(&cfg_path, true).unwrap();

    let updated: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(updated["memory"]["enabled"], true);
    assert_eq!(updated["memory"]["some_setting"], "preserve_me");
}

#[test]
fn test_set_main_switch_no_config_file() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("nonexistent").join("config.json");
    let result = set_main_switch(&cfg_path, true);
    assert!(result.is_err());
}

// ===========================================================================
// cmd_status tests
// ===========================================================================

#[test]
fn test_cmd_status_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    // Do NOT create config.json
    let paths = common::Paths::from_home(&home);

    // Should succeed (returns Ok) but print disabled status
    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
}

#[test]
fn test_cmd_status_enabled() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
    // Main switch should be enabled
    assert_eq!(read_main_switch(&home.join("config.json")), true);
}

#[test]
fn test_cmd_status_disabled() {
    let cfg = serde_json::json!({
        "memory": { "enabled": false }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
    assert_eq!(read_main_switch(&home.join("config.json")), false);
}

#[test]
fn test_cmd_status_sub_switch_off() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Create enhanced_memory config with enabled=false
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let em_cfg_path = config_dir.join("config.enhanced_memory.json");
    let em_cfg = serde_json::json!({ "enabled": false });
    std::fs::write(&em_cfg_path, serde_json::to_string(&em_cfg).unwrap()).unwrap();

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
    // Main is on but sub is off
    assert_eq!(read_main_switch(&home.join("config.json")), true);
}

#[test]
fn test_cmd_status_sub_switch_enabled() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Create enhanced_memory config with enabled=true
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let em_cfg_path = config_dir.join("config.enhanced_memory.json");
    let em_cfg = serde_json::json!({ "enabled": true });
    std::fs::write(&em_cfg_path, serde_json::to_string(&em_cfg).unwrap()).unwrap();

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
}

// ===========================================================================
// cmd_disable tests
// ===========================================================================

#[test]
fn test_cmd_disable() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Create enhanced_memory config with enabled=true
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let em_cfg_path = config_dir.join("config.enhanced_memory.json");
    let em_cfg = serde_json::json!({ "enabled": true });
    std::fs::write(&em_cfg_path, serde_json::to_string(&em_cfg).unwrap()).unwrap();

    let result = cmd_disable(&home, &paths);
    assert!(result.is_ok());

    // Verify main switch turned off
    let cfg_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg_data["memory"]["enabled"], false);

    // Verify sub-switch turned off
    let em_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&em_cfg_path).unwrap()).unwrap();
    assert_eq!(em_data["enabled"], false);
}

#[test]
fn test_cmd_disable_no_config() {
    let tmp = TempDir::new().unwrap();
    let home = tmp.path().join(".nemesisbot");
    // No config.json created
    let paths = common::Paths::from_home(&home);

    let result = cmd_disable(&home, &paths);
    assert!(result.is_err());
}

#[test]
fn test_cmd_disable_no_enhanced_memory_config() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Do NOT create config.enhanced_memory.json
    let result = cmd_disable(&home, &paths);
    assert!(result.is_ok());

    // Main switch should be off
    let cfg_data: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(home.join("config.json")).unwrap()).unwrap();
    assert_eq!(cfg_data["memory"]["enabled"], false);
}

// ===========================================================================
// has_onnx_files tests
// ===========================================================================

#[test]
fn test_has_onnx_files_with_file() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.onnx"), "dummy").unwrap();
    assert!(has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_nested() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    let nested = dir.join("subdir");
    std::fs::create_dir_all(&nested).unwrap();
    std::fs::write(nested.join("model.onnx"), "dummy").unwrap();
    assert!(has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_empty_dir() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    assert!(!has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_wrong_extension() {
    let tmp = TempDir::new().unwrap();
    let dir = tmp.path().join("models");
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::write(dir.join("model.bin"), "dummy").unwrap();
    assert!(!has_onnx_files(&dir));
}

#[test]
fn test_has_onnx_files_nonexistent_dir() {
    assert!(!has_onnx_files(std::path::Path::new("/nonexistent/path")));
}

// ===========================================================================
// detect_plugin_path tests (may or may not find plugin)
// ===========================================================================

#[test]
fn test_detect_plugin_path_returns_option() {
    // This function reads from exe directory, which won't have the plugin
    // during tests. Just verify it returns Option<String>.
    let result = detect_plugin_path();
    // We don't assert true/false since it depends on the test environment
    // Just verify it doesn't panic
    let _ = result;
}

// ===========================================================================
// Integration-style: status checks with various config states
// ===========================================================================

#[test]
fn test_status_with_corrupt_enhanced_memory_config() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Create corrupt enhanced_memory config
    let config_dir = paths.config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let em_cfg_path = config_dir.join("config.enhanced_memory.json");
    std::fs::write(&em_cfg_path, "not valid json{{{").unwrap();

    // Should not panic, parse error should be handled
    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
}

#[test]
fn test_status_with_enabled_false_in_enhanced_memory() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    let config_dir = paths.config_dir();
    std::fs::create_dir_all(&config_dir).unwrap();
    let em_cfg_path = config_dir.join("config.enhanced_memory.json");
    let em_cfg = serde_json::json!({ "enabled": false });
    std::fs::write(&em_cfg_path, serde_json::to_string(&em_cfg).unwrap()).unwrap();

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
}

#[test]
fn test_status_with_model_files() {
    let cfg = serde_json::json!({
        "memory": { "enabled": true }
    });
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    // Create model files
    let config_dir = paths.config_dir();
    let model_dir = config_dir.join("models");
    std::fs::create_dir_all(&model_dir).unwrap();
    std::fs::write(model_dir.join("model.onnx"), "dummy onnx data").unwrap();

    let result = cmd_status(&home, &paths);
    assert!(result.is_ok());
}

// ===========================================================================
// set_main_switch edge cases
// ===========================================================================

#[test]
fn test_set_main_switch_toggle_multiple_times() {
    let tmp = TempDir::new().unwrap();
    let cfg_path = tmp.path().join("config.json");
    let cfg = serde_json::json!({ "memory": { "enabled": false } });
    std::fs::write(&cfg_path, serde_json::to_string(&cfg).unwrap()).unwrap();

    set_main_switch(&cfg_path, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], true);

    set_main_switch(&cfg_path, false).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], false);

    set_main_switch(&cfg_path, true).unwrap();
    let v: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(&cfg_path).unwrap()).unwrap();
    assert_eq!(v["memory"]["enabled"], true);
}

// ===========================================================================
// Paths integration with memory config
// ===========================================================================

#[test]
fn test_paths_enhanced_memory_config_location() {
    let cfg = serde_json::json!({});
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    let em_path = paths.enhanced_memory_config();
    assert!(em_path.to_string_lossy().contains("config"));
    assert!(em_path.to_string_lossy().contains("config.enhanced_memory.json"));
}

#[test]
fn test_paths_config_dir_from_home() {
    let cfg = serde_json::json!({});
    let (_tmp, home) = setup_home(&cfg);
    let paths = setup_paths(&home);

    let config_dir = paths.config_dir();
    assert!(config_dir.to_string_lossy().contains("config"));
}
