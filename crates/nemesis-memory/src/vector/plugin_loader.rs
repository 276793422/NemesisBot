//! ONNX embedding plugin loader.
//!
//! Loads a native shared library (DLL on Windows, SO on Linux/macOS)
//! that provides ONNX-based embedding inference via C ABI.
//!
//! The shared library must export the unified interface:
//! - `plugin_init(model_dir: *const c_char, host: *const HostServices) -> i32`
//! - `plugin_embed(text: *const c_char, out: *mut f32, dim: i32) -> i32`
//! - `plugin_free()`

use std::ffi::CString;
use std::fmt;
use std::os::raw::c_char;
use std::path::Path;
use std::sync::Mutex;

use libloading::{Library, Symbol};
use nemesis_plugin::HostServices;
use tracing::info;

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Error type for plugin operations.
#[derive(Debug, thiserror::Error)]
pub enum PluginError {
    #[error("failed to load library '{path}': {error}")]
    LoadFailed { path: String, error: String },

    #[error("symbol '{name}' not found in library: {error}")]
    SymbolNotFound { name: String, error: String },

    #[error("plugin not initialized (dim={dim})")]
    NotInitialized { dim: i32 },

    #[error("plugin_init returned error code: {code}")]
    InitFailed { code: i32 },

    #[error("plugin_embed returned error code: {code}")]
    EmbedFailed { code: i32 },

    #[error("plugin already closed")]
    Closed,
}

// ---------------------------------------------------------------------------
// EmbeddingPlugin trait
// ---------------------------------------------------------------------------

/// Trait for embedding plugins.
pub trait EmbeddingPlugin: Send + Sync {
    /// Initialize the plugin with a model directory and output dimension.
    fn init(&mut self, model_dir: &str, dim: i32) -> Result<(), PluginError>;

    /// Compute embedding for the given text.
    fn embed(&self, text: &str) -> Result<Vec<f32>, PluginError>;

    /// Get the configured dimension.
    fn dim(&self) -> i32;

    /// Release resources.
    fn close(&mut self);
}

// ---------------------------------------------------------------------------
// NativePlugin (FFI via libloading)
// ---------------------------------------------------------------------------

/// Native embedding plugin loaded from a shared library.
///
/// Mirrors Go's `nativePlugin` struct:
/// - Loads DLL/SO via `libloading`
/// - Calls plugin_init/plugin_embed/plugin_free via C ABI
/// - Thread-safe via Mutex
pub struct NativePlugin {
    /// Inner state protected by Mutex for thread safety.
    inner: Mutex<NativePluginInner>,
}

struct NativePluginInner {
    /// The loaded library.
    library: Option<Library>,
    /// Configured dimension.
    dim: i32,
    /// Whether the plugin has been closed.
    closed: bool,
    /// Host services pointer (for unified interface).
    host_services: Option<*const HostServices>,
}

// SAFETY: The HostServices pointer is read-only and valid for process lifetime.
unsafe impl Send for NativePluginInner {}
unsafe impl Sync for NativePluginInner {}

impl fmt::Debug for NativePluginInner {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePluginInner")
            .field("dim", &self.dim)
            .field("closed", &self.closed)
            .field("library", &self.library.as_ref().map(|_| "..."))
            .finish()
    }
}

impl fmt::Debug for NativePlugin {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("NativePlugin")
            .field("inner", &self.inner.lock().unwrap())
            .finish()
    }
}

impl NativePlugin {
    /// Load a plugin from the given shared library path.
    ///
    /// Verifies that the library exports the unified interface
    /// (plugin_init/plugin_embed/plugin_free).
    pub fn load(path: &str) -> Result<Self, PluginError> {
        if !Path::new(path).exists() {
            return Err(PluginError::LoadFailed {
                path: path.to_string(),
                error: "file not found".to_string(),
            });
        }

        let library = unsafe {
            Library::new(path).map_err(|e| PluginError::LoadFailed {
                path: path.to_string(),
                error: e.to_string(),
            })?
        };

        // Verify unified interface symbols exist
        unsafe {
            let _: Symbol<unsafe extern "C" fn(*const c_char, *const HostServices) -> i32> =
                library
                    .get(b"plugin_init")
                    .map_err(|e| PluginError::SymbolNotFound {
                        name: "plugin_init".to_string(),
                        error: e.to_string(),
                    })?;
            let _: Symbol<unsafe extern "C" fn(*const c_char, *mut f32, i32) -> i32> = library
                .get(b"plugin_embed")
                .map_err(|e| PluginError::SymbolNotFound {
                    name: "plugin_embed".to_string(),
                    error: e.to_string(),
                })?;
            let _: Symbol<unsafe extern "C" fn()> =
                library
                    .get(b"plugin_free")
                    .map_err(|e| PluginError::SymbolNotFound {
                        name: "plugin_free".to_string(),
                        error: e.to_string(),
                    })?;
        }

        info!(
            path = path,
            "[PluginLoader] Native embedding plugin loaded successfully"
        );

        Ok(Self {
            inner: Mutex::new(NativePluginInner {
                library: Some(library),
                dim: 0,
                closed: false,
                host_services: None,
            }),
        })
    }

    /// Set the host services pointer for the unified interface.
    pub fn set_host_services(&mut self, host: *const HostServices) {
        let mut inner = self.inner.lock().unwrap();
        inner.host_services = Some(host);
    }
}

impl EmbeddingPlugin for NativePlugin {
    fn init(&mut self, model_dir: &str, dim: i32) -> Result<(), PluginError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(PluginError::Closed);
        }

        let library = inner.library.as_ref().ok_or(PluginError::Closed)?;

        // Pass model_dir directly to plugin_init
        let c_model_dir =
            CString::new(model_dir).map_err(|_| PluginError::InitFailed { code: -1 })?;

        let host_ptr = inner.host_services.unwrap_or(std::ptr::null());

        unsafe {
            let plugin_init: Symbol<
                unsafe extern "C" fn(*const c_char, *const HostServices) -> i32,
            > = library
                .get(b"plugin_init")
                .map_err(|e| PluginError::SymbolNotFound {
                    name: "plugin_init".to_string(),
                    error: e.to_string(),
                })?;

            let ret = plugin_init(c_model_dir.as_ptr(), host_ptr);
            if ret != 0 {
                return Err(PluginError::InitFailed { code: ret });
            }
        }

        inner.dim = dim;
        info!(
            model_dir = %model_dir,
            dim = dim,
            "[PluginLoader] Embedding plugin initialized"
        );

        Ok(())
    }

    fn embed(&self, text: &str) -> Result<Vec<f32>, PluginError> {
        let inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(PluginError::Closed);
        }
        if inner.dim <= 0 {
            return Err(PluginError::NotInitialized { dim: inner.dim });
        }

        let library = inner.library.as_ref().ok_or(PluginError::Closed)?;

        let c_text = CString::new(text).map_err(|_| PluginError::EmbedFailed { code: -1 })?;

        let dim = inner.dim as usize;
        let mut buf = vec![0.0f32; dim];

        unsafe {
            let embed_fn: Symbol<unsafe extern "C" fn(*const c_char, *mut f32, i32) -> i32> =
                library
                    .get(b"plugin_embed")
                    .map_err(|e| PluginError::SymbolNotFound {
                        name: "plugin_embed".to_string(),
                        error: e.to_string(),
                    })?;

            let ret = embed_fn(c_text.as_ptr(), buf.as_mut_ptr(), inner.dim);
            if ret != 0 {
                return Err(PluginError::EmbedFailed { code: ret });
            }
        }

        Ok(buf)
    }

    fn dim(&self) -> i32 {
        self.inner.lock().unwrap().dim
    }

    fn close(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return;
        }

        if let Some(ref library) = inner.library {
            unsafe {
                if let Ok(free_fn) = library.get::<Symbol<unsafe extern "C" fn()>>(b"plugin_free") {
                    free_fn();
                }
            }
        }

        inner.library = None;
        inner.closed = true;
        info!("[PluginLoader] Embedding plugin closed");
    }
}

impl Drop for NativePlugin {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.closed {
            if let Some(ref library) = inner.library {
                unsafe {
                    if let Ok(free_fn) =
                        library.get::<Symbol<unsafe extern "C" fn()>>(b"plugin_free")
                    {
                        free_fn();
                    }
                }
            }
            inner.library = None;
            inner.closed = true;
        }
    }
}

// ---------------------------------------------------------------------------
// Convenience function
// ---------------------------------------------------------------------------

/// Load an embedding plugin from the given path.
///
/// Returns a boxed plugin that can be used directly or wrapped in an
/// embedding function.
pub fn load_plugin(path: &str) -> Result<Box<dyn EmbeddingPlugin>, PluginError> {
    Ok(Box::new(NativePlugin::load(path)?))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
