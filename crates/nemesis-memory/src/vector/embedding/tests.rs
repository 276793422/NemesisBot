use super::*;
use crate::types::VectorConfig;

#[test]
fn test_new_embedding_func_no_plugin_returns_error() {
    let config = VectorConfig {
        embedding_tier: "plugin".to_string(),
        plugin_path: None,
        config_dir: None,
        host_services: None,
    };
    let result = new_embedding_func(&config);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(
        err.contains("No plugin path configured"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_new_embedding_func_nonexistent_plugin_returns_error() {
    let config = VectorConfig {
        embedding_tier: "plugin".to_string(),
        plugin_path: Some("/nonexistent/path/plugin_onnx.dll".to_string()),
        config_dir: None,
        host_services: None,
    };
    let result = new_embedding_func(&config);
    assert!(result.is_err());
    let err = result.err().unwrap();
    assert!(
        err.contains("Plugin DLL not found"),
        "unexpected error: {}",
        err
    );
}

#[test]
fn test_try_load_plugin_nonexistent() {
    let config = VectorConfig {
        embedding_tier: "plugin".to_string(),
        plugin_path: Some("/nonexistent/path/plugin_onnx.dll".to_string()),
        config_dir: None,
        host_services: None,
    };
    let result = try_load_plugin("/nonexistent/path/plugin_onnx.dll", &config);
    assert!(result.is_err());
}
