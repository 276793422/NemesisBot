//! Platform-aware plugin library path resolution.
//!
//! Provides centralized helpers for locating native plugin libraries (`.dll` /
//! `.so` / `.dylib`) next to the executable, so callers never hardcode a
//! specific file extension.

use std::path::{Path, PathBuf};

/// Return the platform-specific filename for a native plugin library.
///
/// # Examples
///
/// | `base_name`      | Windows            | Linux               | macOS                |
/// |------------------|--------------------|---------------------|----------------------|
/// | `"plugin_ui"`    | `plugin_ui.dll`    | `libplugin_ui.so`   | `libplugin_ui.dylib` |
/// | `"plugin_onnx"`  | `plugin_onnx.dll`  | `libplugin_onnx.so` | `libplugin_onnx.dylib` |
pub fn plugin_library_filename(base_name: &str) -> String {
    if cfg!(target_os = "windows") {
        format!("{}.dll", base_name)
    } else if cfg!(target_os = "macos") {
        format!("lib{}.dylib", base_name)
    } else {
        format!("lib{}.so", base_name)
    }
}

/// Return a human-readable label for the library type on the current platform.
///
/// Used in log/error messages instead of hardcoding "DLL".
pub fn plugin_library_label() -> &'static str {
    if cfg!(target_os = "windows") {
        "DLL"
    } else if cfg!(target_os = "macos") {
        "dylib"
    } else {
        "shared library"
    }
}

/// Find a plugin library next to the current executable.
///
/// Searches `{exe_dir}/plugins/` for the platform-appropriate library name.
/// On Windows, also checks the hyphenated variant (e.g. `plugin-ui.dll` for
/// `plugin_ui`) as a fallback.
///
/// Returns the full path if found, or `None`.
pub fn find_plugin_library(base_name: &str) -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    find_plugin_library_in(&exe_dir, base_name)
}

/// Find a plugin library in a specific directory's `plugins/` subdirectory.
///
/// `exe_dir` should be the directory containing the main executable.
/// On Windows, also checks the hyphenated variant as a fallback.
pub fn find_plugin_library_in(exe_dir: &Path, base_name: &str) -> Option<PathBuf> {
    let plugins_dir = exe_dir.join("plugins");

    let primary = plugins_dir.join(plugin_library_filename(base_name));
    if primary.exists() {
        return Some(primary);
    }

    // Fallback: hyphenated variant (e.g. plugin-ui.dll for plugin_ui)
    let hyphenated = base_name.replace('_', "-");
    if hyphenated != base_name {
        let alt = plugins_dir.join(plugin_library_filename(&hyphenated));
        if alt.exists() {
            return Some(alt);
        }
    }

    None
}

#[cfg(test)]
mod tests;
