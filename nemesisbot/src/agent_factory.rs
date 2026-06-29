//! Agent factory — builds AgentLoop instances from shared configuration.
//!
//! Two factory functions:
//! - `build_agent_loop()` — main agent (bus mode, session store, continuation manager, etc.)
//! - `build_cluster_agent_loop()` — cluster agent (standalone mode, full tools, no bus)
//!
//! Both share the same tool registration and MCP logic via `register_tools_and_mcp()`.
//! The difference is in mode (bus vs standalone) and which optional subsystems are attached.

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
    /// Workflow engine reference — when set, registers the `workflow_run`
    /// agent tool. None keeps the tool absent (e.g., during tests).
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    /// Approval manager slot, filled by the gateway after the agent loop is
    /// built. Lets `skill_manage` request interactive approval when enabled.
    pub approval_slot: nemesis_agent::loop_tools::ApprovalManagerSlot,

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
    /// Returns online peers for dynamic cluster_rpc tool description.
    /// Each tuple: (node_id, node_name, capabilities).
    pub cluster_peers_fn: Option<Arc<dyn Fn() -> Vec<(String, String, Vec<String>)> + Send + Sync>>,
    /// Shared enabled flag for ClusterRpcTool.
    /// Set by factory when tool is registered, toggled by ClusterServiceAdapter
    /// to enable/disable the tool without removing it from the prompt.
    pub cluster_rpc_enabled: parking_lot::RwLock<Option<Arc<std::sync::atomic::AtomicBool>>>,

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

    let max_continuation_permits = cfg.agents.defaults.max_continuation_permits.max(0) as usize;
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(adapter),
        agent_config,
        shared.agent_outbound_tx.clone(),
        ConcurrentMode::Reject,
        8,
        max_continuation_permits,
    );

    // 5. Session store (disk-persisted — new instance, same directory).
    {
        let sess_dir = common::sessions_dir(&shared.home);
        let store = Arc::new(nemesis_agent::session::SessionStore::new_with_storage(
            &sess_dir,
        ));
        // Startup cleanup: remove sessions older than 7 days.
        let deleted = store.cleanup_old_sessions(7);
        if deleted > 0 {
            info!(
                deleted,
                "[AgentFactory] Main SessionStore startup cleanup (TTL=7d)"
            );
        }
        // Daily midnight cleanup. Spawns a task that sleeps until the next local
        // midnight, then runs cleanup_old_sessions(7), and loops forever.
        // Best-effort: if the runtime shuts down, the task is cancelled.
        spawn_daily_cleanup(store.clone(), "Main");
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

    // 7. Build tool config + register all tools + enable MCP.
    let tool_config = build_shared_tool_config(
        shared,
        &cfg,
        &model_name,
        Some(agent_loop.mcp_tool_snapshot()),
    );
    register_tools_and_mcp(&mut agent_loop, shared, &tool_config);

    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (P2: agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    if let Some(ref exec) = tool_config.memory_executor {
        agent_loop.set_memory_executor(exec.clone());
    }

    // 8. Register ClusterRpcTool (using shared call_fn + peers_fn).
    if let (Some(config), Some(call_fn)) =
        (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
    {
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(config.clone());
        cluster_rpc_tool.set_rpc_call_fn(call_fn.clone());
        if let Some(ref peers_fn) = shared.cluster_peers_fn {
            cluster_rpc_tool.set_peers_fn(peers_fn.clone());
        }
        cluster_rpc_tool.set_enabled(true);
        // Store the enabled flag for dynamic cluster start/stop.
        *shared.cluster_rpc_enabled.write() = Some(cluster_rpc_tool.enabled_arc());
        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("[AgentFactory] cluster_rpc tool registered (enabled=true)");
    }

    // 9. Continuation manager (disk-persisted — new instance).
    {
        let cont_mgr = Arc::new(nemesis_agent::ContinuationManager::with_disk_store(
            &workspace_dir,
        ));
        agent_loop.set_continuation_manager(cont_mgr);
    }

    // 10. Inject shared Arc references.
    if let Some(ref forge) = shared.forge {
        agent_loop.set_forge(forge.clone());
    }
    if let Some(ref plugin) = shared.security_plugin {
        agent_loop.set_security_plugin(plugin.clone());
    }
    // Checkpoint store (edit safety net): snapshots writer-tool file changes so
    // a rewind can restore them. One per agent loop under {workspace}/.checkpoints/.
    {
        let ws = shared.home.join("workspace");
        let store = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new(
            Some(ws.join(".checkpoints")),
            ws,
        ));
        agent_loop.set_checkpoint_store(store);
    }
    if let Some(ref mgr) = shared.observer_manager {
        agent_loop.set_observer_manager(mgr.clone());
    }
    if let Some(ref ds) = shared.data_store {
        agent_loop.set_data_store(ds.clone());
    }
    agent_loop.set_channel_manager(shared.enabled_channels.clone());

    // 11. Update Forge's LLM provider (old model may have been deleted).
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

// ---------------------------------------------------------------------------
// Shared: tool registration + MCP
// ---------------------------------------------------------------------------

/// Build SharedToolConfig from shared resources + fresh config.
///
/// Extracted from build_agent_loop so both main and cluster agents
/// share the same tool configuration logic.
fn build_shared_tool_config(
    shared: &Arc<SharedResources>,
    cfg: &nemesis_config::Config,
    model_name: &str,
    mcp_tool_snapshot: Option<Arc<parking_lot::RwLock<Vec<(String, String)>>>>,
) -> nemesis_agent::SharedToolConfig {
    let workspace_dir = shared.home.join("workspace");

    nemesis_agent::SharedToolConfig {
        workspace: Some(workspace_dir.to_string_lossy().to_string()),
        cron_service: Some(shared.cron_service.clone()),
        forge_executor: shared.forge_executor.clone(),
        forge: shared.forge.clone(),
        memory_executor: shared.memory_manager.as_ref().map(|mgr| {
            Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(mgr.clone()))
        }),
        skills_loader: shared.skills_loader.clone(),
        skills_registry: shared.skills_registry.clone(),
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
        spawn: Some(nemesis_agent::loop_tools::SpawnConfig {
            default_model: model_name.to_string(),
            max_concurrent: 4,
        }),
        cluster_rpc: None, // Registered separately with call_fn
        mcp_tool_snapshot,
        workflow_engine: shared.workflow_engine.clone(),
        approval_manager: Some(shared.approval_slot.clone()),
        skills_manage_approval: cfg
            .skills
            .as_ref()
            .map(|s| s.manage_approval)
            .unwrap_or(false),
    }
}

/// Register all tools and enable MCP on the given AgentLoop.
///
/// Shared between main agent and cluster agent. The caller is responsible for
/// registering cluster_rpc with call_fn after this call.
fn register_tools_and_mcp(
    agent_loop: &mut nemesis_agent::r#loop::AgentLoop,
    shared: &Arc<SharedResources>,
    tool_config: &nemesis_agent::SharedToolConfig,
) {
    let all_tools = nemesis_agent::register_shared_tools(tool_config);
    let tool_count = all_tools.len();
    for (name, tool) in all_tools {
        agent_loop.register_tool(name, tool);
    }

    if shared.mcp_enabled {
        agent_loop.enable_mcp_reload(shared.mcp_config_path.clone());
    }

    info!(
        "[AgentFactory] Tools registered: {}{}",
        tool_count,
        if shared.mcp_enabled { " + MCP" } else { "" }
    );
}

// ---------------------------------------------------------------------------
// build_cluster_agent_loop — cluster agent factory
// ---------------------------------------------------------------------------

/// Build a cluster agent loop (standalone mode, no bus).
///
/// Returns `(AgentLoop, AgentConfig)` — the AgentLoop for running tasks,
/// and the AgentConfig for creating per-task AgentInstance (carries system_prompt identity).
///
/// Shares the same tool set and MCP as the main agent.
/// Differences from the main agent:
/// - Standalone mode (`AgentLoop::new` instead of `new_bus`)
/// - No session store, state manager, continuation manager
/// - No security plugin, data store, channel manager
/// - Has its own observer_manager with ClusterRequestLoggerObserver
///   (writes LLM details to cluster_logs/{device_id}/{task_id}/)
/// - Has cluster reference (for cluster_rpc tool to work)
/// - System prompt loaded from `workspace/cluster/IDENTITY.md` + `SOUL.md`
pub fn build_cluster_agent_loop(
    shared: &Arc<SharedResources>,
    cluster: Arc<nemesis_cluster::cluster::Cluster>,
) -> Result<(
    Arc<nemesis_agent::r#loop::AgentLoop>,
    nemesis_agent::types::AgentConfig,
    Option<Arc<crate::cluster_request_logger_observer::ClusterRequestLoggerObserver>>,
)> {
    use nemesis_agent::r#loop::AgentLoop;

    // 1. Re-read config.json from disk.
    let config_path = shared.home.join("config.json");
    let cfg = nemesis_config::load_config(&config_path)
        .map_err(|e| anyhow::anyhow!("Failed to load config: {}", e))?;

    // 2. Resolve LLM model → create provider.
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

    // 3. Load cluster system prompt from workspace/cluster/IDENTITY.md + SOUL.md.
    let system_prompt = load_cluster_system_prompt(&shared.home);

    // 4. Create AgentConfig + AgentLoop (standalone mode, no bus).
    let config = nemesis_agent::types::AgentConfig {
        model: model_name.clone(),
        system_prompt,
        max_turns: 50,
        ..Default::default()
    };
    let adapter = ProviderAdapter::new(provider_arc, model_name.clone());
    let mut agent_loop = AgentLoop::new(Box::new(adapter), config.clone());

    // 5. Set cluster reference (enables cluster_rpc tool).
    agent_loop.set_cluster(cluster as Arc<dyn std::any::Any + Send + Sync>);

    // 5b. Set observer callback to capture cluster task execution details (LLM + tool calls).
    {
        let log_cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync> = Arc::new(
            |event_type: &str, data: &serde_json::Value| {
                // Only log cluster-related trace events.
                let trace_id = data.get("trace_id").and_then(|v| v.as_str()).unwrap_or("");
                if !trace_id.starts_with("cluster") {
                    return;
                }

                // Extract task_id from trace_id: "cluster-XXXXXXXX" or "cluster-resume-XXXXXXXX"
                let task_id = if trace_id.starts_with("cluster-resume-") {
                    &trace_id["cluster-resume-".len()..]
                } else {
                    &trace_id["cluster-".len()..]
                };

                match event_type {
                    "llm_request" => {
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        let model = data.get("model").and_then(|v| v.as_str()).unwrap_or("");
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_llm_start",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "model": model,
                            }),
                        );
                    }
                    "llm_response" => {
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        let duration_ms =
                            data.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                        let tokens = data.get("usage");
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_llm_end",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "duration_ms": duration_ms,
                                "tokens": tokens,
                            }),
                        );
                    }
                    "tool_call" => {
                        let tool_name = data
                            .get("tool_name")
                            .and_then(|v| v.as_str())
                            .unwrap_or("");
                        let success = data.get("success").and_then(|v| v.as_bool()).unwrap_or(false);
                        let duration_ms =
                            data.get("duration_ms").and_then(|v| v.as_u64()).unwrap_or(0);
                        let round = data.get("round").and_then(|v| v.as_u64()).unwrap_or(0);
                        nemesis_cluster::cluster_log::write_cluster_log(
                            "task_tool_call",
                            serde_json::json!({
                                "task_id": task_id,
                                "round": round,
                                "tool": tool_name,
                                "duration_ms": duration_ms,
                                "success": success,
                            }),
                        );
                    }
                    _ => {}
                }
            },
        );
        agent_loop.set_observer_callback(log_cb);
    }

    // 5c. Create dedicated observer_manager + ClusterRequestLoggerObserver.
    //
    // Independent from main agent's observer_manager — completely isolates
    // event dispatch. Observer writes LLM details to
    // `cluster_logs/{device_id}/{ts_ms}_{task_id}/` per task.
    //
    // Task context (task_id + device_id) is set/cleared by cluster_agent_loop
    // around each task execution.
    let cluster_observer: Option<
        Arc<crate::cluster_request_logger_observer::ClusterRequestLoggerObserver>,
    > = {
        let llm_cfg = cfg
            .logging
            .as_ref()
            .and_then(|l| l.llm.as_ref())
            .filter(|l| l.enabled);

        match llm_cfg {
            Some(llm_cfg) => {
                let logging_config = nemesis_agent::request_logger::LoggingConfig {
                    enabled: true,
                    detail_level: match llm_cfg.detail_level.as_str() {
                        "truncated" => nemesis_agent::request_logger::DetailLevel::Truncated,
                        _ => nemesis_agent::request_logger::DetailLevel::Full,
                    },
                    log_dir: if llm_cfg.log_dir.is_empty() {
                        "logs/cluster_logs".to_string()
                    } else {
                        llm_cfg.log_dir.clone()
                    },
                    save_raw: llm_cfg.save_raw,
                };
                let workspace_path = shared.home.join("workspace");
                let observer = Arc::new(
                    crate::cluster_request_logger_observer::ClusterRequestLoggerObserver::new(
                        logging_config,
                        &workspace_path,
                    ),
                );

                // Create dedicated observer_manager and register the observer.
                let mgr = Arc::new(nemesis_observer::Manager::new());
                let mgr_clone = mgr.clone();
                let observer_clone = observer.clone();
                tokio::task::block_in_place(|| {
                    tokio::runtime::Handle::current().block_on(async {
                        mgr_clone.register(observer_clone as Arc<dyn nemesis_observer::Observer>).await;
                    })
                });
                agent_loop.set_observer_manager(mgr);

                info!(
                    "[AgentFactory] ClusterRequestLoggerObserver registered (writes to logs/cluster_logs/{{device_id}}/{{task_id}}/)"
                );
                Some(observer)
            }
            None => {
                info!(
                    "[AgentFactory] ClusterRequestLoggerObserver disabled (logging.llm.enabled = false)"
                );
                None
            }
        }
    };

    // 6. Build tool config + register all tools + enable MCP.
    let tool_config = build_shared_tool_config(shared, &cfg, &model_name, None);
    register_tools_and_mcp(&mut agent_loop, shared, &tool_config);

    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (P2: agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    if let Some(ref exec) = tool_config.memory_executor {
        agent_loop.set_memory_executor(exec.clone());
    }

    // 6b. Attach a dedicated SessionStore so cluster peer_chat can persist and
    // restore conversation history per (source_node_id, chat_id) pair.
    //
    // Path: `{workspace}/sessions/cluster/`
    // - Separate from main agent's `{workspace}/sessions/` to avoid any chance
    //   of file-name collision (sanitize_filename keeps `:` and `/` distinct
    //   from `_`, but a dedicated directory is the simpler invariant).
    // - cluster_agent.rs::execute_new_task reads from this store to restore
    //   history; writes back user + final assistant message after task completes.
    //
    // TTL: sessions older than 7 days are deleted at startup and via a daily
    // midnight task (spawn_daily_cleanup). Bounded disk usage without manual
    // intervention.
    {
        let cluster_sessions_dir = shared.home.join("workspace").join("sessions").join("cluster");
        let cluster_session_store = Arc::new(
            nemesis_agent::session::SessionStore::new_with_storage(&cluster_sessions_dir),
        );
        let deleted = cluster_session_store.cleanup_old_sessions(7);
        if deleted > 0 {
            info!(
                deleted,
                "[AgentFactory] Cluster SessionStore startup cleanup (TTL=7d)"
            );
        }
        spawn_daily_cleanup(cluster_session_store.clone(), "Cluster");
        agent_loop.set_session_store(cluster_session_store);
        info!(
            dir = %cluster_sessions_dir.display(),
            "[AgentFactory] Cluster SessionStore attached (for peer_chat history)"
        );
    }

    // 6b. Checkpoint store (edit safety net) for the cluster agent too.
    {
        let ws = shared.home.join("workspace");
        let store = Arc::new(nemesis_agent::checkpoint::CheckpointStore::new(
            Some(ws.join(".checkpoints")),
            ws,
        ));
        agent_loop.set_checkpoint_store(store);
    }

    // 7. Register cluster_rpc with call_fn + peers_fn (if available).
    if let (Some(rpc_config), Some(call_fn)) =
        (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
    {
        let mut cluster_rpc_tool = nemesis_agent::ClusterRpcTool::new(rpc_config.clone());
        cluster_rpc_tool.set_rpc_call_fn(call_fn.clone());
        if let Some(ref peers_fn) = shared.cluster_peers_fn {
            cluster_rpc_tool.set_peers_fn(peers_fn.clone());
        }
        cluster_rpc_tool.set_enabled(true);
        agent_loop.register_tool("cluster_rpc".to_string(), Box::new(cluster_rpc_tool));
        info!("[AgentFactory] cluster_rpc tool registered for cluster agent (enabled=true)");
    }

    info!(
        model = %model_name,
        tools = agent_loop.tool_count(),
        has_system_prompt = config.system_prompt.is_some(),
        "[AgentFactory] Cluster AgentLoop built successfully"
    );

    Ok((Arc::new(agent_loop), config, cluster_observer))
}

/// Load cluster system prompt from `workspace/cluster/IDENTITY.md` + `SOUL.md`.
///
/// Returns None if neither file exists (cluster agent runs without identity).
fn load_cluster_system_prompt(home: &std::path::Path) -> Option<String> {
    let cluster_dir = home.join("workspace").join("cluster");
    let mut parts = Vec::new();

    if let Ok(content) = std::fs::read_to_string(cluster_dir.join("IDENTITY.md")) {
        if !content.trim().is_empty() {
            parts.push(content);
        }
    }
    if let Ok(content) = std::fs::read_to_string(cluster_dir.join("SOUL.md")) {
        if !content.trim().is_empty() {
            parts.push(content);
        }
    }

    if parts.is_empty() {
        info!("[AgentFactory] No cluster identity files found, running without system prompt");
        None
    } else {
        info!(
            files = parts.len(),
            "[AgentFactory] Cluster system prompt loaded from {} file(s)",
            parts.len()
        );
        Some(parts.join("\n\n---\n\n"))
    }
}

/// Spawn a background task that runs `cleanup_old_sessions(7)` every day at local midnight.
///
/// The task sleeps until the next local midnight, runs cleanup, then loops.
/// If the tokio runtime shuts down (gateway stop), the task is cancelled and
/// no further cleanups run — startup cleanup in `build_*_agent_loop` covers
/// the next start.
///
/// `label` is used for logging only ("Main" / "Cluster").
fn spawn_daily_cleanup(store: Arc<nemesis_agent::session::SessionStore>, label: &str) {
    let label = label.to_string();
    tokio::spawn(async move {
        use chrono::TimeZone;
        loop {
            // Calculate seconds until next local midnight.
            let now = chrono::Local::now();
            let next_midnight = chrono::Local
                .from_local_datetime(
                    &now.date_naive()
                        .succ_opt()
                        .unwrap()
                        .and_hms_opt(0, 0, 0)
                        .unwrap(),
                )
                .unwrap();
            let dur = next_midnight.signed_duration_since(now);
            let sleep_secs = dur.num_seconds().max(60) as u64;

            tokio::time::sleep(std::time::Duration::from_secs(sleep_secs)).await;

            let deleted = store.cleanup_old_sessions(7);
            if deleted > 0 {
                info!(
                    deleted,
                    label = %label,
                    "[AgentFactory] {} SessionStore daily midnight cleanup (TTL=7d)",
                    label
                );
            }
        }
    });
}
