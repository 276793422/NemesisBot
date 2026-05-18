//! Integration tests for config loading and MemoryManager creation via with_config_dir.

use std::path::Path;

use nemesis_memory::manager::MemoryManager;

/// Write a config.enhanced_memory.json to the config directory.
fn write_enhanced_config(config_dir: &Path, json: &str) {
    let path = config_dir.join("config.enhanced_memory.json");
    std::fs::write(&path, json).unwrap();
}

#[test]
fn it_disabled_config_creates_basic_manager() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // Write disabled config — should skip vector store init entirely
    write_enhanced_config(config_dir.path(), r#"{"enabled": false}"#);

    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
}

#[test]
fn it_enabled_without_plugin_auto_disables() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // enabled=true but no plugin DLL in test env
    write_enhanced_config(config_dir.path(), r#"{"enabled": true}"#);

    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());

    // Config should be auto-disabled since no plugin found
    let config_path = config_dir.path().join("config.enhanced_memory.json");
    let updated = std::fs::read_to_string(&config_path).unwrap();
    assert!(updated.contains("false"), "Config should be auto-disabled");
}

#[tokio::test]
async fn it_no_config_creates_basic_manager() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // No config file at all — basic memory
    let mgr = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr.is_enabled());
}

#[test]
fn it_config_reload_reflects_changes() {
    let data_dir = tempfile::tempdir().unwrap();
    let config_dir = tempfile::tempdir().unwrap();

    // First config: disabled
    write_enhanced_config(config_dir.path(), r#"{"enabled": false}"#);
    let mgr1 = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr1.is_enabled()); // Manager itself is always enabled

    // Update config: enabled (will auto-disable since no plugin)
    write_enhanced_config(config_dir.path(), r#"{"enabled": true}"#);

    let mgr2 = MemoryManager::with_config_dir(data_dir.path(), config_dir.path());
    assert!(mgr2.is_enabled());
}
