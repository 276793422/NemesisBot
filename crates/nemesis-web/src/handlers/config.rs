//! Config handler — get/save/set_field config.json + CORS commands.

use crate::handlers::require_workspace;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ConfigHandler {
    _priv: (),
}

impl ConfigHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ConfigHandler {
    fn module_name(&self) -> &str {
        "config"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "get" => self.get(workspace),
            "save" => {
                let data = data.ok_or("missing data")?;
                self.save(workspace, &data)
            }
            "set_field" => {
                let data = data.ok_or("missing data")?;
                let path = crate::handlers::get_str(&data, "path")?;
                let value = data.get("value").cloned().unwrap_or(serde_json::Value::Null);
                self.set_field(workspace, &path, &value)
            }
            "cors.list" => self.cors_list(),
            "cors.add" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_add(&origin)
            }
            "cors.remove" => {
                let data = data.ok_or("missing data")?;
                let origin = crate::handlers::get_str(&data, "origin")?;
                self.cors_remove(&origin)
            }
            "cors.toggle" => {
                let data = data.ok_or("missing data")?;
                let enabled = data
                    .get("enabled")
                    .and_then(|v| v.as_bool())
                    .ok_or("missing or invalid 'enabled' field")?;
                self.cors_toggle(enabled)
            }
            _ => Err(format!("unknown command: config.{}", cmd)),
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

impl ConfigHandler {
    fn get(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(workspace)?;
        let mut json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;
        // Mask sensitive fields
        sanitize_config(&mut json);
        Ok(Some(json))
    }

    fn save(&self, workspace: &str, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let mut config: nemesis_config::Config = serde_json::from_value(data.clone())
            .map_err(|e| format!("invalid config data: {}", e))?;
        save_config_to_disk(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "saved": true })))
    }

    fn set_field(
        &self,
        workspace: &str,
        path: &str,
        value: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(workspace)?;
        let mut json = serde_json::to_value(&config)
            .map_err(|e| format!("failed to serialize config: {}", e))?;

        set_json_path(&mut json, path, value.clone())?;

        config = serde_json::from_value(json)
            .map_err(|e| format!("invalid config after field update: {}", e))?;
        save_config_to_disk(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "updated": true, "path": path })))
    }

    fn cors_list(&self) -> Result<Option<serde_json::Value>, String> {
        // CORSManager access would need to go through AppState
        // For now, read from cors.json directly
        Ok(Some(serde_json::json!({ "origins": [], "message": "CORS manager not connected" })))
    }

    fn cors_add(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({ "added": false, "message": "CORS manager not connected" })))
    }

    fn cors_remove(&self, _origin: &str) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({ "removed": false, "message": "CORS manager not connected" })))
    }

    fn cors_toggle(&self, _enabled: bool) -> Result<Option<serde_json::Value>, String> {
        Ok(Some(serde_json::json!({ "toggled": false, "message": "CORS manager not connected" })))
    }
}

/// Mask sensitive fields in a config JSON object.
fn sanitize_config(json: &mut serde_json::Value) {
    if let Some(obj) = json.as_object_mut() {
        for (key, value) in obj.iter_mut() {
            if crate::handlers::is_sensitive_field(key) {
                if let Some(s) = value.as_str() {
                    if !s.is_empty() {
                        *value = serde_json::Value::String(crate::handlers::mask_sensitive(s));
                    }
                }
            } else {
                sanitize_config(value);
            }
        }
    } else if let Some(arr) = json.as_array_mut() {
        for item in arr.iter_mut() {
            sanitize_config(item);
        }
    }
}

/// Set a value at a dot-separated path in a JSON object.
fn set_json_path(
    json: &mut serde_json::Value,
    path: &str,
    value: serde_json::Value,
) -> Result<(), String> {
    if path.is_empty() {
        return Err("empty path".to_string());
    }
    let parts: Vec<&str> = path.split('.').collect();
    let mut current = json;
    for (i, part) in parts.iter().enumerate() {
        if i == parts.len() - 1 {
            current[part] = value;
            return Ok(());
        }
        if current[part].is_null() {
            current[part] = serde_json::json!({});
        }
        current = &mut current[part];
    }
    Ok(())
}
