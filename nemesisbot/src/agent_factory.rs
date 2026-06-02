//! Agent factory — builds a fresh AgentLoop from disk config.
//!
//! Extracted from gateway.rs to enable true stop/start semantics:
//! - stop = drop old AgentLoop
//! - start = call build_agent_loop() → fresh instance, identical to first boot

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::info;

use nemesis_web::{ForgeProviderBridge, ProviderAdapter};

use crate::common;

// ---------------------------------------------------------------------------
// SharedResources — infrastructure that survives Agent restart
// ---------------------------------------------------------------------------

/// Resources shared across AgentLoop stop/start cycles.
///
/// Created once in `gateway::run()`, passed to `build_agent_loop()` on each
/// start. All fields are either `Arc` references to long-lived infrastructure
/// or values that don't change between restarts.
pub struct SharedResources {
    pub home: PathBuf,
    #[allow(dead_code)] // Reserved for future use (e.g., bus subscription in factory)
    pub bus: Arc<nemesis_bus::MessageBus>,

    // Outbound channel — SharedResources holds the original Sender.
    // Factory clones it for each new AgentLoop.
    // When old AgentLoop is dropped, only the clone is dropped;
    // the original Sender stays alive → outbound bridge keeps running.
    pub agent_outbound_tx: tokio::sync::mpsc::Sender<nemesis_types::channel::OutboundMessage>,

    // Shared infrastructure Arc references (lifecycle independent of AgentLoop)
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    pub cron_service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    pub observer_manager: Option<Arc<nemesis_observer::Manager>>,
    pub data_store: Option<Arc<nemesis_data::DataStore>>,
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    pub memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    pub enabled_channels: Vec<String>,

    // Cluster RPC closure (Cluster itself is mem::forget'd, but rpc_call_fn must survive)
    pub cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(&str, &str, serde_json::Value)
                -> std::pin::Pin<
                    Box<
                        dyn std::future::Future<
                            Output = std::result::Result<serde_json::Value, String>,
                        > + Send,
                    >,
                > + Send
                + Sync,
        >,
    >,
    pub cluster_rpc_config: Option<nemesis_agent::loop_tools::ClusterRpcConfig>,

    // MCP config
    pub mcp_config_path: PathBuf,
    pub mcp_enabled: bool,
}

// ---------------------------------------------------------------------------
// build_agent_loop — factory function
// ---------------------------------------------------------------------------

/// Build a fresh AgentLoop from disk config.
///
/// Re-reads `config.json`, workspace files, creates new provider,
/// registers all tools — identical to first-boot initialization.
pub fn build_agent_loop(shared: &Arc<SharedResources>) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>> {
    use nemesis_agent::types::AgentConfig;
    use nemesis_agent::r#loop::{AgentLoop, ConcurrentMode};

    // 1. Re-read config.json from disk.
    let config_path = shared.home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    // 2. Resolve LLM model → create fresh provider.
    let llm_ref = nemesis_config::get_effective_llm(Some(&cfg));
    let resolution = nemesis_config::resolve_model_config(&cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;
    let model_name = resolution.model_name.clone();

    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key.clone(),
        api_base: resolution.api_base.clone(),
        workspace: shared.home.join("workspace").to_string_lossy().to_string(),
        connect_mode: resolution.connect_mode,
        account_id: String::new(),
        headers: HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;
    let provider_arc: Arc<dyn nemesis_providers::router::LLMProvider> = Arc::from(provider);
    info!("[AgentFactory] Provider created for {}", model_name);

    // 3. Build system prompt from workspace files (IDENTITY.md, SOUL.md, etc.)
    let workspace_dir = shared.home.join("workspace");
    let system_prompt = {
        let mut context_builder = nemesis_agent::context::ContextBuilder::new(&workspace_dir);
        let skills_dir = workspace_dir.join("skills");
        if skills_dir.exists() {
            context_builder.load_skills(&skills_dir);
        }
        context_builder.build_system_prompt(false)
    };
    info!(
        "[AgentFactory] System prompt built ({} chars)",
        system_prompt.len()
    );

    // 4. Create ProviderAdapter + AgentConfig + AgentLoop.
    let adapter = ProviderAdapter::new(provider_arc.clone(), model_name.clone());
    let agent_config = AgentConfig {
        model: model_name.clone(),
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        max_turns: cfg.agents.defaults.max_tool_iterations.max(1) as u32,
        tools: Vec::new(),
    };

    let mut agent_loop = AgentLoop::new_bus(
        Box::new(adapter),
        agent_config,
        shared.agent_outbound_tx.clone(),
        ConcurrentMode::Reject,
        8,
    );

    // 5. Session store (disk-persisted — new instance, same directory).
    {
        let sess_dir = common::sessions_dir(&shared.home);
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        agent_loop.set_session_store(store);
        info!(
            "[AgentFactory] Session store initialized: {}",
            sess_dir.display()
        );
    }

    // 6. Workspace state manager (disk-based — new instance).
    {
        let state_mgr =
            nemesis_state::workspace_state::WorkspaceStateManager::new(&workspace_dir);
        agent_loop.set_state_manager(state_mgr);
    }

    // 7. Build SharedToolConfig (from fresh config + shared Arc refs).
    let shared_config = nemesis_agent::SharedToolConfig {
        workspace: Some(workspace_dir.to_string_lossy().to_string()),
        cron_service: Some(shared.cron_service.clone()),
        forge_executor: shared.forge_executor.clone(),
        forge: shared.forge.clone(),
        memory_executor: shared.memory_manager.as_ref().map(|mgr| {
            Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr.clone()))
        }),
        skills_loader: shared.skills_loader.clone(),
        skills_registry: shared.skills_registry.clone(),
        // web_search: read from fresh config
        web_search: {
            let web = &cfg.tools.web;
            let any_enabled = web.brave.enabled || web.duckduckgo.enabled || web.perplexity.enabled;
            if any_enabled {
                Some(nemesis_agent::loop_tools::WebSearchConfig {
                    brave_api_key: if web.brave.api_key.is_empty() {
                        None
                    } else {
                        Some(web.brave.api_key.clone())
                    },
                    brave_max_results: web.brave.max_results.max(1) as usize,
                    brave_enabled: web.brave.enabled,
                    duckduckgo_max_results: web.duckduckgo.max_results.max(1) as usize,
                    duckduckgo_enabled: web.duckduckgo.enabled,
                    perplexity_api_key: if web.perplexity.api_key.is_empty() {
                        None
                    } else {
                        Some(web.perplexity.api_key.clone())
                    },
                    perplexity_max_results: web.perplexity.max_results.max(1) as usize,
                    perplexity_enabled: web.perplexity.enabled,
                })
            } else {
                None
            }
        },
        // spawn: enabled by default, uses current model
        spawn: Some(nemesis_agent::loop_tools::SpawnConfig {
            default_model: model_name.clone(),
            max_concurrent: 4,
        }),
        // cluster_rpc: None here — registered separately below
        cluster_rpc: None,
        mcp_tool_snapshot: Some(agent_loop.mcp_tool_snapshot()),
    };

    // 8. Register shared tools.
    let all_tools = nemesis_agent::register_shared_tools(&shared_config);
    let tool_count = all_tools.len();
    for (name, tool) in all_tools {
        agent_loop.register_tool(name, tool);
    }

    // 9. Enable MCP dynamic reload.
    if shared.mcp_enabled {
        agent_loop.enable_mcp_reload(shared.mcp_config_path.clone());
    }
    info!(
        "[AgentFactory] Tools registered: {}{}",
        tool_count,
        if shared.mcp_enabled { " + MCP" } else { "" }
    );

    // 10. Register ClusterRpcTool (using shared call_fn).
    if let (Some(config), Some(call_fn)) =
        (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
    {
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(config.clone());
        cluster_rpc_tool.set_rpc_call_fn(call_fn.clone());
        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("[AgentFactory] cluster_rpc tool registered");
    }

    // 11. Continuation manager (disk-persisted — new instance).
    {
        let cont_mgr = Arc::new(nemesis_agent::ContinuationManager::with_disk_store(
            &workspace_dir,
        ));
        agent_loop.set_continuation_manager(cont_mgr);
    }

    // 12. Inject shared Arc references.
    if let Some(ref forge) = shared.forge {
        agent_loop.set_forge(forge.clone());
    }
    if let Some(ref plugin) = shared.security_plugin {
        agent_loop.set_security_plugin(plugin.clone());
    }
    if let Some(ref mgr) = shared.observer_manager {
        agent_loop.set_observer_manager(mgr.clone());
    }
    if let Some(ref ds) = shared.data_store {
        agent_loop.set_data_store(ds.clone());
    }
    agent_loop.set_channel_manager(shared.enabled_channels.clone());

    // 13. Update Forge's LLM provider (old model may have been deleted).
    //     set_provider cascades to reflector + pipeline + learning_engine.
    if let Some(ref forge) = shared.forge {
        let bridge = ForgeProviderBridge::new(provider_arc.clone(), model_name.clone());
        forge.set_provider(Arc::new(bridge));
        info!(
            "[AgentFactory] Forge provider updated to model {}",
            model_name
        );
    }

    info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        "[AgentFactory] AgentLoop built successfully"
    );

    Ok(Arc::new(agent_loop))
}
