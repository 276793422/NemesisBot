//! MCP handler — status/servers/server.add/server.update/server.delete/config.get/config.save.

use crate::handlers::{mask_secret_entry, require_workspace, restore_masked_entries};
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

    fn commands(&self) -> &'static [&'static str] {
        &[
            "status",
            "servers",
            "server.add",
            "server.update",
            "server.delete",
            "config.get",
            "config.save",
        ]
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

/// headers/env 是 `"Key: value"` / `"KEY=value"` 字符串列表——回显前逐条
/// 值部脱敏（凭据回显脱敏批次，2026-09-25；vault 方案 0.4.7 遗留）。
fn mask_string_list(list: &[String]) -> Vec<String> {
    list.iter().map(|s| mask_secret_entry(s)).collect()
}

/// 反序列化 headers/env 入参（宽容形态同 serde flexible_string_list），
/// 失败返回 None 走"字段未提供"语义。
fn parse_string_list(data: &serde_json::Value, key: &str) -> Option<Vec<String>> {
    data.get(key)
        .cloned()
        .and_then(|v| serde_json::from_value::<Vec<String>>(v).ok())
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
                    "headers": mask_string_list(&s.headers),
                    "args": s.args,
                    "env": mask_string_list(&s.env),
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
            headers: parse_string_list(data, "headers").unwrap_or_default(),
            args: parse_string_list(data, "args").unwrap_or_default(),
            env: parse_string_list(data, "env").unwrap_or_default(),
            // 线上键名 timeout；缺省 30（与 serde default 一致；旧值 0 会被
            // discover 的 >0 判断回落 30，直接落 30 语义更直白）
            timeout_secs: data.get("timeout").and_then(|v| v.as_u64()).unwrap_or(30),
            provider_name: crate::handlers::get_opt_str(data, "provider_name").unwrap_or_default(),
            provider_url: crate::handlers::get_opt_str(data, "provider_url").unwrap_or_default(),
            tags: parse_string_list(data, "tags").unwrap_or_default(),
            command: String::new(),
            extra: std::collections::BTreeMap::new(),
        };
        // 新建无存量可还原：掩码值一律 loud 拒绝，绝不把掩码当真值落盘。
        for (label, list) in [("headers", &server.headers), ("env", &server.env)] {
            if list.iter().any(|e| e.contains("****")) {
                return Err(format!("{label} 含掩码值（****），新建时请输入完整值"));
            }
        }
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
        if let Some(mut v) = parse_string_list(data, "headers") {
            // 回显是脱敏形态：掩码条目按键还原存量原值，找不到 = loud 拒绝。
            restore_masked_entries(&mut v, &server.headers)?;
            server.headers = v;
        }
        if let Some(v) = parse_string_list(data, "args") {
            server.args = v;
        }
        if let Some(mut v) = parse_string_list(data, "env") {
            restore_masked_entries(&mut v, &server.env)?;
            server.env = v;
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
        let mut json =
            serde_json::to_value(&config).map_err(|e| format!("failed to serialize: {}", e))?;
        // servers[].headers/env 逐条值部脱敏（凭据回显脱敏批次）。
        if let Some(servers) = json.get_mut("servers").and_then(|v| v.as_array_mut()) {
            for s in servers.iter_mut() {
                for key in ["headers", "env"] {
                    if let Some(serde_json::Value::Array(arr)) = s.get_mut(key) {
                        for entry in arr.iter_mut() {
                            if let serde_json::Value::String(es) = entry {
                                *es = mask_secret_entry(es);
                            }
                        }
                    }
                }
            }
        }
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config: nemesis_config::McpConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid MCP config: {}", e))?;
        // 整包回存同源还原：掩码条目按同名存量 server 还原；无存量 loud 拒绝。
        let existing = load_mcp_config(workspace)?;
        for server in config.servers.iter_mut() {
            if let Some(old) = existing.servers.iter().find(|s| s.name == server.name) {
                restore_masked_entries(&mut server.headers, &old.headers)?;
                restore_masked_entries(&mut server.env, &old.env)?;
            } else {
                restore_masked_entries(&mut server.headers, &[])?;
                restore_masked_entries(&mut server.env, &[])?;
            }
        }
        save_mcp_config(workspace, &config)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}

// Phase 3 覆盖率（2026-08-25）：servers 列表 legacy 归一化显示 +
// server.update 全可选字段 patch 臂（旧清单只盖 url/args/env/timeout）。
#[cfg(test)]
mod tests;
