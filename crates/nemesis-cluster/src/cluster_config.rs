//! Cluster TOML configuration types.
//!
//! Defines `StaticConfig` (peers.toml) and `DynamicState` (state.toml) along
//! with their load/save functions. Uses atomic write (write-to-tmp + rename)
//! to prevent corruption.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::config_loader::ConfigError;

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// Static cluster configuration (peers.toml).
/// Created during onboard and contains the current node's information.
/// Users can manually edit this file to add known peers.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StaticConfig {
    #[serde(default)]
    pub cluster: ClusterMeta,
    #[serde(default)]
    pub node: NodeInfo,
    #[serde(default)]
    pub peers: Vec<PeerConfig>,
}

/// Cluster metadata.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClusterMeta {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub auto_discovery: bool,
    #[serde(default)]
    pub last_updated: String,
    #[serde(default)]
    pub rpc_auth_token: String,
}

impl Default for ClusterMeta {
    fn default() -> Self {
        Self {
            id: "auto-discovered".into(),
            auto_discovery: true,
            last_updated: chrono::Utc::now().to_rfc3339(),
            rpc_auth_token: String::new(),
        }
    }
}

/// Node information in the config file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NodeInfo {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default = "default_role")]
    pub role: String,
    #[serde(default = "default_category")]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

impl Default for NodeInfo {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            role: default_role(),
            category: default_category(),
            tags: Vec::new(),
            capabilities: Vec::new(),
        }
    }
}

fn default_role() -> String {
    "worker".into()
}

fn default_category() -> String {
    "general".into()
}

/// Peer node configuration.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerConfig {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub address: String,
    #[serde(default)]
    pub addresses: Vec<String>,
    #[serde(default)]
    pub rpc_port: u16,
    #[serde(default)]
    pub role: String,
    #[serde(default)]
    pub category: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default = "default_priority")]
    pub priority: u32,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
    #[serde(default)]
    pub status: PeerStatus,
}

impl Default for PeerConfig {
    fn default() -> Self {
        Self {
            id: String::new(),
            name: String::new(),
            address: String::new(),
            addresses: Vec::new(),
            rpc_port: 0,
            role: String::new(),
            category: String::new(),
            tags: Vec::new(),
            capabilities: Vec::new(),
            priority: default_priority(),
            enabled: default_enabled(),
            status: PeerStatus::default(),
        }
    }
}

fn default_priority() -> u32 {
    1
}

fn default_enabled() -> bool {
    true
}

/// Peer runtime status.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PeerStatus {
    #[serde(default)]
    pub state: String,
    #[serde(default)]
    pub last_seen: String,
    #[serde(default)]
    pub uptime: String,
    #[serde(default)]
    pub tasks_completed: u64,
    #[serde(default)]
    pub success_rate: f64,
    #[serde(default)]
    pub avg_response_time: u64,
    #[serde(default)]
    pub last_error: String,
}

impl Default for PeerStatus {
    fn default() -> Self {
        Self {
            state: "unknown".into(),
            last_seen: String::new(),
            uptime: String::new(),
            tasks_completed: 0,
            success_rate: 0.0,
            avg_response_time: 0,
            last_error: String::new(),
        }
    }
}

/// Dynamic cluster state (state.toml).
/// Automatically managed by the cluster module.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DynamicState {
    #[serde(default)]
    pub cluster: ClusterMeta,
    #[serde(default)]
    pub local_node: NodeInfo,
    #[serde(default)]
    pub discovered: Vec<PeerConfig>,
    #[serde(default)]
    pub last_sync: String,
}

impl Default for DynamicState {
    fn default() -> Self {
        Self {
            cluster: ClusterMeta::default(),
            local_node: NodeInfo::default(),
            discovered: Vec::new(),
            last_sync: chrono::Utc::now().to_rfc3339(),
        }
    }
}

// ---------------------------------------------------------------------------
// Load / Save functions
// ---------------------------------------------------------------------------

/// Load static config from a TOML file.
pub fn load_static_config(path: &Path) -> Result<StaticConfig, ConfigError> {
    if !path.exists() {
        return Err(ConfigError::Io(std::io::Error::new(
            std::io::ErrorKind::NotFound,
            format!("config file not found: {}", path.display()),
        )));
    }
    let content = std::fs::read_to_string(path)?;
    let config: StaticConfig = toml::from_str(&content)?;
    Ok(config)
}

/// Save static config to a TOML file using atomic write.
pub fn save_static_config(path: &Path, config: &StaticConfig) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(config).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

    // Atomic write: write to tmp file, then rename
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Load dynamic state from a TOML file.
/// Returns a default empty state if the file doesn't exist.
pub fn load_dynamic_state(path: &Path) -> Result<DynamicState, ConfigError> {
    if !path.exists() {
        return Ok(DynamicState::default());
    }
    let content = std::fs::read_to_string(path)?;
    let state: DynamicState = toml::from_str(&content)?;
    Ok(state)
}

/// Save dynamic state to a TOML file using atomic write.
pub fn save_dynamic_state(path: &Path, state: &DynamicState) -> Result<(), ConfigError> {
    // Ensure parent directory exists
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    // Serialize to TOML
    let toml_str = toml::to_string_pretty(state).map_err(|e| {
        std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
    })?;

    // Atomic write
    atomic_write(path, toml_str.as_bytes())?;
    Ok(())
}

/// Create a default static config.
pub fn create_static_config(node_id: &str, node_name: &str, address: &str) -> StaticConfig {
    StaticConfig {
        cluster: ClusterMeta {
            id: "manual".into(),
            auto_discovery: true,
            last_updated: chrono::Utc::now().to_rfc3339(),
            rpc_auth_token: String::new(),
        },
        node: NodeInfo {
            id: node_id.into(),
            name: node_name.into(),
            address: address.into(),
            role: "worker".into(),
            category: "general".into(),
            tags: Vec::new(),
            capabilities: Vec::new(),
        },
        peers: Vec::new(),
    }
}

/// Load existing config or create a default one.
pub fn load_or_create_config(path: &Path, node_id: &str) -> StaticConfig {
    match load_static_config(path) {
        Ok(config) => config,
        Err(_) => create_static_config(node_id, &format!("Bot {}", node_id), ""),
    }
}

// ---------------------------------------------------------------------------
// Atomic write helper
// ---------------------------------------------------------------------------

/// Write data to a file atomically: write to a `.tmp` file first, then rename.
fn atomic_write(path: &Path, data: &[u8]) -> Result<(), ConfigError> {
    let tmp_path = path.with_extension("toml.tmp");

    std::fs::write(&tmp_path, data)?;

    // Atomic rename (on Windows, this replaces if destination exists)
    match std::fs::rename(&tmp_path, path) {
        Ok(()) => Ok(()),
        Err(e) => {
            // Clean up temp file
            let _ = std::fs::remove_file(&tmp_path);
            Err(ConfigError::Io(e))
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
