//! Security handler — config.get/config.save/audit/stats.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct SecurityHandler {
    _priv: (),
}

impl SecurityHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for SecurityHandler {
    fn module_name(&self) -> &str {
        "security"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "config.get" => self.config_get(workspace),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            "audit" => {
                let limit = data
                    .as_ref()
                    .and_then(|d| d.get("limit"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(50) as usize;
                let offset = data
                    .as_ref()
                    .and_then(|d| d.get("offset"))
                    .and_then(|v| v.as_u64())
                    .unwrap_or(0) as usize;
                self.audit(workspace, limit, offset)
            }
            "stats" => self.stats(workspace),
            _ => Err(format!("unknown command: security.{}", cmd)),
        }
    }
}

fn security_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.security.json")
}

fn security_log_dir(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("logs/security_logs")
}

impl SecurityHandler {
    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = security_config_path(workspace);
        let config = nemesis_config::load_security_config(&path)
            .map_err(|e| format!("failed to load security config: {}", e))?;
        let json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: nemesis_config::SecurityConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid security config: {}", e))?;
        let path = security_config_path(workspace);
        nemesis_config::save_security_config(&path, &config)
            .map_err(|e| format!("failed to save security config: {}", e))?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn audit(
        &self,
        workspace: &str,
        limit: usize,
        offset: usize,
    ) -> Result<Option<serde_json::Value>, String> {
        let log_dir = security_log_dir(workspace);
        if !log_dir.exists() {
            return Ok(Some(serde_json::json!({ "entries": [], "total": 0 })));
        }

        let mut entries = Vec::new();
        let read_dir = std::fs::read_dir(&log_dir)
            .map_err(|e| format!("failed to read security log dir: {}", e))?;

        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines().rev() {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                            entries.push(val);
                        }
                    }
                }
            }
        }

        // Sort by timestamp descending (if available)
        entries.sort_by(|a, b| {
            let ts_a = a.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            let ts_b = b.get("timestamp").and_then(|v| v.as_str()).unwrap_or("");
            ts_b.cmp(ts_a)
        });

        let total = entries.len();
        let page: Vec<_> = entries.into_iter().skip(offset).take(limit).collect();
        Ok(Some(serde_json::json!({
            "entries": page,
            "total": total,
            "limit": limit,
            "offset": offset,
        })))
    }

    fn stats(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let log_dir = security_log_dir(workspace);
        if !log_dir.exists() {
            return Ok(Some(serde_json::json!({
                "total_events": 0,
                "by_level": {},
            })));
        }

        let mut total = 0usize;
        let mut by_level: std::collections::HashMap<String, usize> = std::collections::HashMap::new();

        let read_dir = std::fs::read_dir(&log_dir)
            .map_err(|e| format!("failed to read dir: {}", e))?;
        for entry in read_dir {
            let entry = entry.map_err(|e| format!("failed to read entry: {}", e))?;
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) == Some("jsonl") {
                if let Ok(content) = std::fs::read_to_string(&path) {
                    for line in content.lines() {
                        if let Ok(val) = serde_json::from_str::<serde_json::Value>(line) {
                            total += 1;
                            let level = val
                                .get("risk_level")
                                .or_else(|| val.get("level"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("unknown")
                                .to_string();
                            *by_level.entry(level).or_insert(0) += 1;
                        }
                    }
                }
            }
        }

        Ok(Some(serde_json::json!({
            "total_events": total,
            "by_level": by_level,
        })))
    }
}
