//! Models handler — list/add/delete/set_default/test model configurations.

use crate::handlers::{mask_sensitive, require_home};
#[cfg(feature = "forge")]
use crate::llm_bridge::ForgeProviderBridge;
use crate::llm_bridge::ProviderAdapter;
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
    if let Some(cfg) = nemesis_config::load_live() {
        return Ok(cfg);
    }
    let path = config_path(home);
    nemesis_config::load_config(&path).map_err(|e| format!("failed to load config: {}", e))
}

fn save_config(home: &str, config: &mut nemesis_config::Config) -> Result<(), String> {
    if let Some(r) = nemesis_config::save_live(config.clone()) {
        return r.map_err(|e| format!("failed to save config: {}", e));
    }
    let path = config_path(home);
    nemesis_config::save_config(&path, config).map_err(|e| format!("failed to save config: {}", e))
}

impl ModelsHandler {
    fn list(&self, home: &str) -> Result<Option<serde_json::Value>, String> {
        let config = load_config(home)?;
        // 默认模型以 agents.defaults.llm 为权威（启动 get_effective_llm 读的就是它），
        // 不能用 model_list[0] 位置判——CLI 的 `model add --default` 把新模型追加到
        // 末尾、只改 agents.defaults.llm，位置判会把旧模型误标为默认，dashboard 就
        // 显示错了。default_llm 可能是 model_name / vendor/model 串 / 别名（CLI 设别名）。
        let default_llm = config.agents.defaults.llm.clone();
        let first_name = config
            .model_list
            .first()
            .map(|m| m.model_name.clone())
            .unwrap_or_default();
        let models: Vec<_> = config
            .model_list
            .iter()
            .map(|m| {
                let alias = m.model.split('/').next_back().unwrap_or("");
                let is_default = if default_llm.is_empty() {
                    // 回退：老配置没显式默认时，沿用 list[0] 位置默认，保持旧行为。
                    m.model_name == first_name
                } else {
                    m.model_name == default_llm
                        || m.model == default_llm
                        || (!alias.is_empty() && alias == default_llm)
                };
                serde_json::json!({
                    "model_name": m.model_name,
                    "model": m.model,
                    "api_base": m.api_base,
                    "api_key": if m.api_key.is_empty() { String::new() } else { mask_sensitive(&m.api_key) },
                    "proxy": m.proxy,
                    "is_default": is_default,
                })
            })
            .collect();
        Ok(Some(serde_json::json!({ "models": models })))
    }

    fn add(
        &self,
        home: &str,
        data: &serde_json::Value,
    ) -> Result<Option<serde_json::Value>, String> {
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
        Ok(Some(
            serde_json::json!({ "added": true, "name": model_name }),
        ))
    }

    fn delete(&self, home: &str, name: &str) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(home)?;

        // 守卫：禁止删除当前默认模型。否则 agents.defaults.llm 变成悬空引用，
        // 下次启动 get_effective_llm → resolve_model_config 找不到模型直接失败。
        // default_llm 可能是 model_name、vendor/model 串或别名（CLI 设的是别名），
        // 故把目标模型的所有标识都拿来比对。同时兜住 list[0] 这个 dashboard 位置默认。
        let default_llm = config.agents.defaults.llm.clone();
        let first_name = config
            .model_list
            .first()
            .map(|m| m.model_name.as_str())
            .unwrap_or("")
            .to_string();
        if let Some(m) = config.model_list.iter().find(|m| m.model_name == name) {
            let alias = m.model.split('/').next_back().unwrap_or("");
            let is_default = name == default_llm
                || name == first_name
                || m.model == default_llm
                || (!alias.is_empty() && alias == default_llm);
            if is_default {
                return Err(format!(
                    "cannot delete default model '{}'. Switch the default to another model first.",
                    name
                ));
            }
        }

        let before = config.model_list.len();
        config.model_list.retain(|m| m.model_name != name);
        if config.model_list.len() == before {
            return Err(format!("model '{}' not found", name));
        }
        save_config(home, &mut config)?;
        Ok(Some(serde_json::json!({ "deleted": true, "name": name })))
    }

    fn set_default(
        &self,
        home: &str,
        name: &str,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        let mut config = load_config(home)?;
        let idx = config
            .model_list
            .iter()
            .position(|m| m.model_name == name)
            .ok_or_else(|| format!("model '{}' not found", name))?;
        let model_cfg = config.model_list.remove(idx);
        config.model_list.insert(0, model_cfg.clone());
        // 同步 agents.defaults.llm：启动时 get_effective_llm 只读这个字段、不看
        // model_list 顺序。不写这行，dashboard 切模型只在运行时生效（provider 已换），
        // 重启后回退到旧模型；若旧模型随后被删，启动会因 "model not found" 失败。
        config.agents.defaults.llm = model_cfg.model_name.clone();
        save_config(home, &mut config)?;

        // Swap the runtime provider so the change takes effect immediately.
        if let Some(ref agent_loop) = ctx.state.agent_loop.read().as_ref() {
            let api_base = if model_cfg.api_base.is_empty() {
                nemesis_config::get_default_api_base(&nemesis_config::infer_provider_from_model(
                    &model_cfg.model,
                ))
                .to_string()
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
                    let adapter = Arc::new(ProviderAdapter::new(
                        provider.clone(),
                        model_cfg.model.clone(),
                    ));
                    agent_loop.set_provider_and_model(adapter, model_cfg.model.clone());
                    tracing::info!(model = %model_cfg.model, "[Models] Runtime provider swapped");

                    // Sync Forge's LLM provider — set_provider cascades to all subsystems.
                    #[cfg(feature = "forge")]
                    {
                        if let Some(ref forge) = ctx.state.forge {
                            let bridge =
                                ForgeProviderBridge::new(provider.clone(), model_cfg.model.clone());
                            forge.set_provider(Arc::new(bridge));
                            tracing::info!(model = %model_cfg.model, "[Models] Forge provider updated");
                        }
                    }
                }
                Err(e) => {
                    tracing::warn!(error = %e, "[Models] Failed to create provider for runtime swap, config saved anyway");
                }
            }
        }

        Ok(Some(
            serde_json::json!({ "set_default": true, "name": name }),
        ))
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
