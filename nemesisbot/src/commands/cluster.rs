//! Cluster command - manage bot cluster configuration and status.

use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::Result;
use chrono::Local;
use tracing::{info, warn};

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
    /// Manage cluster node identity (name, role, capabilities, personality)
    Identity {
        #[command(subcommand)]
        action: IdentityAction,
    },
    /// Start a lightweight cluster node (discovery + RPC, no LLM)
    Node {
        /// UDP discovery port (overrides config)
        #[arg(short, long)]
        udp_port: Option<u16>,
        /// RPC port (overrides config)
        #[arg(short, long)]
        rpc_port: Option<u16>,
        /// Node name (overrides config)
        #[arg(short = 'n', long)]
        name: Option<String>,
        /// Broadcast interval in seconds
        #[arg(short, long, default_value = "10")]
        broadcast_interval: u64,
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

#[derive(clap::Subcommand)]
pub enum IdentityAction {
    /// Show current cluster identity content
    Show,
    /// Create identity from template for editing (prints file path)
    Edit,
    /// Reset identity to default (贾维斯 / 专家开发工程师)
    Reset,
}

pub async fn run(action: ClusterAction, local: bool) -> Result<()> {
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
            update_main_config_cluster(&home, true)?;
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
            update_main_config_cluster(&home, false)?;
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
        ClusterAction::Identity { action } => {
            let cluster_dir = common::cluster_dir(&home);
            let identity_path = cluster_dir.join("IDENTITY.md");

            match action {
                IdentityAction::Show => {
                    if identity_path.exists() {
                        let content = std::fs::read_to_string(&identity_path)
                            .unwrap_or_else(|_| "(failed to read file)".to_string());
                        println!("Cluster Identity");
                        println!("=================");
                        println!("  File: {}", identity_path.display());
                        println!();
                        println!("{}", content);
                    } else {
                        println!("Cluster identity not found.");
                        println!("  Use 'nemesisbot cluster identity edit' to create one from template.");
                        println!("  Or run 'nemesisbot onboard default' to install default identity (贾维斯).");
                    }
                }
                IdentityAction::Edit => {
                    // Ensure cluster directory exists
                    let _ = std::fs::create_dir_all(&cluster_dir);

                    if identity_path.exists() {
                        println!("Cluster identity already exists at:");
                        println!("  {}", identity_path.display());
                        println!("Edit this file directly to customize your cluster identity.");
                    } else {
                        // Write template to file
                        let template = crate::CLUSTER_IDENTITY_TEMPLATE;
                        std::fs::write(&identity_path, template)?;
                        println!("Cluster identity template created at:");
                        println!("  {}", identity_path.display());
                        println!("Edit this file to customize your cluster identity.");
                    }
                    println!();
                    println!("Tip: after editing, restart gateway to apply changes.");
                }
                IdentityAction::Reset => {
                    println!("Resetting cluster identity to default...");
                    println!("  Default identity: 贾维斯（老贾）— 专家开发工程师");
                    println!("  This will overwrite the current cluster identity file.");
                    println!();

                    let _ = std::fs::create_dir_all(&cluster_dir);
                    let default_content = crate::DEFAULT_IDENTITY_CLUSTER;
                    std::fs::write(&identity_path, default_content)?;
                    println!("  Cluster identity reset to default (贾维斯).");
                    println!("  File: {}", identity_path.display());
                    println!();
                    println!("Restart gateway to apply changes.");
                }
            }
        }
        ClusterAction::Node { udp_port, rpc_port, name, broadcast_interval } => {
            run_node(&home, udp_port, rpc_port, name, broadcast_interval).await?;
        }
    }
    Ok(())
}

/// Start a lightweight cluster node with UDP discovery and RPC server, no LLM.
async fn run_node(
    home: &Path,
    udp_port_override: Option<u16>,
    rpc_port_override: Option<u16>,
    name_override: Option<String>,
    broadcast_interval: u64,
) -> Result<()> {
    // Load cluster config
    let cluster_cfg_path = common::cluster_config_path(home);
    let cluster_json = if cluster_cfg_path.exists() {
        std::fs::read_to_string(&cluster_cfg_path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        anyhow::bail!(
            "Cluster not initialized. Run 'nemesisbot cluster init' first.\n  Missing: {}",
            cluster_cfg_path.display()
        );
    };

    let node_id = cluster_json
        .get("node_id")
        .or_else(|| cluster_json.get("id"))
        .and_then(|v| v.as_str())
        .unwrap_or("unknown")
        .to_string();
    let node_name = name_override.unwrap_or_else(|| {
        cluster_json
            .get("name")
            .and_then(|v| v.as_str())
            .unwrap_or("unnamed")
            .to_string()
    });
    let udp_port = udp_port_override.unwrap_or_else(|| {
        cluster_json.get("port").and_then(|v| v.as_u64()).unwrap_or(11949) as u16
    });
    let rpc_port = rpc_port_override.unwrap_or_else(|| {
        cluster_json.get("rpc_port").and_then(|v| v.as_u64()).unwrap_or(21949) as u16
    });

    // Init logger
    let cfg_path = common::config_path(home);
    let _ = common::init_logger_from_config(&cfg_path, &[]);

    println!("Cluster Node (lightweight)");
    println!("==========================");
    println!("  Node ID:    {}", node_id);
    println!("  Name:       {}", node_name);
    println!("  UDP Port:   {}", udp_port);
    println!("  RPC Port:   {}", rpc_port);
    println!("  Broadcast:  every {}s", broadcast_interval);
    println!();

    // Create Cluster instance
    let cluster_config = nemesis_cluster::types::ClusterConfig {
        node_id: node_id.clone(),
        bind_address: format!("0.0.0.0:{}", rpc_port),
        peers: vec![],
    };
    let mut cluster = nemesis_cluster::cluster::Cluster::with_workspace(
        cluster_config,
        home.join("workspace"),
    );
    cluster.set_ports(udp_port, rpc_port);
    cluster.set_node_name(&node_name);
    cluster.set_node_type("node");

    // Load static peers from peers.toml
    let peers_path = common::cluster_dir(home).join("peers.toml");
    if peers_path.exists() {
        if let Ok(content) = std::fs::read_to_string(&peers_path) {
            if let Ok(doc) = content.parse::<toml::Value>() {
                if let Some(peers_table) = doc.get("peers").and_then(|v| v.as_table()) {
                    for (key, val) in peers_table {
                        let peer_id = key.replace('_', "-");
                        let addr = val.get("address").and_then(|v| v.as_str()).unwrap_or("");
                        let name = val.get("name").and_then(|v| v.as_str()).unwrap_or(&peer_id);
                        let role = val.get("role").and_then(|v| v.as_str()).unwrap_or("worker");
                        let cat = val.get("category").and_then(|v| v.as_str()).unwrap_or("general");
                        if addr.is_empty() { continue; }
                        let (host, up) = parse_host_port(addr);
                        let rp = if up > 0 { up + 10000 } else { 0 };
                        let addresses = if host.is_empty() { vec![] } else { vec![host] };
                        info!("[Node] Loading static peer: {} ({}) addr={} rpc_port={}", name, peer_id, addr, rp);
                        cluster.handle_discovered_node(&peer_id, name, addresses, rp, role, cat, vec![], vec![], "unknown");
                    }
                }
            }
        }
    }

    // Create and set RPC server (before start)
    let rpc_server_config = nemesis_cluster::rpc::server::RpcServerConfig {
        bind_address: format!("0.0.0.0:{}", rpc_port),
        ..Default::default()
    };
    cluster.set_rpc_server(Arc::new(nemesis_cluster::rpc::server::RpcServer::new(rpc_server_config)));

    // Start cluster (registers local node, creates RPC client, starts sync/recovery loops)
    cluster.start();
    info!("[Node] Cluster started (node_id={}, name={}, udp={}, rpc={})", node_id, node_name, udp_port, rpc_port);

    // Register basic RPC handlers (ping, get_info, get_capabilities)
    if let Err(e) = cluster.register_basic_handlers() {
        warn!("[Node] Failed to register basic RPC handlers: {}", e);
    }

    // Start RPC server
    let rpc_server_ref = cluster.rpc_server()
        .expect("rpc_server just set")
        .clone();
    if let Err(e) = rpc_server_ref.start().await {
        anyhow::bail!("RPC server error on port {}: {}", rpc_port, e);
    }
    info!("[Node] RPC server started on port {}", rpc_port);
    println!("  RPC server started on 0.0.0.0:{}", rpc_port);

    // Start UDP Discovery (managed by Cluster)
    let cluster_arc: Arc<nemesis_cluster::cluster::Cluster> = Arc::new(cluster);
    cluster_arc.start_discovery(cluster_arc.clone());
    info!("[Node] UDP discovery started on port {}", udp_port);
    println!("  UDP discovery started on port {}", udp_port);

    println!();
    println!("  Waiting for peers... (Ctrl+C to stop)");
    println!();

    // Real-time peer display loop
    let mut tick = tokio::time::interval(Duration::from_secs(5));
    let mut last_count: usize = 0;
    loop {
        tokio::select! {
            _ = tick.tick() => {
                let peers = cluster_arc.get_online_peers();
                let count = peers.len();
                if count != last_count {
                    let now = Local::now().format("%H:%M:%S");
                    if count > last_count {
                        println!("[{}] Peers changed: {} -> {} online", now, last_count, count);
                    } else {
                        println!("[{}] Peers changed: {} -> {} online", now, last_count, count);
                    }
                    for p in &peers {
                        let ntype = if p.node_type.is_empty() { "unknown" } else { &p.node_type };
                        let caps = if p.capabilities.is_empty() {
                            "none".to_string()
                        } else {
                            p.capabilities.join(", ")
                        };
                        println!("  - {} ({}) addr={} type={} caps=[{}]",
                            p.base.name, p.base.id, p.base.address, ntype, caps);
                    }
                    println!();
                    last_count = count;
                }
            }
            _ = tokio::signal::ctrl_c() => {
                println!("\n  Stopping cluster node...");
                info!("[Node] Shutdown signal received");
                break;
            }
        }
    }
    Ok(())
}

fn parse_host_port(addr: &str) -> (String, u16) {
    let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
    if parts.len() == 2 {
        (parts[1].to_string(), parts[0].parse().unwrap_or(0))
    } else {
        (addr.to_string(), 0)
    }
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

/// Update the master cluster switch in the main config.json.
/// Creates the "cluster" section if it doesn't exist.
fn update_main_config_cluster(home: &std::path::Path, enabled: bool) -> Result<()> {
    let cfg_path = common::config_path(home);
    if !cfg_path.exists() {
        return Ok(());
    }
    let data = std::fs::read_to_string(&cfg_path)?;
    let mut cfg: serde_json::Value = serde_json::from_str(&data)?;
    if let Some(obj) = cfg.as_object_mut() {
        obj.insert(
            "cluster".to_string(),
            serde_json::json!({ "enabled": enabled }),
        );
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
mod tests;
