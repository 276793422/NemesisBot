//! MCP Manager — centralized MCP server configuration and tool discovery.
//!
//! Manages the MCP config file (`config.mcp.json`), provides CRUD operations
//! for server entries, discovers tools from MCP servers, and tracks file
//! modification time for hot-reload support.

use std::path::PathBuf;
use std::time::SystemTime;

use serde::{Deserialize, Serialize};
use tracing::{info, warn};

use crate::adapter::{self, Tool};
use crate::client::{Client, McpClient};
use crate::stdio_transport::StdioTransport;
use crate::http_transport::HttpTransport;
use crate::types::{McpTool, Resource, ServerConfig, ServerInfo};

// ---------------------------------------------------------------------------
// Config file format
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Serialize, Deserialize)]
struct McpFileConfig {
    #[serde(default)]
    enabled: bool,
    #[serde(default)]
    servers: Vec<ServerConfig>,
    #[serde(default = "default_timeout")]
    timeout: u64,
}

fn default_timeout() -> u64 {
    30
}

// ---------------------------------------------------------------------------
// McpManager
// ---------------------------------------------------------------------------

/// Centralized manager for MCP server configuration and tool discovery.
///
/// Reads/writes `config.mcp.json`, discovers tools from MCP servers,
/// and tracks file modification time for automatic hot-reload.
pub struct McpManager {
    config_path: PathBuf,
    config: McpFileConfig,
    last_mtime: Option<SystemTime>,
}

impl McpManager {
    /// Create a new manager bound to the given config file path.
    ///
    /// Loads the config if the file exists; otherwise starts with empty defaults.
    pub fn new(config_path: PathBuf) -> Self {
        let mut mgr = Self {
            config_path,
            config: McpFileConfig {
                enabled: false,
                servers: Vec::new(),
                timeout: 30,
            },
            last_mtime: None,
        };
        if let Err(e) = mgr.load_config() {
            warn!("[McpManager] Failed to load config on init: {}", e);
        }
        mgr.last_mtime = Self::read_mtime(&mgr.config_path);
        mgr
    }

    // -----------------------------------------------------------------------
    // Config I/O
    // -----------------------------------------------------------------------

    /// Load config from disk. Returns Ok(()) even if the file doesn't exist.
    pub fn load_config(&mut self) -> Result<(), String> {
        if !self.config_path.exists() {
            return Ok(());
        }
        let data = std::fs::read_to_string(&self.config_path)
            .map_err(|e| format!("Failed to read MCP config: {}", e))?;
        self.config = serde_json::from_str(&data)
            .map_err(|e| format!("Failed to parse MCP config: {}", e))?;
        Ok(())
    }

    /// Save current config to disk.
    pub fn save_config(&self) -> Result<(), String> {
        if let Some(parent) = self.config_path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create MCP config dir: {}", e))?;
        }
        let content = serde_json::to_string_pretty(&self.config)
            .map_err(|e| format!("Failed to serialize MCP config: {}", e))?;
        let tmp_path = self.config_path.with_extension("mcp.tmp");
        std::fs::write(&tmp_path, &content)
            .map_err(|e| format!("Failed to write MCP config: {}", e))?;
        std::fs::rename(&tmp_path, &self.config_path)
            .map_err(|e| format!("Failed to rename MCP config: {}", e))?;
        Ok(())
    }

    // -----------------------------------------------------------------------
    // Queries
    // -----------------------------------------------------------------------

    /// Whether MCP is enabled.
    pub fn is_enabled(&self) -> bool {
        self.config.enabled
    }

    /// Return the config file path.
    pub fn config_path(&self) -> &PathBuf {
        &self.config_path
    }

    /// List all configured servers.
    pub fn list_servers(&self) -> &[ServerConfig] {
        &self.config.servers
    }

    /// Find a server by name.
    pub fn get_server(&self, name: &str) -> Option<&ServerConfig> {
        self.config.servers.iter().find(|s| s.name == name)
    }

    // -----------------------------------------------------------------------
    // CRUD
    // -----------------------------------------------------------------------

    /// Add a new server. Returns Err if a server with the same name exists.
    pub fn add_server(&mut self, config: ServerConfig) -> Result<(), String> {
        if self.config.servers.iter().any(|s| s.name == config.name) {
            return Err(format!("Server '{}' already exists", config.name));
        }
        self.config.servers.push(config);
        self.config.enabled = true;
        self.save_config()?;
        Ok(())
    }

    /// Remove a server by name. Returns true if a server was removed.
    pub fn remove_server(&mut self, name: &str) -> Result<bool, String> {
        let before = self.config.servers.len();
        self.config.servers.retain(|s| s.name != name);
        let removed = self.config.servers.len() < before;
        if removed {
            self.save_config()?;
        }
        Ok(removed)
    }

    // -----------------------------------------------------------------------
    // Hot-reload
    // -----------------------------------------------------------------------

    /// Check if the config file has been modified since the last check.
    /// Updates the internal mtime and returns true if changed.
    pub fn check_config_changed(&mut self) -> bool {
        let current = Self::read_mtime(&self.config_path);
        if current != self.last_mtime {
            if let Err(e) = self.load_config() {
                warn!("[McpManager] Failed to reload config: {}", e);
                // Do NOT update last_mtime — will retry next round
                return false;
            }
            self.last_mtime = current;
            info!("[McpManager] Config file changed, reloaded");
            return true;
        }
        false
    }

    /// Return servers that are not yet represented in the registered tools list.
    ///
    /// `registered_tool_prefixes` contains the sanitized server-name prefixes
    /// of already-registered MCP tools (e.g., `["mcp_test_server_"]`).
    pub fn find_new_servers(&self, registered_tool_prefixes: &[String]) -> Vec<&ServerConfig> {
        self.config.servers.iter().filter(|server| {
            let prefix = format!("mcp_{}_", adapter::sanitize_name(&server.name));
            !registered_tool_prefixes.contains(&prefix)
        }).collect()
    }

    // -----------------------------------------------------------------------
    // Discovery
    // -----------------------------------------------------------------------

    /// Connect to an MCP server, discover its tools, and return adapter Tool objects.
    ///
    /// Uses the config name (not the server's self-reported name) for tool prefixing.
    pub async fn discover_tools(
        &self,
        server: &ServerConfig,
    ) -> Result<Vec<Box<dyn Tool>>, String> {
        let transport = StdioTransport::from_config(server);
        let mut client: Box<dyn Client> = Box::new(McpClient::new(Box::new(transport)));

        let timeout = std::time::Duration::from_secs(if server.timeout_secs > 0 {
            server.timeout_secs
        } else {
            30
        });

        // Initialize with timeout
        let init_result = tokio::time::timeout(timeout, client.initialize()).await;
        match init_result {
            Ok(Ok(_)) => {}
            Ok(Err(e)) => {
                // Init failed — client dropped here, killing the subprocess
                drop(client);
                return Err(format!("MCP server '{}' initialization failed: {}", server.name, e));
            }
            Err(_) => {
                // Timeout — client dropped here, killing the subprocess
                drop(client);
                return Err(format!("MCP server '{}' initialization timed out", server.name));
            }
        }

        info!(
            server = %server.name,
            "[McpManager] MCP server initialized"
        );

        // Discover tools using config name
        adapter::create_tools_from_client_named(client, &server.name, server.timeout_secs)
            .await
            .map_err(|e| format!("Failed to discover tools from '{}': {}", server.name, e))
    }

    // -----------------------------------------------------------------------
    // Helpers
    // -----------------------------------------------------------------------

    fn read_mtime(path: &PathBuf) -> Option<SystemTime> {
        std::fs::metadata(path).ok().and_then(|m| m.modified().ok())
    }
}

// ---------------------------------------------------------------------------
// One-shot discovery (for mcp_discover tool)
// ---------------------------------------------------------------------------

/// Result of one-shot MCP server discovery.
///
/// Contains raw metadata (not adapter objects) for formatting/display.
pub struct DiscoveryResult {
    /// Server name and version from the initialize handshake.
    pub server_info: Option<ServerInfo>,
    /// Tools exposed by the server.
    pub tools: Vec<McpTool>,
    /// Resources exposed by the server.
    pub resources: Vec<Resource>,
    /// Prompts exposed by the server.
    pub prompts: Vec<crate::types::Prompt>,
}

/// One-shot discovery: connect to an MCP server, collect metadata, close.
///
/// This is intended for the `mcp_discover` agent tool. It does NOT keep
/// the client alive — the subprocess is killed after discovery.
pub async fn discover_server_metadata(
    command: &str,
    args: Vec<String>,
    env: Vec<String>,
    timeout_secs: u64,
) -> Result<DiscoveryResult, String> {
    let transport = StdioTransport::new(command, args, env);
    let mut client: Box<dyn Client> = Box::new(McpClient::new(Box::new(transport)));

    let duration = std::time::Duration::from_secs(if timeout_secs > 0 { timeout_secs } else { 15 });

    // Initialize with timeout
    let init_result = tokio::time::timeout(duration, client.initialize()).await;
    match init_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP server '{}' initialization failed: {}. \
                 If this is an HTTP-based MCP server, stdio discovery will not work.",
                command, e
            ));
        }
        Err(_) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP server '{}' timed out after {}s. \
                 If this is an HTTP-based MCP server, use the 'url' parameter instead.",
                command, timeout_secs
            ));
        }
    }

    let server_info = client.server_info().cloned();

    // Collect metadata — each call is best-effort
    let tools = client.list_tools().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_tools failed: {}", e);
        Vec::new()
    });
    let resources = client.list_resources().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_resources failed: {}", e);
        Vec::new()
    });
    let prompts = client.list_prompts().await.unwrap_or_else(|e| {
        warn!("[McpManager] list_prompts failed: {}", e);
        Vec::new()
    });

    // Always close to kill the subprocess
    let _ = client.close().await;

    Ok(DiscoveryResult {
        server_info,
        tools,
        resources,
        prompts,
    })
}

/// One-shot HTTP discovery: connect to an MCP server via Streamable HTTP,
/// collect metadata, close.
///
/// This is the HTTP counterpart of [`discover_server_metadata`].
pub async fn discover_server_metadata_http(
    url: &str,
    timeout_secs: u64,
) -> Result<DiscoveryResult, String> {
    let transport = HttpTransport::new(url);
    let mut client: Box<dyn Client> = Box::new(McpClient::new(Box::new(transport)));

    let duration = std::time::Duration::from_secs(if timeout_secs > 0 { timeout_secs } else { 15 });

    let init_result = tokio::time::timeout(duration, client.initialize()).await;
    match init_result {
        Ok(Ok(_)) => {}
        Ok(Err(e)) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP HTTP server '{}' initialization failed: {}",
                url, e
            ));
        }
        Err(_) => {
            let _ = client.close().await;
            return Err(format!(
                "MCP HTTP server '{}' timed out after {}s",
                url, timeout_secs
            ));
        }
    }

    let server_info = client.server_info().cloned();

    let tools = client.list_tools().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_tools failed: {}", e);
        Vec::new()
    });
    let resources = client.list_resources().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_resources failed: {}", e);
        Vec::new()
    });
    let prompts = client.list_prompts().await.unwrap_or_else(|e| {
        warn!("[McpManager] HTTP list_prompts failed: {}", e);
        Vec::new()
    });

    let _ = client.close().await;

    Ok(DiscoveryResult {
        server_info,
        tools,
        resources,
        prompts,
    })
}

#[cfg(test)]
mod tests;
