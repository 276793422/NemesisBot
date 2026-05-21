//! Models handler — list/add/delete/set_default/test model configurations.

use crate::handlers::{mask_sensitive, require_workspace};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;

pub struct ModelsHandler {
    _priv: (),
}

impl ModelsHandler {
    pub fn new() -> Self {
        Self { _priv: () }
    }
}

#[async_trait::async_trait]
impl ModuleHandler for ModelsHandler {
    fn module_name(&self) -> &str {
        "models"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let workspace = require_workspace(ctx)?;
        match cmd {
            "list" => self.list(workspace),
            "add" => {
                let data = data.ok_or("missing data")?;
                self.add(workspace, &data)
            }
            "delete" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.delete(workspace, &name)
            }
            "set_default" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.set_default(workspace, &name)
            }
            "test" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.test(workspace, &name)
            }
            _ => Err(format!("unknown command: models.{}", cmd)),
        }
    }
}

fn config_path(workspace: &str) -> PathBuf {
    PathBuf::from(workspace).join("config.json")
}

fn load_config(workspace: &str) -> Result<nemesis_config::Config, String> {
    let path = config_path(workspace);
    nemesis_config::load_config(&path).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config(workspace: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    let path = config_path(workspace);
    nemesis_config::save_config(&path, config).map_err(|e| format!("failed to save config: {}", e))
}

impl ModelsHandler {
    fn list(&self, workspace: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(workspace)?;
        let default_name = config.model_list.first().map(|m| m.model_name.clone());
        let models: Vec<_> = config
            .model_list
            .iter()
            .map(|m| {
                serde_json::json!({
                    "model_name": m.model_name,
                    "model": m.model,
                    "api_base": m.api_base,
                    "api_key": if m.api_key.is_empty() { String::new() } else { mask_sensitive(&m.api_key) },
                    "proxy": m.proxy,
                    "is_default": default_name.as_ref() == Some(&m.model_name),
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "models": models })))
    }

    fn add(&self, workspace: &str, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let model_name = crate::handlers::get_str(data, "name")?;
        let model = crate::handlers::get_str(data, "model")?;
        let api_key = crate::handlers::get_str(data, "key")?;
        let api_base = crate::handlers::get_opt_str(data, "base_url").unwrap_or_default();
        let proxy = crate::handlers::get_opt_str(data, "proxy").unwrap_or_default();

        let mut config = load_config(workspace)?;
        if config.model_list.iter().any(|m| m.model_name == model_name) {
            return Err(format!("model '{}' already exists", model_name));
        }

        config.model_list.push(nemesis_config::ModelConfig {
            model_name: model_name.clone(),
            model,
            api_base,
            api_key,
            proxy,
            auth_method: String::new(),
            connect_mode: String::new(),
            workspace: String::new(),
        });
        save_config(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "added": true, "name": model_name })))
    }

    fn delete(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(workspace)?;
        let before = config.model_list.len();
        config.model_list.retain(|m| m.model_name != name);
        if config.model_list.len() == before {
            return Err(format!("model '{}' not found", name));
        }
        save_config(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn set_default(&self, workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(workspace)?;
        let idx = config
            .model_list
            .iter()
            .position(|m| m.model_name == name)
            .ok_or_else(|| format!("model '{}' not found", name))?;
        let model = config.model_list.remove(idx);
        config.model_list.insert(0, model);
        save_config(workspace, &mut config)?;
        Ok(Some(serde_json::json!({ "set_default": true, "name": name })))
    }

    fn test(&self, _workspace: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        // Stub — actual model testing requires provider integration
        Ok(Some(serde_json::json!({
            "name": name,
            "status": "not_implemented",
            "message": "Model test requires provider integration"
        })))
    }
}
