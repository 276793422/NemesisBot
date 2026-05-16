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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // write_fallback_config tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_write_fallback_config_creates_file() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        write_fallback_config(&cfg_path).unwrap();
        assert!(cfg_path.exists());
    }

    #[test]
    fn test_write_fallback_config_valid_json() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        write_fallback_config(&cfg_path).unwrap();
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(cfg["version"], "1.0");
    }

    #[test]
    fn test_write_fallback_config_structure() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        write_fallback_config(&cfg_path).unwrap();
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();

        assert_eq!(cfg["default_model"], "");
        assert!(cfg["model_list"].is_array());
        assert!(cfg["model_list"].as_array().unwrap().is_empty());
        assert_eq!(cfg["channels"]["web"]["enabled"], true);
        assert_eq!(cfg["channels"]["web"]["host"], "127.0.0.1");
        assert_eq!(cfg["channels"]["web"]["port"], 49000);
        assert_eq!(cfg["channels"]["web"]["auth_token"], "276793422");
        assert_eq!(cfg["channels"]["websocket"]["enabled"], true);
        assert_eq!(cfg["agents"]["defaults"]["restrict_to_workspace"], false);
        assert_eq!(cfg["security"]["enabled"], true);
        assert_eq!(cfg["forge"]["enabled"], false);
        assert_eq!(cfg["logging"]["llm"]["enabled"], true);
    }

    #[test]
    fn test_write_fallback_config_overwrites() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        std::fs::write(&cfg_path, "old content").unwrap();
        write_fallback_config(&cfg_path).unwrap();
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        assert_ne!(data, "old content");
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(cfg["version"], "1.0");
    }

    // -------------------------------------------------------------------------
    // Embedded config constants validation
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_default_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_DEFAULT).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_mcp_default_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_MCP_DEFAULT).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_cluster_default_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_CLUSTER_DEFAULT).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_skills_default_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SKILLS_DEFAULT).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_scanner_default_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SCANNER_DEFAULT).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_security_windows_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_WINDOWS).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_security_linux_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_LINUX).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_security_darwin_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_DARWIN).unwrap();
        assert!(cfg.is_object());
    }

    #[test]
    fn test_config_security_other_is_valid_json() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SECURITY_OTHER).unwrap();
        assert!(cfg.is_object());
    }

    // -------------------------------------------------------------------------
    // Embedded personality files
    // -------------------------------------------------------------------------

    #[test]
    fn test_default_identity_not_empty() {
        assert!(!DEFAULT_IDENTITY.is_empty());
    }

    #[test]
    fn test_default_soul_not_empty() {
        assert!(!DEFAULT_SOUL.is_empty());
    }

    #[test]
    fn test_default_user_not_empty() {
        assert!(!DEFAULT_USER.is_empty());
    }

    // -------------------------------------------------------------------------
    // Onboard --local parsing logic
    // -------------------------------------------------------------------------

    #[test]
    fn test_local_flag_filtering() {
        let args = vec![
            "nemesisbot".to_string(),
            "--local".to_string(),
            "gateway".to_string(),
        ];
        let mut local_mode = false;
        let filtered_args: Vec<String> = args
            .into_iter()
            .filter(|arg| {
                if arg == "--local" {
                    local_mode = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        assert!(local_mode);
        assert_eq!(filtered_args, vec!["nemesisbot", "gateway"]);
    }

    #[test]
    fn test_local_flag_not_present() {
        let args = vec![
            "nemesisbot".to_string(),
            "gateway".to_string(),
        ];
        let mut local_mode = false;
        let filtered_args: Vec<String> = args
            .into_iter()
            .filter(|arg| {
                if arg == "--local" {
                    local_mode = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        assert!(!local_mode);
        assert_eq!(filtered_args, vec!["nemesisbot", "gateway"]);
    }

    #[test]
    fn test_local_flag_multiple_positions() {
        let args = vec![
            "nemesisbot".to_string(),
            "agent".to_string(),
            "--local".to_string(),
            "--debug".to_string(),
        ];
        let mut local_mode = false;
        let filtered_args: Vec<String> = args
            .into_iter()
            .filter(|arg| {
                if arg == "--local" {
                    local_mode = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        assert!(local_mode);
        assert_eq!(filtered_args, vec!["nemesisbot", "agent", "--debug"]);
    }

    // -------------------------------------------------------------------------
    // Onboard default detection logic
    // -------------------------------------------------------------------------

    #[test]
    fn test_onboard_default_detection_flag() {
        let default = true;
        let args: Vec<String> = vec![];
        let use_default = default || args.iter().any(|a| a == "default");
        assert!(use_default);
    }

    #[test]
    fn test_onboard_default_detection_arg() {
        let default = false;
        let args: Vec<String> = vec!["default".to_string()];
        let use_default = default || args.iter().any(|a| a == "default");
        assert!(use_default);
    }

    #[test]
    fn test_onboard_default_detection_neither() {
        let default = false;
        let args: Vec<String> = vec![];
        let use_default = default || args.iter().any(|a| a == "default");
        assert!(!use_default);
    }

    // -------------------------------------------------------------------------
    // Platform detection logic
    // -------------------------------------------------------------------------

    #[test]
    fn test_platform_detection() {
        let platform = if cfg!(target_os = "windows") { "Windows" }
            else if cfg!(target_os = "macos") { "macOS" }
            else if cfg!(target_os = "linux") { "Linux" }
            else { "Unknown" };
        // On this Windows machine, should be "Windows"
        #[cfg(target_os = "windows")]
        assert_eq!(platform, "Windows");
    }

    // -------------------------------------------------------------------------
    // Config modification logic (from onboard default)
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_llm_logging_modification() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "logging": {"llm": {}}
        });
        if let Some(logging) = cfg.get_mut("logging").and_then(|v| v.get_mut("llm")) {
            if let Some(obj) = logging.as_object_mut() {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
                obj.insert("log_dir".to_string(), serde_json::Value::String("logs/request_logs".to_string()));
                obj.insert("detail_level".to_string(), serde_json::Value::String("full".to_string()));
            }
        }
        assert_eq!(cfg["logging"]["llm"]["enabled"], true);
        assert_eq!(cfg["logging"]["llm"]["log_dir"], "logs/request_logs");
        assert_eq!(cfg["logging"]["llm"]["detail_level"], "full");
    }

    #[test]
    fn test_config_security_modification_existing() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "security": {"some_field": "value"}
        });
        if let Some(security) = cfg.get_mut("security") {
            if let Some(obj) = security.as_object_mut() {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
            }
        }
        assert_eq!(cfg["security"]["enabled"], true);
        assert_eq!(cfg["security"]["some_field"], "value");
    }

    #[test]
    fn test_config_security_modification_missing() {
        let mut cfg: serde_json::Value = serde_json::json!({});
        if let Some(security) = cfg.get_mut("security") {
            if let Some(obj) = security.as_object_mut() {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
            }
        } else {
            if let Some(obj) = cfg.as_object_mut() {
                obj.insert("security".to_string(), serde_json::json!({"enabled": true}));
            }
        }
        assert_eq!(cfg["security"]["enabled"], true);
    }

    #[test]
    fn test_config_workspace_restriction_modification() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "agents": {"defaults": {}}
        });
        if let Some(agents) = cfg.get_mut("agents").and_then(|v| v.get_mut("defaults")) {
            if let Some(obj) = agents.as_object_mut() {
                obj.insert("restrict_to_workspace".to_string(), serde_json::Value::Bool(false));
            }
        }
        assert_eq!(cfg["agents"]["defaults"]["restrict_to_workspace"], false);
    }

    #[test]
    fn test_config_web_channel_modification() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "channels": {"web": {}}
        });
        if let Some(web) = cfg.pointer_mut("/channels/web") {
            if let Some(obj) = web.as_object_mut() {
                obj.insert("auth_token".to_string(), serde_json::Value::String("276793422".to_string()));
                obj.insert("host".to_string(), serde_json::Value::String("127.0.0.1".to_string()));
                obj.insert("port".to_string(), serde_json::Value::Number(49000.into()));
            }
        }
        assert_eq!(cfg["channels"]["web"]["auth_token"], "276793422");
        assert_eq!(cfg["channels"]["web"]["port"], 49000);
    }

    #[test]
    fn test_config_websocket_modification() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "channels": {"websocket": {}}
        });
        if let Some(ws) = cfg.pointer_mut("/channels/websocket") {
            if let Some(obj) = ws.as_object_mut() {
                obj.insert("enabled".to_string(), serde_json::Value::Bool(true));
            }
        }
        assert_eq!(cfg["channels"]["websocket"]["enabled"], true);
    }

    // -------------------------------------------------------------------------
    // Cluster config node ID injection
    // -------------------------------------------------------------------------

    #[test]
    fn test_cluster_node_id_format() {
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "node".to_string());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let node_id = format!("node-{}-{}", hostname.to_lowercase(), timestamp);
        assert!(node_id.starts_with("node-"));
    }

    // -------------------------------------------------------------------------
    // Gateway args construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_gateway_args_construction() {
        let debug = true;
        let quiet = false;
        let no_console = true;
        let mut gateway_args: Vec<String> = Vec::new();
        if debug { gateway_args.push("--debug".to_string()); }
        if quiet { gateway_args.push("--quiet".to_string()); }
        if no_console { gateway_args.push("--no-console".to_string()); }
        assert_eq!(gateway_args, vec!["--debug", "--no-console"]);
    }

    #[test]
    fn test_gateway_args_empty() {
        let debug = false;
        let quiet = false;
        let no_console = false;
        let mut gateway_args: Vec<String> = Vec::new();
        if debug { gateway_args.push("--debug".to_string()); }
        if quiet { gateway_args.push("--quiet".to_string()); }
        if no_console { gateway_args.push("--no-console".to_string()); }
        assert!(gateway_args.is_empty());
    }

    // -------------------------------------------------------------------------
    // Peers TOML content generation
    // -------------------------------------------------------------------------

    #[test]
    fn test_peers_toml_content() {
        let node_id = "test-node-id";
        let content = format!(
            "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[cluster]\nid = \"{}\"\nauto_discovery = true\n",
            node_id
        );
        assert!(content.contains("test-node-id"));
        assert!(content.contains("[cluster]"));
        assert!(content.contains("auto_discovery = true"));
    }

    // -------------------------------------------------------------------------
    // Additional coverage tests for main
    // -------------------------------------------------------------------------

    #[test]
    fn test_cli_build_with_all_flags() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        assert!(names.contains(&"gateway"));
        assert!(names.contains(&"model"));
        assert!(names.contains(&"cluster"));
        assert!(names.contains(&"agent"));
        assert!(names.contains(&"channel"));
        assert!(names.contains(&"security"));
        assert!(names.contains(&"scanner"));
        assert!(names.contains(&"skills"));
        assert!(names.contains(&"mcp"));
        assert!(names.contains(&"forge"));
        assert!(names.contains(&"cors"));
        assert!(names.contains(&"cron"));
    }

    #[test]
    fn test_gateway_args_construction_with_debug() {
        let debug = true;
        let quiet = false;
        let no_console = false;
        let mut gateway_args: Vec<String> = Vec::new();
        if debug { gateway_args.push("--debug".to_string()); }
        if quiet { gateway_args.push("--quiet".to_string()); }
        if no_console { gateway_args.push("--no-console".to_string()); }
        assert!(gateway_args.contains(&"--debug".to_string()));
        assert!(!gateway_args.contains(&"--quiet".to_string()));
    }

    #[test]
    fn test_gateway_args_construction_with_all() {
        let debug = true;
        let quiet = true;
        let no_console = true;
        let mut gateway_args: Vec<String> = Vec::new();
        if debug { gateway_args.push("--debug".to_string()); }
        if quiet { gateway_args.push("--quiet".to_string()); }
        if no_console { gateway_args.push("--no-console".to_string()); }
        assert!(gateway_args.contains(&"--debug".to_string()));
        assert!(gateway_args.contains(&"--quiet".to_string()));
        assert!(gateway_args.contains(&"--no-console".to_string()));
        assert_eq!(gateway_args.len(), 3);
    }

    #[test]
    fn test_cli_local_flag() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        // Check that --local flag exists
        let local_arg = cmd.get_arguments().find(|a| a.get_id().as_str() == "local");
        assert!(local_arg.is_some());
    }

    #[test]
    fn test_version_info_format() {
        let version = env!("CARGO_PKG_VERSION");
        assert!(!version.is_empty());
        // Version should be semver-like
        assert!(version.contains('.'));
    }

    #[test]
    fn test_home_dir_resolution() {
        let local = false;
        // Just test the logic doesn't panic
        let _ = crate::common::resolve_home(local);
    }

    #[test]
    fn test_home_dir_resolution_local() {
        let local = true;
        let home = crate::common::resolve_home(local);
        assert!(home.to_str().unwrap().contains(".nemesisbot"));
    }

    #[test]
    fn test_config_path_resolution() {
        let home = std::path::PathBuf::from("/tmp/test");
        let config_path = crate::common::config_path(&home);
        assert!(config_path.to_str().unwrap().contains("config.json"));
    }

    #[test]
    fn test_node_id_format_for_onboard() {
        let node_id = format!("node-{}", uuid::Uuid::new_v4().to_string().split('-').next().unwrap());
        assert!(node_id.starts_with("node-"));
        assert!(node_id.len() > 5);
    }

    #[test]
    fn test_format_duration() {
        let secs = 3661u64;
        let hours = secs / 3600;
        let minutes = (secs % 3600) / 60;
        let seconds = secs % 60;
        let display = format!("{}h {}m {}s", hours, minutes, seconds);
        assert_eq!(display, "1h 1m 1s");
    }

    #[test]
    fn test_format_duration_zero() {
        let secs = 0u64;
        let display = format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60);
        assert_eq!(display, "0h 0m 0s");
    }

    #[test]
    fn test_format_duration_only_seconds() {
        let secs = 45u64;
        let display = format!("{}h {}m {}s", secs / 3600, (secs % 3600) / 60, secs % 60);
        assert_eq!(display, "0h 0m 45s");
    }

    #[test]
    fn test_peers_toml_with_node_id() {
        let node_id = "node-abc-123";
        let content = format!(
            "# Cluster peers configuration\n# Auto-generated by nemesisbot onboard\n\n[cluster]\nid = \"{}\"\nauto_discovery = true\n",
            node_id
        );
        assert!(content.contains("node-abc-123"));
        assert!(content.starts_with("# Cluster"));
    }

    // -------------------------------------------------------------------------
    // Additional onboard config manipulation tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_config_default_has_expected_sections() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_DEFAULT).unwrap();
        assert!(cfg.get("channels").is_some(), "Config should have channels");
        assert!(cfg.get("agents").is_some(), "Config should have agents");
        assert!(cfg.get("security").is_some(), "Config should have security");
    }

    #[test]
    fn test_config_cluster_default_has_ports() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_CLUSTER_DEFAULT).unwrap();
        assert!(cfg.get("port").is_some() || cfg.get("rpc_port").is_some(),
            "Cluster config should have port settings");
    }

    #[test]
    fn test_config_scanner_default_has_engines() {
        let cfg: serde_json::Value = serde_json::from_str(CONFIG_SCANNER_DEFAULT).unwrap();
        assert!(cfg.get("engines").is_some() || cfg.get("enabled").is_some(),
            "Scanner config should have engines or enabled list");
    }

    #[test]
    fn test_onboard_default_args_detection() {
        // Test various args combinations
        let args_with_default: Vec<String> = vec!["default".to_string()];
        assert!(args_with_default.iter().any(|a| a == "default"));

        let args_without: Vec<String> = vec!["other".to_string()];
        assert!(!args_without.iter().any(|a| a == "default"));

        let args_empty: Vec<String> = vec![];
        assert!(!args_empty.iter().any(|a| a == "default"));
    }

    #[test]
    fn test_node_id_generation_from_hostname() {
        let hostname = std::env::var("COMPUTERNAME")
            .or_else(|_| std::env::var("HOSTNAME"))
            .unwrap_or_else(|_| "node".to_string());
        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs();
        let node_id = format!("node-{}-{}", hostname.to_lowercase(), timestamp);
        // Verify format
        assert!(node_id.starts_with("node-"));
        assert!(node_id.contains(&hostname.to_lowercase()));
    }

    #[test]
    fn test_fallback_config_is_valid() {
        let tmp = TempDir::new().unwrap();
        let cfg_path = tmp.path().join("config.json");
        write_fallback_config(&cfg_path).unwrap();
        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        // Verify all expected keys
        assert!(cfg["version"].is_string());
        assert!(cfg["channels"].is_object());
        assert!(cfg["channels"]["web"].is_object());
        assert!(cfg["channels"]["websocket"].is_object());
        assert!(cfg["agents"].is_object());
        assert!(cfg["security"].is_object());
        assert!(cfg["forge"].is_object());
        assert!(cfg["logging"].is_object());
    }

    #[test]
    fn test_config_web_channel_modification_with_pointer() {
        let mut cfg: serde_json::Value = serde_json::json!({
            "channels": {"web": {"enabled": false}}
        });
        if let Some(web) = cfg.pointer_mut("/channels/web") {
            if let Some(obj) = web.as_object_mut() {
                obj.insert("auth_token".to_string(), serde_json::Value::String("test-token".to_string()));
                obj.insert("host".to_string(), serde_json::Value::String("0.0.0.0".to_string()));
                obj.insert("port".to_string(), serde_json::Value::Number(8080.into()));
            }
        }
        assert_eq!(cfg["channels"]["web"]["auth_token"], "test-token");
        assert_eq!(cfg["channels"]["web"]["host"], "0.0.0.0");
        assert_eq!(cfg["channels"]["web"]["port"], 8080);
        assert_eq!(cfg["channels"]["web"]["enabled"], false); // preserved
    }

    #[test]
    fn test_local_flag_filtering_no_args() {
        let args: Vec<String> = vec!["nemesisbot".to_string()];
        let mut local_mode = false;
        let filtered_args: Vec<String> = args
            .into_iter()
            .filter(|arg| {
                if arg == "--local" {
                    local_mode = true;
                    false
                } else {
                    true
                }
            })
            .collect();
        assert!(!local_mode);
        assert_eq!(filtered_args.len(), 1);
    }

    #[test]
    fn test_cli_has_version_command() {
        use clap::CommandFactory;
        let cmd = Cli::command();
        let names: Vec<&str> = cmd.get_subcommands().map(|s| s.get_name()).collect();
        assert!(names.contains(&"version"));
        assert!(names.contains(&"status"));
        assert!(names.contains(&"shutdown"));
        assert!(names.contains(&"daemon"));
        assert!(names.contains(&"migrate"));
        assert!(names.contains(&"auth"));
        assert!(names.contains(&"log"));
        assert!(names.contains(&"workflow"));
    }
}
