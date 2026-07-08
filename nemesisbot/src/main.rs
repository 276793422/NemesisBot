//! NemesisBot CLI entry point.
//!
//! Routes all commands to their respective handler modules.

mod commands;
mod common;
mod embedded;
mod adapters;
mod agent_factory;
mod exec_worker;
#[cfg(feature = "cluster")]
mod cluster_agent;
#[cfg(feature = "cluster")]
mod cluster_service;
#[cfg(feature = "cluster")]
mod cluster_request_logger_observer;

use clap::{Parser, Subcommand};
use anyhow::Result;

// Embed all config templates at compile time (mirrors Go's //go:embed config)
const CONFIG_DEFAULT: &str = include_str!("../config/config.default.json");
const CONFIG_MCP_DEFAULT: &str = include_str!("../config/config.mcp.default.json");
const CONFIG_CLUSTER_DEFAULT: &str = include_str!("../config/config.cluster.default.json");
const CONFIG_SKILLS_DEFAULT: &str = include_str!("../config/config.skills.default.json");
const CONFIG_SCANNER_DEFAULT: &str = include_str!("../config/config.scanner.default.json");
const CONFIG_ENHANCED_MEMORY_DEFAULT: &str = include_str!("../config/config.enhanced_memory.default.json");
const CONFIG_CHAT_DEFAULT: &str = include_str!("../config/config.chat.default.json");
const CONFIG_FORGE_DEFAULT: &str = include_str!("../config/config.forge.default.json");
const CONFIG_SECURITY_WINDOWS: &str = include_str!("../config/config.security.windows.json");
const CONFIG_SECURITY_LINUX: &str = include_str!("../config/config.security.linux.json");
const CONFIG_SECURITY_DARWIN: &str = include_str!("../config/config.security.darwin.json");
const CONFIG_SECURITY_OTHER: &str = include_str!("../config/config.security.other.json");

// Embed personality files at compile time
const DEFAULT_IDENTITY: &str = include_str!("../default/IDENTITY.md");
const DEFAULT_SOUL: &str = include_str!("../default/SOUL.md");
const DEFAULT_USER: &str = include_str!("../default/USER.md");
const DEFAULT_IDENTITY_CLUSTER: &str = include_str!("../default/IDENTITY_Cluster.md");
#[cfg(feature = "cluster")]
const CLUSTER_IDENTITY_TEMPLATE: &str = include_str!("../config/IDENTITY.cluster.template.md");

#[derive(Parser)]
#[command(name = "nemesisbot", version, about = "NemesisBot - Personal AI Agent System")]
struct Cli {
    /// Use local directory for config (.nemesisbot in current dir)
    #[arg(long)]
    local: bool,

    #[command(subcommand)]
    command: Commands,
}

#[derive(Subcommand)]
enum Commands {
    /// Initialize configuration and workspace
    Onboard {
        /// Use default configuration (also accepts `onboard default` as positional argument)
        #[arg(long, short)]
        default: bool,

        /// Optional subcommand: "default" for default configuration
        #[arg(trailing_var_arg = true)]
        args: Vec<String>,
    },
    /// Start the gateway server
    Gateway {
        /// Enable debug logging
        #[arg(short, long)]
        debug: bool,
        /// Disable all logging
        #[arg(short, long)]
        quiet: bool,
        /// Disable console output (file only)
        #[arg(long)]
        no_console: bool,
    },
    /// Interact with the agent directly
    Agent {
        #[command(subcommand)]
        subcommand: Option<commands::agent::AgentSetCommand>,
        /// Send a single message and exit
        #[arg(short, long)]
        message: Option<String>,
        /// Session key
        #[arg(short, long, default_value = "cli:default")]
        session: String,
        /// Enable debug logging
        #[arg(short, long)]
        debug: bool,
        /// Disable all logging
        #[arg(short, long)]
        quiet: bool,
        /// Disable console output (file only)
        #[arg(long)]
        no_console: bool,
    },
    /// Show system status
    Status,
    /// Manage communication channels
    Channel {
        #[command(subcommand)]
        action: commands::channel::ChannelAction,
    },
    /// Manage bot cluster
    #[cfg(feature = "cluster")]
    Cluster {
        #[command(subcommand)]
        action: commands::cluster::ClusterAction,
    },
    /// Manage CORS configuration
    Cors {
        #[command(subcommand)]
        action: commands::cors::CorsAction,
    },
    /// Manage LLM models
    Model {
        #[command(subcommand)]
        action: commands::model::ModelAction,
    },
    /// Manage scheduled tasks
    Cron {
        #[command(subcommand)]
        action: commands::cron::CronAction,
    },
    /// Manage MCP servers
    Mcp {
        #[command(subcommand)]
        action: commands::mcp::McpAction,
    },
    /// Manage security settings
    #[cfg(feature = "security")]
    Security {
        #[command(subcommand)]
        action: commands::security::SecurityAction,
    },
    /// Manage logging configuration
    Log {
        #[command(subcommand)]
        action: commands::log::LogAction,
    },
    /// Manage authentication
    #[cfg(feature = "auth")]
    Auth {
        #[command(subcommand)]
        action: commands::auth::AuthAction,
    },
    /// Manage skills
    Skills {
        #[command(subcommand)]
        action: commands::skills::SkillsAction,
    },
    /// Manage self-learning module
    #[cfg(feature = "forge")]
    Forge {
        #[command(subcommand)]
        action: commands::forge::ForgeAction,
    },
    /// Manage DAG workflows
    #[cfg(feature = "workflow")]
    Workflow {
        #[command(subcommand)]
        action: commands::workflow::WorkflowAction,
    },
    /// Manage virus scanner
    #[cfg(feature = "security")]
    Scanner {
        #[command(subcommand)]
        action: commands::scanner::ScannerAction,
    },
    /// Sandboxie sandbox management (install / uninstall / status)
    Sandbox {
        #[command(subcommand)]
        action: commands::sandbox::SandboxCommand,
    },
    /// Manage local voice pipeline
    #[cfg(feature = "voice")]
    Voice {
        #[command(subcommand)]
        action: commands::voice::VoiceAction,
    },
    /// Manage enhanced memory
    #[cfg(feature = "memory")]
    Memory {
        #[command(subcommand)]
        action: commands::memory::MemoryAction,
    },
    /// Manage AI personas
    Persona {
        #[command(subcommand)]
        action: commands::persona::PersonaAction,
    },
    /// Graceful shutdown
    Shutdown,
    /// Migrate from OpenClaw
    #[cfg(feature = "migrate")]
    Migrate {
        #[command(flatten)]
        options: commands::migrate::MigrateOptions,
    },
    /// Show version information
    Version,
    /// Open the dashboard UI
    Dashboard,
    /// Internal test commands (hidden)
    #[cfg(feature = "desktop")]
    #[command(hide = true)]
    Test {
        #[command(subcommand)]
        action: commands::test_cmd::TestAction,
    },
}

#[cfg(not(target_os = "macos"))]
#[tokio::main]
async fn main() -> Result<()> {
    // Executor role short-circuit: if spawned as a tool-executor child (env
    // NEMESISBOT_ROLE=executor, set by the gateway's ExecutorChannel when it
    // spawns a child per tool call), run the executor entrypoint instead of CLI
    // dispatch. Must precede Cli::parse_from — the child is spawned with no
    // subcommand. The early return also prevents any fork loop: the child never
    // reaches the gateway code that spawns executors.
    if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("executor") {
        return exec_worker::run().await;
    }

    // Early check for child mode (--multiple flag) before any CLI parsing.
    // This allows the parent process to self-spawn a child that loads plugin-ui.dll.
    #[cfg(feature = "desktop")]
    {
        if nemesis_desktop::child_mode::has_child_mode_flag() {
            match nemesis_desktop::child_mode::run_child_mode().await {
                Ok(()) => return Ok(()),
                Err(e) => {
                    eprintln!("[Child] Error: {}", e);
                    std::process::exit(1);
                }
            }
        }
    }

    // Pre-parse --local from all args (Go-compatible: --local can appear anywhere).
    // Go strips --local from os.Args before command dispatch, so we do the same.
    let mut local_mode = false;
    let filtered_args: Vec<String> = std::env::args()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();

    let mut cli = Cli::parse_from(filtered_args);
    if local_mode {
        cli.local = true;
        println!("Local mode enabled: using ./.nemesisbot");
    }

    // Lazy logging initialization:
    // Commands like gateway/agent call init_logger_from_config() internally
    // which reads config.json and configures tracing properly. We must NOT init
    // a global subscriber here because tracing only allows ONE global init — if we
    // called try_init() here, the config-based init in those commands would silently
    // fail and all logging configuration (level, console, file) would be ignored.
    //
    // Instead, we use a helper function that inits a default subscriber only once
    // and is called by commands that don't have their own config-based init.

    run_command(cli).await
}

/// macOS entry point.
///
/// winit's `EventLoop` must be created and run on the process main thread on
/// macOS (no `with_any_thread` escape hatch). We therefore cannot use
/// `#[tokio::main]` (which owns the main thread for its runtime): instead we
/// build a multi-thread runtime manually, run the gateway on a worker thread,
/// and hand the system tray to the main thread so its event loop runs there.
#[cfg(target_os = "macos")]
fn main() -> Result<()> {
    // Executor role short-circuit (see the non-mac entry for rationale). macOS
    // main is sync, so drive the async executor entrypoint on a current-thread
    // runtime. The executor never needs the main-thread tray handoff.
    if std::env::var("NEMESISBOT_ROLE").as_deref() == Ok("executor") {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return rt.block_on(exec_worker::run());
    }

    // macOS: winit's EventLoop must run on the process main thread. We run the
    // gateway on a dedicated OS thread with its OWN multi-thread runtime and
    // drive it via `block_on` (NOT `tokio::spawn`), so the gateway's future
    // does NOT have to be `Send` — preserving the same property the old
    // `#[tokio::main]`'s `block_on` had (gateway::run holds std MutexGuards
    // across awaits, e.g. CronService). The main thread stays free for the tray.

    // Child mode must run on the main thread (wry/tao also require it on macOS).
    #[cfg(feature = "desktop")]
    if nemesis_desktop::child_mode::has_child_mode_flag() {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        return match rt.block_on(nemesis_desktop::child_mode::run_child_mode()) {
            Ok(()) => Ok(()),
            Err(e) => {
                eprintln!("[Child] Error: {}", e);
                std::process::exit(1);
            }
        };
    }

    // Same `--local` pre-parse as the non-mac entry (Go-compatible: `--local`
    // may appear anywhere; we strip it before clap parsing).
    let mut local_mode = false;
    let filtered_args: Vec<String> = std::env::args()
        .filter(|arg| {
            if arg == "--local" {
                local_mode = true;
                false
            } else {
                true
            }
        })
        .collect();
    let mut cli = Cli::parse_from(filtered_args);
    if local_mode {
        cli.local = true;
        println!("Local mode enabled: using ./.nemesisbot");
    }

    // Only the Gateway command needs the main-thread tray handoff.
    if matches!(&cli.command, Commands::Gateway { .. }) {
        let tray_rx = nemesis_desktop::main_thread_handoff::init();

        // Run the gateway on a dedicated thread. It builds its own multi-thread
        // runtime and drives run_command via `block_on`, so run_command's future
        // need not be Send (it never had to be under the old #[tokio::main]).
        let gateway_handle = std::thread::Builder::new()
            .name("nemesisbot-gateway".into())
            .spawn(move || {
                let gw_rt = tokio::runtime::Builder::new_multi_thread()
                    .enable_all()
                    .build()?;
                gw_rt.block_on(run_command(cli))
            })
            .expect("failed to spawn gateway thread");

        // Block the main thread (no runtime needed — std channel) until the
        // gateway hands off the tray, or the gateway thread finishes first. In
        // the early-finish case the gateway's TrayChannelGuard has closed the
        // channel, so recv() returns Err and we skip the tray loop entirely.
        let tray_opt = tray_rx.recv().ok();
        if let Some(tray) = tray_opt {
            // Runs the winit EventLoop on the main thread until el.exit()
            // (quit menu item, or request_exit() from the gateway after cleanup).
            tray.run_on_current_thread();
        }

        // Ensure gateway cleanup completes before the process exits.
        return match gateway_handle.join() {
            Ok(Ok(())) => Ok(()),
            Ok(Err(e)) => Err(e),
            Err(panic_err) => {
                eprintln!("[main:macos] Gateway thread panicked: {:?}", panic_err);
                std::process::exit(1);
            }
        };
    }

    // Non-Gateway commands: run normally on the main thread.
    let rt = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?;
    rt.block_on(run_command(cli))
}

/// Shared command dispatch, used by both the `#[tokio::main]` entry
/// (Windows / Linux) and the macOS manual-runtime entry.
async fn run_command(cli: Cli) -> Result<()> {
    match cli.command {
        Commands::Onboard { default, args } => {
            // Support both `onboard default` (Go-compatible) and `onboard --default`
            let use_default = default || args.iter().any(|a| a == "default");
            let home = common::resolve_home(cli.local);

            if use_default {
                println!("Initializing NemesisBot with default settings...");
            } else {
                println!("Interactive configuration setup...");
            }

            // Platform detection
            let platform = if cfg!(target_os = "windows") { "Windows" }
                else if cfg!(target_os = "macos") { "macOS" }
                else if cfg!(target_os = "linux") { "Linux" }
                else { "Unknown" };
            println!("  Detected platform: {}", platform);
            println!("  Applying platform-specific security rules...");

            // Create directories
            let _ = std::fs::create_dir_all(&home);
            let _ = std::fs::create_dir_all(home.join("workspace"));
            let _ = std::fs::create_dir_all(home.join("workspace").join("config"));
            let _ = std::fs::create_dir_all(common::cluster_dir(&home));

            let cfg_path = common::config_path(&home);
            let workspace_dir = home.join("workspace");

            // --- Step 1: Main config from embedded default ---
            // Determine whether to write main config (with overwrite confirmation)
            let mut write_main_config = true;
            if cfg_path.exists() {
                print!("  Config already exists at {}, overwrite? (y/N): ", cfg_path.display());
                use std::io::{self as std_io, Write as StdWrite};
                std_io::stdout().flush().ok();
                let mut answer = String::new();
                std_io::stdin().read_line(&mut answer).ok();
                if answer.trim().to_lowercase() != "y" {
                    println!("  Skipping main config (keeping existing).");
                    write_main_config = false;
                }
            }

            if write_main_config {
                // Use compile-time embedded config (always available)
                match serde_json::from_str::<serde_json::Value>(CONFIG_DEFAULT) {
                    Ok(mut cfg) => {
                        // Enable LLM logging
                        if let Some(logging) = cfg.get_mut("logging").and_then(|v| v.get_mut("llm")) {
                            if let Some(obj) = logging.as_object_mut() {
                                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                                obj.insert("log_dir".to_string(), serde_json::Value::String("logs/request_logs".to_string()));
                                obj.insert("detail_level".to_string(), serde_json::Value::String("full".to_string()));
                            }
                        }
                        println!("  LLM logging enabled");

                        // Enable security
                        if let Some(security) = cfg.get_mut("security") {
                            if let Some(obj) = security.as_object_mut() {
                                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                            }
                        } else {
                            if let Some(obj) = cfg.as_object_mut() {
                                obj.insert("security".to_string(), serde_json::json!({"enabled": true}));
                            }
                        }
                        println!("  Security module enabled");

                        // Disable workspace restriction (security module enforces rules)
                        if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults")) {
                            if let Some(obj) = agents.as_object_mut() {
                                obj.insert("restrict_to_workspace".to_string(), serde_json::Value::Bool(false));
                                if cli.local {
                                    obj.insert("workspace".to_string(), serde_json::Value::String(".nemesisbot/workspace".to_string()));
                                }
                            }
                        }

                        // Set web auth token, port, websocket
                        if let Some(web) = cfg.pointer_mut("/channels/web") {
                            if let Some(obj) = web.as_object_mut() {
                                obj.insert("auth_token".to_string(), serde_json::Value::String("276793422".to_string()));
                                obj.insert("host".to_string(), serde_json::Value::String("127.0.0.1".to_string()));
                                obj.insert("port".to_string(), serde_json::Value::Number(49000.into()));
                            }
                        }
                        if let Some(ws) = cfg.pointer_mut("/channels/websocket") {
                            if let Some(obj) = ws.as_object_mut() {
                                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                            }
                        }

                        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                        println!("  Main config saved to .nemesisbot/config.json");
                    }
                    Err(_) => {
                        write_fallback_config(&cfg_path)?;
                    }
                }
            }

            // --- Step 2: MCP config (embedded) ---
            let mcp_cfg_path = common::mcp_config_path(&home);
            let _ = std::fs::write(&mcp_cfg_path, CONFIG_MCP_DEFAULT);
            println!("  MCP config created");

            // --- Step 3: Security config (platform-specific, embedded) ---
            let security_cfg_path = common::security_config_path(&home);
            let _ = std::fs::create_dir_all(security_cfg_path.parent().unwrap());
            let security_content = if cfg!(target_os = "windows") {
                CONFIG_SECURITY_WINDOWS
            } else if cfg!(target_os = "macos") {
                CONFIG_SECURITY_DARWIN
            } else if cfg!(target_os = "linux") {
                CONFIG_SECURITY_LINUX
            } else {
                CONFIG_SECURITY_OTHER
            };
            let _ = std::fs::write(&security_cfg_path, security_content);
            println!("  Security config created");

            // --- Step 4: Cluster config (system params + UDP discovery token) ---
            // config.cluster.json 不再含身份字段（name/role/node_id 等），
            // 身份信息全部由 peers.toml 的 [node] 段承载。
            let cluster_cfg_path = common::cluster_config_path(&home);
            match serde_json::from_str::<serde_json::Value>(CONFIG_CLUSTER_DEFAULT) {
                Ok(mut cluster_cfg) => {
                    if let Some(obj) = cluster_cfg.as_object_mut() {
                        obj.insert("token".to_string(), serde_json::Value::String(uuid::Uuid::new_v4().to_string()));
                    }
                    let _ = std::fs::write(&cluster_cfg_path, serde_json::to_string_pretty(&cluster_cfg).unwrap_or_default());
                }
                Err(_) => {
                    let _ = std::fs::write(&cluster_cfg_path, CONFIG_CLUSTER_DEFAULT);
                }
            }
            println!("  Cluster config created");

            // --- Step 5: Cluster peers.toml (本节点身份) ---
            // peers.toml 只含 [node] 段（本节点身份）+ 可选的 [peers.X] 静态条目。
            // 不再含 [cluster] 段（cluster 元数据已下线）。
            {
                let cluster_dir = common::cluster_dir(&home);
                let _ = std::fs::create_dir_all(&cluster_dir);
                let peers_path = cluster_dir.join("peers.toml");
                let hostname = std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_else(|_| "node".to_string());
                let node_id = format!("node-{}-{}", hostname.to_lowercase(), uuid::Uuid::new_v4());
                let peers_content = format!(
                    "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[node]\nid = \"{}\"\nname = \"Bot {}\"\naddress = \"\"\nrole = \"worker\"\ncategory = \"general\"\ntags = []\ncapabilities = []\n\n# Add peer entries as [peers.Name] tables, e.g.:\n# [peers.MyPeer]\n# address = \"127.0.0.1:11950\"\n# role = \"worker\"\n# category = \"general\"\n",
                    node_id, node_id
                );
                let _ = std::fs::write(&peers_path, peers_content);
                println!("  Peers config created");
            }

            // --- Step 6: Skills config (embedded — includes GitHub sources) ---
            let skills_cfg_path = common::skills_config_path(&home);
            let _ = std::fs::write(&skills_cfg_path, CONFIG_SKILLS_DEFAULT);
            println!("  Skills config created");

            // --- Step 7: Scanner config (embedded) ---
            let scanner_cfg_path = common::scanner_config_path(&home);
            let _ = std::fs::write(&scanner_cfg_path, CONFIG_SCANNER_DEFAULT);

            // --- Step 7.5: Enhanced Memory config (embedded) ---
            let em_cfg_path = common::enhanced_memory_config_path(&home);
            let _ = std::fs::write(&em_cfg_path, CONFIG_ENHANCED_MEMORY_DEFAULT);
            println!("  Enhanced memory config created");

            // --- Step 7.6: Chat config (embedded) ---
            let chat_cfg_path = common::chat_config_path(&home);
            let _ = std::fs::write(&chat_cfg_path, CONFIG_CHAT_DEFAULT);
            println!("  Chat config created");

            // --- Step 7.7: Forge config (embedded) ---
            let forge_cfg_path = common::forge_config_path(&home);
            let _ = std::fs::write(&forge_cfg_path, CONFIG_FORGE_DEFAULT);
            println!("  Forge config created");

            // --- Step 8: Extract embedded workspace templates ---
            // Mirrors Go's copyEmbeddedToTarget() — copies all files from
            // embedded `workspace/` directory (skills, scripts, memory, md files).
            // Uses overwrite mode so re-onboarding restores corrupted templates.
            match embedded::extract_workspace_templates_overwrite(&workspace_dir) {
                Ok(()) => println!("  Workspace templates extracted"),
                Err(e) => println!("  Warning: failed to extract some workspace templates: {}", e),
            }

            // --- Step 9: Override personality files from default/ (embedded) ---
            // These always overwrite — default/ is the authoritative source for
            // fresh installations (mirrors Go's copyDefaultFiles).
            let _ = std::fs::write(workspace_dir.join("IDENTITY.md"), DEFAULT_IDENTITY);
            let _ = std::fs::write(workspace_dir.join("SOUL.md"), DEFAULT_SOUL);
            let _ = std::fs::write(workspace_dir.join("USER.md"), DEFAULT_USER);
            // Cluster identity — extracted to workspace/cluster/IDENTITY.md.
            let cluster_dir = workspace_dir.join("cluster");
            let _ = std::fs::create_dir_all(&cluster_dir);
            let _ = std::fs::write(cluster_dir.join("IDENTITY.md"), DEFAULT_IDENTITY_CLUSTER);
            println!("  Default personality files installed (IDENTITY.md, SOUL.md, USER.md, cluster/IDENTITY.md)");

            // --- Step 10: Create additional directories ---
            let _ = std::fs::create_dir_all(workspace_dir.join("logs"));
            let _ = std::fs::create_dir_all(workspace_dir.join("forge"));
            // Workflow subdirs: definitions/ (YAML), templates/ (starter
            // templates), checkpoints/ (resume snapshots), executions/ (JSONL
            // run logs). All four are created up-front so the gateway can
            // rely on them existing without each callsite having to mkdir.
            for sub in ["definitions", "templates", "checkpoints", "executions"] {
                let _ = std::fs::create_dir_all(workspace_dir.join("workflow").join(sub));
            }

            // --- Step 10: Delete BOOTSTRAP.md from workspace if it exists ---
            // BOOTSTRAP.md is the bootstrap init file; after onboard default the
            // personality is already set up, so it must be removed (mirrors Go).
            let bootstrap = workspace_dir.join("BOOTSTRAP.md");
            if bootstrap.exists() {
                let _ = std::fs::remove_file(&bootstrap);
                println!("  BOOTSTRAP.md removed");
            }

            // --- Step 11: Web and WebSocket configuration ---
            println!("  Web and WebSocket configuration set");

            println!();
            println!("  Initialization complete!");
            println!();
            println!("  Available interfaces:");
            println!("    Web: http://127.0.0.1:49000 (access key: 276793422)");
            println!("    WebSocket: ws://127.0.0.1:49001/ws");
            println!();
            println!("  Next steps:");
            println!("    1. Add your API key: nemesisbot model add --model <vendor/model> --key <key> --default");
            println!("    2. Start gateway:     nemesisbot gateway");
            println!();
            println!("  MCP servers:");
            println!("    Add MCP servers: nemesisbot mcp add -n <name> -c <command>");
            println!("    List MCP servers: nemesisbot mcp list");
        }
        Commands::Gateway { debug, quiet, no_console } => {
            // Build extra args for logger from flags
            let mut gateway_args: Vec<String> = Vec::new();
            if debug { gateway_args.push("--debug".to_string()); }
            if quiet { gateway_args.push("--quiet".to_string()); }
            if no_console { gateway_args.push("--no-console".to_string()); }
            commands::gateway::run(cli.local, &gateway_args).await?;
        }
        Commands::Agent { subcommand, message, session, debug, quiet, no_console } => {
            commands::agent::run(subcommand, message, session, debug, quiet, no_console, cli.local).await?;
        }
        Commands::Status => {
            common::ensure_default_logger();
            commands::status::run(cli.local)?;
        }
        Commands::Channel { action } => {
            common::ensure_default_logger();
            commands::channel::run(action, cli.local)?;
        }
        #[cfg(feature = "cluster")]
        Commands::Cluster { action } => {
            common::ensure_default_logger();
            commands::cluster::run(action, cli.local).await?;
        }
        Commands::Cors { action } => {
            common::ensure_default_logger();
            commands::cors::run(action, cli.local)?;
        }
        Commands::Model { action } => {
            common::ensure_default_logger();
            commands::model::run(action, cli.local)?;
        }
        Commands::Cron { action } => {
            common::ensure_default_logger();
            commands::cron::run(action, cli.local)?;
        }
        Commands::Mcp { action } => {
            common::ensure_default_logger();
            commands::mcp::run(action, cli.local)?;
        }
        #[cfg(feature = "security")]
        Commands::Security { action } => {
            common::ensure_default_logger();
            commands::security::run(action, cli.local).await?;
        }
        Commands::Log { action } => {
            common::ensure_default_logger();
            commands::log::run(action, cli.local)?;
        }
        #[cfg(feature = "auth")]
        Commands::Auth { action } => {
            common::ensure_default_logger();
            commands::auth::run(action, cli.local).await?;
        }
        Commands::Skills { action } => {
            common::ensure_default_logger();
            commands::skills::run(action, cli.local)?;
        }
        #[cfg(feature = "forge")]
        Commands::Forge { action } => {
            common::ensure_default_logger();
            commands::forge::run(action, cli.local)?;
        }
        #[cfg(feature = "workflow")]
        Commands::Workflow { action } => {
            common::ensure_default_logger();
            commands::workflow::run(action, cli.local)?;
        }
        #[cfg(feature = "security")]
        Commands::Scanner { action } => {
            common::ensure_default_logger();
            commands::scanner::run(action, cli.local).await?;
        }
        Commands::Sandbox { action } => {
            common::ensure_default_logger();
            commands::sandbox::run(action, cli.local).await?;
        }
        #[cfg(feature = "voice")]
        Commands::Voice { action } => {
            common::ensure_default_logger();
            commands::voice::run(action, cli.local)?;
        }
        #[cfg(feature = "memory")]
        Commands::Memory { action } => {
            common::ensure_default_logger();
            commands::memory::run(action, cli.local).await?;
        }
        Commands::Persona { action } => {
            common::ensure_default_logger();
            let home = common::resolve_home(cli.local);
            let workspace = common::workspace_path(&home);
            commands::persona::run(action, &home.to_string_lossy(), &workspace.to_string_lossy()).await?;
        }
        Commands::Shutdown => {
            common::ensure_default_logger();
            commands::shutdown::run(cli.local)?;
        }
        #[cfg(feature = "migrate")]
        Commands::Migrate { options } => {
            common::ensure_default_logger();
            commands::migrate::run(options, cli.local)?;
        }
        Commands::Version => {
            common::ensure_default_logger();
            common::print_version_info();
        }
        Commands::Dashboard => {
            common::ensure_default_logger();
            if let Err(e) = commands::dashboard::run(cli.local).await {
                eprintln!("Error: {}", e);
                std::process::exit(1);
            }
        }
        #[cfg(feature = "desktop")]
        Commands::Test { action } => {
            common::ensure_default_logger();
            commands::test_cmd::run(action).await?;
        }
    }

    Ok(())
}

/// Write fallback minimal config when no embedded config is available.
fn write_fallback_config(cfg_path: &std::path::Path) -> anyhow::Result<()> {
    let default_cfg = serde_json::json!({
        "version": "1.0",
        "default_model": "",
        "model_list": [],
        "channels": {
            "web": {"enabled": true, "host": "127.0.0.1", "port": 49000, "auth_token": "276793422"},
            "websocket": {"enabled": true, "host": "127.0.0.1", "port": 49001},
        },
        "agents": {"defaults": {"restrict_to_workspace": false}},
        "security": {"enabled": true},
        "forge": {"enabled": false},
        "logging": {"llm": {"enabled": true, "log_dir": "logs/request_logs", "detail_level": "full"}},
    });
    std::fs::write(cfg_path, serde_json::to_string_pretty(&default_cfg).unwrap_or_default())?;
    println!("  Main config saved to {}", cfg_path.display());
    Ok(())
}

// Shared lock for env-mutating tests across nemesisbot's test modules
// (common::tests, commands::migrate::tests). Env is process-global → parallel
// tests race on set_var/set_current_dir; every env-mutating test acquires this
// lock so the binary is reliable under default parallel `cargo test`.
#[cfg(test)]
static GLOBAL_STATE_LOCK: std::sync::Mutex<()> = std::sync::Mutex::new(());

#[cfg(test)]
mod tests;
