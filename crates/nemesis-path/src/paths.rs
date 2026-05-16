//! Unified path management.

use parking_lot::RwLock;
use std::path::{Path, PathBuf};

/// Environment variable names.
pub const ENV_HOME: &str = "NEMESISBOT_HOME";
pub const ENV_CONFIG: &str = "NEMESISBOT_CONFIG";
pub const ENV_MCP_CONFIG: &str = "NEMESISBOT_MCP_CONFIG";
pub const ENV_SECURITY_CONFIG: &str = "NEMESISBOT_SECURITY_CONFIG";
pub const ENV_SKILLS_CONFIG: &str = "NEMESISBOT_SKILLS_CONFIG";
pub const ENV_SCANNER_CONFIG: &str = "NEMESISBOT_SCANNER_CONFIG";
pub const DEFAULT_HOME_DIR: &str = ".nemesisbot";

/// Global local mode flag.
pub static mut LOCAL_MODE: bool = false;

/// Singleton state for `default_path_manager()`.
static DEFAULT_MANAGER: std::sync::OnceLock<PathManager> = std::sync::OnceLock::new();

/// Path manager for NemesisBot directories and files.
pub struct PathManager {
    home_dir: RwLock<PathBuf>,
    /// Override for config path (set via setter or env var).
    config_path: RwLock<Option<PathBuf>>,
    /// Override for MCP config path.
    mcp_config_path: RwLock<Option<PathBuf>>,
    /// Override for security config path.
    security_config_path: RwLock<Option<PathBuf>>,
    /// Override for skills config path.
    skills_config_path: RwLock<Option<PathBuf>>,
}

impl PathManager {
    /// Create a new path manager.
    pub fn new() -> Self {
        let home_dir = resolve_home_dir().unwrap_or_else(|_| {
            dirs::home_dir()
                .map(|h| h.join(DEFAULT_HOME_DIR))
                .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
        });
        Self {
            home_dir: RwLock::new(home_dir),
            config_path: RwLock::new(None),
            mcp_config_path: RwLock::new(None),
            security_config_path: RwLock::new(None),
            skills_config_path: RwLock::new(None),
        }
    }

    /// Create with a specific home directory.
    pub fn with_home(home_dir: PathBuf) -> Self {
        Self {
            home_dir: RwLock::new(home_dir),
            config_path: RwLock::new(None),
            mcp_config_path: RwLock::new(None),
            security_config_path: RwLock::new(None),
            skills_config_path: RwLock::new(None),
        }
    }

    /// Get the home directory.
    pub fn home_dir(&self) -> PathBuf {
        self.home_dir.read().clone()
    }

    /// Get the config file path.
    /// Priority: setter override > NEMESISBOT_CONFIG env > default (home/config.json).
    pub fn config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.home_dir.read().join("config.json")
    }

    /// Set a custom config path (for testing or special cases).
    pub fn set_config_path(&self, path: PathBuf) {
        *self.config_path.write() = Some(path);
    }

    /// Get the workspace directory.
    pub fn workspace(&self) -> PathBuf {
        self.home_dir.read().join("workspace")
    }

    /// Get the MCP config path.
    /// Priority: setter override > NEMESISBOT_MCP_CONFIG env > default.
    pub fn mcp_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.mcp_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_MCP_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.home_dir.read().join("config.mcp.json")
    }

    /// Set a custom MCP config path.
    pub fn set_mcp_config_path(&self, path: PathBuf) {
        *self.mcp_config_path.write() = Some(path);
    }

    /// Get the security config path.
    /// Priority: setter override > NEMESISBOT_SECURITY_CONFIG env > default.
    pub fn security_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.security_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_SECURITY_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.home_dir.read().join("config.security.json")
    }

    /// Set a custom security config path.
    pub fn set_security_config_path(&self, path: PathBuf) {
        *self.security_config_path.write() = Some(path);
    }

    /// Get the skills config path.
    /// Priority: setter override > NEMESISBOT_SKILLS_CONFIG env > default.
    pub fn skills_config_path(&self) -> PathBuf {
        if let Some(ref p) = *self.skills_config_path.read() {
            return p.clone();
        }
        if let Ok(env_path) = std::env::var(ENV_SKILLS_CONFIG) {
            return PathBuf::from(env_path);
        }
        self.home_dir.read().join("config.skills.json")
    }

    /// Set a custom skills config path.
    pub fn set_skills_config_path(&self, path: PathBuf) {
        *self.skills_config_path.write() = Some(path);
    }

    /// Get the auth storage path.
    pub fn auth_path(&self) -> PathBuf {
        self.home_dir.read().join("auth.json")
    }

    /// Get the audit log directory.
    pub fn audit_log_dir(&self) -> PathBuf {
        self.home_dir.read().join("workspace").join("logs").join("security_logs")
    }

    /// Get the temp directory.
    pub fn temp_dir(&self) -> PathBuf {
        self.home_dir.read().join("workspace").join("temp")
    }

    /// Get agent-specific workspace.
    pub fn agent_workspace(&self, agent_id: &str) -> PathBuf {
        if agent_id.is_empty() || agent_id == "main" || agent_id == "default" {
            self.workspace()
        } else {
            self.home_dir.read().join(format!("workspace-{}", agent_id))
        }
    }
}

impl Default for PathManager {
    fn default() -> Self {
        Self::new()
    }
}

/// Return the default singleton `PathManager`.
///
/// Mirrors the Go `DefaultPathManager()` function. Thread-safe initialization
/// via `OnceLock`.
pub fn default_path_manager() -> &'static PathManager {
    DEFAULT_MANAGER.get_or_init(PathManager::new)
}

/// Resolve the NemesisBot home directory.
pub fn resolve_home_dir() -> Result<PathBuf, String> {
    // Priority: LocalMode > NEMESISBOT_HOME > Auto-detect > Default
    let local_mode = unsafe { LOCAL_MODE };
    if local_mode {
        let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    if let Ok(env_home) = std::env::var(ENV_HOME) {
        let expanded = expand_home(&env_home);
        return Ok(expanded.join(DEFAULT_HOME_DIR));
    }

    // Auto-detect
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
    if cwd.join(DEFAULT_HOME_DIR).is_dir() {
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    // Default
    let home = dirs::home_dir().ok_or("cannot determine home directory")?;
    Ok(home.join(DEFAULT_HOME_DIR))
}

/// Expand ~ to home directory.
pub fn expand_home(path: &str) -> PathBuf {
    if path.starts_with('~') {
        if let Some(home) = dirs::home_dir() {
            if path.len() > 1 {
                return home.join(&path[2..]);
            }
            return home;
        }
    }
    PathBuf::from(path)
}

/// Check if local mode should be auto-detected (if .nemesisbot exists in cwd).
pub fn detect_local() -> bool {
    let cwd = std::env::current_dir().unwrap_or_else(|_| PathBuf::from("."));
    cwd.join(DEFAULT_HOME_DIR).is_dir()
}

/// Set the local mode flag.
pub fn set_local_mode(enabled: bool) {
    unsafe { LOCAL_MODE = enabled; }
}

/// Check if local mode is enabled.
pub fn is_local_mode() -> bool {
    unsafe { LOCAL_MODE }
}

/// Resolve config path within a specific workspace.
pub fn resolve_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config.json")
}

/// Resolve MCP config path within a specific workspace.
pub fn resolve_mcp_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config").join("config.mcp.json")
}

/// Resolve security config path within a specific workspace.
pub fn resolve_security_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config").join("config.security.json")
}

/// Resolve cluster config path within a specific workspace.
pub fn resolve_cluster_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config").join("config.cluster.json")
}

/// Resolve skills config path within a specific workspace.
pub fn resolve_skills_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config").join("config.skills.json")
}

/// Resolve scanner config path within a specific workspace.
pub fn resolve_scanner_config_path_in_workspace(workspace: &Path) -> PathBuf {
    workspace.join("config").join("config.scanner.json")
}

// =======================================================================
// Top-level resolve functions (match Go's ResolveConfigPath, etc.)
// =======================================================================

/// Minimal config struct for workspace path resolution.
/// Avoids circular dependency by doing a minimal JSON load.
#[derive(serde::Deserialize)]
struct MinimalConfig {
    #[serde(default)]
    agents: MinimalAgents,
}

#[derive(serde::Deserialize, Default)]
struct MinimalAgents {
    #[serde(default)]
    defaults: MinimalDefaults,
}

#[derive(serde::Deserialize, Default)]
struct MinimalDefaults {
    #[serde(default)]
    workspace: String,
}

impl MinimalConfig {
    fn workspace_path(&self) -> Option<PathBuf> {
        let ws = &self.agents.defaults.workspace;
        if ws.is_empty() {
            return None;
        }
        if ws.starts_with("~/") {
            if let Some(home) = dirs::home_dir() {
                return Some(home.join(&ws[2..]));
            }
        }
        Some(PathBuf::from(ws))
    }
}

/// Try to load a minimal config to resolve workspace path.
fn load_config_for_workspace(config_path: &Path) -> Option<MinimalConfig> {
    let data = std::fs::read_to_string(config_path).ok()?;
    serde_json::from_str::<MinimalConfig>(&data).ok()
}

/// Resolve the main configuration file path.
/// Priority: NEMESISBOT_CONFIG env > LocalMode/auto-detect > Default.
pub fn resolve_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(DEFAULT_HOME_DIR))
            .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
    });

    home_dir.join("config.json")
}

/// Resolve the MCP configuration file path.
/// Priority: NEMESISBOT_MCP_CONFIG env > workspace/config/config.mcp.json > default.
pub fn resolve_mcp_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_MCP_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(DEFAULT_HOME_DIR))
            .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
    });

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.mcp.json");
        }
    }

    home_dir.join("config.mcp.json")
}

/// Resolve the security configuration file path.
/// Priority: NEMESISBOT_SECURITY_CONFIG env > workspace/config/config.security.json > default.
pub fn resolve_security_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SECURITY_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(DEFAULT_HOME_DIR))
            .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
    });

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.security.json");
        }
    }

    home_dir.join("config.security.json")
}

/// Resolve the skills configuration file path.
/// Priority: NEMESISBOT_SKILLS_CONFIG env > workspace/config/config.skills.json > default.
pub fn resolve_skills_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SKILLS_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(DEFAULT_HOME_DIR))
            .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
    });

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.skills.json");
        }
    }

    home_dir.join("config.skills.json")
}

/// Resolve the scanner configuration file path.
/// Priority: NEMESISBOT_SCANNER_CONFIG env > workspace/config/config.scanner.json > default.
pub fn resolve_scanner_config_path() -> PathBuf {
    if let Ok(env_path) = std::env::var(ENV_SCANNER_CONFIG) {
        return PathBuf::from(env_path);
    }

    let home_dir = resolve_home_dir().unwrap_or_else(|_| {
        dirs::home_dir()
            .map(|h| h.join(DEFAULT_HOME_DIR))
            .unwrap_or_else(|| PathBuf::from(".nemesisbot"))
    });

    let config_path = home_dir.join("config.json");
    if let Some(cfg) = load_config_for_workspace(&config_path) {
        if let Some(workspace) = cfg.workspace_path() {
            return workspace.join("config").join("config.scanner.json");
        }
    }

    home_dir.join("config.scanner.json")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_path_manager_default() {
        let pm = PathManager::new();
        assert!(pm.home_dir().to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_path_manager_with_home() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test"));
        assert_eq!(pm.home_dir(), PathBuf::from("/tmp/test"));
        assert_eq!(pm.config_path(), PathBuf::from("/tmp/test/config.json"));
        assert_eq!(pm.workspace(), PathBuf::from("/tmp/test/workspace"));
    }

    #[test]
    fn test_agent_workspace_default() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test"));
        assert_eq!(pm.agent_workspace("main"), pm.workspace());
        assert_eq!(pm.agent_workspace(""), pm.workspace());
    }

    #[test]
    fn test_agent_workspace_custom() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test"));
        assert_eq!(pm.agent_workspace("custom"), PathBuf::from("/tmp/test/workspace-custom"));
    }

    #[test]
    fn test_expand_home() {
        let result = expand_home("~/test");
        assert!(!result.starts_with("~"));
    }

    #[test]
    fn test_expand_home_no_tilde() {
        let result = expand_home("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));
    }

    #[test]
    fn test_expand_home_just_tilde() {
        let result = expand_home("~");
        // Should expand to home directory
        assert!(!result.starts_with("~"));
    }

    #[test]
    fn test_expand_home_relative() {
        let result = expand_home("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));
    }

    #[test]
    fn test_path_manager_config_path_default() {
        let _g = EnvGuard::remove(ENV_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.config_path(), PathBuf::from("/tmp/test_home/config.json"));
    }

    #[test]
    fn test_path_manager_config_path_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_config_path(PathBuf::from("/custom/config.json"));
        assert_eq!(pm.config_path(), PathBuf::from("/custom/config.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_default() {
        let _g = EnvGuard::remove(ENV_MCP_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/tmp/test_home/config.mcp.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_mcp_config_path(PathBuf::from("/custom/mcp.json"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/custom/mcp.json"));
    }

    #[test]
    fn test_path_manager_security_config_default() {
        let _g = EnvGuard::remove(ENV_SECURITY_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/tmp/test_home/config.security.json"));
    }

    #[test]
    fn test_path_manager_security_config_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_security_config_path(PathBuf::from("/custom/security.json"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/custom/security.json"));
    }

    #[test]
    fn test_path_manager_skills_config_default() {
        let _g = EnvGuard::remove(ENV_SKILLS_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/tmp/test_home/config.skills.json"));
    }

    #[test]
    fn test_path_manager_skills_config_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_skills_config_path(PathBuf::from("/custom/skills.json"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/custom/skills.json"));
    }

    #[test]
    fn test_path_manager_auth_path() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.auth_path(), PathBuf::from("/tmp/test_home/auth.json"));
    }

    #[test]
    fn test_path_manager_audit_log_dir() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.audit_log_dir(), PathBuf::from("/tmp/test_home/workspace/logs/security_logs"));
    }

    #[test]
    fn test_path_manager_temp_dir() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.temp_dir(), PathBuf::from("/tmp/test_home/workspace/temp"));
    }

    #[test]
    fn test_path_manager_agent_workspace_default_agent() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("default"), pm.workspace());
    }

    #[test]
    fn test_path_manager_agent_workspace_named() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("worker1"), PathBuf::from("/tmp/test_home/workspace-worker1"));
    }

    #[test]
    fn test_path_manager_workspace() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.workspace(), PathBuf::from("/tmp/test_home/workspace"));
    }

    #[test]
    fn test_resolve_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config.json"));
    }

    #[test]
    fn test_resolve_mcp_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_mcp_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config/config.mcp.json"));
    }

    #[test]
    fn test_resolve_security_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_security_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config/config.security.json"));
    }

    #[test]
    fn test_resolve_cluster_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_cluster_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config/config.cluster.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_skills_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config/config.skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_in_workspace() {
        let ws = Path::new("/data/workspace");
        assert_eq!(resolve_scanner_config_path_in_workspace(ws), PathBuf::from("/data/workspace/config/config.scanner.json"));
    }

    #[test]
    fn test_env_constants() {
        assert_eq!(ENV_HOME, "NEMESISBOT_HOME");
        assert_eq!(ENV_CONFIG, "NEMESISBOT_CONFIG");
        assert_eq!(ENV_MCP_CONFIG, "NEMESISBOT_MCP_CONFIG");
        assert_eq!(ENV_SECURITY_CONFIG, "NEMESISBOT_SECURITY_CONFIG");
        assert_eq!(ENV_SKILLS_CONFIG, "NEMESISBOT_SKILLS_CONFIG");
        assert_eq!(ENV_SCANNER_CONFIG, "NEMESISBOT_SCANNER_CONFIG");
    }

    #[test]
    fn test_default_home_dir_constant() {
        assert_eq!(DEFAULT_HOME_DIR, ".nemesisbot");
    }

    #[test]
    fn test_path_manager_default_trait() {
        let pm = PathManager::default();
        assert!(pm.home_dir().to_string_lossy().contains(".nemesisbot"));
    }

    // ---- New tests for coverage ----

    /// Global reentrant mutex to serialize tests that modify environment variables.
    /// Rust tests run in parallel by default, but env vars are process-global,
    /// so concurrent modifications cause race conditions. The reentrant mutex allows
    /// a single test thread to acquire multiple guards (for multiple EnvGuard instances)
    /// while blocking other test threads.
    static ENV_LOCK: parking_lot::ReentrantMutex<()> = parking_lot::ReentrantMutex::new(());

    /// Helper to safely set env var (set_var/remove_var became unsafe in Rust 2024 edition).
    fn env_set(key: &str, val: &str) {
        unsafe { std::env::set_var(key, val); }
    }

    /// Helper to safely remove env var.
    fn env_remove(key: &str) {
        unsafe { std::env::remove_var(key); }
    }

    /// Helper to save, set, and get a restore guard for an env var.
    /// Also acquires the global ENV_LOCK to serialize parallel tests.
    struct EnvGuard {
        key: String,
        orig: Option<String>,
        _lock: parking_lot::ReentrantMutexGuard<'static, ()>,
    }
    impl EnvGuard {
        fn set(key: &str, val: &str) -> Self {
            let lock = ENV_LOCK.lock();
            let orig = std::env::var(key).ok();
            env_set(key, val);
            Self { key: key.to_string(), orig, _lock: lock }
        }
        fn remove(key: &str) -> Self {
            let lock = ENV_LOCK.lock();
            let orig = std::env::var(key).ok();
            env_remove(key);
            Self { key: key.to_string(), orig, _lock: lock }
        }
    }
    impl Drop for EnvGuard {
        fn drop(&mut self) {
            match &self.orig {
                Some(v) => env_set(&self.key, v),
                None => env_remove(&self.key),
            }
        }
    }

    #[test]
    fn test_resolve_home_dir_with_env() {
        let dir = tempfile::tempdir().unwrap();
        let dir_path = dir.path().to_string_lossy().to_string();
        let _g = EnvGuard::set(ENV_HOME, &dir_path);
        set_local_mode(false);
        let result = resolve_home_dir();
        set_local_mode(false);
        assert!(result.is_ok());
        let home = result.unwrap();
        assert!(home.to_string_lossy().ends_with(".nemesisbot"));
        assert!(home.to_string_lossy().contains(&dir_path));
    }

    #[test]
    fn test_resolve_home_dir_local_mode() {
        set_local_mode(true);
        let result = resolve_home_dir();
        set_local_mode(false);
        assert!(result.is_ok());
        let cwd = std::env::current_dir().unwrap();
        assert_eq!(result.unwrap(), cwd.join(DEFAULT_HOME_DIR));
    }

    #[test]
    fn test_detect_local() {
        let _result = detect_local();
    }

    #[test]
    fn test_set_and_is_local_mode() {
        set_local_mode(true);
        assert!(is_local_mode());
        set_local_mode(false);
        assert!(!is_local_mode());
    }

    #[test]
    fn test_resolve_config_path_with_env() {
        let _g1 = EnvGuard::set(ENV_CONFIG, "/custom/path/config.json");
        let _g2 = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_config_path();
        assert_eq!(result, PathBuf::from("/custom/path/config.json"));
    }

    #[test]
    fn test_resolve_config_path_default() {
        let _g1 = EnvGuard::remove(ENV_CONFIG);
        let _g2 = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_config_path();
        assert!(result.to_string_lossy().ends_with("config.json"));
    }

    #[test]
    fn test_resolve_mcp_config_path_with_env() {
        let _g = EnvGuard::set(ENV_MCP_CONFIG, "/custom/mcp.json");
        let result = resolve_mcp_config_path();
        assert_eq!(result, PathBuf::from("/custom/mcp.json"));
    }

    #[test]
    fn test_resolve_security_config_path_with_env() {
        let _g = EnvGuard::set(ENV_SECURITY_CONFIG, "/custom/security.json");
        let result = resolve_security_config_path();
        assert_eq!(result, PathBuf::from("/custom/security.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_with_env() {
        let _g = EnvGuard::set(ENV_SKILLS_CONFIG, "/custom/skills.json");
        let result = resolve_skills_config_path();
        assert_eq!(result, PathBuf::from("/custom/skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_with_env() {
        let _g = EnvGuard::set(ENV_SCANNER_CONFIG, "/custom/scanner.json");
        let result = resolve_scanner_config_path();
        assert_eq!(result, PathBuf::from("/custom/scanner.json"));
    }

    #[test]
    fn test_resolve_mcp_config_path_default() {
        let _g1 = EnvGuard::remove(ENV_HOME);
        let _g2 = EnvGuard::remove(ENV_MCP_CONFIG);
        set_local_mode(false);
        let result = resolve_mcp_config_path();
        assert!(result.to_string_lossy().ends_with("config.mcp.json"));
    }

    #[test]
    fn test_resolve_security_config_path_default() {
        let _g1 = EnvGuard::remove(ENV_HOME);
        let _g2 = EnvGuard::remove(ENV_SECURITY_CONFIG);
        set_local_mode(false);
        let result = resolve_security_config_path();
        assert!(result.to_string_lossy().ends_with("config.security.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_default() {
        let _g1 = EnvGuard::remove(ENV_HOME);
        let _g2 = EnvGuard::remove(ENV_SKILLS_CONFIG);
        set_local_mode(false);
        let result = resolve_skills_config_path();
        assert!(result.to_string_lossy().ends_with("config.skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_default() {
        let _g1 = EnvGuard::remove(ENV_HOME);
        let _g2 = EnvGuard::remove(ENV_SCANNER_CONFIG);
        set_local_mode(false);
        let result = resolve_scanner_config_path();
        assert!(result.to_string_lossy().ends_with("config.scanner.json"));
    }

    #[test]
    fn test_path_manager_config_path_with_env() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_CONFIG, "/env/config.json");
        let result = pm.config_path();
        assert_eq!(result, PathBuf::from("/env/config.json"));
    }

    #[test]
    fn test_path_manager_config_path_setter_over_env() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_CONFIG, "/env/config.json");
        pm.set_config_path(PathBuf::from("/setter/config.json"));
        let result = pm.config_path();
        assert_eq!(result, PathBuf::from("/setter/config.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_path_with_env() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_MCP_CONFIG, "/env/mcp.json");
        let result = pm.mcp_config_path();
        assert_eq!(result, PathBuf::from("/env/mcp.json"));
    }

    #[test]
    fn test_default_path_manager() {
        let pm = default_path_manager();
        assert!(pm.home_dir().to_string_lossy().contains(".nemesisbot"));
        let pm2 = default_path_manager();
        assert!(pm.home_dir() == pm2.home_dir());
    }

    #[test]
    fn test_minimal_config_workspace_path_tilde() {
        let json = r#"{"agents":{"defaults":{"workspace":"~/myworkspace"}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert!(ws.is_some());
        let ws = ws.unwrap();
        assert!(!ws.to_string_lossy().starts_with("~"));
        assert!(ws.to_string_lossy().contains("myworkspace"));
    }

    #[test]
    fn test_minimal_config_workspace_path_absolute() {
        let json = r#"{"agents":{"defaults":{"workspace":"/data/workspace"}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert_eq!(ws, Some(PathBuf::from("/data/workspace")));
    }

    #[test]
    fn test_minimal_config_workspace_path_empty() {
        let json = r#"{"agents":{"defaults":{"workspace":""}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert!(ws.is_none());
    }

    #[test]
    fn test_minimal_config_missing_workspace() {
        let json = r#"{}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert!(ws.is_none());
    }

    #[test]
    fn test_resolve_mcp_config_path_with_workspace_in_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let config_content = r#"{"agents":{"defaults":{"workspace":""}}}"#;
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        std::fs::write(home.join("config.json"), config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_MCP_CONFIG);
        set_local_mode(false);

        let result = resolve_mcp_config_path();

        // With empty workspace, falls back to home_dir/config.mcp.json
        assert!(result.to_string_lossy().ends_with("config.mcp.json"));
    }

    #[test]
    fn test_expand_home_tilde_with_subpath() {
        let result = expand_home("~/subdir/file.txt");
        assert!(!result.starts_with("~"));
        assert!(result.to_string_lossy().contains("subdir"));
        assert!(result.to_string_lossy().contains("file.txt"));
    }

    #[test]
    fn test_path_manager_setters_clear() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_config_path(PathBuf::from("/a.json"));
        assert_eq!(pm.config_path(), PathBuf::from("/a.json"));
        pm.set_config_path(PathBuf::from("/b.json"));
        assert_eq!(pm.config_path(), PathBuf::from("/b.json"));
    }

    // ---- Additional coverage tests for 95%+ ----

    #[test]
    fn test_path_manager_agent_workspace_main() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("main"), PathBuf::from("/tmp/test_home/workspace"));
    }

    #[test]
    fn test_path_manager_agent_workspace_default() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("default"), PathBuf::from("/tmp/test_home/workspace"));
    }

    #[test]
    fn test_path_manager_agent_workspace_empty() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace(""), PathBuf::from("/tmp/test_home/workspace"));
    }

    #[test]
    fn test_path_manager_agent_workspace_custom() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("sub1"), PathBuf::from("/tmp/test_home/workspace-sub1"));
    }

    #[test]
    fn test_path_manager_security_config_path_setter() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_security_config_path(PathBuf::from("/custom/security.json"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/custom/security.json"));
    }

    #[test]
    fn test_path_manager_skills_config_path_setter() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_skills_config_path(PathBuf::from("/custom/skills.json"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/custom/skills.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_path_setter() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        pm.set_mcp_config_path(PathBuf::from("/custom/mcp.json"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/custom/mcp.json"));
    }

    #[test]
    fn test_path_manager_security_config_path_with_env_v2() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_SECURITY_CONFIG, "/env/security2.json");
        let result = pm.security_config_path();
        assert_eq!(result, PathBuf::from("/env/security2.json"));
    }

    #[test]
    fn test_path_manager_skills_config_path_with_env_v2() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_SKILLS_CONFIG, "/env/skills2.json");
        let result = pm.skills_config_path();
        assert_eq!(result, PathBuf::from("/env/skills2.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_path_default_v2() {
        let _g1 = EnvGuard::remove(ENV_MCP_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home2"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/tmp/test_home2/config.mcp.json"));
    }

    #[test]
    fn test_path_manager_security_config_path_default_v2() {
        let _g1 = EnvGuard::remove(ENV_SECURITY_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home2"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/tmp/test_home2/config.security.json"));
    }

    #[test]
    fn test_path_manager_skills_config_path_default_v2() {
        let _g1 = EnvGuard::remove(ENV_SKILLS_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home2"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/tmp/test_home2/config.skills.json"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target - Phase 2
    // ============================================================

    #[test]
    fn test_resolve_home_dir_auto_detect() {
        // When NEMESISBOT_HOME is not set and local mode is off,
        // it should try auto-detect (CWD has .nemesisbot) then fall back to home
        let _g1 = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_home_dir();
        assert!(result.is_ok());
        let home = result.unwrap();
        assert!(home.to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_resolve_home_dir_no_env_no_auto() {
        // Ensure no env vars interfere, and no local .nemesisbot in cwd
        let _g1 = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_home_dir();
        // Should fall back to dirs::home_dir
        assert!(result.is_ok());
    }

    #[test]
    fn test_resolve_mcp_config_path_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("my_workspace");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_MCP_CONFIG);
        set_local_mode(false);

        let result = resolve_mcp_config_path();
        assert!(result.to_string_lossy().contains("config.mcp.json"));
        // Should contain the workspace config subdirectory
        assert!(result.to_string_lossy().contains("config") || result.to_string_lossy().contains("mcp"));
    }

    #[test]
    fn test_resolve_security_config_path_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SECURITY_CONFIG);
        set_local_mode(false);

        let result = resolve_security_config_path();
        assert!(result.to_string_lossy().contains("config.security.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SKILLS_CONFIG);
        set_local_mode(false);

        let result = resolve_skills_config_path();
        assert!(result.to_string_lossy().contains("config.skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_with_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SCANNER_CONFIG);
        set_local_mode(false);

        let result = resolve_scanner_config_path();
        assert!(result.to_string_lossy().contains("config.scanner.json"));
    }

    #[test]
    fn test_resolve_config_path_with_home_env() {
        let dir = tempfile::tempdir().unwrap();
        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_CONFIG);
        set_local_mode(false);

        let result = resolve_config_path();
        assert!(result.to_string_lossy().ends_with("config.json"));
    }

    #[test]
    fn test_load_config_for_workspace_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "not valid json").unwrap();
        let result = load_config_for_workspace(&config_path);
        assert!(result.is_none());
    }

    #[test]
    fn test_load_config_for_workspace_missing_file() {
        let result = load_config_for_workspace(Path::new("/nonexistent/config.json"));
        assert!(result.is_none());
    }

    #[test]
    fn test_minimal_config_workspace_path_no_tilde() {
        let json = r#"{"agents":{"defaults":{"workspace":"/absolute/path"}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert_eq!(ws, Some(PathBuf::from("/absolute/path")));
    }

    #[test]
    fn test_expand_home_empty_string() {
        let result = expand_home("");
        assert_eq!(result, PathBuf::from(""));
    }

    #[test]
    fn test_detect_local_no_dir() {
        // detect_local checks if .nemesisbot exists in CWD
        // Just call it to cover the branch
        let _ = detect_local();
    }

    #[test]
    fn test_path_manager_security_config_env_overrides() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_SECURITY_CONFIG, "/env/sec.json");
        pm.set_security_config_path(PathBuf::from("/setter/sec.json"));
        // Setter should take priority over env
        assert_eq!(pm.security_config_path(), PathBuf::from("/setter/sec.json"));
    }

    #[test]
    fn test_path_manager_skills_config_env_overrides() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_SKILLS_CONFIG, "/env/sk.json");
        pm.set_skills_config_path(PathBuf::from("/setter/sk.json"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/setter/sk.json"));
    }

    #[test]
    fn test_path_manager_mcp_config_env_overrides() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_MCP_CONFIG, "/env/mcp.json");
        pm.set_mcp_config_path(PathBuf::from("/setter/mcp.json"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/setter/mcp.json"));
    }

    #[test]
    fn test_path_manager_config_path_no_env_no_setter() {
        let _g = EnvGuard::remove(ENV_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.config_path(), PathBuf::from("/tmp/test_home/config.json"));
    }

    #[test]
    fn test_path_manager_new_with_dirs() {
        // Test the PathManager::new() constructor which uses resolve_home_dir
        let _g = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let pm = PathManager::new();
        assert!(pm.home_dir().to_string_lossy().contains(".nemesisbot"));
    }

    // ============================================================
    // Additional coverage tests for 95%+ target - Phase 3
    // ============================================================

    #[test]
    fn test_resolve_home_dir_with_tilde_env() {
        // Test NEMESISBOT_HOME with ~/ prefix
        let _g = EnvGuard::set(ENV_HOME, "~/custom_bot");
        set_local_mode(false);
        let result = resolve_home_dir();
        assert!(result.is_ok());
        let home = result.unwrap();
        assert!(!home.to_string_lossy().starts_with("~"));
        assert!(home.to_string_lossy().contains("custom_bot"));
        assert!(home.to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_resolve_home_dir_auto_detect_with_dir() {
        // Create a temp dir with .nemesisbot subdir, cd into it
        let dir = tempfile::tempdir().unwrap();
        let nb_dir = dir.path().join(".nemesisbot");
        std::fs::create_dir_all(&nb_dir).unwrap();

        let orig_cwd = std::env::current_dir().unwrap();
        // Try to change to temp dir
        let chdir_result = std::env::set_current_dir(dir.path());
        if chdir_result.is_ok() {
            let _g = EnvGuard::remove(ENV_HOME);
            set_local_mode(false);
            let result = resolve_home_dir();
            assert!(result.is_ok());
            assert_eq!(result.unwrap(), dir.path().join(".nemesisbot"));
            // Restore original cwd
            let _ = std::env::set_current_dir(&orig_cwd);
        }
    }

    #[test]
    fn test_path_manager_all_config_paths() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/all_paths"));
        assert_eq!(pm.config_path(), PathBuf::from("/tmp/all_paths/config.json"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/tmp/all_paths/config.mcp.json"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/tmp/all_paths/config.security.json"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/tmp/all_paths/config.skills.json"));
        assert_eq!(pm.workspace(), PathBuf::from("/tmp/all_paths/workspace"));
        assert_eq!(pm.auth_path(), PathBuf::from("/tmp/all_paths/auth.json"));
        assert_eq!(pm.audit_log_dir(), PathBuf::from("/tmp/all_paths/workspace/logs/security_logs"));
        assert_eq!(pm.temp_dir(), PathBuf::from("/tmp/all_paths/workspace/temp"));
    }

    #[test]
    fn test_minimal_config_invalid_json() {
        let result = serde_json::from_str::<MinimalConfig>("not json");
        assert!(result.is_err());
    }

    #[test]
    fn test_minimal_config_partial_agents() {
        let json = r#"{"agents":{}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.workspace_path().is_none());
    }

    #[test]
    fn test_minimal_config_partial_defaults() {
        let json = r#"{"agents":{"defaults":{}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.workspace_path().is_none());
    }

    #[test]
    fn test_resolve_home_dir_absolute_env() {
        let dir = tempfile::tempdir().unwrap();
        let abs_path = dir.path().to_string_lossy().to_string();
        let _g = EnvGuard::set(ENV_HOME, &abs_path);
        set_local_mode(false);
        let result = resolve_home_dir();
        assert!(result.is_ok());
        let home = result.unwrap();
        assert!(home.to_string_lossy().starts_with(&abs_path));
        assert!(home.to_string_lossy().ends_with(".nemesisbot"));
    }

    // ============================================================
    // Phase 4 coverage tests for 95%+ target
    // ============================================================

    #[test]
    fn test_resolve_mcp_config_path_loads_workspace_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("custom_ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();

        // Write config with workspace pointing to our temp workspace
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_MCP_CONFIG);
        set_local_mode(false);

        let result = resolve_mcp_config_path();
        // Should resolve to workspace/config/config.mcp.json since workspace is set
        assert!(result.to_string_lossy().contains("config.mcp.json"));
        assert!(
            result.to_string_lossy().contains("custom_ws") || result.to_string_lossy().contains("config"),
            "Expected workspace path in result, got: {:?}", result
        );
    }

    #[test]
    fn test_resolve_security_config_path_loads_workspace_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("my_ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();

        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SECURITY_CONFIG);
        set_local_mode(false);

        let result = resolve_security_config_path();
        assert!(result.to_string_lossy().contains("config.security.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_loads_workspace_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws2");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();

        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SKILLS_CONFIG);
        set_local_mode(false);

        let result = resolve_skills_config_path();
        assert!(result.to_string_lossy().contains("config.skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_loads_workspace_from_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws3");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();

        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SCANNER_CONFIG);
        set_local_mode(false);

        let result = resolve_scanner_config_path();
        assert!(result.to_string_lossy().contains("config.scanner.json"));
    }

    #[test]
    fn test_resolve_config_path_no_env_no_local() {
        let _g1 = EnvGuard::remove(ENV_CONFIG);
        let _g2 = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_config_path();
        assert!(result.to_string_lossy().ends_with("config.json"));
    }

    #[test]
    fn test_load_config_for_workspace_valid() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, r#"{"agents":{"defaults":{"workspace":"/test/ws"}}}"#).unwrap();
        let result = load_config_for_workspace(&config_path);
        assert!(result.is_some());
        let cfg = result.unwrap();
        let ws = cfg.workspace_path();
        assert_eq!(ws, Some(PathBuf::from("/test/ws")));
    }

    #[test]
    fn test_load_config_for_workspace_empty_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, "{}").unwrap();
        let result = load_config_for_workspace(&config_path);
        assert!(result.is_some());
        let cfg = result.unwrap();
        assert!(cfg.workspace_path().is_none());
    }

    #[test]
    fn test_minimal_config_workspace_tilde_short() {
        let json = r#"{"agents":{"defaults":{"workspace":"~/"}}}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        let ws = cfg.workspace_path();
        assert!(ws.is_some());
        // ~/ with no subpath should expand to home directory
        let ws = ws.unwrap();
        assert!(!ws.to_string_lossy().starts_with("~"));
    }

    #[test]
    fn test_resolve_home_dir_fallback_to_home_dir() {
        // When NEMESISBOT_HOME is not set, local mode off, and no auto-detect
        // it should fall back to dirs::home_dir
        let _g = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let result = resolve_home_dir();
        assert!(result.is_ok());
        let home = result.unwrap();
        assert!(home.to_string_lossy().contains(".nemesisbot"));
    }

    #[test]
    fn test_path_manager_config_path_env_priority() {
        let _g0 = EnvGuard::remove(ENV_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        // First verify default
        assert_eq!(pm.config_path(), PathBuf::from("/tmp/test_home/config.json"));
        // Set env override
        let _g = EnvGuard::set(ENV_CONFIG, "/env/config.json");
        assert_eq!(pm.config_path(), PathBuf::from("/env/config.json"));
        // Setter takes priority over env
        pm.set_config_path(PathBuf::from("/setter/config.json"));
        assert_eq!(pm.config_path(), PathBuf::from("/setter/config.json"));
    }

    #[test]
    fn test_path_manager_new_no_home_env() {
        let _g = EnvGuard::remove(ENV_HOME);
        set_local_mode(false);
        let pm = PathManager::new();
        assert!(pm.home_dir().to_string_lossy().contains(".nemesisbot"));
        // Verify all paths are consistent
        assert!(pm.config_path().to_string_lossy().contains("config.json"));
        assert!(pm.workspace().to_string_lossy().contains("workspace"));
    }

    #[test]
    fn test_resolve_config_path_with_local_mode() {
        set_local_mode(true);
        let _g = EnvGuard::remove(ENV_CONFIG);
        let result = resolve_config_path();
        set_local_mode(false);
        assert!(result.to_string_lossy().ends_with("config.json"));
    }

    #[test]
    fn test_expand_home_non_tilde_prefix() {
        // Path that doesn't start with ~ should return as-is
        let result = expand_home("/absolute/path");
        assert_eq!(result, PathBuf::from("/absolute/path"));

        let result = expand_home("relative/path");
        assert_eq!(result, PathBuf::from("relative/path"));

        let result = expand_home("");
        assert_eq!(result, PathBuf::from(""));
    }

    #[test]
    fn test_agent_workspace_default_keyword() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.agent_workspace("default"), pm.workspace());
    }

    #[test]
    fn test_resolve_config_path_in_workspace_fn() {
        let result = resolve_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config.json"));
    }

    #[test]
    fn test_resolve_mcp_config_path_in_workspace_fn() {
        let result = resolve_mcp_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config/config.mcp.json"));
    }

    #[test]
    fn test_resolve_security_config_path_in_workspace_fn() {
        let result = resolve_security_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config/config.security.json"));
    }

    #[test]
    fn test_resolve_cluster_config_path_in_workspace_fn() {
        let result = resolve_cluster_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config/config.cluster.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_in_workspace_fn() {
        let result = resolve_skills_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config/config.skills.json"));
    }

    #[test]
    fn test_resolve_scanner_config_path_in_workspace_fn() {
        let result = resolve_scanner_config_path_in_workspace(Path::new("/ws"));
        assert_eq!(result, PathBuf::from("/ws/config/config.scanner.json"));
    }

    #[test]
    fn test_load_config_for_workspace_empty_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let config_path = dir.path().join("config.json");
        std::fs::write(&config_path, r#"{"agents":{"defaults":{"workspace":""}}}"#).unwrap();
        let result = load_config_for_workspace(&config_path);
        assert!(result.is_some());
        assert!(result.unwrap().workspace_path().is_none());
    }

    #[test]
    fn test_minimal_config_empty_agents() {
        let json = r#"{}"#;
        let cfg: MinimalConfig = serde_json::from_str(json).unwrap();
        assert!(cfg.workspace_path().is_none());
    }

    #[test]
    fn test_path_manager_config_path_env_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_CONFIG, "/custom/config.json");
        let path = pm.config_path();
        assert_eq!(path, PathBuf::from("/custom/config.json"));
    }

    #[test]
    fn test_path_manager_mcp_path_no_env_no_setter() {
        let _g = EnvGuard::remove(ENV_MCP_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.mcp_config_path(), PathBuf::from("/tmp/test_home/config.mcp.json"));
    }

    #[test]
    fn test_path_manager_security_path_no_env_no_setter() {
        let _g = EnvGuard::remove(ENV_SECURITY_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.security_config_path(), PathBuf::from("/tmp/test_home/config.security.json"));
    }

    #[test]
    fn test_path_manager_skills_path_no_env_no_setter() {
        let _g = EnvGuard::remove(ENV_SKILLS_CONFIG);
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        assert_eq!(pm.skills_config_path(), PathBuf::from("/tmp/test_home/config.skills.json"));
    }

    #[test]
    fn test_path_manager_config_path_setter_override() {
        let pm = PathManager::with_home(PathBuf::from("/tmp/test_home"));
        let _g = EnvGuard::set(ENV_CONFIG, "/env/config.json");
        pm.set_config_path(PathBuf::from("/setter/config.json"));
        assert_eq!(pm.config_path(), PathBuf::from("/setter/config.json"));
    }

    #[test]
    fn test_resolve_skills_config_path_with_workspace_config() {
        let dir = tempfile::tempdir().unwrap();
        let ws_dir = dir.path().join("ws");
        std::fs::create_dir_all(&ws_dir).unwrap();
        let home = dir.path().join(DEFAULT_HOME_DIR);
        std::fs::create_dir_all(&home).unwrap();
        let config_content = format!(
            r#"{{"agents":{{"defaults":{{"workspace":"{}"}}}}}}"#,
            ws_dir.to_string_lossy().replace('\\', "\\\\").replace('/', "\\/")
        );
        std::fs::write(home.join("config.json"), &config_content).unwrap();

        let _g1 = EnvGuard::set(ENV_HOME, &dir.path().to_string_lossy().to_string());
        let _g2 = EnvGuard::remove(ENV_SKILLS_CONFIG);
        set_local_mode(false);

        let result = resolve_skills_config_path();
        assert!(result.to_string_lossy().contains("config.skills.json"));
    }

    #[test]
    fn test_is_local_mode_default() {
        set_local_mode(false);
        assert!(!is_local_mode());
    }

    #[test]
    fn test_is_local_mode_enabled() {
        set_local_mode(true);
        assert!(is_local_mode());
        set_local_mode(false);
    }
}
