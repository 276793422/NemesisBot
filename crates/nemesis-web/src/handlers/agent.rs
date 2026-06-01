//! Agent handler — status/start/stop with full config reload on start.

use crate::handlers::require_home;
use crate::ws_router::{ModuleHandler, RequestContext};
use std::collections::HashMap;
use std::sync::Arc;

pub struct AgentHandler;

#[async_trait::async_trait]
impl ModuleHandler for AgentHandler {
    fn module_name(&self) -> &str {
        "agent"
    }

    async fn handle_cmd(
        &self,
        cmd: &str,
        _data: Option<serde_json::Value>,
        ctx: &RequestContext,
    ) -> Result<Option<serde_json::Value>, String> {
        match cmd {
            "status" => self.status(ctx),
            "start" => self.start(ctx),
            "stop" => self.stop(ctx),
            _ => Err(format!("unknown command: agent.{}", cmd)),
        }
    }
}

impl AgentHandler {
    fn status(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        let running = ctx
            .state
            .agent_service
            .as_ref()
            .map(|s| s.is_running())
            .unwrap_or(false);
        let model_name = ctx.state.model_name.lock().clone();
        let model_base = ctx.state.model_base.lock().clone();
        let model_has_key = ctx.state.model_has_key.load(std::sync::atomic::Ordering::SeqCst);
        let session_count = ctx.state.session_count.load(std::sync::atomic::Ordering::SeqCst);

        Ok(Some(serde_json::json!({
            "running": running,
            "model_name": model_name,
            "model_base": model_base,
            "model_has_key": model_has_key,
            "active_sessions": session_count,
        })))
    }

    fn start(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        // Reload all configs before starting the loop.
        let home = require_home(ctx)?;
        if let Some(ref agent_loop) = ctx.state.agent_loop {
            if let Err(e) = reload_all_configs(&home, agent_loop, &ctx.state) {
                tracing::warn!(error = %e, "[Agent] Config reload failed, starting with existing config");
            }
        }

        match ctx.state.agent_service {
            Some(ref svc) => {
                svc.start()?;
                tracing::info!("[Agent] Started with fresh config");
                Ok(Some(serde_json::json!({ "started": true })))
            }
            None => Err("Agent not available".to_string()),
        }
    }

    fn stop(&self, ctx: &RequestContext) -> Result<Option<serde_json::Value>, String> {
        match ctx.state.agent_service {
            Some(ref svc) => {
                svc.stop()?;

                // Unload internal components.
                if let Some(ref agent_loop) = ctx.state.agent_loop {
                    agent_loop.unload_components();
                }

                tracing::info!("[Agent] Stopped and components unloaded");
                Ok(Some(serde_json::json!({ "stopped": true })))
            }
            None => Err("Agent not available".to_string()),
        }
    }
}

/// Reload all agent configurations from disk.
fn reload_all_configs(
    home: &str,
    agent_loop: &Arc<nemesis_agent::r#loop::AgentLoop>,
    state: &crate::api_handlers::AppState,
) -> Result<(), String> {
    let home_path = std::path::Path::new(home);

    // 1. Re-read config.json.
    let config_path = home_path.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| format!("failed to load config: {}", e))?;

    // 2. Resolve model.
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| format!("failed to resolve model: {}", e))?;

    // 3. Re-read system prompt from IDENTITY.md + SOUL.md.
    let workspace_dir = home_path.join("workspace");
    let system_prompt = if workspace_dir.exists() {
        let mut builder = nemesis_agent::context::ContextBuilder::new(&workspace_dir);
        let skills_dir = workspace_dir.join("skills");
        if skills_dir.exists() {
            builder.load_skills(&skills_dir);
        }
        let prompt = builder.build_system_prompt(false);
        if prompt.is_empty() { None } else { Some(prompt) }
    } else {
        None
    };

    // 4. Create new provider.
    let model_name = resolution.model_name.clone();
    let api_base = if resolution.api_base.is_empty() {
        nemesis_config::get_default_api_base(
            &nemesis_config::infer_provider_from_model(&model_name),
        ).to_string()
    } else {
        resolution.api_base.clone()
    };

    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: model_name.clone(),
        api_key: resolution.api_key.clone(),
        api_base: api_base.clone(),
        workspace: String::new(),
        connect_mode: resolution.connect_mode.clone(),
        account_id: String::new(),
        headers: HashMap::new(),
    };

    match nemesis_providers::factory::create_provider(&factory_cfg) {
        Ok(provider) => {
            let adapter = Arc::new(crate::handlers::models::ProviderAdapter::new(provider, model_name.clone()));
            agent_loop.set_provider_and_model(adapter, model_name.clone());
        }
        Err(e) => {
            tracing::warn!(error = %e, "[Agent] Failed to create provider during reload");
        }
    }

    // 5. Update system prompt and max turns.
    agent_loop.reload_system_prompt(system_prompt);
    agent_loop.reload_max_turns(cfg.agents.defaults.max_tool_iterations.max(1) as u32);

    // 6. Update AppState model tracking fields.
    *state.model_name.lock() = model_name.clone();
    *state.model_base.lock() = api_base.clone();
    state.model_has_key.store(!resolution.api_key.is_empty(), std::sync::atomic::Ordering::Release);

    // 7. Rebuild SharedToolConfig and tools.
    let shared_config = nemesis_agent::SharedToolConfig {
        workspace: Some(workspace_dir.to_string_lossy().to_string()),
        cron_service: state.cron_service.clone(),
        forge_executor: state.forge_executor.clone(),
        forge: state.forge.clone(),
        memory_executor: state.memory_manager.as_ref().map(|mgr| {
            Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr.clone()))
        }),
        skills_loader: state.skills_loader.clone(),
        skills_registry: state.skills_registry.clone(),
        mcp_tool_snapshot: Some(agent_loop.mcp_tool_snapshot()),
        ..Default::default()
    };

    let new_tools = nemesis_agent::register_shared_tools(&shared_config);
    agent_loop.reload_tools(new_tools);

    // 8. Reload security config.
    if let Some(ref plugin) = state.security_plugin {
        if let Err(e) = plugin.reload_config() {
            tracing::warn!(error = %e, "[Agent] Failed to reload security config");
        }
    }

    // 9. Reload Forge config.
    if let Some(ref forge) = state.forge {
        let forge_config_path = home_path.join("workspace").join("config").join("config.forge.json");
        let forge_config = if forge_config_path.exists() {
            nemesis_forge::config::load_forge_config(&forge_config_path)
        } else {
            nemesis_forge::config::ForgeConfig::default()
        };
        if forge.is_running() {
            let forge_clone = forge.clone();
            let config_clone = forge_config.clone();
            tokio::spawn(async move {
                forge_clone.stop().await;
                if config_clone.learning.enabled {
                    forge_clone.start().await;
                }
            });
        } else if forge_config.learning.enabled {
            let forge_clone = forge.clone();
            tokio::spawn(async move {
                forge_clone.start().await;
            });
        }
    }

    // 10. Reload cron jobs.
    if let Some(ref cron_svc) = state.cron_service {
        if let Ok(svc) = cron_svc.lock() {
            let _ = svc.reload();
        }
    }

    tracing::info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        "[Agent] All configs reloaded"
    );

    Ok(())
}
