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

use crate::common;

use nemesis_agent::session::SessionManager;

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
        /// Mode: reject, queue, or steer
        mode: String,
        /// Queue size (only for queue/steer mode)
        #[arg(long)]
        queue_size: Option<usize>,
    },
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

            // D-3（复核 2026-09-16）：会话日志平化迁移（SAN-01/D4）——REPL
            // 入口与 gateway 同源执行（幂等 best-effort）：process_direct 的
            // 持久化路径读写 session_logs，旧嵌套目录历史先平化再服务。
            nemesis_agent::chat_log::migrate_nested_session_logs();

            // ASM-05（2026-09-16 横扫存量加固）：迁移到 agent_factory 工厂装配
            // （run.rs headless 同范式）——手写装配是安全矩阵的空洞：无
            // security_plugin（8 层管线整体旁路）、无 estop、无 workspace_root
            // （路径重写静默失效）、无 tier/pricing/config_path。ASM-08 启动
            // 断言在工厂内兜底。U15 credentials 全局路径一并补齐（与 run.rs
            // 同源；此前手写路径漏设，credentials.yaml 的 key 解析不到）。
            nemesis_config::credentials::set_global_credentials_path(
                nemesis_config::credentials::credentials_path_for_home(&home),
            );
            // P0 vault（B1）：`vault:<alias>` 解析器同点注入。
            #[cfg(feature = "security")]
            crate::vault_runtime::install(&home);
            let security_enabled = cfg.security.as_ref().map(|s| s.enabled).unwrap_or(true);
            let security_plugin =
                crate::security_setup::build_security_plugin(&home, security_enabled).await;
            // RequestLoggerObserver（logging.llm.enabled）——与 gateway Step 9d
            // 同一 helper（ASM-05 收敛；原手写装配里的第三份逐字拷贝删除）。
            // manager 先建好经 SharedResources 传入（工厂原生路径，Arc 化前
            // 在工厂内部 set，不经 &mut）。
            let observer_mgr = Arc::new(nemesis_observer::Manager::new());
            let has_request_logger =
                crate::agent_factory::register_request_logger_observer(&observer_mgr, &cfg, &home);
            // C-F3（复核 2026-09-16）：恢复 ASM-05 工厂化时丢失的 minimal 装配
            // ——HEAD 手写装配原有 skills_loader/skills_registry/forge(+executor)
            // /workflow_engine/cron_service，工厂化改写时 `..Default::default()`
            // 把四件静默归 None（skills/forge/workflow_run/cron 工具组从 REPL
            // 消失）。全部 minimal 构造（无后台任务）：REPL 短会话只要求工具
            // 注册齐全，不复刻 gateway 的全量初始化。
            let workspace_dir = common::workspace_path(&home);
            let workspace_str = workspace_dir.to_string_lossy().to_string();
            let skills_loader = Arc::new(nemesis_skills::loader::SkillsLoader::new(
                &workspace_str,
                &workspace_dir.join("skills").to_string_lossy(),
                "",
            ));
            // Skills registry from config (light; network only on actual search/install).
            let skills_registry = {
                let p = nemesis_path::resolve_skills_config_path_in_workspace(&workspace_dir);
                if p.exists() {
                    std::fs::read_to_string(&p)
                        .ok()
                        .and_then(|c| {
                            serde_json::from_str::<nemesis_skills::types::RegistryConfig>(&c).ok()
                        })
                        .map(|rc| {
                            std::sync::Arc::new(
                                nemesis_skills::registry::RegistryManager::from_config(rc),
                            )
                        })
                } else {
                    None
                }
            };
            // Minimal forge — Forge::new only; no init_reflector/pipeline/learning/start
            // (those spawn background work). The forge tools still register.
            #[cfg(feature = "forge")]
            let forge = Arc::new(nemesis_forge::forge::Forge::new(
                nemesis_forge::config::ForgeConfig::default(),
                workspace_dir.clone(),
            ));
            #[cfg(feature = "forge")]
            let forge_executor = Arc::new(nemesis_forge::forge_tools::ForgeToolExecutor::new(
                forge.clone(),
            ));
            // Minimal workflow engine — no load_workflows/spawn_cron (no background).
            #[cfg(feature = "workflow")]
            let workflow_engine = Arc::new(nemesis_workflow::engine::WorkflowEngine::new());
            let cron_service = Arc::new(std::sync::Mutex::new(
                nemesis_cron::service::CronService::new(
                    &common::cron_store_path(&home).to_string_lossy(),
                ),
            ));
            let shared = Arc::new(crate::agent_factory::SharedResources {
                home: home.clone(),
                workspace: workspace_dir,
                config_store: Arc::new(nemesis_config::ConfigStore::from_config(
                    cfg.clone(),
                    cfg_path,
                )),
                security_plugin,
                mcp_enabled: cfg.mcp.as_ref().map(|m| m.enabled).unwrap_or(false),
                mcp_config_path: common::mcp_config_path(&home),
                observer_manager: if has_request_logger {
                    Some(observer_mgr)
                } else {
                    None
                },
                cron_service,
                skills_loader: Some(skills_loader),
                skills_registry,
                #[cfg(feature = "forge")]
                forge: Some(forge),
                #[cfg(not(feature = "forge"))]
                forge: None,
                #[cfg(feature = "forge")]
                forge_executor: Some(forge_executor),
                #[cfg(not(feature = "forge"))]
                forge_executor: None,
                #[cfg(feature = "workflow")]
                workflow_engine: Some(workflow_engine),
                #[cfg(not(feature = "workflow"))]
                workflow_engine: None,
                ..Default::default()
            });
            let agent_loop = match crate::agent_factory::build_agent_loop(&shared) {
                Ok(al) => {
                    println!("  OK Agent loop initialized");
                    if has_request_logger {
                        println!("  OK Request logger attached (logging.llm.enabled)");
                    }
                    al
                }
                Err(e) => {
                    eprintln!("  Failed to initialize agent: {}", e);
                    eprintln!();
                    eprintln!("  Note: Agent mode requires a configured LLM model.");
                    eprintln!(
                        "  Run 'nemesisbot model add --model <provider/model> --key YOUR_KEY --default'"
                    );
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
                    let session_mgr = SessionManager::new(std::time::Duration::from_secs(3600));
                    println!("  OK Session manager ready");
                    println!();
                    println!("Interactive mode. Type 'exit' or 'quit' to stop.");
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
                                                if let Some(default_id) =
                                                    registry.default_agent_id()
                                                {
                                                    registry.with_agent(&default_id, |inst| {
                                                        let history = inst.get_history();
                                                        if history.is_empty() {
                                                            println!("  No conversation history.");
                                                        } else {
                                                            println!("  Conversation history ({} turns):", history.len());
                                                            for (i, turn) in history.iter().enumerate() {
                                                                let preview = if turn.content.len() > 80 {
                                                                        let cut = nemesis_types::utils::floor_char_boundary(&turn.content, 77);
                                                                        format!("{}...", &turn.content[..cut])
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
                                            if let Some(registry) = agent_loop.get_registry()
                                                && let Some(default_id) =
                                                    registry.default_agent_id()
                                            {
                                                registry.with_agent(&default_id, |inst| {
                                                    inst.clear_history();
                                                });
                                            }
                                            println!("  History cleared.");
                                            println!();
                                            continue;
                                        }
                                        "/status" => {
                                            let state =
                                                if let Some(registry) = agent_loop.get_registry() {
                                                    if let Some(default_id) =
                                                        registry.default_agent_id()
                                                    {
                                                        registry
                                                            .with_agent(&default_id, |inst| {
                                                                format!(
                                                                    "{:?} ({} messages)",
                                                                    inst.state(),
                                                                    inst.message_count()
                                                                )
                                                            })
                                                            .unwrap_or_else(|| {
                                                                "no instance".to_string()
                                                            })
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
                if resolution.is_err() {
                    println!(
                        "  WARNING: Model '{}' not found in configured model_list.",
                        model
                    );
                    println!(
                        "  Available models can be added with: nemesisbot model add --model <vendor/model> --key YOUR_KEY"
                    );
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
                        obj.insert("agents".to_string(), serde_json::json!({"defaults": {}}));
                    }
                    if let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut()) {
                        if !agents.contains_key("defaults") {
                            agents.insert("defaults".to_string(), serde_json::json!({}));
                        }
                        if let Some(defaults) =
                            agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                        {
                            defaults.insert(
                                "llm".to_string(),
                                serde_json::Value::String(model.clone()),
                            );
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
                // E2: steer accepted — the runtime has supported it since I1;
                // this gate just forgot to allow it (loop.rs parses all three).
                if mode != "reject" && mode != "queue" && mode != "steer" {
                    anyhow::bail!(
                        "Invalid mode '{}'. Must be 'reject', 'queue', or 'steer'.",
                        mode
                    );
                }
                let cfg_path = common::config_path(&home);
                if cfg_path.exists() {
                    let data = std::fs::read_to_string(&cfg_path)?;
                    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                    if let Some(obj) = cfg.as_object_mut()
                        && let Some(agents) = obj.get_mut("agents").and_then(|v| v.as_object_mut())
                        && let Some(defaults) =
                            agents.get_mut("defaults").and_then(|v| v.as_object_mut())
                    {
                        defaults.insert(
                            "concurrent_request_mode".to_string(),
                            serde_json::Value::String(mode.clone()),
                        );
                        // Inbox capacity applies to queue AND steer (both use
                        // the per-session inbox; reject bounces, no inbox).
                        if mode == "queue" || mode == "steer" {
                            defaults.insert(
                                "queue_size".to_string(),
                                serde_json::json!(queue_size.unwrap_or(8)),
                            );
                        }
                    }
                    std::fs::write(
                        &cfg_path,
                        serde_json::to_string_pretty(&cfg).unwrap_or_default(),
                    )?;
                }
                if mode == "queue" || mode == "steer" {
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
mod tests;
