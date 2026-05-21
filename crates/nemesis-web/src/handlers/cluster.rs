//! Cluster handler — status/config.get/config.save/peers.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ClusterHandler {
    _priv: (),
}

impl ClusterHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ClusterHandler {
    fn module_name(&self) -> &str {
        "cluster"
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
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            "peers" => self.peers(workspace),
            _ => Err(format!("unknown command: cluster.{}", cmd)),
        }
    }
}

fn cluster_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.cluster.json")
}

fn peers_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("cluster/peers.toml")
}

impl ClusterHandler {
    fn status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config_path = cluster_config_path(workspace);
        let config = if config_path.exists() {
            let content = std::fs::read_to_string(&config_path)
                .map_err(|e| format!("failed to read cluster config: {}", e))?;
            serde_json::from_str::<serde_json::Value>(&content).ok()
        } else {
            None
        };

        let peers_path = peers_path(workspace);
        let mut peers_count: usize = 0;
        let mut node_role: Option<String> = None;
        let mut node_name: Option<String> = None;
        if peers_path.exists() {
            let content = std::fs::read_to_string(&peers_path).unwrap_or_default();
            // Count [peers.xxx] sections only
            peers_count = content
                .lines()
                .filter(|l| l.starts_with("[peers.") && l.ends_with(']'))
                .count();
            // Extract node role and name from [node] section
            let mut in_node = false;
            for line in content.lines() {
                if line.trim() == "[node]" {
                    in_node = true;
                    continue;
                }
                if in_node {
                    if line.starts_with('[') {
                        break; // next section
                    }
                    if let Some(val) = line.strip_prefix("role") {
                        let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
                        if !val.is_empty() {
                            node_role = Some(val.to_string());
                        }
                    }
                    if let Some(val) = line.strip_prefix("name") {
                        let val = val.trim().trim_start_matches('=').trim().trim_matches('"');
                        if !val.is_empty() {
                            node_name = Some(val.to_string());
                        }
                    }
                }
            }
        }

        Ok(Some(serde_json::json!({
            "config": config,
            "peers_count": peers_count,
            "config_exists": config_path.exists(),
            "role": node_role,
            "node_name": node_name,
        })))
    }

    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = cluster_config_path(workspace);
        if !path.exists() {
            return Ok(Some(serde_json::json!({})));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read cluster config: {}", e))?;
        let config: serde_json::Value = serde_json::from_str(&content)
            .map_err(|e| format!("invalid cluster config: {}", e))?;
        Ok(Some(config))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let path = cluster_config_path(workspace);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("failed to create config dir: {}", e))?;
        }
        let json = serde_json::to_string_pretty(data)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        std::fs::write(&path, json)
            .map_err(|e| format!("failed to write cluster config: {}", e))?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn peers(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = peers_path(workspace);
        if !path.exists() {
            return Ok(Some(serde_json::json!({ "peers": [] })));
        }
        let content = std::fs::read_to_string(&path)
            .map_err(|e| format!("failed to read peers.toml: {}", e))?;

        // Parse peers.toml into a simple structure
        // The file format is TOML, return as raw content for now
        Ok(Some(serde_json::json!({
            "peers": content,
            "format": "toml",
        })))
    }
}
