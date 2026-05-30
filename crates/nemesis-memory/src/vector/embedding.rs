//! Embedding function factory — loads ONNX plugin only.
//!
//! If the plugin is not configured or fails to load, returns an error.
//! No silent fallback to local hash or API tier.
//!
//! The ONNX plugin runs on a dedicated background thread so that its
//! blocking operations (including Drop / resource cleanup) never execute
//! inside a tokio async context (which would cause a panic:
//! "Cannot drop a runtime in a context where blocking is not allowed").

use std::path::Path;
use std::sync::Mutex;

use crate::types::VectorConfig;
use crate::vector::embedding_config;
use crate::vector::plugin_loader::{EmbeddingPlugin, NativePlugin};

/// An embedding function that produces a fixed-dimension vector from text.
pub type EmbeddingFunc = Box<dyn Fn(&str) -> Result<Vec<f32>, String> + Send + Sync>;

/// Create an embedding function based on configuration.
///
/// Requires a valid plugin path pointing to an ONNX plugin DLL.
/// Returns `Err` if the plugin is missing or fails to load.
pub fn new_embedding_func(config: &VectorConfig) -> Result<EmbeddingFunc, String> {
    let plugin_path = config.plugin_path.as_deref().unwrap_or("");
    if plugin_path.is_empty() {
        return Err("No plugin path configured. Enhanced memory requires plugin_onnx.dll.".into());
    }
    if !Path::new(plugin_path).exists() {
        return Err(format!("Plugin DLL not found: {}", plugin_path));
    }
    try_load_plugin(plugin_path, config)
        .map_err(|e| format!("Failed to load ONNX plugin: {}. Enhanced memory is unavailable.", e))
}

/// Attempt to load a native embedding plugin and wrap it as an EmbeddingFunc.
///
/// The plugin lives on a dedicated background thread so that its blocking
/// Drop implementation never runs inside a tokio async context.
fn try_load_plugin(
    plugin_path: &str,
    config: &VectorConfig,
) -> Result<EmbeddingFunc, crate::vector::plugin_loader::PluginError> {
    // 1. Load embedding config and resolve model file paths (no download)
    let config_dir = config.config_dir.as_deref().unwrap_or(".");
    let emb_config = embedding_config::load_embedding_config(Path::new(config_dir));

    let (model_dir, dim) = embedding_config::resolve_model_files(&emb_config, Path::new(config_dir))
        .map_err(|_| crate::vector::plugin_loader::PluginError::InitFailed { code: -6 })?;

    // 2. Load plugin DLL
    let mut plugin = NativePlugin::load(plugin_path)?;

    // 3. Set host services
    if let Some(host) = &config.host_services {
        plugin.set_host_services(*host);
    }

    // 4. Init plugin with model directory
    plugin.init(&model_dir, dim)?;

    // 5. Move plugin to a dedicated background thread.
    //    The thread runs for the lifetime of the returned EmbeddingFunc.
    //    When the closure (EmbeddingFunc) is dropped, it sends a shutdown
    //    signal; the thread then drops the plugin outside any async runtime.
    let (tx, rx) = std::sync::mpsc::channel::<(String, std::sync::mpsc::Sender<Result<Vec<f32>, String>>)>();
    let shutdown = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    let shutdown_clone = shutdown.clone();
    std::thread::Builder::new()
        .name("onnx-embed".into())
        .spawn(move || {
            let plugin = Mutex::new(plugin);
            // Process embed requests until channel is closed or shutdown
            while let Ok((text, reply)) = rx.recv() {
                let guard = plugin.lock().map_err(|e| e.to_string());
                let result = match guard {
                    Ok(g) => g.embed(&text).map_err(|e| e.to_string()),
                    Err(e) => Err(e),
                };
                let _ = reply.send(result);
            }
            // Plugin is dropped here, on this dedicated thread — never
            // inside a tokio runtime.
            drop(plugin);
        })
        .map_err(|_e| crate::vector::plugin_loader::PluginError::InitFailed {
            code: -99,
        })?;

    Ok(Box::new(move |text: &str| {
        if shutdown_clone.load(std::sync::atomic::Ordering::Relaxed) {
            return Err("Embedding function has been shut down".into());
        }
        let (reply_tx, reply_rx) = std::sync::mpsc::channel();
        tx.send((text.to_string(), reply_tx))
            .map_err(|_| "Embedding thread has exited".to_string())?;
        reply_rx
            .recv()
            .map_err(|_| "Embedding thread did not respond".to_string())?
    }))
}

#[cfg(test)]
mod tests;
