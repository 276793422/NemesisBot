//! W2c batch tests (Phase 3 / quality-hardening goal).
//!
//! Targets vector/embedding.rs try_load_plugin error staging not covered by
//! tests.rs (which only covers the empty / nonexistent plugin-path guards):
//! - stage 1: plugin file exists but model files missing → InitFailed(-6)
//! - stage 2: model files present but the plugin file is not a loadable
//!   library → LoadFailed (a real Library::new rejection, no ONNX runtime)

use super::*;

#[test]
fn new_embedding_func_existing_plugin_no_model_init_failed() {
    // Stage 1 of try_load_plugin: resolve_model_files runs BEFORE the DLL
    // load. A config dir without any installed model → InitFailed { -6 }.
    let dir = tempfile::tempdir().unwrap();
    let fake_plugin = dir.path().join("plugin_onnx.dll");
    std::fs::write(&fake_plugin, "not a real dll").unwrap();

    let config_dir = tempfile::tempdir().unwrap(); // empty — no model.onnx
    let cfg = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some(fake_plugin.to_string_lossy().to_string()),
        config_dir: Some(config_dir.path().to_string_lossy().to_string()),
        host_services: None,
    };

    let err = match new_embedding_func(&cfg) {
        Ok(_) => panic!("expected model-missing error"),
        Err(e) => e,
    };
    assert!(
        err.contains("Failed to load ONNX plugin"),
        "expected plugin-load prefix, got: {}",
        err
    );
    assert!(
        err.contains("-6"),
        "expected InitFailed model-missing code, got: {}",
        err
    );
}

#[test]
fn new_embedding_func_existing_plugin_with_model_load_failed() {
    // Stage 2: model.onnx present in the config dir lets resolve succeed,
    // then NativePlugin::load on a non-library file must fail with
    // LoadFailed (the path appears in the error).
    let dir = tempfile::tempdir().unwrap();
    let fake_plugin = dir.path().join("plugin_onnx.dll");
    std::fs::write(&fake_plugin, "definitely not a PE DLL").unwrap();

    let config_dir = tempfile::tempdir().unwrap();
    // Default config (active=medium, name/dimension valid) + model marker.
    std::fs::write(config_dir.path().join("model.onnx"), "model-stub").unwrap();

    let cfg = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some(fake_plugin.to_string_lossy().to_string()),
        config_dir: Some(config_dir.path().to_string_lossy().to_string()),
        host_services: None,
    };

    let err = match new_embedding_func(&cfg) {
        Ok(_) => panic!("expected library-load error"),
        Err(e) => e,
    };
    assert!(
        err.contains("failed to load library"),
        "expected LoadFailed rejection, got: {}",
        err
    );
    assert!(
        err.contains("plugin_onnx.dll"),
        "expected offending path in error, got: {}",
        err
    );
}
