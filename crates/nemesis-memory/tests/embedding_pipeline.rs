//! Integration tests for the embedding pipeline.
//!
//! Tests that new_embedding_func returns errors when plugin is missing.

use nemesis_memory::types::VectorConfig;
use nemesis_memory::vector::new_embedding_func;

#[test]
fn it_no_plugin_returns_error() {
    let config = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: None,
        config_dir: None,
        host_services: None,
    };

    let result = new_embedding_func(&config);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.contains("No plugin path configured"));
}

#[test]
fn it_nonexistent_plugin_returns_error() {
    let config = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some("/nonexistent/plugin.dll".into()),
        config_dir: None,
        host_services: None,
    };

    let result = new_embedding_func(&config);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(err.contains("Plugin DLL not found"));
}
