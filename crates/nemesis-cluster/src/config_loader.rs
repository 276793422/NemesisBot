//! Cluster configuration loader.
//!
//! Reads cluster configuration from a JSON file and produces a `ClusterConfig`.
//! For TOML-based static/dynamic configuration, see `cluster_config.rs`.

use std::path::Path;

use crate::types::ClusterConfig;

/// Error type for configuration loading.
#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON parse error: {0}")]
    Json(#[from] serde_json::Error),
    #[error("TOML parse error: {0}")]
    Toml(#[from] toml::de::Error),
    #[error("TOML serialization error: {0}")]
    TomlSerialize(#[from] toml::ser::Error),
}

/// Load cluster configuration from a JSON file.
pub fn load_config(path: &Path) -> Result<ClusterConfig, ConfigError> {
    let content = std::fs::read_to_string(path)?;
    let config: ClusterConfig = serde_json::from_str(&content)?;
    Ok(config)
}

/// Save cluster configuration to a JSON file.
pub fn save_config(path: &Path, config: &ClusterConfig) -> Result<(), ConfigError> {
    let json = serde_json::to_string_pretty(config)?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, json)?;
    Ok(())
}

/// Load configuration from an optional path, falling back to defaults.
pub fn load_or_default(path: Option<&Path>) -> ClusterConfig {
    match path {
        Some(p) => load_config(p).unwrap_or_default(),
        None => ClusterConfig::default(),
    }
}

/// App configuration (from config.cluster.json).
/// Mirrors Go's `AppConfig` struct.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct AppConfig {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_udp_port")]
    pub port: u16,
    #[serde(default = "default_rpc_port")]
    pub rpc_port: u16,
    #[serde(default = "default_broadcast_interval")]
    pub broadcast_interval: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_udp_port(),
            rpc_port: default_rpc_port(),
            broadcast_interval: default_broadcast_interval(),
        }
    }
}

fn default_udp_port() -> u16 {
    11949
}

fn default_rpc_port() -> u16 {
    21949
}

fn default_broadcast_interval() -> u64 {
    30
}

/// Load app configuration from workspace/config/config.cluster.json.
pub fn load_app_config(workspace: &Path) -> AppConfig {
    let config_path = workspace.join("config").join("config.cluster.json");
    if config_path.exists() {
        match std::fs::read_to_string(&config_path) {
            Ok(content) => serde_json::from_str(&content).unwrap_or_default(),
            Err(_) => AppConfig::default(),
        }
    } else {
        AppConfig::default()
    }
}

/// Save app configuration to workspace/config/config.cluster.json.
pub fn save_app_config(workspace: &Path, config: &AppConfig) -> Result<(), ConfigError> {
    let config_path = workspace.join("config").join("config.cluster.json");
    let json = serde_json::to_string_pretty(config)?;
    if let Some(parent) = config_path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(&config_path, json)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_save_and_load_config() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster.json");

        let config = ClusterConfig {
            node_id: "test-node-001".into(),
            bind_address: "0.0.0.0:9100".into(),
            peers: vec!["10.0.0.1:9100".into(), "10.0.0.2:9100".into()],
        };

        save_config(&path, &config).unwrap();
        assert!(path.exists());

        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.node_id, "test-node-001");
        assert_eq!(loaded.bind_address, "0.0.0.0:9100");
        assert_eq!(loaded.peers.len(), 2);
    }

    #[test]
    fn test_load_or_default_missing_file() {
        let config = load_or_default(Some(Path::new("/nonexistent/cluster.json")));
        assert!(config.node_id.is_empty()); // default
    }

    #[test]
    fn test_load_or_default_none() {
        let config = load_or_default(None);
        assert_eq!(config.bind_address, "0.0.0.0:9000");
    }

    #[test]
    fn test_app_config_default() {
        let config = AppConfig::default();
        assert!(!config.enabled);
        assert_eq!(config.port, 11949);
        assert_eq!(config.rpc_port, 21949);
        assert_eq!(config.broadcast_interval, 30);
    }

    #[test]
    fn test_app_config_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let workspace = dir.path();

        let config = AppConfig {
            enabled: true,
            port: 12345,
            rpc_port: 22345,
            broadcast_interval: 60,
        };

        save_app_config(workspace, &config).unwrap();
        let loaded = load_app_config(workspace);
        assert!(loaded.enabled);
        assert_eq!(loaded.port, 12345);
        assert_eq!(loaded.rpc_port, 22345);
    }

    // ============================================================
    // Additional config_loader tests for missing coverage
    // ============================================================

    #[test]
    fn test_config_error_io() {
        let err = ConfigError::Io(std::io::Error::new(std::io::ErrorKind::NotFound, "not found"));
        assert!(err.to_string().contains("not found"));
    }

    #[test]
    fn test_config_error_json() {
        let err = ConfigError::Json(serde_json::from_str::<ClusterConfig>("bad json").unwrap_err());
        assert!(err.to_string().contains("JSON"));
    }

    #[test]
    fn test_load_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("bad.json");
        std::fs::write(&path, "not valid json").unwrap();
        let result = load_config(&path);
        assert!(result.is_err());
    }

    #[test]
    fn test_save_config_creates_parent_dir() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("nested").join("dir").join("cluster.json");

        let config = ClusterConfig::default();
        save_config(&path, &config).unwrap();
        assert!(path.exists());

        let loaded = load_config(&path).unwrap();
        assert_eq!(loaded.bind_address, "0.0.0.0:9000");
    }

    #[test]
    fn test_load_or_default_with_valid_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("cluster.json");

        let config = ClusterConfig {
            node_id: "test-123".into(),
            bind_address: "0.0.0.0:9200".into(),
            peers: vec![],
        };
        save_config(&path, &config).unwrap();

        let loaded = load_or_default(Some(&path));
        assert_eq!(loaded.node_id, "test-123");
        assert_eq!(loaded.bind_address, "0.0.0.0:9200");
    }

    #[test]
    fn test_app_config_serialization_roundtrip() {
        let config = AppConfig {
            enabled: true,
            port: 9999,
            rpc_port: 19999,
            broadcast_interval: 45,
        };
        let json = serde_json::to_string_pretty(&config).unwrap();
        let parsed: AppConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.enabled, true);
        assert_eq!(parsed.port, 9999);
        assert_eq!(parsed.rpc_port, 19999);
        assert_eq!(parsed.broadcast_interval, 45);
    }

    #[test]
    fn test_app_config_deserialization_defaults() {
        let json = r#"{}"#;
        let config: AppConfig = serde_json::from_str(json).unwrap();
        assert!(!config.enabled);
        assert_eq!(config.port, 11949);
        assert_eq!(config.rpc_port, 21949);
        assert_eq!(config.broadcast_interval, 30);
    }

    #[test]
    fn test_load_app_config_nonexistent_dir() {
        let config = load_app_config(Path::new("/nonexistent/workspace"));
        assert!(!config.enabled);
        assert_eq!(config.port, 11949);
    }

    #[test]
    fn test_load_app_config_invalid_json() {
        let dir = tempfile::tempdir().unwrap();
        let config_dir = dir.path().join("config");
        std::fs::create_dir_all(&config_dir).unwrap();
        std::fs::write(config_dir.join("config.cluster.json"), "not valid json").unwrap();

        let config = load_app_config(dir.path());
        assert_eq!(config.port, 11949); // Falls back to default
    }

    #[test]
    fn test_cluster_config_serialization() {
        let config = ClusterConfig {
            node_id: "node-test".into(),
            bind_address: "0.0.0.0:9999".into(),
            peers: vec!["10.0.0.1:9000".into()],
        };
        let json = serde_json::to_string(&config).unwrap();
        let parsed: ClusterConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.node_id, "node-test");
        assert_eq!(parsed.peers.len(), 1);
    }
}
