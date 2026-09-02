//! MCP handler — status/servers/server.add/server.update/server.delete/config.get/config.save.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::{Path, PathBuf};

pub struct McpHandler {
    _priv: (),
}

impl Default for McpHandler {
    fn default() -> Self {
        Self::new()
    }
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
    // 委托 nemesis-path 唯一拼接点。
    nemesis_path::resolve_mcp_config_path_in_workspace(Path::new(workspace))
}

fn load_mcp_config(workspace: &str) -> Result<nemesis_config::McpConfig, String> {
    let path = mcp_config_path(workspace);
    nemesis_config::load_mcp_config(&path).map_err(|e| format!("failed to load MCP config: {}", e))
}

fn save_mcp_config(workspace: &str, config: &nemesis_config::McpConfig) -> Result<(), String> {
    let path = mcp_config_path(workspace);
    nemesis_config::save_mcp_config(&path, config)
        .map_err(|e| format!("failed to save MCP config: {}", e))
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
                // Normalize for display
                let url = if s.url.is_empty() { &s.command } else { &s.url };
                let transport_type = if s.transport_type.is_empty() {
                    "stdio"
                } else {
                    &s.transport_type
                };
                serde_json::json!({
                    "name": s.name,
                    "transport_type": transport_type,
                    "url": url,
                    "description": s.description,
                    "headers": s.headers,
                    "args": s.args,
                    "env": s.env,
                    // 线上键名保持 timeout（UI 契约）；值来自规范字段 timeout_secs
                    "timeout": s.timeout_secs,
                    "provider_name": s.provider_name,
                    "provider_url": s.provider_url,
                    "tags": s.tags,
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

        let mut server = nemesis_config::McpServerConfig {
            name: name.clone(),
            transport_type: crate::handlers::get_opt_str(data, "transport_type")
                .unwrap_or_else(|| "stdio".to_string()),
            url: crate::handlers::get_opt_str(data, "url").unwrap_or_default(),
            description: crate::handlers::get_opt_str(data, "description").unwrap_or_default(),
            headers: data
                .get("headers")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            args: data
                .get("args")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            env: data
                .get("env")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            // 线上键名 timeout；缺省 30（与 serde default 一致；旧值 0 会被
            // discover 的 >0 判断回落 30，直接落 30 语义更直白）
            timeout_secs: data.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30),
            provider_name: crate::handlers::get_opt_str(data, "provider_name").unwrap_or_default(),
            provider_url: crate::handlers::get_opt_str(data, "provider_url").unwrap_or_default(),
            tags: data
                .get("tags")
                .and_then(|v| serde_json::from_value(v.clone()).ok())
                .unwrap_or_default(),
            command: String::new(),
        };
        // Legacy compat: if url empty but command provided, use command as url
        server.normalize();
        config.servers.push(server);
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

        if let Some(v) = data.get("transport_type").and_then(|v| v.as_str()) {
            server.transport_type = v.to_string();
        }
        if let Some(v) = data.get("url").and_then(|v| v.as_str()) {
            server.url = v.to_string();
        }
        if let Some(v) = data.get("description").and_then(|v| v.as_str()) {
            server.description = v.to_string();
        }
        if let Some(v) = data.get("headers").cloned()
            && let Ok(parsed) = serde_json::from_value::<Vec<String>>(v)
        {
            server.headers = parsed;
        }
        if let Some(v) = data.get("args").cloned()
            && let Ok(parsed) = serde_json::from_value::<Vec<String>>(v)
        {
            server.args = parsed;
        }
        if let Some(v) = data.get("env").cloned()
            && let Ok(parsed) = serde_json::from_value::<Vec<String>>(v)
        {
            server.env = parsed;
        }
        if let Some(v) = data
            .get("timeout")
            .or_else(|| data.get("timeout_secs"))
            .and_then(|v| v.as_u64())
        {
            server.timeout_secs = v;
        }
        if let Some(v) = data.get("provider_name").and_then(|v| v.as_str()) {
            server.provider_name = v.to_string();
        }
        if let Some(v) = data.get("provider_url").and_then(|v| v.as_str()) {
            server.provider_url = v.to_string();
        }
        if let Some(v) = data.get("tags").cloned()
            && let Ok(parsed) = serde_json::from_value::<Vec<String>>(v)
        {
            server.tags = parsed;
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
        let json =
            serde_json::to_value(&config).map_err(|e| format!("failed to serialize: {}", e))?;
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

// Phase 3 覆盖率（2026-08-25）：servers 列表 legacy 归一化显示 +
// server.update 全可选字段 patch 臂（旧清单只盖 url/args/env/timeout）。
#[cfg(test)]
mod tests;
