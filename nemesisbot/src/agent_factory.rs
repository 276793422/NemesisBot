//! Agent factory — builds AgentLoop instances from shared configuration.
//!
//! Two factory functions:
//! - `build_agent_loop()` — main agent (bus mode, session store, continuation manager, etc.)
//! - `build_cluster_agent_loop()` — cluster agent (standalone mode, tier-resolved tool set, no bus)
//!
//! Both share the same tool registration and MCP logic via `register_tools_and_mcp()`.
//! The difference is in mode (bus vs standalone) and which optional subsystems are attached.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;

use anyhow::Result;
use tracing::info;

#[cfg(feature = "forge")]
use nemesis_web::ForgeProviderBridge;
use nemesis_web::ProviderAdapter;

use crate::common;

#[cfg(test)]
mod tests;

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
    #[cfg(feature = "forge")]
    pub forge: Option<Arc<nemesis_forge::forge::Forge>>,
    #[cfg(not(feature = "forge"))]
    pub forge: Option<()>,
    #[cfg(feature = "forge")]
    pub forge_executor: Option<Arc<nemesis_forge::forge_tools::ForgeToolExecutor>>,
    #[cfg(not(feature = "forge"))]
    pub forge_executor: Option<()>,
    pub cron_service: Arc<std::sync::Mutex<nemesis_cron::service::CronService>>,
    #[cfg(feature = "security")]
    pub security_plugin: Option<Arc<nemesis_security::pipeline::SecurityPlugin>>,
    #[cfg(not(feature = "security"))]
    #[allow(dead_code)]
    pub security_plugin: Option<()>,
    pub observer_manager: Option<Arc<nemesis_observer::Manager>>,
    pub data_store: Option<Arc<nemesis_data::DataStore>>,
    pub skills_loader: Option<Arc<nemesis_skills::loader::SkillsLoader>>,
    pub skills_registry: Option<Arc<nemesis_skills::registry::RegistryManager>>,
    #[cfg(feature = "memory")]
    pub memory_manager: Option<Arc<nemesis_memory::manager::MemoryManager>>,
    #[cfg(not(feature = "memory"))]
    #[allow(dead_code)]
    pub memory_manager: Option<()>,
    pub enabled_channels: Vec<String>,
    /// Workflow engine reference — when set, registers the `workflow_run`
    /// agent tool. None keeps the tool absent (e.g., during tests).
    #[cfg(feature = "workflow")]
    pub workflow_engine: Option<Arc<nemesis_workflow::engine::WorkflowEngine>>,
    #[cfg(not(feature = "workflow"))]
    #[allow(dead_code)]
    pub workflow_engine: Option<()>,
    /// Approval manager slot, filled by the gateway after the agent loop is
    /// built. Lets `skill_manage` request interactive approval when enabled.
    pub approval_slot: nemesis_agent::loop_tools::ApprovalManagerSlot,

    // Cluster RPC closure (Cluster itself is mem::forget'd, but rpc_call_fn must survive)
    pub cluster_rpc_call_fn: Option<
        Arc<
            dyn Fn(
                    &str,
                    &str,
                    serde_json::Value,
                ) -> std::pin::Pin<
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

    /// 全局急停状态（kill switch）。gateway::run 里建一次，跨 agent 重启存活。
    /// 两个工厂都把每个 AgentLoop 绑到这同一个 Arc，所以急停状态在 stop/start
    /// 后自动保持——这才是真急停（不会一重启自己解除）。
    pub estop: Arc<nemesis_agent::estop::EstopState>,

    /// Runtime config cache (single source of truth). Consumers read the live
    /// config via `config_store.handle()`; dashboard handlers mutate via
    /// `config_store.update(...)` (in-memory + persist). Lets executor.sandbox
    /// / tier / DLP toggles flip live without a gateway restart.
    pub config_store: Arc<nemesis_config::ConfigStore>,
}

/// Default `SharedResources` for tests: empty/dummy infrastructure. Real
/// instances are always built fully by `build_agent_loop`; this exists so test
/// literals can use `..Default::default()` and stay immune to new fields.
impl Default for SharedResources {
    fn default() -> Self {
        // Sender has no Default — create a throwaway channel.
        let (agent_outbound_tx, _dropped_rx) = tokio::sync::mpsc::channel(16);
        Self {
            home: PathBuf::default(),
            bus: Arc::new(nemesis_bus::MessageBus::default()),
            agent_outbound_tx,
            forge: None,
            forge_executor: None,
            cron_service: Arc::new(std::sync::Mutex::new(
                nemesis_cron::service::CronService::new(""),
            )),
            security_plugin: None,
            observer_manager: None,
            data_store: None,
            skills_loader: None,
            skills_registry: None,
            memory_manager: None,
            enabled_channels: Vec::new(),
            workflow_engine: None,
            approval_slot: Default::default(),
            cluster_rpc_call_fn: None,
            cluster_rpc_config: None,
            cluster_peers_fn: None,
            cluster_rpc_enabled: parking_lot::RwLock::new(None),
            mcp_config_path: PathBuf::default(),
            mcp_enabled: false,
            estop: Arc::new(nemesis_agent::estop::EstopState::new()),
            config_store: Arc::new(nemesis_config::ConfigStore::from_config(
                nemesis_config::Config::default(),
                PathBuf::default(),
            )),
        }
    }
}

// ---------------------------------------------------------------------------
// build_agent_loop — factory function
// ---------------------------------------------------------------------------

/// Build a fresh AgentLoop from disk config.
///
/// Re-reads `config.json`, workspace files, creates new provider,
/// registers all tools — identical to first-boot initialization.
pub fn build_agent_loop(
    shared: &Arc<SharedResources>,
) -> Result<Arc<nemesis_agent::r#loop::AgentLoop>> {
    use nemesis_agent::r#loop::AgentLoop;
    use nemesis_agent::types::AgentConfig;

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
    // Phase 4a (small-model-tool-robustness): resolve the startup model's tier
    // from config.json, and remember the path so the tier can be re-resolved
    // live when the model switches or config.json changes on disk (dashboard
    // add / CLI model set-tier). config.json is the single source of truth —
    // no stale snapshot.
    let cfg_json: serde_json::Value = std::fs::read_to_string(shared.home.join("config.json"))
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let resolved_tier = nemesis_types::capability::resolve_active_tier(&cfg_json, &model_name);
    info!(
        "[AgentFactory] Model '{}' capability tier: {}",
        model_name, resolved_tier
    );
    let agent_config = AgentConfig {
        model: model_name.clone(),
        system_prompt: if system_prompt.is_empty() {
            None
        } else {
            Some(system_prompt)
        },
        max_turns: if cfg.agents.defaults.max_tool_iterations <= 0 {
            // 0 (or negative) = unlimited opt-in. The run-loop treats
            // max_turns == 0 as "no cap"; see AgentLoop process_message.
            0
        } else {
            cfg.agents.defaults.max_tool_iterations as u32
        },
        tools: Vec::new(),
        models: std::collections::HashMap::new(),
    };

    let max_continuation_permits = cfg.agents.defaults.max_continuation_permits.max(0) as usize;
    // I1 (U7): concurrent_request_mode from config ("reject" default =
    // legacy behavior; "queue"/"steer" enable the session inbox).
    let mode_str = cfg.agents.defaults.concurrent_request_mode.trim().to_lowercase();
    if !matches!(mode_str.as_str(), "reject" | "queue" | "steer") {
        tracing::warn!(
            "[AgentFactory] unknown concurrent_request_mode '{}' — falling back to reject",
            cfg.agents.defaults.concurrent_request_mode
        );
    }
    let concurrent_mode = nemesis_agent::r#loop::parse_concurrent_mode(&mode_str);
    let queue_size = cfg.agents.defaults.queue_size.max(1) as usize;
    let mut agent_loop = AgentLoop::new_bus(
        Box::new(adapter),
        agent_config,
        shared.agent_outbound_tx.clone(),
        concurrent_mode,
        queue_size,
        max_continuation_permits,
    );
    // Phase 4a: apply the resolved startup tier + remember the config path so
    // runtime model switches and dashboard/CLI config edits re-resolve it live.
    agent_loop.set_tier(resolved_tier);
    agent_loop.set_config_path(shared.home.join("config.json"));
    // G4 (U4): enable tool-result spill under <home>/logs/spill — oversized
    // results (>64k chars) land there whole with a locator in-conversation.
    let spill_root = shared.home.join("logs").join("spill");
    agent_loop.set_spill_root(spill_root.clone());
    // U4 retention: startup sweep + daily midnight task. retention=0 disables.
    let spill_retention = cfg.agents.defaults.spill_retention_days.max(0) as u64;
    if spill_retention > 0 {
        let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, spill_retention);
        if deleted > 0 {
            info!(
                deleted,
                retention_days = spill_retention,
                "[AgentFactory] spill startup cleanup (TTL={}d)",
                spill_retention
            );
        }
    }
    spawn_daily_spill_cleanup(spill_root, shared.home.join("config.json"));
    // H3 (P2.2): skills-catalog digest injection — same loader the
    // skills_list tools use, so the advertised catalog matches reality.
    if let Some(ref loader) = shared.skills_loader {
        agent_loop.set_skills_loader(loader.clone());
    }
    // H5 (U18): workspace instruction chain (AGENTS.md/CLAUDE.md) rides the
    // same merged context-digest injection.
    agent_loop.set_workspace_root(shared.home.join("workspace"));
    // Full-review M4: snapshot role from config ("user" default; "system"
    // for strict chat templates).
    agent_loop.set_snapshot_role(&cfg.agents.defaults.snapshot_role);
    // 绑定全局急停状态（每次重建都重新绑到 SharedResources 上的同一个 Arc，
    // 所以急停状态在 agent stop/start 后自动保持）。
    agent_loop.set_estop(shared.estop.clone());
    // K2 (U14): CC hooks.json 方言层——workspace config 目录下有 hooks.json
    // 就加载并挂上（工具钩子 + 生命周期钩子）。集群 agent 不挂（远端节点跑
    // 本地用户任务，hook 拦截语义不跨节点复制——挂账决策）。加载失败 =
    // warn + 跳过（fail-open，见 cc_hooks::load_from_dir）。
    if let Some(bridge) = nemesis_agent::cc_hooks::CcHookBridge::load_from_dir(
        &shared.home.join("config"),
        shared.home.join("workspace"),
    ) {
        bridge.register(&agent_loop);
    }

    // 5. Session store (disk-persisted — new instance, same directory).
    {
        let sess_dir = common::sessions_dir(&shared.home);
        // Migrate legacy single-session main → multi-session legacy format
        // (idempotent + best-effort) BEFORE SessionStore loads from disk.
        nemesis_agent::session::SessionStore::migrate_legacy_main(&sess_dir);
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
        let state_mgr = nemesis_state::workspace_state::WorkspaceStateManager::new(&workspace_dir);
        agent_loop.set_state_manager(state_mgr);
    }

    // 7. Build tool config + register all tools + enable MCP.
    let tool_config = build_shared_tool_config(
        shared,
        &cfg,
        &model_name,
        Some(agent_loop.mcp_tool_snapshot()),
    );

    // Executor separation: if enabled, MOVE tools are wrapped in RemoteExecutorTool
    // and dispatched to a per-call child process. sandbox=false → stdio transport
    // (Layer 1). sandbox=true → named-pipe transport + Start.exe wrap (real
    // Sandboxie box, Layer 2); requires the `sandbox` feature + `nemesisbot sandbox
    // install`. When the `sandbox` feature is compiled out, sandbox=true is ignored.
    //
    // U10: the readiness/probe/degrade logic now lives in `exec_world::
    // build_executor_channel` (single source of truth) — the workflow engine's
    // ExecutionWorld uses the exact same assembly, so the agent tool layer and
    // workflow scripts share one switch chain (executor.enabled / executor.sandbox).
    let executor_channel = crate::exec_world::build_executor_channel(
        &shared.home,
        &workspace_dir,
        shared.config_store.handle(),
    )?;
    register_tools_and_mcp(
        &mut agent_loop,
        shared,
        &tool_config,
        executor_channel.as_ref(),
    );

    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (P2: agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    #[cfg(feature = "memory")]
    {
        if let Some(ref exec) = tool_config.memory_executor {
            agent_loop.set_memory_executor(exec.clone());
        }
    }

    // P3.1 (sixth batch): wire the auto-inject channel — the memory manager
    // (read-only retrieval, deliberately NOT the approval-gated executor)
    // plus the auto_inject/top_k flags from config.enhanced_memory.json.
    // Default (auto_inject=false) keeps message output byte-identical.
    // `shared.memory_manager` is None when `memory.enabled=false` in
    // config.json → no retrieval, injection stays off regardless of flags.
    #[cfg(feature = "memory")]
    {
        let config_dir = shared.home.join("workspace").join("config");
        let (auto, top_k) = {
            let emb = nemesis_memory::vector::embedding_config::load_embedding_config(
                &config_dir,
            );
            (emb.auto_inject, emb.auto_inject_top_k)
        };
        agent_loop.set_memory_inject(shared.memory_manager.clone(), auto, top_k);
        if auto {
            info!("[AgentFactory] memory auto-inject enabled (top_k={top_k})");
        }
    }
    #[cfg(not(feature = "memory"))]
    {
        // Flags-only stub (no manager exists without the memory crate);
        // injection is structurally a no-op here regardless.
        agent_loop.set_memory_inject(false, 3);
    }

    // 8. Register ClusterRpcTool (using shared call_fn + peers_fn).
    if let (Some(config), Some(call_fn)) = (&shared.cluster_rpc_config, &shared.cluster_rpc_call_fn)
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
        agent_loop.set_continuation_manager(cont_mgr.clone());
        // rpc_cache TTL (2026-08-25): continuation snapshots whose callback
        // never arrives (peer died, task lost) used to accumulate in
        // {workspace}/cluster/rpc_cache/ forever. Same 7-day TTL as the
        // SessionStore: startup sweep + daily midnight.
        spawn_rpc_cache_cleanup(cont_mgr);
    }

    // 10. Inject shared Arc references.
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = shared.forge {
            agent_loop.set_forge(forge.clone());
        }
    }
    #[cfg(feature = "security")]
    {
        if let Some(ref plugin) = shared.security_plugin {
            agent_loop.set_security_plugin(plugin.clone());
        }
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
    #[cfg(feature = "forge")]
    {
        if let Some(ref forge) = shared.forge {
            let bridge = ForgeProviderBridge::new(provider_arc.clone(), model_name.clone());
            forge.set_provider(Arc::new(bridge));
            info!(
                "[AgentFactory] Forge provider updated to model {}",
                model_name
            );
        }
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
        #[cfg(feature = "memory")]
        memory_executor: shared.memory_manager.as_ref().map(|mgr| {
            Arc::new(nemesis_memory::memory_tools::MemoryToolExecutor::new(
                mgr.clone(),
            ))
        }),
        #[cfg(not(feature = "memory"))]
        memory_executor: None,
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
        // H7 (U13 half): opt-in claude_code delegation tool.
        claude_code_tool_enabled: cfg.agents.claude_code_tool.enabled,
        claude_code_tool_timeout_secs: cfg.agents.claude_code_tool.timeout_secs,
        claude_code_tool_permission_mode: cfg.agents.claude_code_tool.permission_mode.clone(),
        // I4 (U13 other half): opt-in codex delegation tool.
        codex_tool_enabled: cfg.agents.codex_tool.enabled,
        codex_tool_timeout_secs: cfg.agents.codex_tool.timeout_secs,
        codex_tool_sandbox: cfg.agents.codex_tool.sandbox.clone(),
        // L1 (U19): opt-in read-only LSP tool (further gated by a PATH
        // probe at registration — see register_shared_tools).
        lsp_tool_enabled: cfg.agents.lsp_tool.enabled,
        lsp_tool_timeout_secs: cfg.agents.lsp_tool.timeout_secs,
        lsp_tool_idle_secs: cfg.agents.lsp_tool.idle_secs,
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
    executor_channel: Option<&Arc<nemesis_agent::ExecutorChannel>>,
) {
    let mut all_tools = nemesis_agent::register_shared_tools(tool_config);

    // Executor separation (Layer 1): replace the MOVE tool set with
    // RemoteExecutorTool bridges that proxy execute() to a child process.
    // Metadata (description / parameters / preview) delegates to the local
    // impl, so the LLM sees byte-identical schemas and the checkpoint safety
    // net still snapshots file writes.
    if let Some(channel) = executor_channel {
        for name in nemesis_agent::MOVE_TOOLS {
            if let Some(local) = all_tools.remove(*name) {
                all_tools.insert(
                    (*name).to_string(),
                    Box::new(nemesis_agent::RemoteExecutorTool::new(
                        (*name).to_string(),
                        local,
                        channel.clone(),
                    )),
                );
            }
        }
    }

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
#[cfg(feature = "cluster")]
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
        max_turns: 60,
        ..Default::default()
    };
    let adapter = ProviderAdapter::new(provider_arc, model_name.clone());
    let mut agent_loop = AgentLoop::new(Box::new(adapter), config.clone());

    // 5. Set cluster reference (enables cluster_rpc tool).
    agent_loop.set_cluster(cluster as Arc<dyn std::any::Any + Send + Sync>);
    // 绑定全局急停状态（集群 agent 同样吃急停——peer_chat 跑完整工具链，不能漏）。
    agent_loop.set_estop(shared.estop.clone());

    // D1 (2026-08-24 arch review, U-list D1): the cluster agent must resolve
    // the same startup capability tier as the main agent. Before this, the
    // cluster loop always ran the AgentLoop::new default (Big = full 42-tool
    // set) even when the active model is a small model — the main agent's
    // mini-tier filtering never applied cluster-side. Same wiring as
    // build_agent_loop: resolve from the raw config.json Value (model_tier
    // is a dynamic field there), then remember the config path so the loop's
    // check_config_reload re-resolves the tier live when config.json changes
    // on disk (dashboard model add / CLI `model set-tier`) cluster-side too.
    let cfg_json: serde_json::Value = std::fs::read_to_string(&config_path)
        .ok()
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or(serde_json::Value::Null);
    let resolved_tier = nemesis_types::capability::resolve_active_tier(&cfg_json, &model_name);
    info!(
        "[AgentFactory] Cluster model '{}' capability tier: {}",
        model_name, resolved_tier
    );
    agent_loop.set_tier(resolved_tier);
    agent_loop.set_config_path(config_path.clone());

    // D2 (2026-08-24 arch review, U-list D2): enable tool-result spill for
    // the cluster agent too — cluster peer_chat is exactly the long-task /
    // huge-output case; without a spill root, oversized results (>64k chars)
    // degrade to the prune profile (head+tail 3600) with no full copy on
    // disk. Shares the main agent's spill root and retention setting.
    // NOTE: the daily-midnight cleanup task is already spawned by
    // build_agent_loop for this same root — deliberately NOT spawned again
    // here (a second timer would double-scan).
    let spill_root = shared.home.join("logs").join("spill");
    agent_loop.set_spill_root(spill_root.clone());
    let spill_retention = cfg.agents.defaults.spill_retention_days.max(0) as u64;
    if spill_retention > 0 {
        let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, spill_retention);
        if deleted > 0 {
            info!(
                deleted,
                retention_days = spill_retention,
                "[AgentFactory] cluster spill startup cleanup (TTL={}d)",
                spill_retention
            );
        }
    }

    // 5b. Set observer callback to capture cluster task execution details (LLM + tool calls).
    {
        let log_cb: Arc<dyn Fn(&str, &serde_json::Value) + Send + Sync> =
            Arc::new(|event_type: &str, data: &serde_json::Value| {
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
                        let duration_ms = data
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
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
                        let tool_name =
                            data.get("tool_name").and_then(|v| v.as_str()).unwrap_or("");
                        let success = data
                            .get("success")
                            .and_then(|v| v.as_bool())
                            .unwrap_or(false);
                        let duration_ms = data
                            .get("duration_ms")
                            .and_then(|v| v.as_u64())
                            .unwrap_or(0);
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
            });
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
                        mgr_clone
                            .register(observer_clone as Arc<dyn nemesis_observer::Observer>)
                            .await;
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
    // Cluster agent does not use executor separation yet (B.0 scope: main agent
    // only). Pass None → all tools stay local.
    register_tools_and_mcp(&mut agent_loop, shared, &tool_config, None);

    // D3 (2026-08-24 arch review): P3.1 auto memory injection (per-round
    // injection of relevant local user memories before each LLM call) is
    // deliberately NOT wired for the cluster agent — same policy as the
    // hooks exemption in build_agent_loop (comment above there): a remote
    // peer's task context must not receive this local user's personal
    // memories. The wiring below is a different thing: it only stashes the
    // memory executor for the interactive memory tools (memory_store /
    // memory_forget with approval gating).
    //
    // Stash the memory executor so the gateway can attach an approval gate
    // post-construction (P2: agent memory_store/forget require interactive
    // approval, never bypassed by YOLO/auto).
    #[cfg(feature = "memory")]
    {
        if let Some(ref exec) = tool_config.memory_executor {
            agent_loop.set_memory_executor(exec.clone());
        }
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
        let cluster_sessions_dir = shared
            .home
            .join("workspace")
            .join("sessions")
            .join("cluster");
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
#[cfg(feature = "cluster")]
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

/// rpc_cache continuation-snapshot TTL sweep (mirrors `spawn_daily_cleanup`).
///
/// 2026-08-25: `ContinuationManager::cleanup_old_snapshots(7d)` had no
/// production caller — snapshots whose callback never arrives accumulated
/// in `{workspace}/cluster/rpc_cache/` forever (and would be re-recovered
/// into memory by `recover_to_manager` after every restart). This runs a
/// sweep immediately at startup, then daily at local midnight, same as the
/// SessionStore TTL. Once-guard: `build_agent_loop` runs again on agent
/// rebuilds — only the first spawn survives for the process lifetime.
fn spawn_rpc_cache_cleanup(mgr: Arc<nemesis_agent::ContinuationManager>) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static RPC_CACHE_CLEANUP_SPAWNED: AtomicBool = AtomicBool::new(false);
    if RPC_CACHE_CLEANUP_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    const TTL: std::time::Duration = std::time::Duration::from_secs(7 * 24 * 3600);
    tokio::spawn(async move {
        use chrono::TimeZone;
        // Startup sweep.
        let removed = mgr.cleanup_old_snapshots(TTL).await;
        if removed > 0 {
            info!(
                removed,
                "[AgentFactory] rpc_cache continuation startup cleanup (TTL=7d)"
            );
        }
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

            let removed = mgr.cleanup_old_snapshots(TTL).await;
            if removed > 0 {
                info!(
                    removed,
                    "[AgentFactory] rpc_cache continuation midnight cleanup (TTL=7d)"
                );
            }
        }
    });
}

/// U4: daily-midnight spill retention sweep (mirrors `spawn_daily_cleanup`).
///
/// Retention days are re-read from config.json on EVERY run, so a dashboard
/// edit of `agents.spill_retention_days` applies at the next midnight without
/// a restart; `0` skips that night's sweep. Once-guard: `build_agent_loop`
/// runs again on agent rebuilds (persona activate etc.) — only the first
/// spawn survives for the process lifetime (one home per process).
fn spawn_daily_spill_cleanup(spill_root: std::path::PathBuf, config_path: std::path::PathBuf) {
    use std::sync::atomic::{AtomicBool, Ordering};
    static SPILL_CLEANUP_SPAWNED: AtomicBool = AtomicBool::new(false);
    if SPILL_CLEANUP_SPAWNED.swap(true, Ordering::SeqCst) {
        return;
    }
    tokio::spawn(async move {
        use chrono::TimeZone;
        loop {
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

            // Live-read retention (default 7 on unreadable config; negative -> 0 -> skip).
            let retention = nemesis_config::load_config(&config_path)
                .map(|c| c.agents.defaults.spill_retention_days)
                .unwrap_or(7)
                .max(0) as u64;
            if retention == 0 {
                continue;
            }
            let deleted = nemesis_agent::spill::cleanup_expired(&spill_root, retention);
            if deleted > 0 {
                info!(
                    deleted,
                    retention_days = retention,
                    "[AgentFactory] spill daily midnight cleanup (TTL={}d)",
                    retention
                );
            }
        }
    });
}

/// I5 (P3.4): semantic-embedder assembly helper. The providers Router is a
/// complete-but-not-yet-wired module (no production consumer builds it
/// today); this helper is the ready-made bridge for when it enters the
/// factory chain: it wires the memory manager's embedding backend (ONNX
/// plugin when loaded, n-gram fallback otherwise) into the router as the
/// SemanticEmbedder. Compilation-verified; invoked at Router adoption time.
#[allow(dead_code)]
#[cfg(feature = "memory")]
pub fn attach_semantic_embedder(
    router: &nemesis_providers::router::Router,
    memory: Option<&std::sync::Arc<nemesis_memory::manager::MemoryManager>>,
) {
    let Some(memory) = memory else {
        return; // no memory subsystem => Semantic degrades to fallback
    };
    let mgr = memory.clone();
    router.set_semantic_embedder(std::sync::Arc::new(move |text: &str| {
        mgr.embed_text(text)
    }));
}
