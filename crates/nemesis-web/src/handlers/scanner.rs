//! Scanner handler — config.get/config.save for scanner configuration.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ScannerHandler {
    _priv: (),
}

impl ScannerHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ScannerHandler {
    fn module_name(&self) -> &str {
        "scanner"
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
            _ => Err(format!("unknown command: scanner.{}", cmd)),
        }
    }
}

fn scanner_config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config/config.scanner.json")
}

impl ScannerHandler {
    fn config_get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let path = scanner_config_path(workspace);
        let config = nemesis_config::load_scanner_config(&path)
            .map_err(|e| format!("failed to load scanner config: {}", e))?;
        let json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize: {}", e))?;
        Ok(Some(json))
    }

    fn config_save(
        &self,
        workspace: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let config: nemesis_config::ScannerFullConfig = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid scanner config: {}", e))?;
        let path = scanner_config_path(workspace);
        nemesis_config::save_scanner_config(&path, &config)
            .map_err(|e| format!("failed to save scanner config: {}", e))?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }
}
