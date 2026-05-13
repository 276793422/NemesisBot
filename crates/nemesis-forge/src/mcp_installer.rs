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
    pub async fn install(&self, name: &str, command: &str, args: Vec<String>) -> std::io::Result<()> {
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
        serde_json::from_str(&content).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })
    }

    /// Save the MCP config to disk.
    pub async fn save_config(&self, config: &MCPConfig) -> std::io::Result<()> {
        let path = self.config_path();
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await?;
        }
        let json = serde_json::to_string_pretty(config).map_err(|e| {
            std::io::Error::new(std::io::ErrorKind::InvalidData, e.to_string())
        })?;
        tokio::fs::write(&path, json).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn test_install_and_is_installed() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());

        assert!(!installer.is_installed("test-server").await);

        installer
            .install("test-server", "uv", vec!["run".into(), "server.py".into()])
            .await
            .unwrap();

        assert!(installer.is_installed("test-server").await);
    }

    #[tokio::test]
    async fn test_uninstall() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());

        installer
            .install("to-remove", "python", vec!["server.py".into()])
            .await
            .unwrap();

        assert!(installer.is_installed("to-remove").await);

        installer.uninstall("to-remove").await.unwrap();
        assert!(!installer.is_installed("to-remove").await);
    }

    #[tokio::test]
    async fn test_config_persistence() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());

        installer
            .install("persist-test", "go", vec!["run".into()])
            .await
            .unwrap();

        // Create new installer instance
        let installer2 = MCPInstaller::new(dir.path());
        assert!(installer2.is_installed("persist-test").await);
    }

    // --- Additional mcp_installer tests ---

    #[tokio::test]
    async fn test_load_config_empty() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        let config = installer.load_config().await.unwrap();
        assert!(config.servers.is_empty());
    }

    #[tokio::test]
    async fn test_save_and_load_config() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        let mut config = MCPConfig::default();
        config.servers.push(MCPServerConfig {
            name: "test-server".into(),
            command: "python".into(),
            args: vec!["server.py".into()],
        });
        installer.save_config(&config).await.unwrap();
        let loaded = installer.load_config().await.unwrap();
        assert_eq!(loaded.servers.len(), 1);
        assert_eq!(loaded.servers[0].name, "test-server");
    }

    #[tokio::test]
    async fn test_install_multiple_servers() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        installer.install("server-a", "go", vec!["run".into()]).await.unwrap();
        installer.install("server-b", "python", vec!["main.py".into()]).await.unwrap();
        assert!(installer.is_installed("server-a").await);
        assert!(installer.is_installed("server-b").await);
    }

    #[tokio::test]
    async fn test_uninstall_nonexistent() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        // Should not panic
        installer.uninstall("nonexistent").await.unwrap();
    }

    #[tokio::test]
    async fn test_reinstall_overwrites() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        installer.install("server", "go", vec!["v1".into()]).await.unwrap();
        installer.install("server", "python", vec!["v2".into()]).await.unwrap();
        let config = installer.load_config().await.unwrap();
        assert_eq!(config.servers[0].command, "python");
    }

    #[tokio::test]
    async fn test_config_path_in_workspace() {
        let dir = tempfile::tempdir().unwrap();
        let installer = MCPInstaller::new(dir.path());
        let path = installer.config_path();
        assert!(path.to_string_lossy().contains("config"));
    }
}
