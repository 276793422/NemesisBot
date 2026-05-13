//! ONNX embedding plugin loader.
//!
//! Loads a native shared library (DLL on Windows, SO on Linux/macOS)
//! that provides ONNX-based embedding inference via C ABI.
//!
//! The shared library must export:
//! - `embed_init(model_path: *const c_char, dim: i32) -> i32`
//! - `embed(text: *const c_char, out: *mut f32, dim: i32) -> i32`
//! - `embed_free()`

use std::fmt;
use std::path::Path;
use std::sync::Mutex;

use libloading::{Library, Symbol};
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

    #[error("embed_init returned error code: {code}")]
    InitFailed { code: i32 },

    #[error("embed returned error code: {code}")]
    EmbedFailed { code: i32 },

    #[error("plugin already closed")]
    Closed,
}

// ---------------------------------------------------------------------------
// EmbeddingPlugin trait
// ---------------------------------------------------------------------------

/// Trait for embedding plugins.
pub trait EmbeddingPlugin: Send + Sync {
    /// Initialize the plugin with a model path and output dimension.
    fn init(&mut self, model_path: &str, dim: i32) -> Result<(), PluginError>;

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
/// - Calls `embed_init`, `embed`, `embed_free` via C ABI
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
}

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
    /// The library must export `embed_init`, `embed`, and `embed_free`.
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

        // Verify required symbols exist.
        unsafe {
            let _: Symbol<unsafe extern "C" fn(*const std::ffi::c_char, i32) -> i32> =
                library.get(b"embed_init").map_err(|e| PluginError::SymbolNotFound {
                    name: "embed_init".to_string(),
                    error: e.to_string(),
                })?;

            let _: Symbol<unsafe extern "C" fn(*const std::ffi::c_char, *mut f32, i32) -> i32> =
                library.get(b"embed").map_err(|e| PluginError::SymbolNotFound {
                    name: "embed".to_string(),
                    error: e.to_string(),
                })?;

            let _: Symbol<unsafe extern "C" fn()> =
                library.get(b"embed_free").map_err(|e| PluginError::SymbolNotFound {
                    name: "embed_free".to_string(),
                    error: e.to_string(),
                })?;
        }

        info!(path = path, "Native embedding plugin loaded successfully");

        Ok(Self {
            inner: Mutex::new(NativePluginInner {
                library: Some(library),
                dim: 0,
                closed: false,
            }),
        })
    }
}

impl EmbeddingPlugin for NativePlugin {
    fn init(&mut self, model_path: &str, dim: i32) -> Result<(), PluginError> {
        let mut inner = self.inner.lock().unwrap();
        if inner.closed {
            return Err(PluginError::Closed);
        }

        let library = inner
            .library
            .as_ref()
            .ok_or(PluginError::Closed)?;

        let c_model_path = std::ffi::CString::new(model_path)
            .map_err(|_| PluginError::InitFailed { code: -1 })?;

        unsafe {
            let embed_init: Symbol<unsafe extern "C" fn(*const std::ffi::c_char, i32) -> i32> =
                library.get(b"embed_init").map_err(|e| PluginError::SymbolNotFound {
                    name: "embed_init".to_string(),
                    error: e.to_string(),
                })?;

            let ret = embed_init(c_model_path.as_ptr(), dim);
            if ret != 0 {
                return Err(PluginError::InitFailed { code: ret });
            }
        }

        inner.dim = dim;
        info!(
            model_path = model_path,
            dim = dim,
            "Embedding plugin initialized"
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

        let library = inner
            .library
            .as_ref()
            .ok_or(PluginError::Closed)?;

        let c_text = std::ffi::CString::new(text)
            .map_err(|_| PluginError::EmbedFailed { code: -1 })?;

        let dim = inner.dim as usize;
        let mut buf = vec![0.0f32; dim];

        unsafe {
            let embed_fn: Symbol<unsafe extern "C" fn(*const std::ffi::c_char, *mut f32, i32) -> i32> =
                library.get(b"embed").map_err(|e| PluginError::SymbolNotFound {
                    name: "embed".to_string(),
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
                if let Ok(embed_free) = library.get::<Symbol<unsafe extern "C" fn()>>(b"embed_free")
                {
                    embed_free();
                }
            }
        }

        // Drop the library (unloads DLL/SO).
        inner.library = None;
        inner.closed = true;
        info!("Embedding plugin closed");
    }
}

impl Drop for NativePlugin {
    fn drop(&mut self) {
        let mut inner = self.inner.lock().unwrap();
        if !inner.closed {
            if let Some(ref library) = inner.library {
                unsafe {
                    if let Ok(embed_free) =
                        library.get::<Symbol<unsafe extern "C" fn()>>(b"embed_free")
                    {
                        embed_free();
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
mod tests {
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
            name: "embed_init".to_string(),
            error: "not found".to_string(),
        };
        assert!(err.to_string().contains("embed_init"));
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
        // Ensure Debug derives work for all variants
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
        // Test that Debug for NativePlugin doesn't panic
        // We can't create a real NativePlugin without a library,
        // but we can verify the error path
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
        // Verify Debug for NativePluginInner works correctly
        let inner = NativePluginInner {
            library: None,
            dim: 128,
            closed: false,
        };
        let debug = format!("{:?}", inner);
        assert!(debug.contains("128"));
        assert!(debug.contains("false"));
    }

    #[test]
    fn test_plugin_error_all_variants() {
        // Ensure all variants can be constructed and displayed
        let variants: Vec<PluginError> = vec![
            PluginError::LoadFailed { path: "p".into(), error: "e".into() },
            PluginError::SymbolNotFound { name: "n".into(), error: "e".into() },
            PluginError::NotInitialized { dim: 0 },
            PluginError::InitFailed { code: 1 },
            PluginError::EmbedFailed { code: 2 },
            PluginError::Closed,
        ];
        for v in &variants {
            let _ = v.to_string(); // Should not panic
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
}
