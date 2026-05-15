//! Cluster command - manage bot cluster configuration and status.

use anyhow::Result;

use crate::common;

/// Cluster subcommands.
#[derive(clap::Subcommand)]
pub enum ClusterAction {
    /// Show cluster status and configuration
    Status,
    /// Show or modify cluster configuration
    Config {
        /// UDP discovery port
        #[arg(short, long, default_value = "11949")]
        udp_port: u16,
        /// RPC communication port
        #[arg(short, long, default_value = "21949")]
        rpc_port: u16,
        /// Broadcast interval in seconds
        #[arg(short, long, default_value = "30")]
        broadcast_interval: u64,
    },
    /// Show or modify current node information
    Info {
        /// Node name
        #[arg(short, long)]
        name: Option<String>,
        /// Node role (manager/coordinator/worker/observer/standby)
        #[arg(short, long)]
        role: Option<String>,
        /// Node category (design/development/testing/ops/deployment/analysis/general)
        #[arg(short, long)]
        category: Option<String>,
        /// Tags (comma-separated)
        #[arg(short, long)]
        tags: Option<String>,
        /// Node address
        #[arg(short, long)]
        address: Option<String>,
        /// Node capabilities (comma-separated)
        #[arg(long)]
        capabilities: Option<String>,
    },
    /// Manage configured peer nodes
    Peers {
        #[command(subcommand)]
        action: Option<PeerAction>,
    },
    /// Manage RPC authentication token
    Token {
        #[command(subcommand)]
        action: TokenAction,
    },
    /// Initialize cluster configuration
    Init {
        /// Node name
        #[arg(short, long)]
        name: Option<String>,
        /// Node role (manager/coordinator/worker/observer/standby)
        #[arg(short, long)]
        role: Option<String>,
        /// Node category (design/development/testing/ops/deployment/analysis/general)
        #[arg(short, long)]
        category: Option<String>,
        /// Tags (comma-separated)
        #[arg(short, long)]
        tags: Option<String>,
        /// Node address
        #[arg(short, long)]
        address: Option<String>,
        /// Node capabilities (comma-separated)
        #[arg(long)]
        capabilities: Option<String>,
    },
    /// Enable cluster
    Enable,
    /// Disable cluster
    Disable,
    /// Start cluster services (alias for enable)
    Start,
    /// Stop cluster services (alias for disable)
    Stop,
    /// Reset cluster configuration
    Reset {
        /// Hard reset: also clear peers.toml
        #[arg(long)]
        hard: bool,
    },
}

#[derive(clap::Subcommand)]
pub enum PeerAction {
    List,
    Add {
        /// Peer ID (required)
        #[arg(long)]
        id: String,
        /// Peer name
        #[arg(short, long)]
        name: Option<String>,
        /// Peer address
        #[arg(short, long)]
        address: Option<String>,
        /// Peer role (default: worker)
        #[arg(short, long)]
        role: Option<String>,
        /// Peer category (default: general)
        #[arg(short, long)]
        category: Option<String>,
        /// Tags (comma-separated)
        #[arg(short, long)]
        tags: Option<String>,
        /// Capabilities (comma-separated)
        #[arg(long)]
        capabilities: Option<String>,
        /// Priority (default: 0)
        #[arg(short, long)]
        priority: Option<i32>,
    },
    Remove {
        /// Peer ID to remove
        #[arg(long)]
        id: String,
    },
    /// Enable a peer
    Enable {
        /// Peer ID to enable
        #[arg(long)]
        id: String,
    },
    /// Disable a peer
    Disable {
        /// Peer ID to disable
        #[arg(long)]
        id: String,
    },
}

#[derive(clap::Subcommand)]
pub enum TokenAction {
    /// Generate a new authentication token
    Generate {
        /// Token length in bytes (default 32)
        #[arg(long, default_value = "32")]
        length: usize,
        /// Save to cluster config
        #[arg(long)]
        save: bool,
    },
    /// Show current token (masked by default)
    Show {
        /// Show full token
        #[arg(long)]
        full: bool,
    },
    /// Set a token value
    Set {
        /// Token value (omit with --generate to auto-generate)
        token: Option<String>,
        /// Auto-generate a token
        #[arg(long)]
        generate: bool,
        /// Token length for auto-generated token
        #[arg(long, default_value = "32")]
        length: usize,
    },
    /// Verify a token against the saved one
    Verify {
        /// Token to verify
        token: String,
    },
    /// Revoke/remove the token
    Revoke,
}

pub fn run(action: ClusterAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);

    match action {
        ClusterAction::Status => {
            println!("Cluster Status");
            println!("===============");
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                println!("  Config: {} [found]", cfg_path.display());
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if let Some(enabled) = cfg.get("enabled").and_then(|v| v.as_bool()) {
                            println!("  Enabled: {}", enabled);
                        }
                        if let Some(name) = cfg.get("name").and_then(|v| v.as_str()) {
                            println!("  Node name: {}", name);
                        }
                        if let Some(role) = cfg.get("role").and_then(|v| v.as_str()) {
                            println!("  Role: {}", role);
                        }
                        if let Some(port) = cfg.get("port").and_then(|v| v.as_u64()) {
                            println!("  UDP Port: {}", port);
                        }
                        if let Some(rpc_port) = cfg.get("rpc_port").and_then(|v| v.as_u64()) {
                            println!("  RPC Port: {}", rpc_port);
                        }
                        if let Some(interval) = cfg.get("broadcast_interval").and_then(|v| v.as_u64()) {
                            println!("  Broadcast Interval: {}s", interval);
                        }
                        if let Some(node_id) = cfg.get("node_id").and_then(|v| v.as_str()) {
                            println!("  Node ID: {}", node_id);
                        }
                    }
                }
                let peers_path = common::cluster_dir(&home).join("peers.toml");
                if peers_path.exists() {
                    println!("  Peers config: {} [found]", peers_path.display());
                } else {
                    println!("  Peers config: {} [not found]", peers_path.display());
                }
            } else {
                println!("  Config: {} [not found]", cfg_path.display());
                println!("  Cluster is not initialized. Run: nemesisbot cluster init");
            }
        }
        ClusterAction::Config { udp_port, rpc_port, broadcast_interval } => {
            let cfg_path = common::cluster_config_path(&home);
            println!("Cluster Configuration");
            println!("======================");
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        // Read current values for display
                        let cur_udp = cfg.get("port").and_then(|v| v.as_u64()).unwrap_or(11949) as u16;
                        let cur_rpc = cfg.get("rpc_port").and_then(|v| v.as_u64()).unwrap_or(21949) as u16;
                        let cur_interval = cfg.get("broadcast_interval").and_then(|v| v.as_u64()).unwrap_or(30);

                        // Display current values
                        println!("  UDP Port: {}", cur_udp);
                        println!("  RPC Port: {}", cur_rpc);
                        println!("  Broadcast Interval: {}s", cur_interval);

                        // Only update and save if values differ from what's currently stored
                        if udp_port != cur_udp || rpc_port != cur_rpc || broadcast_interval != cur_interval {
                            if let Some(obj) = cfg.as_object_mut() {
                                obj.insert("port".to_string(), serde_json::Value::Number(udp_port.into()));
                                obj.insert("rpc_port".to_string(), serde_json::Value::Number(rpc_port.into()));
                                obj.insert("broadcast_interval".to_string(), serde_json::Value::Number(broadcast_interval.into()));
                                let _ = std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                                println!("Configuration updated.");
                            }
                        }
                    }
                }
            } else {
                println!("Config file not found. Run 'nemesisbot cluster init' first.");
            }
        }
        ClusterAction::Info { name, role, category, tags, address, capabilities } => {
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(mut cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        // Update info fields if provided
                        let mut changed = false;
                        if let Some(obj) = cfg.as_object_mut() {
                            if let Some(n) = name { obj.insert("name".to_string(), serde_json::Value::String(n)); changed = true; }
                            if let Some(r) = role { obj.insert("role".to_string(), serde_json::Value::String(r)); changed = true; }
                            if let Some(c) = category { obj.insert("category".to_string(), serde_json::Value::String(c)); changed = true; }
                            if let Some(t) = tags { obj.insert("tags".to_string(), serde_json::Value::String(t)); changed = true; }
                            if let Some(a) = address { obj.insert("address".to_string(), serde_json::Value::String(a)); changed = true; }
                            if let Some(c) = capabilities { obj.insert("capabilities".to_string(), serde_json::Value::String(c)); changed = true; }
                            if changed {
                                let _ = std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default());
                            }
                        }
                        // Display info
                        println!("Node Information");
                        println!("================");
                        println!("  Name: {}", cfg.get("name").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Role: {}", cfg.get("role").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Category: {}", cfg.get("category").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Tags: {}", cfg.get("tags").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Address: {}", cfg.get("address").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Capabilities: {}", cfg.get("capabilities").and_then(|v| v.as_str()).unwrap_or("(not set)"));
                        println!("  Enabled: {}", cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false));
                        if changed {
                            println!("Configuration updated.");
                        }
                    }
                }
            } else {
                println!("  Not configured. Run 'nemesisbot cluster init' first.");
            }
        }
        ClusterAction::Peers { action } => {
            match action {
                Some(PeerAction::List) => {
                    println!("Configured Peers");
                    println!("=================");
                    let peers_path = common::cluster_dir(&home).join("peers.toml");
                    if peers_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&peers_path) {
                            println!("{}", data);
                        }
                    } else {
                        println!("  No peers configured.");
                    }
                }
                Some(PeerAction::Add { id, name, address, role, category, tags, capabilities, priority }) => {
                    let display_name = name.as_deref().unwrap_or(&id);
                    let peer_addr = address.as_deref().unwrap_or("127.0.0.1:11949");
                    let peer_role = role.as_deref().unwrap_or("worker");
                    let peer_cat = category.as_deref().unwrap_or("general");
                    println!("Adding peer: {} ({}, addr: {}, role: {}, category: {})", display_name, id, peer_addr, peer_role, peer_cat);
                    let peers_dir = common::cluster_dir(&home);
                    let _ = std::fs::create_dir_all(&peers_dir);
                    let peers_path = peers_dir.join("peers.toml");
                    let existing = if peers_path.exists() {
                        std::fs::read_to_string(&peers_path).unwrap_or_default()
                    } else {
                        String::new()
                    };
                    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
                    let mut entry = format!("\n[peers.{}]\naddress = \"{}\"\nrole = \"{}\"\ncategory = \"{}\"\n", key_safe, peer_addr, peer_role, peer_cat);
                    if let Some(t) = &tags {
                        entry.push_str(&format!("tags = \"{}\"\n", t));
                    }
                    if let Some(c) = &capabilities {
                        entry.push_str(&format!("capabilities = \"{}\"\n", c));
                    }
                    if let Some(p) = priority {
                        entry.push_str(&format!("priority = {}\n", p));
                    }
                    let _ = std::fs::write(&peers_path, existing + &entry);
                    println!("Peer added: {} ({})", display_name, id);
                }
                Some(PeerAction::Remove { id }) => {
                    println!("Removing peer: {}", id);
                    let key_safe = id.replace('.', "_").replace(':', "_").replace('-', "_");
                    let peers_path = common::cluster_dir(&home).join("peers.toml");
                    if peers_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&peers_path) {
                            if let Ok(mut doc) = data.parse::<toml::Value>() {
                                if let Some(peers) = doc.as_table_mut().and_then(|t| t.get_mut("peers")).and_then(|v| v.as_table_mut()) {
                                    if peers.remove(&key_safe).is_some() {
                                        let _ = std::fs::write(&peers_path, toml::to_string_pretty(&doc).unwrap_or_default());
                                        println!("  Peer {} removed.", id);
                                    } else {
                                        println!("  Peer {} not found.", id);
                                    }
                                }
                            }
                        }
                    } else {
                        println!("  No peers file found.");
                    }
                }
                Some(PeerAction::Enable { id }) => {
                    println!("Enabling peer: {}", id);
                    let peers_path = common::cluster_dir(&home).join("peers.toml");
                    if peers_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&peers_path) {
                            match enable_peer_in_toml(&data, &id, true) {
                                Ok(new_data) => {
                                    let _ = std::fs::write(&peers_path, &new_data);
                                    println!("  Peer {} enabled.", id);
                                }
                                Err(msg) => println!("  {}", msg),
                            }
                        }
                    } else {
                        println!("  No peers file found.");
                    }
                }
                Some(PeerAction::Disable { id }) => {
                    println!("Disabling peer: {}", id);
                    let peers_path = common::cluster_dir(&home).join("peers.toml");
                    if peers_path.exists() {
                        if let Ok(data) = std::fs::read_to_string(&peers_path) {
                            match enable_peer_in_toml(&data, &id, false) {
                                Ok(new_data) => {
                                    let _ = std::fs::write(&peers_path, &new_data);
                                    println!("  Peer {} disabled.", id);
                                }
                                Err(msg) => println!("  {}", msg),
                            }
                        }
                    } else {
                        println!("  No peers file found.");
                    }
                }
                None => {
                    println!("Usage: nemesisbot cluster peers <list|add|remove>");
                }
            }
        }
        ClusterAction::Token { action } => {
            match action {
                TokenAction::Generate { length, save } => {
                    if length < 16 || length > 128 {
                        anyhow::bail!("Token length must be between 16 and 128 bytes");
                    }
                    let token = generate_token(length);
                    println!("Generated token: {}", token);
                    if save {
                        let cfg_path = common::cluster_config_path(&home);
                        if cfg_path.exists() {
                            let data = std::fs::read_to_string(&cfg_path)?;
                            let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                            if let Some(obj) = cfg.as_object_mut() {
                                obj.insert("token".to_string(), serde_json::Value::String(token.clone()));
                                std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                                println!("Token saved to cluster config.");
                            }
                        } else {
                            println!("Config file not found. Run 'nemesisbot cluster init' first, or use without --save.");
                        }
                    } else {
                        println!("(Not saved. Use --save to persist to config.)");
                    }
                }
                TokenAction::Show { full } => {
                    let cfg_path = common::cluster_config_path(&home);
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        match cfg.get("token").and_then(|v| v.as_str()) {
                            Some(t) => {
                                if full {
                                    println!("Current token: {}", t);
                                } else {
                                    println!("Current token: {}", mask_token(t));
                                }
                                println!("  RPC authentication is enabled.");
                            }
                            None => {
                                println!("  No RPC token configured.");
                                println!("  RPC authentication is disabled (any token will be accepted).");
                                println!("  To generate: nemesisbot cluster token generate --save");
                            }
                        }
                    } else {
                        println!("No cluster config found.");
                    }
                }
                TokenAction::Set { token, generate, length } => {
                    let value = if let Some(t) = token {
                        if t.len() < 16 || t.len() > 128 {
                            anyhow::bail!("Token must be between 16 and 128 characters");
                        }
                        t
                    } else if generate {
                        if length < 16 || length > 128 {
                            anyhow::bail!("Token length must be between 16 and 128 bytes");
                        }
                        generate_token(length)
                    } else {
                        println!("Error: provide a token value or use --generate.");
                        return Ok(());
                    };
                    let cfg_path = common::cluster_config_path(&home);
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(obj) = cfg.as_object_mut() {
                            obj.insert("token".to_string(), serde_json::Value::String(value.clone()));
                            std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                            println!("Token set: {}", mask_token(&value));
                        }
                    } else {
                        println!("Config file not found. Run 'nemesisbot cluster init' first.");
                    }
                }
                TokenAction::Verify { token } => {
                    let cfg_path = common::cluster_config_path(&home);
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let cfg: serde_json::Value = serde_json::from_str(&data)?;
                        match cfg.get("token").and_then(|v| v.as_str()) {
                            Some(saved) => {
                                if crate::common::constant_time_eq(saved.as_bytes(), token.as_bytes()) {
                                    println!("Token matches.");
                                } else {
                                    println!("Token does NOT match.");
                                }
                            }
                            None => println!("No token configured to verify against."),
                        }
                    } else {
                        println!("No cluster config found.");
                    }
                }
                TokenAction::Revoke => {
                    let cfg_path = common::cluster_config_path(&home);
                    if cfg_path.exists() {
                        let data = std::fs::read_to_string(&cfg_path)?;
                        let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
                        if let Some(obj) = cfg.as_object_mut() {
                            obj.remove("token");
                            std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
                            println!("Token revoked. Generate a new one with 'nemesisbot cluster token generate'.");
                        }
                    } else {
                        println!("No cluster config found.");
                    }
                }
            }
        }
        ClusterAction::Init { name, role, category, tags, address, capabilities } => {
            println!("Initializing cluster configuration...");
            let cfg_path = common::cluster_config_path(&home);
            let dir = cfg_path.parent().unwrap();
            let _ = std::fs::create_dir_all(dir);

            // Check if config already exists and prompt for confirmation
            if cfg_path.exists() {
                // In non-interactive mode (piped stdin), just overwrite
                let _is_term = false; // CLI always uses Stdio::piped
                use std::io::{self, Write, IsTerminal};
                if io::stdin().is_terminal() {
                    print!("  Cluster config already exists. Reinitialize? This will overwrite existing configuration. (y/N): ");
                    io::stdout().flush().ok();
                    let mut answer = String::new();
                    io::stdin().read_line(&mut answer).ok();
                    if answer.trim().to_lowercase() != "y" {
                        println!("  Aborted.");
                        return Ok(());
                    }
                }
            }

            // Generate proper node ID (UUID-based for uniqueness)
            let node_id = format!("node-{}", uuid::Uuid::new_v4());
            let default_name = format!("Bot {}", node_id);

            let mut config = serde_json::json!({
                "enabled": false,
                "node_id": node_id,
                "name": name.unwrap_or_else(|| default_name.clone()),
                "role": role.unwrap_or_else(|| "worker".to_string()),
                "category": category.unwrap_or_else(|| "development".to_string()),
                "port": 11949,
                "rpc_port": 21949,
                "broadcast_interval": 30,
                "token": uuid::Uuid::new_v4().to_string(),
            });
            if let Some(obj) = config.as_object_mut() {
                if let Some(t) = tags { obj.insert("tags".to_string(), serde_json::Value::String(t)); }
                if let Some(a) = address { obj.insert("address".to_string(), serde_json::Value::String(a)); }
                if let Some(c) = capabilities { obj.insert("capabilities".to_string(), serde_json::Value::String(c)); }
            }

            let _ = std::fs::write(&cfg_path, serde_json::to_string_pretty(&config).unwrap_or_default());
            println!("Cluster configuration initialized at: {}", cfg_path.display());
            println!("Enable with: nemesisbot cluster enable");
        }
        ClusterAction::Enable => {
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            println!("Cluster is already enabled.");
                            return Ok(());
                        }
                    }
                }
            }
            update_cluster_config(&home, "enabled", true)?;
            println!("Cluster enabled. Restart gateway to apply.");
        }
        ClusterAction::Disable => {
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if !cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            println!("Cluster is already disabled.");
                            return Ok(());
                        }
                    }
                }
            }
            update_cluster_config(&home, "enabled", false)?;
            println!("Cluster disabled. Restart gateway to apply.");
        }
        ClusterAction::Start => {
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            println!("Cluster is already enabled.");
                            return Ok(());
                        }
                    }
                }
            }
            update_cluster_config(&home, "enabled", true)?;
            println!("Cluster enabled. Restart gateway to apply.");
        }
        ClusterAction::Stop => {
            let cfg_path = common::cluster_config_path(&home);
            if cfg_path.exists() {
                if let Ok(data) = std::fs::read_to_string(&cfg_path) {
                    if let Ok(cfg) = serde_json::from_str::<serde_json::Value>(&data) {
                        if !cfg.get("enabled").and_then(|v| v.as_bool()).unwrap_or(false) {
                            println!("Cluster is already disabled.");
                            return Ok(());
                        }
                    }
                }
            }
            update_cluster_config(&home, "enabled", false)?;
            println!("Cluster disabled. Restart gateway to apply.");
        }
        ClusterAction::Reset { hard } => {
            if !hard {
                // Soft reset: only clear state.toml (discovered nodes)
                println!("Soft reset: clearing discovered nodes...");
                let state_path = common::cluster_dir(&home).join("state.toml");
                if state_path.exists() {
                    let _ = std::fs::remove_file(&state_path);
                    println!("  Discovered nodes cleared.");
                } else {
                    println!("  No state file found (nothing to clear).");
                }
                println!("Use --hard to also clear peers.toml and all cluster data.");
                return Ok(());
            }
            // Hard reset: clear everything
            println!("WARNING: Hard reset - clearing all cluster configuration.");
            print!("  WARNING: This will remove all cluster data including peers. Continue? (y/N): ");
            use std::io::{self, Write};
            io::stdout().flush().ok();
            let mut answer = String::new();
            io::stdin().read_line(&mut answer).ok();
            if answer.trim().to_lowercase() != "y" {
                println!("  Aborted.");
                return Ok(());
            }
            let cfg_path = common::cluster_config_path(&home);
            let _ = std::fs::remove_file(&cfg_path);
            let peers_path = common::cluster_dir(&home).join("peers.toml");
            let _ = std::fs::remove_file(&peers_path);
            let state_path = common::cluster_dir(&home).join("state.toml");
            let _ = std::fs::remove_file(&state_path);
            println!("Cluster configuration reset (hard).");
        }
    }
    Ok(())
}

fn update_cluster_config(home: &std::path::Path, key: &str, value: impl Into<serde_json::Value>) -> Result<()> {
    let cfg_path = common::cluster_config_path(home);
    if !cfg_path.exists() {
        anyhow::bail!("Cluster not initialized. Run 'nemesisbot cluster init' first.");
    }
    let data = std::fs::read_to_string(&cfg_path)?;
    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(key.to_string(), value.into());
        std::fs::write(&cfg_path, serde_json::to_string_pretty(&cfg).unwrap_or_default())?;
    }
    Ok(())
}

/// Generate a cryptographically secure random standard base64 token of the given byte length.
fn generate_token(byte_len: usize) -> String {
    let mut bytes = vec![0u8; byte_len];
    // Use getrandom for cryptographically secure randomness
    if let Err(e) = getrandom::getrandom(&mut bytes) {
        // Fallback: if system RNG fails, log warning and use uuid-based entropy
        eprintln!("Warning: crypto RNG failed ({}), using fallback", e);
        let uuid = uuid::Uuid::new_v4();
        for (i, b) in uuid.as_bytes().iter().enumerate() {
            if i < bytes.len() {
                bytes[i] = *b;
            }
        }
    }
    base64_encode(&bytes)
}

/// Simple standard base64 encoding with padding (to match Go's base64.StdEncoding).
fn base64_encode(data: &[u8]) -> String {
    const TABLE: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";
    let mut out = String::with_capacity((data.len() + 2) / 3 * 4);
    let mut i = 0;
    while i + 3 <= data.len() {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8) | (data[i + 2] as u32);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push(TABLE[(n & 0x3F) as usize] as char);
        i += 3;
    }
    if data.len() - i == 2 {
        let n = ((data[i] as u32) << 16) | ((data[i + 1] as u32) << 8);
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 6) & 0x3F) as usize] as char);
        out.push('=');
    } else if data.len() - i == 1 {
        let n = (data[i] as u32) << 16;
        out.push(TABLE[((n >> 18) & 0x3F) as usize] as char);
        out.push(TABLE[((n >> 12) & 0x3F) as usize] as char);
        out.push_str("==");
    }
    out
}

/// Mask a token showing first 4 and last 4 chars with **** in between.
fn mask_token(token: &str) -> String {
    if token.len() <= 8 {
        return "****".to_string();
    }
    format!("{}****{}", &token[..4], &token[token.len() - 4..])
}

/// Enable or disable a specific peer in peers.toml using proper TOML parsing.
///
/// Uses the `toml` crate to parse the TOML document, locates the peer section
/// by address, and sets the `enabled` field accordingly. This avoids naive
/// string replacement which could modify the wrong entry.
///
/// # Arguments
/// * `toml_content` - The current contents of peers.toml
/// * `addr` - The peer address to modify (e.g. "192.168.1.10:11949")
/// * `enabled` - Whether to enable (true) or disable (false) the peer
///
/// # Returns
/// The modified TOML content, or an error message string.
fn enable_peer_in_toml(toml_content: &str, addr: &str, enabled: bool) -> Result<String, String> {
    let mut doc: toml::Value = toml_content.parse::<toml::Value>()
        .map_err(|e| format!("Failed to parse peers.toml: {}", e))?;

    // Navigate to [peers] section
    let peers = doc.as_table_mut()
        .and_then(|t| t.get_mut("peers"))
        .and_then(|v| v.as_table_mut())
        .ok_or_else(|| "No [peers] section found in peers.toml".to_string())?;

    // Search for the peer with matching address.
    // The peer key is the sanitized address (dots, colons, hyphens → underscores).
    let key_safe = addr.replace('.', "_").replace(':', "_").replace('-', "_");

    // Try the sanitized key first, then fall back to scanning all peers for matching address.
    let target_key = if peers.contains_key(&key_safe) {
        key_safe.clone()
    } else {
        // Scan all peer entries for one whose address matches
        let mut found = None;
        for (key, val) in peers.iter() {
            if let Some(peer_table) = val.as_table() {
                if let Some(peer_addr) = peer_table.get("address").and_then(|v| v.as_str()) {
                    if peer_addr == addr {
                        found = Some(key.clone());
                        break;
                    }
                }
            }
        }
        found.ok_or_else(|| format!("Peer {} not found in peers.toml", addr))?
    };

    // Set the enabled field on the found peer
    if let Some(peer_table) = peers.get_mut(&target_key).and_then(|v| v.as_table_mut()) {
        peer_table.insert("enabled".to_string(), toml::Value::Boolean(enabled));
    }

    // Serialize back to TOML, preserving human-readable formatting
    toml::to_string_pretty(&doc)
        .map_err(|e| format!("Failed to serialize peers.toml: {}", e))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    fn make_home(tmp: &TempDir) -> std::path::PathBuf {
        let home = tmp.path().join(".nemesisbot");
        let config_dir = home.join("workspace").join("config");
        let _ = std::fs::create_dir_all(&config_dir);
        home
    }

    fn write_cluster_config(home: &std::path::Path, json: &serde_json::Value) {
        let cfg_path = crate::common::cluster_config_path(home);
        if let Some(parent) = cfg_path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        std::fs::write(&cfg_path, serde_json::to_string_pretty(json).unwrap()).unwrap();
    }

    #[test]
    fn test_base64_encode_empty() {
        assert_eq!(base64_encode(&[]), "");
    }

    #[test]
    fn test_base64_encode_hello() {
        // "Hello" = [72, 101, 108, 108, 111] → base64 "SGVsbG8="
        assert_eq!(base64_encode(b"Hello"), "SGVsbG8=");
    }

    #[test]
    fn test_base64_encode_single_byte() {
        // 'A' = [65] → "QQ=="
        assert_eq!(base64_encode(b"A"), "QQ==");
    }

    #[test]
    fn test_base64_encode_two_bytes() {
        // "AB" = [65, 66] → "QUI="
        assert_eq!(base64_encode(b"AB"), "QUI=");
    }

    #[test]
    fn test_base64_encode_three_bytes() {
        // "ABC" = [65, 66, 67] → "QUJD"
        assert_eq!(base64_encode(b"ABC"), "QUJD");
    }

    #[test]
    fn test_base64_encode_known_vectors() {
        // Test vectors from RFC 4648
        assert_eq!(base64_encode(b""), "");
        assert_eq!(base64_encode(b"f"), "Zg==");
        assert_eq!(base64_encode(b"fo"), "Zm8=");
        assert_eq!(base64_encode(b"foo"), "Zm9v");
        assert_eq!(base64_encode(b"foob"), "Zm9vYg==");
        assert_eq!(base64_encode(b"fooba"), "Zm9vYmE=");
        assert_eq!(base64_encode(b"foobar"), "Zm9vYmFy");
    }

    #[test]
    fn test_mask_token_short() {
        assert_eq!(mask_token("abc"), "****");
        assert_eq!(mask_token("12345678"), "****");
    }

    #[test]
    fn test_mask_token_long() {
        assert_eq!(mask_token("abcdefghijklmnop"), "abcd****mnop");
    }

    #[test]
    fn test_mask_token_exactly_9() {
        // 9 chars: first 4 + **** + last 4
        assert_eq!(mask_token("123456789"), "1234****6789");
    }

    #[test]
    fn test_generate_token_length() {
        let token = generate_token(32);
        // base64 of 32 bytes = 44 chars (ceil(32/3)*4 = 44)
        assert_eq!(token.len(), 44);
        // Should be valid base64 characters
        for c in token.chars() {
            assert!(c.is_ascii_alphanumeric() || c == '+' || c == '/' || c == '=');
        }
    }

    #[test]
    fn test_generate_token_16_bytes() {
        let token = generate_token(16);
        // base64 of 16 bytes = 24 chars (ceil(16/3)*4 = 24)
        assert_eq!(token.len(), 24);
    }

    #[test]
    fn test_generate_token_unique() {
        let t1 = generate_token(32);
        let t2 = generate_token(32);
        assert_ne!(t1, t2, "Two generated tokens should differ");
    }

    #[test]
    fn test_update_cluster_config_creates_file() {
        let tmp = TempDir::new().unwrap();
        let home = make_home(&tmp);
        let cfg_path = crate::common::cluster_config_path(&home);

        // Write initial config
        let initial = serde_json::json!({"enabled": false, "name": "test"});
        std::fs::write(&cfg_path, serde_json::to_string(&initial).unwrap()).unwrap();

        update_cluster_config(&home, "enabled", true).unwrap();

        let data = std::fs::read_to_string(&cfg_path).unwrap();
        let cfg: serde_json::Value = serde_json::from_str(&data).unwrap();
        assert_eq!(cfg["enabled"], true);
        assert_eq!(cfg["name"], "test");
    }

    #[test]
    fn test_update_cluster_config_no_file() {
        let tmp = TempDir::new().unwrap();
        let home = make_home(&tmp);
        let result = update_cluster_config(&home, "enabled", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_enable_peer_in_toml_basic() {
        let toml_content = r#"
[peers]
[peers.node1]
address = "192.168.1.10:11949"
role = "worker"
"#;
        let result = enable_peer_in_toml(toml_content, "192.168.1.10:11949", true);
        assert!(result.is_ok());
        let doc: toml::Value = result.unwrap().parse().unwrap();
        assert_eq!(doc["peers"]["node1"]["enabled"], toml::Value::Boolean(true));
    }

    #[test]
    fn test_enable_peer_in_toml_disable() {
        let toml_content = r#"
[peers]
[peers.my_node]
address = "10.0.0.1:21949"
role = "manager"
enabled = true
"#;
        let result = enable_peer_in_toml(toml_content, "10.0.0.1:21949", false);
        assert!(result.is_ok());
        let doc: toml::Value = result.unwrap().parse().unwrap();
        assert_eq!(doc["peers"]["my_node"]["enabled"], toml::Value::Boolean(false));
    }

    #[test]
    fn test_enable_peer_in_toml_no_peers_section() {
        let result = enable_peer_in_toml("[other]\nkey = \"value\"", "1.2.3.4:11949", true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("No [peers] section"));
    }

    #[test]
    fn test_enable_peer_in_toml_peer_not_found() {
        let toml_content = "[peers]\n[peers.node1]\naddress = \"1.1.1.1:11949\"";
        let result = enable_peer_in_toml(toml_content, "9.9.9.9:11949", true);
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("not found"));
    }

    #[test]
    fn test_enable_peer_in_toml_invalid_toml() {
        let result = enable_peer_in_toml("not valid {{{{", "1.1.1.1:11949", true);
        assert!(result.is_err());
    }

    #[test]
    fn test_enable_peer_in_toml_sanitized_key_match() {
        // When the sanitized key matches, it should find the peer even without scanning
        let toml_content = "[peers]\n[peers.192_168_1_10_11949]\naddress = \"192.168.1.10:11949\"";
        let result = enable_peer_in_toml(toml_content, "192.168.1.10:11949", true);
        assert!(result.is_ok());
    }
}
