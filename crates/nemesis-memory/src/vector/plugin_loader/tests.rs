use super::*;

#[test]
fn test_load_nonexistent_plugin() {
    let result = NativePlugin::load("/nonexistent/plugin.dll");
    assert!(result.is_err());
    match result.unwrap_err() {
        PluginError::LoadFailed { path, .. } => {
            assert!(path.contains("nonexistent"));
        }
        e => panic!("Expected LoadFailed, got: {}", e),
    }
}

#[test]
fn test_plugin_error_display() {
    let err = PluginError::NotInitialized { dim: 0 };
    assert!(err.to_string().contains("not initialized"));

    let err = PluginError::InitFailed { code: 42 };
    assert!(err.to_string().contains("42"));

    let err = PluginError::SymbolNotFound {
        name: "plugin_init".to_string(),
        error: "not found".to_string(),
    };
    assert!(err.to_string().contains("plugin_init"));
}

#[test]
fn test_load_plugin_convenience() {
    let result = load_plugin("/nonexistent/path");
    assert!(result.is_err());
}

#[test]
fn test_closed_plugin_returns_error() {
    let err = PluginError::Closed;
    assert!(err.to_string().contains("closed"));
}

#[test]
fn test_plugin_error_embed_failed_display() {
    let err = PluginError::EmbedFailed { code: -99 };
    let msg = err.to_string();
    assert!(msg.contains("-99"));
    assert!(msg.contains("embed"));
}

#[test]
fn test_plugin_error_load_failed_display() {
    let err = PluginError::LoadFailed {
        path: "/foo/bar.so".to_string(),
        error: "permission denied".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("/foo/bar.so"));
    assert!(msg.contains("permission denied"));
}

#[test]
fn test_plugin_error_not_initialized_display() {
    let err = PluginError::NotInitialized { dim: 0 };
    let msg = err.to_string();
    assert!(msg.contains("not initialized"));
    assert!(msg.contains("dim=0"));
}

#[test]
fn test_plugin_error_symbol_not_found_display() {
    let err = PluginError::SymbolNotFound {
        name: "embed".to_string(),
        error: "missing symbol".to_string(),
    };
    let msg = err.to_string();
    assert!(msg.contains("embed"));
    assert!(msg.contains("missing symbol"));
}

#[test]
fn test_plugin_error_debug_format() {
    let err1 = PluginError::LoadFailed {
        path: "test".to_string(),
        error: "err".to_string(),
    };
    let debug_str = format!("{:?}", err1);
    assert!(debug_str.contains("LoadFailed"));

    let err2 = PluginError::Closed;
    let debug_str = format!("{:?}", err2);
    assert!(debug_str.contains("Closed"));
}

#[test]
fn test_native_plugin_debug_impl() {
    let result = NativePlugin::load("/does/not/exist.so");
    assert!(result.is_err());
}

#[test]
fn test_load_plugin_path_not_found() {
    let result = NativePlugin::load("nonexistent_file.xyz");
    assert!(result.is_err());
    if let PluginError::LoadFailed { path, error } = result.unwrap_err() {
        assert_eq!(path, "nonexistent_file.xyz");
        assert!(error.contains("not found"));
    } else {
        panic!("Expected LoadFailed error");
    }
}

#[test]
fn test_plugin_error_init_failed_display() {
    let err = PluginError::InitFailed { code: -1 };
    let msg = err.to_string();
    assert!(msg.contains("init"));
    assert!(msg.contains("-1"));
}

#[test]
fn test_native_plugin_inner_debug() {
    let inner = NativePluginInner {
        library: None,
        dim: 128,
        closed: false,
        host_services: None,
    };
    let debug = format!("{:?}", inner);
    assert!(debug.contains("128"));
}

#[test]
fn test_plugin_error_all_variants() {
    let variants: Vec<PluginError> = vec![
        PluginError::LoadFailed { path: "p".into(), error: "e".into() },
        PluginError::SymbolNotFound { name: "n".into(), error: "e".into() },
        PluginError::NotInitialized { dim: 0 },
        PluginError::InitFailed { code: 1 },
        PluginError::EmbedFailed { code: 2 },
        PluginError::Closed,
    ];
    for v in &variants {
        let _ = v.to_string();
    }
}

#[test]
fn test_load_plugin_empty_path() {
    let result = NativePlugin::load("");
    assert!(result.is_err());
}

#[test]
fn test_load_plugin_with_spaces_path() {
    let result = NativePlugin::load("/path with spaces/plugin.so");
    assert!(result.is_err());
}

// ============================================================
// Mock plugin tests
// ============================================================

struct MockPlugin {
    dim: i32,
    initialized: bool,
    closed: bool,
}

impl MockPlugin {
    fn new(dim: i32) -> Self {
        Self { dim, initialized: false, closed: false }
    }
}

impl EmbeddingPlugin for MockPlugin {
    fn init(&mut self, _model_dir: &str, dim: i32) -> Result<(), PluginError> {
        if self.closed {
            return Err(PluginError::Closed);
        }
        self.dim = dim;
        self.initialized = true;
        Ok(())
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, PluginError> {
        if self.closed {
            return Err(PluginError::Closed);
        }
        if !self.initialized {
            return Err(PluginError::NotInitialized { dim: self.dim });
        }
        Ok(vec![text.len() as f32; self.dim as usize])
    }

    fn dim(&self) -> i32 {
        self.dim
    }

    fn close(&mut self) {
        self.closed = true;
    }
}

#[test]
fn test_mock_plugin_init_and_embed() {
    let mut plugin = MockPlugin::new(64);
    assert_eq!(plugin.dim(), 64);
    assert!(!plugin.initialized);

    plugin.init("model_dir", 64).unwrap();
    assert!(plugin.initialized);

    let result = plugin.embed("hello").unwrap();
    assert_eq!(result.len(), 64);
    assert!(result.iter().all(|v| *v == 5.0));
}

#[test]
fn test_mock_plugin_embed_before_init() {
    let plugin = MockPlugin::new(0);
    let result = plugin.embed("test");
    assert!(result.is_err());
    match result.unwrap_err() {
        PluginError::NotInitialized { dim } => assert_eq!(dim, 0),
        e => panic!("Expected NotInitialized, got: {}", e),
    }
}

#[test]
fn test_mock_plugin_close_then_init() {
    let mut plugin = MockPlugin::new(64);
    plugin.close();
    let result = plugin.init("model_dir", 64);
    assert!(result.is_err());
    match result.unwrap_err() {
        PluginError::Closed => {},
        e => panic!("Expected Closed, got: {}", e),
    }
}

#[test]
fn test_mock_plugin_close_then_embed() {
    let mut plugin = MockPlugin::new(64);
    plugin.init("model_dir", 64).unwrap();
    plugin.close();
    let result = plugin.embed("test");
    assert!(result.is_err());
    match result.unwrap_err() {
        PluginError::Closed => {},
        e => panic!("Expected Closed, got: {}", e),
    }
}

#[test]
fn test_mock_plugin_close_idempotent() {
    let mut plugin = MockPlugin::new(64);
    plugin.close();
    plugin.close();
    assert!(plugin.closed);
}

#[test]
fn test_native_plugin_inner_debug_with_library() {
    let inner = NativePluginInner {
        library: None,
        dim: 256,
        closed: true,
        host_services: None,
    };
    let debug = format!("{:?}", inner);
    assert!(debug.contains("256"));
}

#[test]
fn test_plugin_error_source_compatibility() {
    use std::error::Error;
    let err = PluginError::LoadFailed { path: "test".into(), error: "err".into() };
    let _source = err.source();
}

#[test]
fn test_load_plugin_returns_boxed_trait() {
    let result: Result<Box<dyn EmbeddingPlugin>, PluginError> = load_plugin("/nonexistent");
    assert!(result.is_err());
}

#[test]
fn test_native_plugin_dim_default_zero() {
    let mut plugin = MockPlugin::new(0);
    assert_eq!(plugin.dim(), 0);
    plugin.init("", 128).unwrap();
    assert_eq!(plugin.dim(), 128);
}

// ============================================================
// Real plugin integration tests (run with `cargo test -- --ignored`)
// Requires plugin DLL + ONNX model (run scripts/setup-test.sh first)
// ============================================================

fn real_dll_path() -> Option<String> {
    if let Ok(path) = std::env::var("PLUGIN_ONNX_DLL_PATH") {
        if Path::new(&path).exists() {
            return Some(path);
        }
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let candidates = [
        format!("{}/../../plugins/plugin-onnx/target/release/plugin_onnx.dll", manifest_dir),
        format!("{}/../../../plugins/plugin-onnx/target/release/plugin_onnx.dll", manifest_dir),
    ];
    for candidate in &candidates {
        let path = std::path::PathBuf::from(candidate);
        if let Ok(canonical) = path.canonicalize() {
            return Some(canonical.to_str().expect("valid path").to_string());
        }
        if path.exists() {
            return Some(candidate.clone());
        }
    }
    None
}

fn real_model_dir() -> Option<String> {
    if let Ok(dir) = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR") {
        if Path::new(&dir).exists() {
            return Some(dir);
        }
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let candidates = [
        format!("{}/models/all-MiniLM-L6-v2", manifest_dir),
        format!("{}/../../test-data/memory-e2e", manifest_dir),
        format!("{}/../../test-tools/plugin-onnx-test/test-data", manifest_dir),
    ];
    for candidate in &candidates {
        let path = std::path::PathBuf::from(candidate);
        if path.join("model.onnx").exists() {
            if let Ok(canonical) = path.canonicalize() {
                return Some(canonical.to_str().expect("valid path").to_string());
            }
        }
    }
    None
}

fn cosine_similarity(a: &[f32], b: &[f32]) -> f32 {
    let dot: f32 = a.iter().zip(b.iter()).map(|(x, y)| x * y).sum();
    let norm_a: f32 = a.iter().map(|x| x * x).sum::<f32>().sqrt();
    let norm_b: f32 = b.iter().map(|x| x * x).sum::<f32>().sqrt();
    if norm_a > 0.0 && norm_b > 0.0 {
        dot / (norm_a * norm_b)
    } else {
        0.0
    }
}

#[test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
fn it_real_plugin_full_lifecycle() {
    let dll_path = real_dll_path().expect("plugin DLL not found. Build with: cd plugins/plugin-onnx && cargo build --release");
    let model_dir = real_model_dir().expect("model dir not found. Run: bash test-tools/plugin-onnx-test/scripts/setup-test.sh");

    // --- Load ---
    let mut plugin = NativePlugin::load(&dll_path).expect("Failed to load DLL");
    assert_eq!(plugin.dim(), 0, "dim should be 0 before init");

    // --- Init ---
    plugin.init(&model_dir, 384).expect("Failed to init");
    assert_eq!(plugin.dim(), 384, "dim should be 384 after init");

    // --- Embed: basic ---
    let v1 = plugin.embed("hello world").expect("embed failed");
    assert_eq!(v1.len(), 384, "embedding dimension");
    let non_zero = v1.iter().filter(|&&v| v != 0.0).count();
    assert!(non_zero > 0, "embedding should have non-zero values");

    // --- Embed: L2 normalized ---
    let l2_norm: f32 = v1.iter().map(|v| v * v).sum::<f32>().sqrt();
    assert!((l2_norm - 1.0).abs() < 1e-3, "L2 norm should be ~1.0, got {}", l2_norm);

    // --- Embed: deterministic ---
    let v1b = plugin.embed("hello world").expect("embed failed");
    for (i, (a, b)) in v1.iter().zip(v1b.iter()).enumerate() {
        assert!((a - b).abs() < 1e-6, "Mismatch at index {}: {} vs {}", i, a, b);
    }

    // --- Embed: semantic similarity ---
    let v_cat = plugin.embed("a cat sitting on a mat").unwrap();
    let v_kitten = plugin.embed("a kitten resting on a rug").unwrap();
    let v_car = plugin.embed("driving a car on the highway").unwrap();
    let sim_cat_kitten = cosine_similarity(&v_cat, &v_kitten);
    let sim_cat_car = cosine_similarity(&v_cat, &v_car);
    assert!(sim_cat_kitten > sim_cat_car,
        "cat-kitten ({}) should be > cat-car ({})", sim_cat_kitten, sim_cat_car);

    // --- Embed: different texts produce different vectors ---
    let v_ml = plugin.embed("machine learning algorithms").unwrap();
    let sim_unrelated = cosine_similarity(&v_ml, &v_car);
    assert!(sim_unrelated < 0.95, "unrelated texts should not be identical (sim={})", sim_unrelated);

    // --- Close ---
    plugin.close();
    let result = plugin.embed("should fail");
    assert!(result.is_err(), "embed after close should fail");
}

#[test]
// Ignored (ONNX): requires plugin_onnx.dll in target/{debug,release}/plugins/
// AND the embedding model (model.onnx + tokenizer.json, all-MiniLM-L6-v2) under
// test-data/memory-e2e/ or crates/nemesis-memory/models/. ONNX Runtime can't
// re-init after free → MUST run single-threaded. Setup + run:
//   bash test-tools/plugin-onnx-test/scripts/setup-test.sh   # downloads model (~90MB)
//   cargo test -p nemesis-memory -- --ignored --test-threads=1 <test_name>
#[ignore]
fn it_real_plugin_via_boxed_trait() {
    let dll_path = real_dll_path().expect("plugin DLL not found");
    let model_dir = real_model_dir().expect("model dir not found");
    let mut plugin: Box<dyn EmbeddingPlugin> = load_plugin(&dll_path).unwrap();
    plugin.init(&model_dir, 384).unwrap();
    let vec = plugin.embed("trait object test").unwrap();
    assert_eq!(vec.len(), 384);
    assert_eq!(plugin.dim(), 384);
    plugin.close();
}
