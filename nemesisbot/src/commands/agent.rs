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
