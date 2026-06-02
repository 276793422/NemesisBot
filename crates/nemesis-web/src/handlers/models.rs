//! Models handler — list/add/delete/set_default/test model configurations.

use crate::handlers::{mask_sensitive, require_home};
use crate::llm_bridge::{ForgeProviderBridge, ProviderAdapter};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::path::PathBuf;
use std::sync::Arc;

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
        let home = require_home(ctx)?;
        match cmd {
            "list" => self.list(home),
            "add" => {
                let data = data.ok_or("missing data")?;
                self.add(home, &data)
            }
            "delete" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.delete(home, &name)
            }
            "set_default" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.set_default(home, &name, ctx)
            }
            "test" => {
                let data = data.ok_or("missing data")?;
                let name = crate::handlers::get_str(&data, "name")?;
                self.test(home, &name)
            }
            _ => Err(format!("unknown command: models.{}", cmd)),
        }
    }
}

fn config_path(home: &str) -> PathBuf {
    PathBuf::from(home).join("config.json")
}

fn load_config(home: &str) -> Result<nemesis_config::Config, String> {
    let path = config_path(home);
    nemesis_config::load_config(&path).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    let path = config_path(home);
    nemesis_config::save_config(&path, config).map_err(|e| format!("failed to save config: {}", e))
}

impl ModelsHandler {
    fn list(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
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

    fn add(&self, home: &str, data: &serde_json::Value) -> Result<Option<serde_json::Value>, String> {
        let model_name = crate::handlers::get_str(data, "name")?;
        let model = crate::handlers::get_str(data, "model")?;
        let api_key = crate::handlers::get_str(data, "key")?;
        let api_base = crate::handlers::get_opt_str(data, "base_url").unwrap_or_default();
        let proxy = crate::handlers::get_opt_str(data, "proxy").unwrap_or_default();

        let mut config = load_config(home)?;
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
        save_config(home, &mut config)?;
        Ok(Some(serde_json::json!({ "added": true, "name": model_name })))
    }

    fn delete(&self, home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(home)?;
        let before = config.model_list.len();
        config.model_list.retain(|m| m.model_name != name);
        if config.model_list.len() == before {
            return Err(format!("model '{}' not found", name));
        }
        save_config(home, &mut config)?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn set_default(&self, home: &str, name: &str, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(home)?;
        let idx = config
            .model_list
            .iter()
            .position(|m| m.model_name == name)
            .ok_or_else(|| format!("model '{}' not found", name))?;
        let model_cfg = config.model_list.remove(idx);
        config.model_list.insert(0, model_cfg.clone());
        save_config(home, &mut config)?;

        // Swap the runtime provider so the change takes effect immediately.
        if let Some(ref agent_loop) = ctx.state.agent_loop.read().as_ref() {
            let api_base = if model_cfg.api_base.is_empty() {
                nemesis_config::get_default_api_base(
                    &nemesis_config::infer_provider_from_model(&model_cfg.model)
                ).to_string()
            } else {
                model_cfg.api_base.clone()
            };

            let factory_cfg = nemesis_providers::factory::FactoryConfig {
                llm_ref: model_cfg.model.clone(),
                api_key: model_cfg.api_key.clone(),
                api_base,
                workspace: String::new(),
                connect_mode: model_cfg.connect_mode.clone(),
                account_id: String::new(),
                headers: std::collections::HashMap::new(),
            };
            match nemesis_providers::factory::create_provider(&factory_cfg) {
                Ok(provider) => {
                    let adapter = Arc::new(ProviderAdapter::new(provider.clone(), model_cfg.model.clone()));
                    agent_loop.set_provider_and_model(adapter, model_cfg.model.clone());
                    tracing::info!(model = %model_cfg.model, "[Models] Runtime provider swapped");

                    // Sync Forge's LLM provider — set_provider cascades to all subsystems.
                    if let Some(ref forge) = ctx.state.forge {
                        let bridge = ForgeProviderBridge::new(provider.clone(), model_cfg.model.clone());
                        forge.set_provider(Arc::new(bridge));
                        tracing::info!(model = %model_cfg.model, "[Models] Forge provider updated");
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Models] Failed to create provider for runtime swap, config saved anyway");
                }
            }
        }

        Ok(Some(serde_json::json!({ "set_default": true, "name": name })))
    }

    fn test(&self, _home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        // Stub — actual model testing requires provider integration
        Ok(Some(serde_json::json!({
            "name": name,
            "status": "not_implemented",
            "message": "Model test requires provider integration"
        })))
    }
}
