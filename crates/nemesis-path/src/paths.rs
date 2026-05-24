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
///
/// Priority:
/// 1. LocalMode → `{cwd}/.nemesisbot`
/// 2. `NEMESISBOT_HOME` env → `{NEMESISBOT_HOME}/.nemesisbot`
/// 3. Auto-detect cwd → if `{cwd}/.nemesisbot` exists
/// 4. Exe directory → if `{exe_dir}/.nemesisbot` exists
/// 5. Default → `~/.nemesisbot`
pub fn resolve_home_dir() -> Result<PathBuf, String> {
    // Priority 1: LocalMode
    let local_mode = unsafe { LOCAL_MODE };
    if local_mode {
        let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    // Priority 2: NEMESISBOT_HOME env var
    if let Ok(env_home) = std::env::var(ENV_HOME) {
        let expanded = expand_home(&env_home);
        return Ok(expanded.join(DEFAULT_HOME_DIR));
    }

    // Priority 3: Exe directory
    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            if exe_dir.join(DEFAULT_HOME_DIR).is_dir() {
                return Ok(exe_dir.join(DEFAULT_HOME_DIR));
            }
        }
    }

    // Priority 4: Auto-detect cwd
    let cwd = std::env::current_dir().map_err(|e| format!("cwd: {}", e))?;
    if cwd.join(DEFAULT_HOME_DIR).is_dir() {
        return Ok(cwd.join(DEFAULT_HOME_DIR));
    }

    // Priority 5: Default ~/.nemesisbot
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
mod tests;
