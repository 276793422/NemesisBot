//! MCP installer - manages MCP server registration in config files.
//!
//! Handles adding, removing, and checking MCP server entries in the
//! workspace configuration.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// MCP server configuration entry.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPServerConfig {
    /// Server name.
    pub name: String,
    /// Command to run.
    pub command: String,
    /// Command arguments.
    #[serde(default)]
    pub args: Vec<String>,
}

/// MCP configuration file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MCPConfig {
    /// Whether MCP is enabled.
    #[serde(default)]
    pub enabled: bool,
    /// Registered servers.
    #[serde(default)]
    pub servers: Vec<MCPServerConfig>,
}

impl Default for MCPConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            servers: Vec::new(),
        }
    }
}

/// Manages MCP server registration.
pub struct MCPInstaller {
    workspace: PathBuf,
}

impl MCPInstaller {
    /// Create a new MCP installer for the given workspace.
    pub fn new(workspace: impl Into<PathBuf>) -> Self {
        Self {
            workspace: workspace.into(),
        }
    }

    /// Get the config file path.
    pub fn config_path(&self) -> PathBuf {
        self.workspace.join("config").join("config.mcp.json")
    }

    /// Install (add or update) an MCP server.
    pub async fn install(
        &self,
        name: &str,
        command: &str,
        args: Vec<String>,
    ) -> std::io::Result<()> {
        let mut config = self.load_config().await?;

        let server = MCPServerConfig {
            name: name.to_string(),
            command: command.to_string(),
            args,
        };

        // Update existing or append
        let mut found = false;
        for s in &mut config.servers {
            if s.name == name {
                *s = server.clone();
                found = true;
                break;
            }
        }
        if !found {
            config.servers.push(server);
        }
        config.enabled = true;

        self.save_config(&config).await
    }

    /// Uninstall (remove) an MCP server.
    pub async fn uninstall(&self, name: &str) -> std::io::Result<()> {
        let mut config = self.load_config().await?;
        config.servers.retain(|s| s.name != name);
        self.save_config(&config).await
    }

    /// Check if an MCP server is installed.
    pub async fn is_installed(&self, name: &str) -> bool {
        match self.load_config().await {
            Ok(config) => config.servers.iter().any(|s| s.name == name),
            Err(_) => false,
        }
    }

    /// Load the MCP config from disk.
    pub async fn load_config(&self) -> std::io::Result<MCPConfig> {
        let path = self.config_path();
        if !path.exists() {
            return Ok(MCPConfig::default());
        }
        let content = tokio::fs::read_to_string(&path).await?;
        serde_json::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))
    }

    /// Save the MCP config to disk.
    pub async fn save_config(&self, config: &MCPConfig) -> std::io::Result<()> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string()))?;
        tokio::fs::write(&path, json).await
    }
}

#[cfg(test)]
mod tests;
