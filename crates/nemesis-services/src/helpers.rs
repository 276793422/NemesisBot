//! Helpers - Configuration path resolution and bootstrap check.
//!
//! Mirrors the Go `helpers.go` utility functions.

use std::path::PathBuf;

/// Returns the path to the configuration file.
///
/// Checks for local mode first (`.nemesisbot/config.json` in the current directory),
/// then falls back to the user's home directory (`~/.nemesisbot/config.json`).
pub fn get_config_path() -> PathBuf {
    // Check local mode: .nemesisbot in current directory
    let local_path = PathBuf::from(".nemesisbot").join("config.json");
    if local_path.parent().map_or(false, |p| p.exists()) {
        return local_path;
    }

    // Fall back to home directory
    if let Some(home_dir) = dirs_home_dir() {
        return home_dir.join(".nemesisbot").join("config.json");
    }

    // Last resort: use local path
    local_path
}

/// Checks if `BOOTSTRAP.md` exists in the workspace directory.
///
/// If it exists, the heartbeat LLM call should be skipped.
pub fn should_skip_heartbeat_for_bootstrap(workspace: &std::path::Path) -> bool {
    workspace.join("BOOTSTRAP.md").exists()
}

/// Attempt to resolve the user's home directory.
fn dirs_home_dir() -> Option<PathBuf> {
    // Try the standard approach first
    std::env::var("HOME")
        .or_else(|_| std::env::var("USERPROFILE"))
        .ok()
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .or_else(|| {
            // Fallback to dirs crate behavior
            home::home_dir()
        })
}

// Use the `home` crate if available, otherwise fall back to env vars.
mod home {
    use std::path::PathBuf;

    pub fn home_dir() -> Option<PathBuf> {
        std::env::var("HOME")
            .or_else(|_| std::env::var("USERPROFILE"))
            .ok()
            .map(PathBuf::from)
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
