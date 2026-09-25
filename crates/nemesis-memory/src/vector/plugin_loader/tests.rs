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
        PluginError::LoadFailed {
            path: "p".into(),
            error: "e".into(),
        },
        PluginError::SymbolNotFound {
            name: "n".into(),
            error: "e".into(),
        },
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
        Self {
            dim,
            initialized: false,
            closed: false,
        }
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
        PluginError::Closed => {}
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
        PluginError::Closed => {}
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
    let err = PluginError::LoadFailed {
        path: "test".into(),
        error: "err".into(),
    };
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
    if let Ok(path) = std::env::var("PLUGIN_ONNX_DLL_PATH")
        && Path::new(&path).exists()
    {
        return Some(path);
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let candidates = [
        format!(
            "{}/../../plugins/plugin-onnx/target/release/plugin_onnx.dll",
            manifest_dir
        ),
        format!(
            "{}/../../../plugins/plugin-onnx/target/release/plugin_onnx.dll",
            manifest_dir
        ),
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
    if let Ok(dir) = std::env::var("PLUGIN_ONNX_TEST_MODEL_DIR")
        && Path::new(&dir).exists()
    {
        return Some(dir);
    }
    let manifest_dir = env!("CARGO_MANIFEST_DIR");
    let candidates = [
        format!("{}/models/all-MiniLM-L6-v2", manifest_dir),
        format!("{}/../../test-data/memory-e2e", manifest_dir),
        format!(
            "{}/../../test-tools/plugin-onnx-test/test-data",
            manifest_dir
        ),
    ];
    for candidate in &candidates {
        let path = std::path::PathBuf::from(candidate);
        if path.join("model.onnx").exists()
            && let Ok(canonical) = path.canonicalize()
        {
            return Some(canonical.to_str().expect("valid path").to_string());
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
    let dll_path = real_dll_path().expect(
        "plugin DLL not found. Build with: cd plugins/plugin-onnx && cargo build --release",
    );
    let model_dir = real_model_dir()
        .expect("model dir not found. Run: bash test-tools/plugin-onnx-test/scripts/setup-test.sh");

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
    assert!(
        (l2_norm - 1.0).abs() < 1e-3,
        "L2 norm should be ~1.0, got {}",
        l2_norm
    );

    // --- Embed: deterministic ---
    let v1b = plugin.embed("hello world").expect("embed failed");
    for (i, (a, b)) in v1.iter().zip(v1b.iter()).enumerate() {
        assert!(
            (a - b).abs() < 1e-6,
            "Mismatch at index {}: {} vs {}",
            i,
            a,
            b
        );
    }

    // --- Embed: semantic similarity ---
    let v_cat = plugin.embed("a cat sitting on a mat").unwrap();
    let v_kitten = plugin.embed("a kitten resting on a rug").unwrap();
    let v_car = plugin.embed("driving a car on the highway").unwrap();
    let sim_cat_kitten = cosine_similarity(&v_cat, &v_kitten);
    let sim_cat_car = cosine_similarity(&v_cat, &v_car);
    assert!(
        sim_cat_kitten > sim_cat_car,
        "cat-kitten ({}) should be > cat-car ({})",
        sim_cat_kitten,
        sim_cat_car
    );

    // --- Embed: different texts produce different vectors ---
    let v_ml = plugin.embed("machine learning algorithms").unwrap();
    let sim_unrelated = cosine_similarity(&v_ml, &v_car);
    assert!(
        sim_unrelated < 0.95,
        "unrelated texts should not be identical (sim={})",
        sim_unrelated
    );

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

// ---- S5 coverage: Debug impls (display-layer, no DLL needed) ----

#[test]
fn debug_impls_render_unloaded_plugin_state() {
    let inner = NativePluginInner {
        library: None,
        dim: 4,
        closed: false,
        host_services: None,
    };
    let dbg = format!("{:?}", inner);
    assert!(dbg.contains("NativePluginInner"), "got: {dbg}");
    assert!(dbg.contains("dim: 4"), "got: {dbg}");
    assert!(dbg.contains("closed: false"), "got: {dbg}");
    assert!(dbg.contains("library: None"), "got: {dbg}");

    let plugin = NativePlugin {
        inner: std::sync::Mutex::new(inner),
    };
    let plugin_dbg = format!("{:?}", plugin);
    assert!(plugin_dbg.contains("NativePlugin"), "got: {plugin_dbg}");
    assert!(
        plugin_dbg.contains("NativePluginInner"),
        "got: {plugin_dbg}"
    );
}

// ---- R1 coverage: Library::new failure on an existing non-library file ----

#[test]
fn load_rejects_existing_non_library_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("not_a_real_plugin.dll");
    std::fs::write(&fake, b"this is not a valid PE/ELF library").unwrap();

    // The file exists (so the early "file not found" arm does not fire) and
    // the OS loader cleanly rejects the invalid image.
    let p = fake.to_string_lossy().to_string();
    let err = match NativePlugin::load(&p) {
        Err(e) => e,
        Ok(_) => panic!("loading a text blob as a DLL must fail"),
    };
    assert!(
        format!("{err:?}").contains("not_a_real_plugin.dll"),
        "error must carry the offending path, got: {err:?}"
    );
}

// ---------------------------------------------------------------------------
// AGT 覆盖率批次（2026-09-24）：NativePlugin::load 对「存在但非动态库」文件
// 的诚实失败（Library::new err → LoadFailed，非 file-not-found）。
// init/embed/dim/close/drop 全需真实 plugin DLL（进程内 dlopen + 符号校验），
// 单测不可达，由 plugin-onnx-test E2E 覆盖。
// ---------------------------------------------------------------------------

#[test]
fn agt_load_existing_non_library_file_fails_gracefully() {
    let dir = tempfile::tempdir().unwrap();
    let p = dir.path().join("not_a_library.txt");
    std::fs::write(&p, "definitely not a shared library").unwrap();

    match NativePlugin::load(p.to_str().unwrap()) {
        Err(PluginError::LoadFailed { path, error }) => {
            assert!(path.contains("not_a_library"), "{path}");
            assert!(
                !error.contains("file not found"),
                "文件存在 → 不是 file-not-found，而是加载失败: {error}"
            );
        }
        _ => panic!("expected LoadFailed for non-library file"),
    }
    // 便捷入口同路径
    assert!(load_plugin(p.to_str().unwrap()).is_err());
}

// ===========================================================================
// Wave4 覆盖批次：真实 C-ABI stub DLL（测试时用工具链 rustc 现场编译
// cdylib，零依赖零网络）打通 NativePlugin 的 dlopen 全链路——此前这些
// 行被标注「单测不可达，由 plugin-onnx-test E2E 覆盖」。rustc 不可用时
// 按 SKIP 约定跳过（eprintln + return），套件保持绿。
// ===========================================================================

const STUB_PLUGIN_SRC: &str = r#"
use std::ffi::CStr;
use std::os::raw::c_char;
static INITED: std::sync::atomic::AtomicI32 = std::sync::atomic::AtomicI32::new(0);

#[no_mangle]
pub unsafe extern "C" fn plugin_init(model_dir: *const c_char, _host: *const core::ffi::c_void) -> i32 {
    if model_dir.is_null() { return 1; }
    let dir = CStr::from_ptr(model_dir).to_string_lossy().into_owned();
    if dir.contains("FAIL") { return 7; }
    INITED.store(1, std::sync::atomic::Ordering::SeqCst);
    0
}

#[no_mangle]
pub unsafe extern "C" fn plugin_embed(text: *const c_char, out: *mut f32, dim: i32) -> i32 {
    if INITED.load(std::sync::atomic::Ordering::SeqCst) == 0 { return 2; }
    if text.is_null() || out.is_null() || dim <= 0 { return 3; }
    let s = CStr::from_ptr(text).to_bytes();
    let base = if s.is_empty() { 0.5f32 } else { s[0] as f32 / 255.0 };
    for i in 0..dim as usize { *out.add(i) = base + i as f32 * 1e-6; }
    0
}

#[no_mangle]
pub unsafe extern "C" fn plugin_free() {
    INITED.store(0, std::sync::atomic::Ordering::SeqCst);
}
"#;

/// 缺 plugin_free 导出的变体（打 SymbolNotFound 臂）。
const STUB_PLUGIN_NO_FREE_SRC: &str = r#"
use std::os::raw::c_char;

#[no_mangle]
pub unsafe extern "C" fn plugin_init(_model_dir: *const c_char, _host: *const core::ffi::c_void) -> i32 {
    0
}

#[no_mangle]
pub unsafe extern "C" fn plugin_embed(_text: *const c_char, _out: *mut f32, _dim: i32) -> i32 {
    0
}
"#;

/// 编译一次，全程共享（Windows 上已加载 DLL 的文件句柄重复 load 没问题）。
pub(crate) fn stub_dlls() -> Option<(std::path::PathBuf, std::path::PathBuf)> {
    static DLLS: std::sync::OnceLock<Option<(std::path::PathBuf, std::path::PathBuf)>> =
        std::sync::OnceLock::new();
    DLLS.get_or_init(|| {
        // 产物要活到进程结束：落到 env::temp_dir() 下带 pid 隔离的固定
        // 子目录，进程退出后由 OS/temp 清理策略回收（不能放在会析构的
        // TempDir 里）。
        let out_dir = std::env::temp_dir().join(format!("nmb-mem-stubdll-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&out_dir);
        std::fs::create_dir_all(&out_dir).ok()?;

        let compile = |name: &str, src: &str| -> Option<std::path::PathBuf> {
            let src_path = out_dir.join(format!("{name}.rs"));
            std::fs::write(&src_path, src).ok()?;
            let dll = out_dir.join(format!("{name}.dll"));
            let mut cmd = std::process::Command::new(
                std::env::var("RUSTC").unwrap_or_else(|_| "rustc".into()),
            );
            cmd.args([
                "--crate-type",
                "cdylib",
                "--edition",
                "2021",
                "-C",
                "debuginfo=0",
            ])
            .arg(&src_path)
            .arg("-o")
            .arg(&dll);
            #[cfg(windows)]
            {
                use std::os::windows::process::CommandExt;
                cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
            }
            let out = cmd.output().ok()?;
            if !out.status.success() {
                eprintln!(
                    "SKIP: stub plugin compile failed: {}",
                    String::from_utf8_lossy(&out.stderr)
                );
                return None;
            }
            Some(dll)
        };

        let full = compile("plugin_stub_full", STUB_PLUGIN_SRC)?;
        let no_free = compile("plugin_stub_nofree", STUB_PLUGIN_NO_FREE_SRC)?;
        Some((full, no_free))
    })
    .clone()
}

/// stub DLL 是同一进程内共享的一份映像（dlopen 引用计数），其 INITED
/// 静态被所有句柄共享——任何 close/free 都会把并发测试的 embed 打回
/// error 2。所有用 stub 的测试必须持这把锁串行执行。
pub(crate) fn stub_lock() -> std::sync::MutexGuard<'static, ()> {
    static LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());
    LOCK.lock().unwrap_or_else(|e| e.into_inner())
}

#[test]
fn stub_dll_loads_and_symbol_verification_passes() {
    let _stub_guard = stub_lock();
    let Some((full, _)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    let plugin = NativePlugin::load(full.to_str().unwrap()).expect("stub DLL must load");
    assert_eq!(EmbeddingPlugin::dim(&plugin), 0, "未 init 时 dim=0");
    // Debug fmt 带已加载库的形态（library: Some(...)）
    let dbg = format!("{plugin:?}");
    assert!(dbg.contains("NativePlugin"), "{dbg}");
}

#[test]
fn stub_plugin_not_initialized_embed_reports_dim_zero() {
    let _stub_guard = stub_lock();
    let Some((full, _)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    let plugin = NativePlugin::load(full.to_str().unwrap()).unwrap();
    let err = EmbeddingPlugin::embed(&plugin, "hello").unwrap_err();
    match err {
        PluginError::NotInitialized { dim } => assert_eq!(dim, 0),
        e => panic!("预期 NotInitialized，实际: {e}"),
    }
}

#[test]
fn stub_plugin_full_lifecycle_init_embed_close() {
    let _stub_guard = stub_lock();
    let Some((full, _)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    let mut plugin = NativePlugin::load(full.to_str().unwrap()).unwrap();
    plugin.set_host_services(std::ptr::null());
    EmbeddingPlugin::init(&mut plugin, "model_dir_stub", 384).expect("stub init must succeed");
    assert_eq!(EmbeddingPlugin::dim(&plugin), 384);

    let v = EmbeddingPlugin::embed(&plugin, "hello").expect("stub embed must succeed");
    assert_eq!(v.len(), 384);
    // stub 契约：首元素 = 'h'(104)/255，其余递增 1e-6——确定性可断言。
    assert!((v[0] - 104.0 / 255.0).abs() < 1e-6, "v[0]={}", v[0]);
    let v2 = EmbeddingPlugin::embed(&plugin, "hello").unwrap();
    assert_eq!(v, v2, "同文本嵌入必须确定");

    EmbeddingPlugin::close(&mut plugin);
    let err = EmbeddingPlugin::embed(&plugin, "after close").unwrap_err();
    assert!(matches!(err, PluginError::Closed), "{err}");
    // close 幂等（第二次 close 直接返回）
    EmbeddingPlugin::close(&mut plugin);
    assert!(matches!(
        EmbeddingPlugin::embed(&plugin, "still closed"),
        Err(PluginError::Closed)
    ));
}

#[test]
fn stub_plugin_init_failure_carries_plugin_code() {
    let _stub_guard = stub_lock();
    let Some((full, _)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    let mut plugin = NativePlugin::load(full.to_str().unwrap()).unwrap();
    // stub 约定：model_dir 含 FAIL → plugin_init 返回 7
    let err = EmbeddingPlugin::init(&mut plugin, "FAIL_dir", 8).unwrap_err();
    assert!(matches!(err, PluginError::InitFailed { code: 7 }), "{err}");
}

#[test]
fn stub_plugin_drop_without_close_is_safe() {
    let _stub_guard = stub_lock();
    let Some((full, _)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    {
        let mut plugin = NativePlugin::load(full.to_str().unwrap()).unwrap();
        EmbeddingPlugin::init(&mut plugin, "model_dir_stub", 16).unwrap();
        // 作用域结束触发 Drop → plugin_free（Drop 的未 close 臂）
    }
    // 同一 DLL 可再次加载（Drop 已释放）
    let again = NativePlugin::load(full.to_str().unwrap()).unwrap();
    assert_eq!(EmbeddingPlugin::dim(&again), 0);
}

#[test]
fn stub_dll_missing_plugin_free_symbol_reports_symbol_not_found() {
    let _stub_guard = stub_lock();
    let Some((_, no_free)) = stub_dlls() else {
        eprintln!("SKIP: rustc unavailable or stub compile failed");
        return;
    };
    match NativePlugin::load(no_free.to_str().unwrap()) {
        Err(PluginError::SymbolNotFound { name, .. }) => {
            assert_eq!(name, "plugin_free");
        }
        other => panic!("预期 SymbolNotFound(plugin_free)，实际: {other:?}"),
    }
}

// ===========================================================================
// Wave5 覆盖批次：Library::new 的非 PE 失败臂、真系统 DLL 的符号缺失臂、
// test_fixture 三个 resolver + shared_embed_func 的无插件 Err 路径。
// 全部确定性：不下载、不构建 plugin_onnx、不碰外设。
// ===========================================================================

/// 存在但不是合法动态库的文件 → Library::new 失败 → LoadFailed（非 not-found）。
#[test]
fn w5_load_rejects_non_library_file() {
    let dir = tempfile::tempdir().unwrap();
    let fake = dir.path().join("junk_plugin.dll");
    std::fs::write(&fake, b"this is definitely not a PE image").unwrap();
    match NativePlugin::load(fake.to_str().unwrap()) {
        Err(PluginError::LoadFailed { path, error }) => {
            assert!(path.ends_with("junk_plugin.dll"), "{path}");
            assert!(
                !error.contains("file not found"),
                "应是加载失败而非缺文件: {error}"
            );
        }
        other => panic!("预期 LoadFailed，实际: {other:?}"),
    }
}

/// 真实可加载的系统 DLL（winmm.dll）缺 plugin_init 符号 → SymbolNotFound
/// （135-142 的第一个符号臂；后两个符号臂需「有 init 缺 embed」的库，无法
/// 无编译伪造，豁免）。
#[cfg(windows)]
#[test]
fn w5_load_system_dll_without_plugin_symbols_reports_symbol_not_found() {
    let winmm = r"C:\Windows\System32\winmm.dll";
    if !std::path::Path::new(winmm).exists() {
        eprintln!("SKIP: winmm.dll 不存在（非典型 Windows）");
        return;
    }
    match NativePlugin::load(winmm) {
        Err(PluginError::SymbolNotFound { name, .. }) => {
            assert_eq!(name, "plugin_init");
        }
        other => panic!("预期 SymbolNotFound(plugin_init)，实际: {other:?}"),
    }
}

// --- test_fixture：resolver 与 shared_embed_func 的无插件确定性路径 ---

/// resolve_plugin_dll / resolve_config_dir / plugin_store_config 三者一致：
/// store_config 恒 Some（plugin_path 字段镜像 resolver 结论），字段形状正确。
#[test]
fn w5_fixture_resolvers_are_consistent() {
    let dll = crate::vector::test_fixture::resolve_plugin_dll();
    let cfg_dir = crate::vector::test_fixture::resolve_config_dir();
    let store = crate::vector::test_fixture::plugin_store_config("nb-w5-storage")
        .expect("plugin_store_config 恒 Some（字段承载可空 resolver 结论）");

    assert_eq!(
        store.plugin_path.as_deref(),
        dll.as_deref(),
        "plugin_path 必须镜像 resolver"
    );
    assert_eq!(store.storage_path, "nb-w5-storage");
    assert!((store.similarity_threshold - 0.1).abs() < 1e-9);
    if let Some(d) = &dll {
        assert!(std::path::Path::new(d).exists(), "DLL 路径必须存在: {d}");
    }
    if let Some(c) = &cfg_dir {
        assert!(
            std::path::Path::new(c)
                .join("config.enhanced_memory.json")
                .exists(),
            "config_dir 必须含配置文件: {c}"
        );
    }
}

/// shared_embed_func 在无插件/无模型环境下的诚实 Err（init_shared 前两个
/// 错误臂 + OnceLock 缓存路径）；若本机恰好齐备则走 Ok，同样接受。
#[test]
fn w5_fixture_shared_embed_func_honest_without_plugin() {
    let dll = crate::vector::test_fixture::resolve_plugin_dll();
    let mut err_text = String::new();
    let mut got_ok = false;
    match crate::vector::test_fixture::shared_embed_func() {
        Err(e) => err_text = e,
        Ok(_) => got_ok = true,
    };
    if dll.is_none() {
        assert!(!got_ok, "无 DLL 必须报错");
        assert!(err_text.contains("plugin_onnx.dll"), "{err_text}");
    } else {
        // 环境齐备：函数必须成功给出可调用 embed（不实际触发推理）。
        assert!(got_ok, "有 DLL 时 shared_embed_func 应成功");
    }
    // OnceLock 缓存：第二次调用拿到同样结论且不重新初始化。
    let again_err = crate::vector::test_fixture::shared_embed_func().is_err();
    assert_eq!(again_err, !got_ok, "缓存结论必须稳定");
}
