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
mod tests {
    use super::*;

    #[test]
    fn test_static_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");

        let config = StaticConfig {
            cluster: ClusterMeta {
                id: "test-cluster".into(),
                auto_discovery: true,
                last_updated: "2026-04-29T00:00:00Z".into(),
                rpc_auth_token: "secret-token".into(),
            },
            node: NodeInfo {
                id: "node-001".into(),
                name: "Test Bot".into(),
                address: "0.0.0.0:21949".into(),
                role: "worker".into(),
                category: "development".into(),
                tags: vec!["test".into()],
                capabilities: vec!["llm".into()],
            },
            peers: vec![PeerConfig {
                id: "peer-001".into(),
                name: "Remote Bot".into(),
                address: "10.0.0.1:21949".into(),
                addresses: vec!["10.0.0.1".into()],
                rpc_port: 21949,
                role: "worker".into(),
                category: "general".into(),
                tags: Vec::new(),
                capabilities: vec!["llm".into()],
                priority: 1,
                enabled: true,
                status: PeerStatus {
                    state: "online".into(),
                    last_seen: "2026-04-29T00:00:00Z".into(),
                    ..PeerStatus::default()
                },
            }],
        };

        save_static_config(&path, &config).unwrap();
        assert!(path.exists());

        let loaded = load_static_config(&path).unwrap();
        assert_eq!(loaded.cluster.id, "test-cluster");
        assert_eq!(loaded.node.id, "node-001");
        assert_eq!(loaded.peers.len(), 1);
        assert_eq!(loaded.peers[0].rpc_port, 21949);
    }

    #[test]
    fn test_dynamic_state_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("state.toml");

        let state = DynamicState {
            cluster: ClusterMeta {
                id: "auto-discovered".into(),
                auto_discovery: true,
                last_updated: "2026-04-29T00:00:00Z".into(),
                rpc_auth_token: String::new(),
            },
            local_node: NodeInfo {
                id: "local-001".into(),
                name: "Local".into(),
                address: "0.0.0.0:21949".into(),
                ..NodeInfo::default()
            },
            discovered: vec![PeerConfig {
                id: "discovered-001".into(),
                name: "Found Bot".into(),
                address: "10.0.0.2:21949".into(),
                ..PeerConfig::default()
            }],
            last_sync: "2026-04-29T00:00:00Z".into(),
        };

        save_dynamic_state(&path, &state).unwrap();
        let loaded = load_dynamic_state(&path).unwrap();
        assert_eq!(loaded.discovered.len(), 1);
        assert_eq!(loaded.discovered[0].id, "discovered-001");
    }

    #[test]
    fn test_load_static_config_not_found() {
        let result = load_static_config(Path::new("/nonexistent/peers.toml"));
        assert!(result.is_err());
    }

    #[test]
    fn test_load_dynamic_state_not_found_returns_default() {
        let result = load_dynamic_state(Path::new("/nonexistent/state.toml"));
        assert!(result.is_ok());
        let state = result.unwrap();
        assert!(state.discovered.is_empty());
    }

    #[test]
    fn test_create_static_config() {
        let config = create_static_config("node-123", "Test Bot", "0.0.0.0:9000");
        assert_eq!(config.node.id, "node-123");
        assert_eq!(config.node.name, "Test Bot");
        assert!(config.peers.is_empty());
    }

    #[test]
    fn test_load_or_create_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");

        // First call: file doesn't exist -> creates default (but doesn't save)
        let config = load_or_create_config(&path, "node-xyz");
        assert_eq!(config.node.id, "node-xyz");

        // Manually save it
        save_static_config(&path, &config).unwrap();

        // Second call: file exists -> loads it
        let loaded = load_or_create_config(&path, "different-id");
        assert_eq!(loaded.node.id, "node-xyz");
    }

    #[test]
    fn test_peer_status_default() {
        let status = PeerStatus::default();
        assert_eq!(status.state, "unknown");
        assert_eq!(status.tasks_completed, 0);
    }

    #[test]
    fn test_toml_serialization_format() {
        let config = create_static_config("node-1", "Bot 1", "0.0.0.0:21949");
        let toml_str = toml::to_string_pretty(&config).unwrap();
        assert!(toml_str.contains("[cluster]"));
        assert!(toml_str.contains("[node]"));
    }

    // -- Additional tests: cluster config validation, peer management, role defaults --

    #[test]
    fn test_static_config_default_values() {
        let meta = ClusterMeta::default();
        let node = NodeInfo::default();
        let config = StaticConfig {
            cluster: meta,
            node: node.clone(),
            peers: vec![],
        };
        assert!(config.cluster.id == "auto-discovered");
        assert!(config.peers.is_empty());
        assert!(config.node.id.is_empty());
        assert_eq!(config.node.role, "worker");
        assert_eq!(config.node.category, "general");
    }

    #[test]
    fn test_cluster_meta_default() {
        let meta = ClusterMeta::default();
        assert!(meta.auto_discovery);
        assert!(meta.rpc_auth_token.is_empty());
    }

    #[test]
    fn test_peer_config_default() {
        let peer = PeerConfig::default();
        assert_eq!(peer.priority, 1);
        assert!(peer.enabled);
        assert_eq!(peer.rpc_port, 0);
        assert_eq!(peer.status.state, "unknown");
        assert_eq!(peer.status.success_rate, 0.0);
    }

    #[test]
    fn test_dynamic_state_default() {
        let state = DynamicState::default();
        assert!(state.discovered.is_empty());
        assert!(state.cluster.auto_discovery);
    }

    #[test]
    fn test_static_config_with_multiple_peers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");

        let config = StaticConfig {
            cluster: ClusterMeta::default(),
            node: NodeInfo {
                id: "master-1".into(),
                name: "Master".into(),
                address: "0.0.0.0:21949".into(),
                role: "master".into(),
                category: "development".into(),
                tags: vec!["primary".into()],
                capabilities: vec!["llm".into(), "forge".into()],
            },
            peers: vec![
                PeerConfig {
                    id: "worker-1".into(),
                    name: "Worker 1".into(),
                    address: "10.0.0.2:21949".into(),
                    rpc_port: 21949,
                    role: "worker".into(),
                    enabled: true,
                    ..PeerConfig::default()
                },
                PeerConfig {
                    id: "worker-2".into(),
                    name: "Worker 2".into(),
                    address: "10.0.0.3:21949".into(),
                    rpc_port: 21949,
                    role: "worker".into(),
                    enabled: false,
                    ..PeerConfig::default()
                },
            ],
        };

        save_static_config(&path, &config).unwrap();
        let loaded = load_static_config(&path).unwrap();

        assert_eq!(loaded.peers.len(), 2);
        assert_eq!(loaded.peers[0].id, "worker-1");
        assert!(loaded.peers[0].enabled);
        assert_eq!(loaded.peers[1].id, "worker-2");
        assert!(!loaded.peers[1].enabled);
    }

    #[test]
    fn test_node_info_default_role_is_worker() {
        let node = NodeInfo::default();
        assert_eq!(node.role, "worker");
    }

    #[test]
    fn test_peer_config_with_tags_and_capabilities() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("peers.toml");

        let config = StaticConfig {
            cluster: ClusterMeta::default(),
            node: NodeInfo::default(),
            peers: vec![PeerConfig {
                id: "p1".into(),
                name: "TaggedPeer".into(),
                address: "10.0.0.1:21949".into(),
                tags: vec!["gpu".into(), "high-mem".into()],
                capabilities: vec!["llm".into(), "voice".into(), "vision".into()],
                ..PeerConfig::default()
            }],
        };

        save_static_config(&path, &config).unwrap();
        let loaded = load_static_config(&path).unwrap();

        assert_eq!(loaded.peers[0].tags.len(), 2);
        assert!(loaded.peers[0].tags.contains(&"gpu".to_string()));
        assert_eq!(loaded.peers[0].capabilities.len(), 3);
        assert!(loaded.peers[0].capabilities.contains(&"vision".to_string()));
    }
}
