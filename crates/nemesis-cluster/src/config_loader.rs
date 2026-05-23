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
    /// LLM request timeout in seconds for B-side peer_chat processing.
    /// This is the maximum time to wait for the LLM API to respond.
    /// Default: 7200 (2 hours). Set to 0 to disable timeout.
    #[serde(default = "default_llm_timeout_secs")]
    pub llm_timeout_secs: u64,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            port: default_udp_port(),
            rpc_port: default_rpc_port(),
            broadcast_interval: default_broadcast_interval(),
            llm_timeout_secs: default_llm_timeout_secs(),
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

fn default_llm_timeout_secs() -> u64 {
    7200 // 2 hours
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
mod tests;
