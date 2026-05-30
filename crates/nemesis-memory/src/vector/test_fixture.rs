//! Shared test fixture for ONNX plugin tests.
//!
//! The ONNX plugin DLL has a process-global `PERMANENTLY_FREED: AtomicBool`.
//! Once `plugin_free()` brings INIT_COUNT to 0, PERMANENTLY_FREED is set true
//! and the plugin can never be re-initialized in the same process.
//!
//! This module creates a **single** plugin instance that lives for the entire
//! test process lifetime. A background thread holds the plugin, and embedding
//! requests are dispatched via mpsc channels. The Sender is stored in a
//! `OnceLock` and is never dropped, so the background thread never exits and
//! PERMANENTLY_FREED never becomes true.
//!
//! Each test calls `shared_embed_func()` to get its own `EmbeddingFunc` (which
//! clones the Sender), creates a `VectorStore` with `new_from_embed()`, and
//! drops it freely without affecting the shared plugin.

use std::path::PathBuf;
use std::sync::OnceLock;

use crate::vector::embedding::EmbeddingFunc;
use crate::vector::plugin_loader::EmbeddingPlugin;
use crate::vector::store::StoreConfig;

// ---------------------------------------------------------------------------
// Shared state
// ---------------------------------------------------------------------------

/// Opaque holder for the channel Sender that talks to the background plugin thread.
struct SharedEmbedState {
    tx: std::sync::mpsc::Sender<(String, std::sync::mpsc::Sender<Result<Vec<f32>, String>>)>,
}

/// Process-global singleton. Initialized once; never dropped.
static SHARED: OnceLock<Result<SharedEmbedState, String>> = OnceLock::new();

// ---------------------------------------------------------------------------
// Public API
// ---------------------------------------------------------------------------

/// Return a new `EmbeddingFunc` backed by the shared ONNX plugin.
///
/// The returned closure can be freely dropped. The underlying plugin is
/// never released.
///
/// Returns `Err` if the plugin DLL or model files are not available, with a
/// human-readable message explaining what's missing.
pub fn shared_embed_func() -> Result<EmbeddingFunc, String> {
    let state = SHARED.get_or_init(init_shared);

    match state {
        Ok(s) => {
            let tx = s.tx.clone();
            Ok(Box::new(move |text: &str| {
                let (reply_tx, reply_rx) = std::sync::mpsc::channel();
                tx.send((text.to_string(), reply_tx))
                    .map_err(|_| "Shared embedding thread has exited".to_string())?;
                reply_rx
                    .recv()
                    .map_err(|_| "Shared embedding thread did not respond".to_string())?
            }))
        }
        Err(e) => Err(e.clone()),
    }
}

/// Build a `StoreConfig` suitable for use with the shared plugin.
///
/// Returns `None` if the plugin DLL or config directory cannot be resolved
/// (i.e. the test environment is not set up).
pub fn plugin_store_config(storage_path: &str) -> Option<StoreConfig> {
    Some(StoreConfig {
        plugin_path: resolve_plugin_dll(),
        config_dir: resolve_config_dir(),
        storage_path: storage_path.to_string(),
        similarity_threshold: 0.1,
        ..Default::default()
    })
}

/// Resolve the plugin DLL path. Returns `None` if not found.
pub fn resolve_plugin_dll() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let manifest = PathBuf::from(&manifest_dir);
    let root = manifest.parent().unwrap().parent().unwrap();

    for profile in &["release", "debug"] {
        let dll = root.join("target").join(profile).join("plugins").join("plugin_onnx.dll");
        if dll.exists() {
            return Some(dll.to_string_lossy().to_string());
        }
    }
    None
}

/// Resolve the config directory containing `config.enhanced_memory.json` and model files.
/// Returns `None` if not found.
pub fn resolve_config_dir() -> Option<String> {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let manifest = PathBuf::from(&manifest_dir);
    let root = manifest.parent().unwrap().parent().unwrap();

    // Preferred: test-data/memory-e2e/ (has model.onnx at root level)
    let alt = root.join("test-data").join("memory-e2e");
    if alt.join("config.enhanced_memory.json").exists() && alt.join("model.onnx").exists() {
        return Some(alt.to_string_lossy().to_string());
    }

    // Fallback: crates/nemesis-memory/config/
    let config_dir = manifest.join("config");
    if config_dir.join("config.enhanced_memory.json").exists() {
        return Some(config_dir.to_string_lossy().to_string());
    }

    None
}

// ---------------------------------------------------------------------------
// Internal initialization
// ---------------------------------------------------------------------------

fn init_shared() -> Result<SharedEmbedState, String> {
    let plugin_path = match resolve_plugin_dll() {
        Some(p) => p,
        None => return Err("plugin_onnx.dll not found. Build: cd plugins/plugin-onnx && cargo build --release".into()),
    };

    let config_dir = match resolve_config_dir() {
        Some(c) => c,
        None => return Err("Config dir with config.enhanced_memory.json not found. Run: bash test-tools/plugin-onnx-test/scripts/setup-test.sh".into()),
    };

    // Load embedding config and resolve model file paths
    let emb_config = crate::vector::embedding_config::load_embedding_config(
        std::path::Path::new(&config_dir),
    );
    let (model_dir, dim) = crate::vector::embedding_config::resolve_model_files(
        &emb_config,
        std::path::Path::new(&config_dir),
    )
    .map_err(|e| format!("Model files not found: {}", e))?;

    // Load plugin DLL
    let mut plugin = crate::vector::plugin_loader::NativePlugin::load(&plugin_path)
        .map_err(|e| format!("Failed to load plugin DLL: {}", e))?;

    // Init plugin
    plugin
        .init(&model_dir, dim)
        .map_err(|e| format!("Failed to init plugin: {}", e))?;

    // Spawn background thread that owns the plugin forever
    let (tx, rx) = std::sync::mpsc::channel::<(String, std::sync::mpsc::Sender<Result<Vec<f32>, String>>)>();

    std::thread::Builder::new()
        .name("onnx-shared-test-embed".into())
        .spawn(move || {
            let plugin = std::sync::Mutex::new(plugin);
            // Process embed requests until channel is closed.
            // Since `tx` is stored in OnceLock and never dropped, this
            // thread runs forever (until process exit).
            while let Ok((text, reply)) = rx.recv() {
                let result = match plugin.lock() {
                    Ok(g) => g.embed(&text).map_err(|e| e.to_string()),
                    Err(e) => Err(e.to_string()),
                };
                let _ = reply.send(result);
            }
            // Thread only exits if all Senders are dropped — which never happens
            // because the OnceLock holds one permanently.
        })
        .map_err(|_| "Failed to spawn shared embedding thread".to_string())?;

    Ok(SharedEmbedState { tx })
}
