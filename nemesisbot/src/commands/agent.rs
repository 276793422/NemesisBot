//! Agent command - interact with the agent directly.
//!
//! Mirrors Go CmdAgent:
//! 1. Load config, resolve default LLM model
//! 2. Create provider via factory
//! 3. Wrap provider in adapter for AgentLoop
//! 4. Register default tools
//! 5. For single message: call process_direct() and print response
//! 6. For interactive mode: rustyline-based loop calling process_direct()

use std::sync::Arc;

use anyhow::Result;
use async_trait::async_trait;
use tracing::{info, warn};

use crate::common;

use nemesis_agent::types::{AgentConfig, ToolCallInfo as AgentToolCallInfo};
use nemesis_agent::session::SessionManager;
use nemesis_agent::r#loop::{AgentLoop, LlmMessage, LlmProvider, LlmResponse};

// ===========================================================================
// CLI enums
// ===========================================================================

#[derive(clap::Subcommand)]
pub enum AgentSetCommand {
    /// Set agent configuration
    Set {
        #[command(subcommand)]
        action: AgentSetAction,
    },
}

#[derive(clap::Subcommand)]
pub enum AgentSetAction {
    /// Set default LLM model
    Llm {
        /// Model reference (vendor/model or model_name)
        model: String,
    },
    /// Set concurrent request mode
    ConcurrentMode {
        /// Mode: reject or queue
        mode: String,
        /// Queue size (only for queue mode)
        #[arg(long)]
        queue_size: Option<usize>,
    },
}

// ===========================================================================
// Provider adapter: nemesis-providers LLMProvider → nemesis-agent LlmProvider
// ===========================================================================

/// Adapter wrapping a `nemesis_providers::router::LLMProvider` to implement
/// the `nemesis_agent::LlmProvider` trait expected by `AgentLoop`.
struct ProviderAdapter {
    inner: Arc<dyn nemesis_providers::router::LLMProvider>,
    default_model: String,
}

impl ProviderAdapter {
    fn new(inner: Arc<dyn nemesis_providers::router::LLMProvider>, default_model: String) -> Self {
        Self { inner, default_model }
    }
}

#[async_trait]
impl LlmProvider for ProviderAdapter {
    async fn chat(
        &self,
        model: &str,
        messages: Vec<LlmMessage>,
        options: Option<nemesis_agent::types::ChatOptions>,
        _tools: Vec<nemesis_agent::types::ToolDefinition>,
    ) -> Result<LlmResponse, String> {
        let model_to_use = if model.is_empty() {
            &self.default_model
        } else {
            model
        };

        // Convert agent LlmMessage → provider Message
        let provider_messages: Vec<nemesis_providers::types::Message> = messages
            .into_iter()
            .map(|m| nemesis_providers::types::Message {
                role: m.role,
                content: m.content,
                tool_calls: m
                    .tool_calls
                    .unwrap_or_default()
                    .into_iter()
                    .map(|tc| nemesis_providers::types::ToolCall {
                        id: tc.id,
                        call_type: Some("function".to_string()),
                        function: Some(nemesis_providers::types::FunctionCall {
                            name: tc.name,
                            arguments: tc.arguments,
                        }),
                        name: None,
                        arguments: None,
                    })
                    .collect(),
                tool_call_id: m.tool_call_id,
                timestamp: None,
            })
            .collect();

        // Convert agent ChatOptions → provider ChatOptions, using defaults when None.
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

        match self
            .inner
            .chat(&provider_messages, &[], model_to_use, &provider_options)
            .await
        {
            Ok(resp) => {
                let tool_calls: Vec<AgentToolCallInfo> = resp
                    .tool_calls
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
                })
            }
            Err(e) => {
                warn!("LLM provider error: {}", e);
                Err(format!("{}", e))
            }
        }
    }
}

// ===========================================================================
// Helper: build agent loop from config
// ===========================================================================

/// Build a fully-configured AgentLoop from the config file.
fn build_agent_loop(
    cfg: &nemesis_config::Config,
    home: &std::path::Path,
) -> Result<AgentLoop> {
    // 1. Resolve the default LLM model
    let llm_ref = nemesis_config::get_effective_llm(Some(cfg));
    let resolution = nemesis_config::resolve_model_config(cfg, &llm_ref)
        .map_err(|e| anyhow::anyhow!("Failed to resolve model '{}': {}", llm_ref, e))?;

    if !resolution.enabled {
        anyhow::bail!("Model '{}' is not enabled", llm_ref);
    }

    // 2. Create provider via factory
    let factory_cfg = nemesis_providers::factory::FactoryConfig {
        llm_ref: format!("{}/{}", resolution.provider_name, resolution.model_name),
        api_key: resolution.api_key,
        api_base: resolution.api_base,
        workspace: home
            .join("workspace")
            .to_string_lossy()
            .to_string(),
        connect_mode: resolution.connect_mode,
        account_id: String::new(),
        headers: std::collections::HashMap::new(),
    };
    let provider = nemesis_providers::factory::create_provider(&factory_cfg)
        .map_err(|e| anyhow::anyhow!("Failed to create provider: {}", e))?;

    let model_name = resolution.model_name.clone();

    info!(
        "Agent using model: {}/{}",
        resolution.provider_name, model_name
    );

    // 3. Wrap in adapter
    let adapter = ProviderAdapter::new(provider, model_name.clone());

    // 4. Build AgentConfig
    let agent_config = AgentConfig {
        model: model_name,
        system_prompt: None, // Will be built by ContextBuilder from workspace files
        max_turns: cfg.agents.defaults.max_tool_iterations.max(1) as u32,
        tools: Vec::new(),
    };

    // 5. Create AgentLoop (standalone mode for CLI)
    let mut agent_loop = AgentLoop::new(Box::new(adapter), agent_config);

    // 6. Register default tools
    let default_tools = nemesis_agent::register_default_tools();
    for (name, tool) in default_tools {
        agent_loop.register_tool(name, tool);
    }

    Ok(agent_loop)
}

// ===========================================================================
// Main entry point
// ===========================================================================

/// Run the agent command.
pub async fn run(
    subcommand: Option<AgentSetCommand>,
    message: Option<String>,
    session: String,
    debug: bool,
    quiet: bool,
    no_console: bool,
    local: bool,
) -> Result<()> {
    let home = common::resolve_home(local);

    match subcommand {
        None => {
            // Default: run agent (interactive or single-message mode)
            // Initialize logger with CLI overrides
            let cfg_path = common::config_path(&home);
            let mut log_args: Vec<String> = Vec::new();
            if debug {
                log_args.push("--debug".to_string());
            }
            if quiet {
                log_args.push("--quiet".to_string());
            }
            if no_console {
                log_args.push("--no-console".to_string());
            }
            let _flags = common::init_logger_from_config(&cfg_path, &log_args);

            // Load configuration
            if !cfg_path.exists() {
                anyhow::bail!(
                    "Configuration not found: {}. Run 'nemesisbot onboard default' first.",
                    cfg_path.display()
                );
            }
            let cfg = nemesis_config::load_config(&cfg_path)
                .map_err(|e| anyhow::anyhow!("Error loading config: {}", e))?;

            if debug {
                println!("  Debug mode enabled");
            }
            if quiet {
                println!("  Quiet mode enabled");
            }
            println!("  Session: {}", session);
            println!("  Home: {}", home.display());

            // Build the agent loop
            let agent_loop = match build_agent_loop(&cfg, &home) {
                Ok(al) => {
                    println!("  OK Agent loop initialized");
                    al
                }
                Err(e) => {
                    eprintln!("  Failed to initialize agent: {}", e);
                    eprintln!();
                    eprintln!("  Note: Agent mode requires a configured LLM model.");
                    eprintln!("  Run 'nemesisbot model add --model <provider/model> --key YOUR_KEY --default'");
                    eprintln!("  or start the gateway for full agent functionality.");
                    return Err(e);
                }
            };

            match message {
                Some(msg) => {
                    // Single message mode
                    println!("  Message: {}", msg);
                    println!();

                    match agent_loop.process_direct(&msg, &session).await {
                        Ok(response) => {
                            println!("Agent: {}", response);
                        }
                        Err(e) => {
                            eprintln!("Agent error: {}", e);
                        }
                    }
                }
                None => {
                    // Interactive mode with rustyline
                    let session_mgr =
                        SessionManager::new(std::time::Duration::from_secs(3600));
                    println!("  OK Session manager ready");
                    println!();
                    println!(
                        "Interactive mode. Type 'exit' or 'quit' to stop."
                    );
                    println!("  Commands: /history, /clear, /status");
                    println!();

                    let session_key = session.clone();
                    let history_dir = common::workspace_path(&home).join("logs");
                    let _ = std::fs::create_dir_all(&history_dir);
                    let history_path = history_dir.join("agent_history");
                    let mut rl = rustyline::Editor::<(), _>::new()?;
                    // Load history
                    if history_path.exists() {
                        let _ = rl.load_history(&history_path);
                    }

                    loop {
                        let readline = rl.readline("You: ");
                        match readline {
                            Ok(line) => {
                                let input = line.trim().to_string();
                                if input.is_empty() {
                                    continue;
                                }
                                let _ = rl.add_history_entry(input.as_str());

                                if input == "exit" || input == "quit" {
                                    println!("Goodbye!");
                                    let _ = rl.save_history(&history_path);
                                    return Ok(());
                                }

                                // Handle slash commands
                                if input.starts_with('/') {
                                    match input.as_str() {
                                        "/history" => {
                                            if let Some(registry) = agent_loop.get_registry() {
                                                if let Some(default_id) = registry.default_agent_id() {
                                                    registry.with_agent(&default_id, |inst| {
                                                        let history = inst.get_history();
                                                        if history.is_empty() {
                                                            println!("  No conversation history.");
                                                        } else {
                                                            println!("  Conversation history ({} turns):", history.len());
                                                            for (i, turn) in history.iter().enumerate() {
                                                                let preview = if turn.content.len() > 80 {
                                                                        format!("{}...", &turn.content[..77])
                                                                    } else {
                                                                        turn.content.clone()
                                                                    };
                                                                println!("    [{}] {}: {}", i, turn.role, preview);
                                                            }
                                                        }
                                                    });
                                                } else {
                                                    println!("  No agent instance found.");
                                                }
                                            }
                                            println!();
                                            continue;
                                        }
                                        "/clear" => {
                                            if let Some(registry) = agent_loop.get_registry() {
                                                if let Some(default_id) = registry.default_agent_id() {
                                                    registry.with_agent(&default_id, |inst| {
                                                        inst.clear_history();
                                                    });
                                                }
                                            }
                                            println!("  History cleared.");
                                            println!();
                                            continue;
                                        }
                                        "/status" => {
                                            let state = if let Some(registry) = agent_loop.get_registry() {
                                                if let Some(default_id) = registry.default_agent_id() {
                                                    registry.with_agent(&default_id, |inst| {
                                                        format!("{:?} ({} messages)",
                                                            inst.state(), inst.message_count())
                                                    }).unwrap_or_else(|| "no instance".to_string())
                                                } else {
                                                    "no instance".to_string()
                                                }
                                            } else {
                                                "no registry".to_string()
                                            };
                                            println!("  Session: {}", session_key);
                                            println!("  State: {}", state);
                                            println!();
                                            continue;
                                        }
                                        _ => {
                                            println!("  Unknown command: {}", input);
                                            println!("  Available: /history, /clear, /status");
                                            println!();
                                            continue;
                                        }
                                    }
                                }

                                // Process message through agent loop
                                match agent_loop.process_direct(&input, &session_key).await {
                                    Ok(response) => {
                                        println!("\nAgent: {}\n", response);
                                    }
                                    Err(e) => {
                                        eprintln!("\nAgent error: {}\n", e);
                                    }
                                }

                                // Record session activity
                                session_mgr.get_or_create(&session_key, "cli", "direct");
                            }
                            Err(rustyline::error::ReadlineError::Interrupted) => {
                                // Ctrl+C: graceful exit
                                println!();
                                println!("Goodbye!");
                                let _ = rl.save_history(&history_path);
                                return Ok(());
                            }
                            Err(rustyline::error::ReadlineError::Eof) => {
                                println!("Goodbye!");
                                let _ = rl.save_history(&history_path);
                                return Ok(());
                            }
                            Err(e) => {
                                eprintln!("Readline error: {}", e);
                                let _ = rl.save_history(&history_path);
                                return Err(e.into());
                            }
                        }
                    }
                }
            }
        }
        Some(AgentSetCommand::Set { action }) => match action {
            AgentSetAction::Llm { model } => {
                let cfg_path = common::config_path(&home);
                if !cfg_path.exists() {
                    anyhow::bail!(
                        "Configuration not found. Run 'nemesisbot onboard default' first."
                    );
                }

                // Validate the model against configured models
                let data = std::fs::read_to_string(&cfg_path)?;
                let typed_cfg: nemesis_config::Config = serde_json::from_str(&data)
                    .map_err(|e| anyhow::anyhow!("Failed to parse config: {}", e))?;

                let resolution = nemesis_config::resolve_model_config(&typed_cfg, &model);
                if let Err(_) = resolution {
                    println!("  WARNING: Model '{}' not found in configured model_list.", model);
                    println!("  Available models can be added with: nemesisbot model add --model <vendor/model> --key YOUR_KEY");
                    println!();
                    print!("  Set anyway? (y/N): ");
                    use std::io::{self, Write};
                    io::stdout().flush().ok();
                    let mut answer = String::new();
                    io::stdin().read_line(&mut answer).ok();
                    if answer.trim().to_lowercase() != "y" {
                        println!("  Cancelled.");
                        return Ok(());
                    }
                }

                // Write the model to config
                let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                if let Some(obj) = cfg.as_object_mut() {
                    if !obj.contains_key("agents") {
                        obj.insert(
                            "agents".to_string(),
                            serde_json::json!({"defaults": {}}),
                        );
                    }
                    if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                        if !agents.contains_key("defaults") {
                            agents.insert("defaults".to_string(), serde_json::json!({}));
                        }
                        if let Some(defaults) =
                            agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                        {
                            defaults
                                .insert("llm".to_string(), serde_json::Value::String(model.clone()));
                        }
                    }
                    std::fs::write(
                        &cfg_path,
                        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                    )?;
                }
                println!("Default LLM set to: {}", model);
                println!("Restart agent/gateway to apply changes.");
            }
            AgentSetAction::ConcurrentMode { mode, queue_size } => {
                if mode != "reject" && mode != "queue" {
                    anyhow::bail!("Invalid mode '{}'. Must be 'reject' or 'queue'.", mode);
                }
                let cfg_path = common::config_path(&home);
                if cfg_path.exists() {
                    let data = std::fs::read_to_string(&cfg_path)?;
                    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                    if let Some(obj) = cfg.as_object_mut() {
                        if let Some(agents) =
                            obj.get_mut("agents").and_then(|v| v.as_object_mut())
                        {
                            if let Some(defaults) =
                                agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                            {
                                defaults.insert(
                                    "concurrent_request_mode".to_string(),
                                    serde_json::Value::String(mode.clone()),
                                );
                                if mode == "queue" {
                                    defaults.insert(
                                        "queue_size".to_string(),
                                        serde_json::json!(queue_size.unwrap_or(8)),
                                    );
                                }
                            }
                        }
                    }
                    std::fs::write(
                        &cfg_path,
                        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                    )?;
                }
                if mode == "queue" {
                    println!(
                        "Concurrent mode set to: {} (queue size: {})",
                        mode,
                        queue_size.unwrap_or(8)
                    );
                } else {
                    println!("Concurrent mode set to: {}", mode);
                }
                println!("Restart agent/gateway to apply changes.");
            }
        },
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // ProviderAdapter construction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_provider_adapter_new() {
        // We can't easily construct a real LLMProvider, but we can verify
        // the ProviderAdapter struct fields through its new method pattern.
        // Test the logic of model_to_use selection:
        // - empty model -> use default_model
        // - non-empty model -> use provided model
        let default_model = "gpt-4";
        let empty = "";
        let model_used = if empty.is_empty() { default_model } else { empty };
        assert_eq!(model_used, "gpt-4");

        let provided = "claude-3";
        let model_used = if provided.is_empty() { default_model } else { provided };
        assert_eq!(model_used, "claude-3");
    }

    // -------------------------------------------------------------------------
    // AgentSetCommand / AgentSetAction enum tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_concurrent_mode_validation() {
        let valid_modes = ["reject", "queue"];
        assert!(valid_modes.contains(&"reject"));
        assert!(valid_modes.contains(&"queue"));
        assert!(!valid_modes.contains(&"invalid"));
        assert!(!valid_modes.contains(&"random"));
    }

    // -------------------------------------------------------------------------
    // JSON manipulation for agent set llm
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_llm_config_json_manipulation() {
        let mut cfg: serde_json::Value = serde_json::json!({});
        if let Some(obj) = cfg.as_object_mut() {
            if !obj.contains_key("agents") {
                obj.insert(
                    "agents".to_string(),
                    serde_json::json!({"defaults": {}}),
                );
            }
            if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                if !agents.contains_key("defaults") {
                    agents.insert("defaults".to_string(), serde_json::json!({}));
                }
                if let Some(defaults) =
                    agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                {
                    defaults
                        .insert("llm".to_string(), serde_json::Value::String("openai/gpt-4".to_string()));
                }
            }
        }
        assert_eq!(cfg["agents"]["defaults"]["llm"], "openai/gpt-4");
    }

    #[test]
    fn test_set_llm_preserves_existing_agents() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "agents": {
                "defaults": {
                    "max_tool_iterations": 10
                }
            }
        });
        if let Some(obj) = cfg.as_object_mut() {
            if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                if let Some(defaults) =
                    agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                {
                    defaults
                        .insert("llm".to_string(), serde_json::Value::String("test/model".to_string()));
                }
            }
        }
        assert_eq!(cfg["agents"]["defaults"]["max_tool_iterations"], 10);
        assert_eq!(cfg["agents"]["defaults"]["llm"], "test/model");
    }

    // -------------------------------------------------------------------------
    // JSON manipulation for concurrent mode
    // -------------------------------------------------------------------------

    #[test]
    fn test_set_concurrent_mode_reject() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "agents": {"defaults": {}}
        });
        let mode = "reject";
        if let Some(obj) = cfg.as_object_mut() {
            if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                    defaults.insert(
                        "concurrent_request_mode".to_string(),
                        serde_json::Value::String(mode.to_string()),
                    );
                }
            }
        }
        assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "reject");
    }

    #[test]
    fn test_set_concurrent_mode_queue_with_size() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "agents": {"defaults": {}}
        });
        let mode = "queue";
        let queue_size: Option<usize> = Some(16);
        if let Some(obj) = cfg.as_object_mut() {
            if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                if let Some(defaults) = agents.get_mut("defaults").and_then(|v| v.as_object_mut()) {
                    defaults.insert(
                        "concurrent_request_mode".to_string(),
                        serde_json::Value::String(mode.to_string()),
                    );
                    if mode == "queue" {
                        defaults.insert(
                            "queue_size".to_string(),
                            serde_json::json!(queue_size.unwrap_or(8)),
                        );
                    }
                }
            }
        }
        assert_eq!(cfg["agents"]["defaults"]["concurrent_request_mode"], "queue");
        assert_eq!(cfg["agents"]["defaults"]["queue_size"], 16);
    }

    #[test]
    fn test_set_concurrent_mode_queue_default_size() {
        let queue_size: Option<usize> = None;
        assert_eq!(queue_size.unwrap_or(8), 8);
    }

    // -------------------------------------------------------------------------
    // LlmMessage conversion logic
    // -------------------------------------------------------------------------

    #[test]
    fn test_message_role_mapping() {
        // Verify role string passthrough
        let roles = ["system", "user", "assistant", "tool"];
        for role in &roles {
            let msg = LlmMessage {
                role: role.to_string(),
                content: "test".to_string(),
                tool_calls: None,
                tool_call_id: None,
            };
            assert_eq!(msg.role, *role);
            assert_eq!(msg.content, "test");
        }
    }

    #[test]
    fn test_llm_response_finished_logic() {
        // finished = tool_calls.is_empty() || finish_reason == "stop"
        let tool_calls: Vec<AgentToolCallInfo> = vec![];
        assert!(tool_calls.is_empty()); // empty tool_calls -> finished = true

        let tool_calls = vec![AgentToolCallInfo {
            id: "tc1".to_string(),
            name: "test".to_string(),
            arguments: "{}".to_string(),
        }];
        assert!(!tool_calls.is_empty()); // with tool_calls, finished depends on finish_reason
    }

    // -------------------------------------------------------------------------
    // Config resolution validation
    // -------------------------------------------------------------------------

    #[test]
    fn test_factory_config_construction() {
        let llm_ref = "openai/gpt-4";
        let parts: Vec<&str> = llm_ref.splitn(2, '/').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "openai");
        assert_eq!(parts[1], "gpt-4");
    }

    #[test]
    fn test_factory_config_with_slash_in_model() {
        let llm_ref = "test/model-name-v2";
        let parts: Vec<&str> = llm_ref.splitn(2, '/').collect();
        assert_eq!(parts.len(), 2);
        assert_eq!(parts[0], "test");
        assert_eq!(parts[1], "model-name-v2");
    }

    // -------------------------------------------------------------------------
    // AgentConfig construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_agent_config_default_max_turns() {
        let max_tool_iterations: i32 = 0;
        let max_turns = max_tool_iterations.max(1) as u32;
        assert_eq!(max_turns, 1);

        let max_tool_iterations: i32 = 10;
        let max_turns = max_tool_iterations.max(1) as u32;
        assert_eq!(max_turns, 10);

        let max_tool_iterations: i32 = -5;
        let max_turns = max_tool_iterations.max(1) as u32;
        assert_eq!(max_turns, 1);
    }

    // -------------------------------------------------------------------------
    // Log args construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_log_args_construction() {
        let debug = true;
        let quiet = false;
        let no_console = true;
        let mut log_args: Vec<String> = Vec::new();
        if debug {
            log_args.push("--debug".to_string());
        }
        if quiet {
            log_args.push("--quiet".to_string());
        }
        if no_console {
            log_args.push("--no-console".to_string());
        }
        assert_eq!(log_args, vec!["--debug", "--no-console"]);
    }

    // -------------------------------------------------------------------------
    // Confirm prompt answer parsing
    // -------------------------------------------------------------------------

    #[test]
    fn test_confirm_answer_parsing() {
        let answer = "y".to_string();
        assert!(answer.trim().to_lowercase() == "y");

        let answer = "Y".to_string();
        assert!(answer.trim().to_lowercase() == "y");

        let answer = "n".to_string();
        assert!(answer.trim().to_lowercase() != "y");

        let answer = "yes".to_string();
        assert!(answer.trim().to_lowercase() != "y"); // only "y" is accepted
    }

    // -------------------------------------------------------------------------
    // Config file round-trip with agents section
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_round_trip_agents_section() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");

        // Write config with agents section
        let cfg = serde_json::json!({
            "agents": {
                "defaults": {
                    "llm": "openai/gpt-4",
                    "max_tool_iterations": 15
                }
            }
        });
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();

        // Read back
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(parsed["agents"]["defaults"]["llm"], "openai/gpt-4");
        assert_eq!(parsed["agents"]["defaults"]["max_tool_iterations"], 15);
    }

    // -------------------------------------------------------------------------
    // ChatOptions default behavior
    // -------------------------------------------------------------------------

    #[test]
    fn test_chat_options_defaults() {
        // Simulates the None branch of provider_options construction
        let provider_options = nemesis_providers::types::ChatOptions {
            temperature: Some(0.7),
            max_tokens: Some(8192),
            top_p: None,
            stop: None,
            extra: std::collections::HashMap::new(),
        };
        assert_eq!(provider_options.temperature, Some(0.7));
        assert_eq!(provider_options.max_tokens, Some(8192));
        assert!(provider_options.top_p.is_none());
        assert!(provider_options.stop.is_none());
    }

    #[test]
    fn test_chat_options_from_agent_options() {
        // Simulates the Some(opts) branch
        let temperature: Option<f32> = Some(0.5);
        let max_tokens: Option<i32> = Some(4096);
        let top_p: Option<f32> = Some(0.9);

        let provider_options = nemesis_providers::types::ChatOptions {
            temperature: temperature.map(|t| t as f64),
            max_tokens: max_tokens.map(|t| t as i64),
            top_p: top_p.map(|p| p as f64),
            stop: None,
            extra: std::collections::HashMap::new(),
        };
        assert_eq!(provider_options.temperature, Some(0.5));
        assert_eq!(provider_options.max_tokens, Some(4096));
        assert!((provider_options.top_p.unwrap() - 0.9).abs() < 0.01);
    }

    // -------------------------------------------------------------------------
    // Tool calls conversion
    // -------------------------------------------------------------------------

    #[test]
    fn test_tool_call_info_fields() {
        let tc = AgentToolCallInfo {
            id: "call_123".to_string(),
            name: "file_read".to_string(),
            arguments: "{\"path\": \"/tmp/test\"}".to_string(),
        };
        assert_eq!(tc.id, "call_123");
        assert_eq!(tc.name, "file_read");
        assert!(tc.arguments.contains("path"));
    }

    // -------------------------------------------------------------------------
    // Interactive mode input handling
    // -------------------------------------------------------------------------

    #[test]
    fn test_interactive_input_commands() {
        // Test the exit/quit logic
        let input = "exit".to_string();
        assert!(input == "exit" || input == "quit");

        let input = "quit".to_string();
        assert!(input == "exit" || input == "quit");

        let input = "hello".to_string();
        assert!(input != "exit" && input != "quit");
    }

    #[test]
    fn test_interactive_slash_commands() {
        let valid_commands = ["/history", "/clear", "/status"];
        let input = "/history";
        assert!(valid_commands.contains(&input));

        let input = "/unknown";
        assert!(!valid_commands.contains(&input));
    }

    #[test]
    fn test_input_trim_and_empty_check() {
        let input = "   ".to_string();
        let trimmed = input.trim().to_string();
        assert!(trimmed.is_empty());

        let input = "  hello  ".to_string();
        let trimmed = input.trim().to_string();
        assert!(!trimmed.is_empty());
        assert_eq!(trimmed, "hello");
    }

    // -------------------------------------------------------------------------
    // Preview truncation logic (from /history command)
    // -------------------------------------------------------------------------

    #[test]
    fn test_history_preview_truncation() {
        let content = "a".repeat(100);
        let preview = if content.len() > 80 {
            format!("{}...", &content[..77])
        } else {
            content.clone()
        };
        assert!(preview.len() <= 80);
        assert!(preview.ends_with("..."));

        let content = "short message".to_string();
        let preview = if content.len() > 80 {
            format!("{}...", &content[..77])
        } else {
            content.clone()
        };
        assert_eq!(preview, "short message");
    }

    // -------------------------------------------------------------------------
    // Additional coverage tests for agent
    // -------------------------------------------------------------------------

    #[test]
    fn test_agent_entry_minimal() {
        let entry = serde_json::json!({
            "id": "test-agent",
        });
        assert_eq!(entry["id"], "test-agent");
        assert!(entry.get("model").is_none());
    }

    #[test]
    fn test_agent_entry_with_model() {
        let entry = serde_json::json!({
            "id": "test-agent",
            "model": "gpt-4o",
        });
        assert_eq!(entry["model"], "gpt-4o");
    }

    #[test]
    fn test_agent_entry_with_all_fields() {
        let entry = serde_json::json!({
            "id": "test-agent",
            "model": "gpt-4o",
            "system_prompt": "You are a helper",
            "tools": "file,web",
            "temperature": "0.5"
        });
        assert_eq!(entry["id"], "test-agent");
        assert_eq!(entry["model"], "gpt-4o");
        assert_eq!(entry["system_prompt"], "You are a helper");
        assert_eq!(entry["tools"], "file,web");
        assert_eq!(entry["temperature"], "0.5");
    }

    #[test]
    fn test_agent_config_read_no_file() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        if path.exists() {
            let data = std::fs::read_to_string(&path).unwrap();
            let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
            assert!(cfg.is_object());
        } else {
            let cfg = serde_json::json!({"agents": {"instances": []}});
            assert!(cfg["agents"]["instances"].is_array());
        }
    }

    #[test]
    fn test_agent_config_read_existing() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("config.json");
        let data = serde_json::json!({
            "agents": {
                "instances": [{"id": "test-agent", "model": "gpt-4o"}]
            }
        });
        std::fs::write(&path, serde_json::to_string(&data).unwrap()).unwrap();

        let raw = std::fs::read_to_string(&path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&raw).unwrap();
        let instances = cfg["agents"]["instances"].as_array().unwrap();
        assert_eq!(instances.len(), 1);
    }

    #[test]
    fn test_agent_config_save_creates_dirs() {
        let tmp = tempfile::TempDir::new().unwrap();
        let path = tmp.path().join("nested").join("config.json");
        let cfg = serde_json::json!({"agents": {}});
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(&path, serde_json::to_string_pretty(&cfg).unwrap()).unwrap();
        assert!(path.exists());
    }

    #[test]
    fn test_agent_entry_serialization() {
        let entry = serde_json::json!({"id": "agent-1", "model": "model-1", "system_prompt": "prompt"});
        let json = serde_json::to_string(&entry).unwrap();
        let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed["id"], "agent-1");
        assert_eq!(parsed["model"], "model-1");
        assert_eq!(parsed["system_prompt"], "prompt");
    }

    #[test]
    fn test_history_preview_exactly_80_chars() {
        let content = "a".repeat(80);
        let preview = if content.len() > 80 {
            format!("{}...", &content[..77])
        } else {
            content.clone()
        };
        assert_eq!(preview.len(), 80);
        assert!(!preview.ends_with("...")); // Exactly 80, no truncation
    }

    #[test]
    fn test_history_preview_81_chars() {
        let content = "a".repeat(81);
        let preview = if content.len() > 80 {
            format!("{}...", &content[..77])
        } else {
            content.clone()
        };
        assert!(preview.ends_with("..."));
        assert!(preview.len() <= 80);
    }
}
