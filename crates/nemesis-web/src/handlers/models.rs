//! Models handler — list/add/delete/set_default/test model configurations.

use crate::handlers::{mask_sensitive, require_home};
use crate::ws_router::{ModuleHandler, RequestContext};
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use nemesis_agent::r#loop::{LlmMessage, LlmProvider, LlmResponse};
use nemesis_agent::types::ToolCallInfo as AgentToolCallInfo;

/// Wraps a `nemesis_providers::LLMProvider` so it satisfies the agent's `LlmProvider` trait.
pub(crate) struct ProviderAdapter {
    inner: Arc<dyn nemesis_providers::router::LLMProvider>,
    default_model: String,
}

impl ProviderAdapter {
    pub(crate) fn new(inner: Arc<dyn nemesis_providers::router::LLMProvider>, default_model: String) -> Self {
        Self { inner, default_model }
    }
}

#[async_trait::async_trait]
impl LlmProvider for ProviderAdapter {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let model_to_use = if model.is_empty() { &self.default_model } else { model };

        let provider_messages: Vec<nemesis_providers::types::Message> = messages
            .into_iter()
            .map(|m| nemesis_providers::types::Message {
                role: m.role,
                content: m.content,
                tool_calls: m.tool_calls.unwrap_or_default().into_iter().map(|tc| {
                    nemesis_providers::types::ToolCall {
                        id: tc.id,
                        call_type: Some("function".to_string()),
                        function: Some(nemesis_providers::types::FunctionCall {
                            name: tc.name,
                            arguments: tc.arguments,
                        }),
                        name: None,
                        arguments: None,
                    }
                }).collect(),
                tool_call_id: m.tool_call_id,
                timestamp: None,
                reasoning_content: m.reasoning_content,
                extra: std::collections::HashMap::new(),
            })
            .collect();

        let provider_options = match options {
            Some(opts) => nemesis_providers::types::ChatOptions {
                temperature: opts.temperature.map(|t| t as f64),
                max_tokens: opts.max_tokens.map(|t| t as i64),
                top_p: opts.top_p.map(|p| p as f64),
                stop: opts.stop,
                extra: std::collections::HashMap::new(),
            },
            None => nemesis_providers::types::ChatOptions {
                temperature: Some(0.7),
                max_tokens: Some(8192),
                top_p: None,
                stop: None,
                extra: std::collections::HashMap::new(),
            },
        };

        let provider_tools: Vec<nemesis_providers::types::ToolDefinition> = tools
            .into_iter()
            .map(|t| nemesis_providers::types::ToolDefinition {
                tool_type: t.tool_type,
                function: nemesis_providers::types::ToolFunctionDefinition {
                    name: t.function.name,
                    description: t.function.description,
                    parameters: t.function.parameters,
                },
            })
            .collect();

        match self.inner.chat(&provider_messages, &provider_tools, model_to_use, &provider_options).await {
            Ok(resp) => {
                let tool_calls: Vec<AgentToolCallInfo> = resp.tool_calls
                    .into_iter()
                    .filter_map(|tc| {
                        let func = tc.function?;
                        Some(AgentToolCallInfo {
                            id: tc.id,
                            name: func.name,
                            arguments: func.arguments,
                        })
                    })
                    .collect();
                let finished = tool_calls.is_empty() || resp.finish_reason == "stop";
                Ok(LlmResponse {
                    content: resp.content,
                    tool_calls,
                    finished,
                    reasoning_content: resp.reasoning_content,
                    usage: resp.usage.map(|u| nemesis_agent::loop_executor::ObserverUsageInfo {
                        prompt_tokens: u.prompt_tokens,
                        completion_tokens: u.completion_tokens,
                        total_tokens: u.total_tokens,
                        cached_tokens: u.cached_tokens,
                        cache_creation_tokens: u.cache_creation_tokens,
                        cache_read_tokens: u.cache_read_tokens,
                    }),
                    raw_request_body: resp.raw_request_body,
                    raw_response_body: resp.raw_response_body,
                })
            }
            Err(e) => Err(format!("{}", e)),
        }
    }
}

/// Async LLM provider adapter for Forge's Reflector + Pipeline.
///
/// Duplicated from `agent_factory.rs` because nemesis-web cannot import nemesisbot.
pub(crate) struct ForgeProviderBridge {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

impl ForgeProviderBridge {
    pub(crate) fn new(provider: Arc<dyn nemesis_providers::router::LLMProvider>, model: String) -> Self {
        Self { provider, model }
    }
}

#[async_trait::async_trait]
impl nemesis_forge::reflector_llm::LLMCaller for ForgeProviderBridge {
    async fn chat(
        &self,
        system_prompt: &str,
        user_prompt: &str,
        max_tokens: Option<i64>,
    ) -> std::result::Result<String, String> {
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".to_string(),
                content: system_prompt.to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".to_string(),
                content: user_prompt.to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
        ];

        let options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.7),
            max_tokens,
            top_p: None,
            stop: None,
            extra: HashMap::new(),
        };

        let response = self
            .provider
            .chat(&messages, &[], &self.model, &options)
            .await
            .map_err(|e| format!("{:?}", e))?;

        if response.content.is_empty() && response.tool_calls.is_empty() {
            Err("LLM returned no content".to_string())
        } else {
            Ok(response.content)
        }
    }
}

/// Sync LLM provider adapter for Forge's LearningEngine.
///
/// Duplicated from `agent_factory.rs` because nemesis-web cannot import nemesisbot.
pub(crate) struct ForgeLearningProvider {
    provider: Arc<dyn nemesis_providers::router::LLMProvider>,
    model: String,
}

impl ForgeLearningProvider {
    pub(crate) fn new(provider: Arc<dyn nemesis_providers::router::LLMProvider>, model: String) -> Self {
        Self { provider, model }
    }
}

impl nemesis_forge::learning_engine::LLMProvider for ForgeLearningProvider {
    fn chat(
        &self,
        system: &str,
        user: &str,
        max_tokens: u32,
    ) -> std::result::Result<String, String> {
        let messages = vec![
            nemesis_providers::types::Message {
                role: "system".to_string(),
                content: system.to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
            nemesis_providers::types::Message {
                role: "user".to_string(),
                content: user.to_string(),
                tool_calls: vec![],
                tool_call_id: None,
                timestamp: None,
                reasoning_content: None,
                extra: HashMap::new(),
            },
        ];

        let options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(max_tokens as i64),
            top_p: None,
            stop: None,
            extra: HashMap::new(),
        };

        let future = self
            .provider
            .chat(&messages, &[], &self.model, &options);

        let response = match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                tokio::task::block_in_place(|| handle.block_on(future))
            }
            Err(_) => {
                let rt = tokio::runtime::Runtime::new()
                    .map_err(|e| format!("failed to create runtime: {}", e))?;
                rt.block_on(future)
            }
        }
        .map_err(|e| format!("{:?}", e))?;

        if response.content.is_empty() {
            Err("LLM returned no content".to_string())
        } else {
            Ok(response.content)
        }
    }
}

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

                    // Sync Forge's LLM provider — old model may have been deleted.
                    if let Some(ref forge) = ctx.state.forge {
                        let bridge = ForgeProviderBridge::new(provider.clone(), model_cfg.model.clone());
                        forge.set_provider(Arc::new(bridge));
                        tracing::info!(model = %model_cfg.model, "[Models] Forge provider updated");

                        if let Some(le) = forge.learning_engine() {
                            le.set_provider(Arc::new(ForgeLearningProvider::new(provider, model_cfg.model.clone())));
                            tracing::info!(model = %model_cfg.model, "[Models] Forge learning engine provider updated");
                        }
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
