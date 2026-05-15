//! NemesisBot CLI entry point.
//!
//! Routes all commands to their respective handler modules.

mod commands;
mod common;
mod embedded;
mod adapters;

use clap::{Parser, Subcommand};
use anyhow::Result;

// Embed all config templates at compile time (mirrors Go's //go:embed config)
const CONFIG_DEFAULT: &str = include_str!("../config/config.default.json");
const CONFIG_MCP_DEFAULT: &str = include_str!("../config/config.mcp.default.json");
const CONFIG_CLUSTER_DEFAULT: &str = include_str!("../config/config.cluster.default.json");
const CONFIG_SKILLS_DEFAULT: &str = include_str!("../config/config.skills.default.json");
const CONFIG_SCANNER_DEFAULT: &str = include_str!("../config/config.scanner.default.json");
const CONFIG_SECURITY_WINDOWS: &str = include_str!("../config/config.security.windows.json");
const CONFIG_SECURITY_LINUX: &str = include_str!("../config/config.security.linux.json");
const CONFIG_SECURITY_DARWIN: &str = include_str!("../config/config.security.darwin.json");
const CONFIG_SECURITY_OTHER: &str = include_str!("../config/config.security.other.json");

// Embed personality files at compile time
const DEFAULT_IDENTITY: &str = include_str!("../default/IDENTITY.md");
const DEFAULT_SOUL: &str = include_str!("../default/SOUL.md");
const DEFAULT_USER: &str = include_str!("../default/USER.md");

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
    Forge {
        #[command(subcommand)]
        action: commands::forge::ForgeAction,
    },
    /// Manage DAG workflows
    Workflow {
        #[command(subcommand)]
        action: commands::workflow::WorkflowAction,
    },
    /// Manage virus scanner
    Scanner {
        #[command(subcommand)]
        action: commands::scanner::ScannerAction,
    },
    /// Graceful shutdown
    Shutdown,
    /// Run as a background daemon
    Daemon {
        #[command(subcommand)]
        action: commands::daemon::DaemonAction,
    },
    /// Migrate from OpenClaw
    Migrate {
        #[command(flatten)]
        options: commands::migrate::MigrateOptions,
    },
    /// Show version information
    Version,
    /// Internal test commands (hidden)
    #[command(hide = true)]
    Test {
        #[command(subcommand)]
        action: commands::test_cmd::TestAction,
    },
}

#[tokio::main]
async fn main() -> Result<()> {
    // Early check for child mode (--multiple flag) before any CLI parsing.
    // This allows the parent process to self-spawn a child that loads plugin-ui.dll.
    if nemesis_desktop::child_mode::has_child_mode_flag() {
        match nemesis_desktop::child_mode::run_child_mode().await {
            Ok(()) => return Ok(()),
            Err(e) => {
                eprintln!("[Child] Error: {}", e);
                std::process::exit(1);
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

    // Initialize tracing/logger with config-based setup.
    // Note: tracing_subscriber::fmt::init() is now called inside
    // init_logger_from_config if not in quiet mode. For commands that
    // don't use agent (which calls init_logger), we init a default here.
    let _ = tracing_subscriber::fmt::try_init();

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

            // --- Step 4: Cluster config (embedded, with node ID injection) ---
            let cluster_cfg_path = common::cluster_config_path(&home);
            match serde_json::from_str::<serde_json::Value>(CONFIG_CLUSTER_DEFAULT) {
                Ok(mut cluster_cfg) => {
                    // Inject dynamic node ID
                    let hostname = std::env::var("COMPUTERNAME")
                        .or_else(|_| std::env::var("HOSTNAME"))
                        .unwrap_or_else(|_| "node".to_string());
                    let timestamp = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap_or_default()
                        .as_secs();
                    let node_id = format!("node-{}-{}", hostname.to_lowercase(), timestamp);
                    if let Some(obj) = cluster_cfg.as_object_mut() {
                        obj.insert("node_id".to_string(), serde_json::Value::String(node_id.clone()));
                        obj.insert("name".to_string(), serde_json::Value::String(format!("Bot {}", node_id)));
                        obj.insert("token".to_string(), serde_json::Value::String(uuid::Uuid::new_v4().to_string()));
                    }
                    let _ = std::fs::write(&cluster_cfg_path, serde_json::to_string_pretty(&cluster_cfg).unwrap_or_default());
                }
                Err(_) => {
                    let _ = std::fs::write(&cluster_cfg_path, CONFIG_CLUSTER_DEFAULT);
                }
            }
            println!("  Cluster config created");

            // --- Step 5: Cluster peers.toml with proper sections ---
            // Always (re)create peers.toml — mirrors Go's initializeClusterConfig().
            // Path: workspace/cluster/peers.toml (NOT home/cluster/).
            {
                let cluster_dir = common::cluster_dir(&home);
                let _ = std::fs::create_dir_all(&cluster_dir);
                let peers_path = cluster_dir.join("peers.toml");
                let hostname = std::env::var("COMPUTERNAME")
                    .or_else(|_| std::env::var("HOSTNAME"))
                    .unwrap_or_else(|_| "node".to_string());
                let node_id = format!("node-{}-{}", hostname.to_lowercase(), uuid::Uuid::new_v4());
                let peers_content = format!(
                    "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[cluster]\nid = \"{}\"\nauto_discovery = true\nlast_updated = \"{}\"\n\n[node]\nid = \"{}\"\nname = \"Bot {}\"\naddress = \"\"\nrole = \"worker\"\ncategory = \"general\"\ntags = []\ncapabilities = []\n\n# Add peer entries as [peers.Name] tables, e.g.:\n# [peers.MyPeer]\n# address = \"127.0.0.1:11950\"\n# role = \"worker\"\n# category = \"general\"\n",
                    node_id, chrono::Utc::now().to_rfc3339(), node_id, node_id
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
            println!("  Default personality files installed (IDENTITY.md, SOUL.md, USER.md)");

            // --- Step 10: Create additional directories ---
            let _ = std::fs::create_dir_all(workspace_dir.join("logs"));
            let _ = std::fs::create_dir_all(workspace_dir.join("forge"));
            let _ = std::fs::create_dir_all(workspace_dir.join("workflow"));

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
            commands::status::run(cli.local)?;
        }
        Commands::Channel { action } => {
            commands::channel::run(action, cli.local)?;
        }
        Commands::Cluster { action } => {
            commands::cluster::run(action, cli.local)?;
        }
        Commands::Cors { action } => {
            commands::cors::run(action, cli.local)?;
        }
        Commands::Model { action } => {
            commands::model::run(action, cli.local)?;
        }
        Commands::Cron { action } => {
            commands::cron::run(action, cli.local)?;
        }
        Commands::Mcp { action } => {
            commands::mcp::run(action, cli.local)?;
        }
        Commands::Security { action } => {
            commands::security::run(action, cli.local).await?;
        }
        Commands::Log { action } => {
            commands::log::run(action, cli.local)?;
        }
        Commands::Auth { action } => {
            commands::auth::run(action, cli.local).await?;
        }
        Commands::Skills { action } => {
            commands::skills::run(action, cli.local)?;
        }
        Commands::Forge { action } => {
            commands::forge::run(action, cli.local)?;
        }
        Commands::Workflow { action } => {
            commands::workflow::run(action, cli.local)?;
        }
        Commands::Scanner { action } => {
            commands::scanner::run(action, cli.local).await?;
        }
        Commands::Shutdown => {
            commands::shutdown::run(cli.local)?;
        }
        Commands::Daemon { action } => {
            commands::daemon::run(action, cli.local).await?;
        }
        Commands::Migrate { options } => {
            commands::migrate::run(options, cli.local)?;
        }
        Commands::Version => {
            common::print_version_info();
        }
        Commands::Test { action } => {
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
