//! MCP handler — status/servers/server.add/server.update/server.delete/config.get/config.save.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct McpHandler {
    _priv: (),
}

impl McpHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for McpHandler {
    fn module_name(&self) -> &str {
        "mcp"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "status" => self.status(workspace),
            "servers" => self.servers(workspace),
            "server.add" => {
                let data = data.ok_or("missing data")?;
                self.server_add(workspace, &data)
            }
            "server.update" => {
                let data = data.ok_or("missing data")?;
                self.server_update(workspace, &data)
            }
            "server.delete" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.server_delete(workspace, &name)
            }
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            _ => Err(format!("unknown command: mcp.{}", cmd)),
        }
    }
}

fn mcp_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.mcp.json")
}

fn load_mcp_config(workspace: &str) -> Result<nemesis_config::McpConfig, String> {
    let path = mcp_config_path(workspace);
    nemesis_config::load_mcp_config(&path).map_err(|e| format!("failed to load MCP config: {}", e))
}

fn save_mcp_config(workspace: &str, config: &nemesis_config::McpConfig) -> Result<(), String> {
    let path = mcp_config_path(workspace);
    nemesis_config::save_mcp_config(&path, config).map_err(|e| format!("failed to save MCP config: {}", e))
}

impl McpHandler {
    fn status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        Ok(Some(serde_json::json!({
            "enabled": config.enabled,
            "servers_count": config.servers.len(),
        })))
    }

    fn servers(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        let servers: Vec<_> = config
            .servers
            .iter()
            .map(|s| {
                serde_json::json!({
                    "name": s.name,
                    "command": s.command,
                    "args": s.args,
                    "env": s.env,
                    "timeout": s.timeout,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "servers": servers })))
    }

    fn server_add(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        let mut config = load_mcp_config(workspace)?;

        if config.servers.iter().any(|s| s.name == name) {
            return Err(format!("MCP server '{}' already exists", name));
        }

        let args: Vec<String> = data
            .get("args")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();
        let env: Vec<String> = data
            .get("env")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
            .unwrap_or_default();

        config.servers.push(nemesis_config::McpServerConfig {
            name: name.clone(),
            command: crate::handlers::get_opt_str(data, "command").unwrap_or_default(),
            args,
            env,
            timeout: 0,
        });
        save_mcp_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "added": true, "name": name })))
    }

    fn server_update(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let name = crate::handlers::get_str(data, "name")?;
        let mut config = load_mcp_config(workspace)?;

        let server = config
            .servers
            .iter_mut()
            .find(|s| s.name == name)
            .ok_or_else(|| format!("MCP server '{}' not found", name))?;

        if let Some(cmd) = data.get("command").and_then(|v| v.as_str()) {
            server.command = cmd.to_string();
        }
        if let Some(args) = data.get("args").cloned() {
            if let Ok(parsed) = serde_json::from_value::<Vec<String>>(args) {
                server.args = parsed;
            }
        }
        if let Some(env) = data.get("env").cloned() {
            if let Ok(parsed) = serde_json::from_value::<Vec<String>>(env) {
                server.env = parsed;
            }
        }
        if let Some(timeout) = data.get("timeout").and_then(|v| v.as_i64()) {
            server.timeout = timeout;
        }

        save_mcp_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "updated": true, "name": name })))
    }

    fn server_delete(
        &self,
        workspace: &str,
        name: &str,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_mcp_config(workspace)?;
        let before = config.servers.len();
        config.servers.retain(|s| s.name != name);
        if config.servers.len() == before {
            return Err(format!("MCP server '{}' not found", name));
        }
        save_mcp_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_mcp_config(workspace)?;
        let json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: nemesis_config::McpConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid MCP config: {}", e))?;
        save_mcp_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}
