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

// ===========================================================================
// Wave4 覆盖批次：new_embedding_func 端到端（stub DLL + 哑 model.onnx）——
// 打通 try_load_plugin 的线程迁移 + 通道往返 + 关闭句柄全链路（此前被
// 标注需真实 ONNX 插件才能覆盖）。rustc 不可用时按 SKIP 约定跳过。
// ===========================================================================

#[test]
fn new_embedding_func_end_to_end_with_stub_plugin_serves_embeddings() {
    let _stub_guard = crate::vector::plugin_loader::tests::stub_lock();
    let Some((full, _)) = crate::vector::plugin_loader::tests::stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    let config_dir = tempfile::tempdir().unwrap();
    // resolve_model_files 的 config_dir/model.onnx 存在臂——哑文件即可
    // （stub 插件不读模型内容）。
    std::fs::write(config_dir.path().join("model.onnx"), b"stub onnx").unwrap();

    let cfg = VectorConfig {
        embedding_tier: "plugin".into(),
        plugin_path: Some(full.to_string_lossy().into_owned()),
        config_dir: Some(config_dir.path().to_string_lossy().into_owned()),
        host_services: None,
    };

    let f = match new_embedding_func(&cfg) {
        Ok(f) => f,
        Err(e) => {
            eprintln!("SKIP: embedding func unavailable: {e}");
            return;
        }
    };

    // 经后台线程通道往返：stub 契约 = 首元素 首字节/255，维度 = medium 档 384。
    let v = f("hello").expect("embed through channel must work");
    assert_eq!(v.len(), 384);
    assert!((v[0] - 104.0 / 255.0).abs() < 1e-6, "v[0]={}", v[0]);

    // 不同文本 → 不同向量（stub 按首字节填充）。
    let w = f("xyz").unwrap();
    assert!((w[0] - 120.0 / 255.0).abs() < 1e-6, "w[0]={}", w[0]);

    // 并发多次调用保持稳定（通道串行化）。
    for i in 0..4 {
        let r = f("hello").unwrap();
        assert_eq!(r, v, "iter {i} 偏离");
    }
}
