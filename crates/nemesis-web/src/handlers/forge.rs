//! Forge handler — status/artifacts/reflect/config.save.

use crate::handlers::{list_workspace_dir, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ForgeHandler {
    _priv: (),
}

impl ForgeHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ForgeHandler {
    fn module_name(&self) -> &str {
        "forge"
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
            "artifacts" => self.artifacts(workspace),
            "reflect" => self.reflect(),
            "config.save" => {
                let data = data.ok_or("missing data")?;
                self.config_save(workspace, &data)
            }
            _ => Err(format!("unknown command: forge.{}", cmd)),
        }
    }
}

fn config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config.json")
}

fn load_config(workspace: &str) -> Result<nemesis_config::Config, String> {
    nemesis_config::load_config(&config_path(workspace)).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config_to_disk(workspace: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    nemesis_config::save_config(&config_path(workspace), config).map_err(|e| format!("failed to save config: {}", e))
}

impl ForgeHandler {
    fn status(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(workspace)?;
        let enabled = config.forge.as_ref().map(|f| f.enabled).unwrap_or(false);

        let forge_dir = PathBuf::from(workspace).join("forge");
        let artifacts_count = if forge_dir.exists() {
            std::fs::read_dir(&forge_dir)
                .map(|rd| rd.count())
                .unwrap_or(0)
        } else {
            0
        };

        Ok(Some(serde_json::json!({
            "enabled": enabled,
            "artifacts_count": artifacts_count,
            "forge_dir_exists": forge_dir.exists(),
        })))
    }

    fn artifacts(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let forge_dir = PathBuf::from(workspace).join("forge");
        if !forge_dir.exists() {
            return Ok(Some(serde_json::json!({ "artifacts": [] })));
        }

        let mut artifacts = Vec::new();
        let entries = list_workspace_dir(workspace, "forge")?;
        for name in entries {
            let entry_path = forge_dir.join(&name);
            if entry_path.is_dir() {
                artifacts.push(serde_json::json!({
                    "name": name,
                    "type": "directory",
                }));
            } else {
                let size = entry_path.metadata().map(|m| m.len()).unwrap_or(0);
                artifacts.push(serde_json::json!({
                    "name": name,
                    "type": "file",
                    "size": size,
                }));
            }
        }

        Ok(Some(serde_json::json!({ "artifacts": artifacts })))
    }

    fn reflect(&self) -> Result<Option<serde_json::Value>, String> {
        // Stub — requires Forge runtime integration
        Ok(Some(serde_json::json!({
            "triggered": false,
            "message": "Forge reflect requires runtime integration"
        })))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let enabled = data
            .get("enabled")
            .and_then(|v| v.as_bool())
            .ok_or("missing or invalid 'enabled' field")?;

        let mut config = load_config(workspace)?;
        let forge = config.forge.get_or_insert_with(Default::default);
        forge.enabled = enabled;
        save_config_to_disk(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "saved": true, "enabled": enabled })))
    }
}
