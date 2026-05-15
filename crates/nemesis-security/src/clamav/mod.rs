//! ClamAV integration modules.
//!
//! Provides ClamAV client, daemon management, scanner, updater, config
//! generation, and security pipeline hook.

pub mod client;
pub mod config;
pub mod daemon;
pub mod manager;
pub mod scanner;
pub mod updater;
pub mod hook;

use std::path::Path;

/// Find an executable in the ClamAV installation directory.
pub(crate) fn find_executable(clamav_path: &str, name: &str) -> String {
    let exe_name = if cfg!(target_os = "windows") {
        format!("{}.exe", name)
    } else {
        name.to_string()
    };
    Path::new(clamav_path)
        .join(&exe_name)
        .to_string_lossy()
        .to_string()
}
