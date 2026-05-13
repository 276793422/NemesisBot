//! Daemon command - run NemesisBot as a background daemon.
//!
//! Supports cluster daemon mode for distributed node management.
//! Loads cluster configuration, creates a real Cluster instance,
//! and runs a heartbeat loop until graceful shutdown.

use std::path::Path;
use std::time::Duration;

use anyhow::Result;
use tracing::{info, warn};

use crate::common;

use nemesis_cluster::cluster::Cluster;
use nemesis_cluster::cluster_config;
use nemesis_cluster::types::ClusterConfig;
use serde_json;

#[derive(clap::Subcommand)]
pub enum DaemonAction {
    /// Run cluster daemon
    Cluster {
        /// Cluster mode: auto, worker, coordinator
        mode: Option<String>,
    },
}

/// Run the daemon command.
pub async fn run(action: DaemonAction, local: bool) -> Result<()> {
    let home = common::resolve_home(local);

    match action {
        DaemonAction::Cluster { mode } => {
            let mode_str = mode.as_deref().unwrap_or("auto");
            let valid_modes = ["auto", "worker", "coordinator"];
            if !valid_modes.contains(&mode_str) {
                anyhow::bail!(
                    "Invalid cluster mode '{}'. Must be one of: {}",
                    mode_str,
                    valid_modes.join(", ")
                );
            }

            println!("NemesisBot Cluster Daemon");
            println!("=========================");
            println!("  Mode: {}", mode_str);
            println!("  Home: {}", home.display());
            println!();

            // Load main configuration
            let cfg_path = common::config_path(&home);
            if !cfg_path.exists() {
                anyhow::bail!(
                    "Configuration not found: {}. Run 'nemesisbot onboard default' first.",
                    cfg_path.display()
                );
            }

            let _cfg = nemesis_config::load_config(&cfg_path)
                .map_err(|e| anyhow::anyhow!("Error loading config: {}", e))?;

            // Check cluster status from config.json (read as raw JSON for flexibility)
            let cluster_cfg_path = common::cluster_config_path(&home);
            let raw_data = std::fs::read_to_string(&cfg_path)?;
            let raw_cfg: serde_json::Value = serde_json::from_str(&raw_data)?;
            let cluster_enabled = raw_cfg.get("cluster")
                .and_then(|c| c.get("enabled"))
                .and_then(|v| v.as_bool())
                .unwrap_or(false);

            if !cluster_enabled {
                println!("  WARNING: Cluster is not enabled in configuration.");
                println!("  Enable it with: nemesisbot cluster enable");
            }

            println!("  Cluster config: {}", if cluster_cfg_path.exists() { "found" } else { "not found" });
            println!("  Cluster enabled: {}", cluster_enabled);
            println!();

            // Initialize logger
            let _ = common::init_logger_from_config(&cfg_path, &[]);

            // Start cluster daemon loop
            run_cluster_daemon(&home, mode_str).await?;

            Ok(())
        }
    }
}

/// Run the cluster daemon main loop.
///
/// Loads cluster config, creates a real Cluster instance,
/// displays node information, and runs a heartbeat loop
/// until Ctrl+C or shutdown signal.
async fn run_cluster_daemon(home: &Path, mode: &str) -> Result<()> {
    info!("Starting cluster daemon (mode={})", mode);

    // --- Load cluster configuration ---
    let cluster_cfg_path = common::cluster_config_path(home);
    let cluster_json = if cluster_cfg_path.exists() {
        std::fs::read_to_string(&cluster_cfg_path)
            .ok()
            .and_then(|data| serde_json::from_str::<serde_json::Value>(&data).ok())
            .unwrap_or(serde_json::json!({}))
    } else {
        serde_json::json!({})
    };

    // Extract ports and node ID from cluster config
    let udp_port = cluster_json.get("port")
        .and_then(|v| v.as_u64())
        .unwrap_or(11949) as u16;
    let rpc_port = cluster_json.get("rpc_port")
        .and_then(|v| v.as_u64())
        .unwrap_or(21949) as u16;
    let node_id = cluster_json.get("id")
        .and_then(|v| v.as_str())
        .map(|s| s.to_string())
        .unwrap_or_else(|| generate_node_id());

    // --- Load peers from peers.toml ---
    let peers_path = common::cluster_dir(&home).join("peers.toml");
    let peer_addresses = load_peer_addresses(&peers_path);

    // --- Create and start Cluster instance ---
    let workspace = common::workspace_path(&home);
    let config = ClusterConfig {
        node_id: node_id.clone(),
        bind_address: format!("0.0.0.0:{}", rpc_port),
        peers: peer_addresses.clone(),
    };

    let mut cluster = Cluster::with_workspace(config, workspace);

    // Set ports (separate from ClusterConfig which doesn't have port fields)
    cluster.set_ports(udp_port, rpc_port);

    // Display node information
    println!("  Node Information:");
    println!("    ID:       {}", cluster.node_id());
    println!("    Name:     {}", cluster.node_name());
    println!("    Address:  {}", cluster.address());
    println!("    Role:     {}", cluster.role());
    println!("    UDP Port: {}", cluster.udp_port());
    println!("    RPC Port: {}", cluster.rpc_port());
    println!("    Peers:    {} configured", peer_addresses.len());
    println!();

    // Start the cluster
    cluster.start();
    info!(
        "Cluster started: node_id={}, rpc_port={}, udp_port={}",
        cluster.node_id(),
        cluster.rpc_port(),
        cluster.udp_port()
    );
    println!("  Cluster instance started and running.");
    println!();

    // --- Set up shutdown signal handling ---
    let (shutdown_tx, mut shutdown_rx) = tokio::sync::broadcast::channel::<()>(1);

    // Ctrl+C handler
    let shutdown_tx_ctrlc = shutdown_tx.clone();
    tokio::spawn(async move {
        tokio::signal::ctrl_c().await.ok();
        info!("Ctrl+C received, shutting down...");
        println!("\n  Shutdown signal received, stopping daemon...");
        let _ = shutdown_tx_ctrlc.send(());
    });

    // --- Register basic RPC handlers (hello, ping, get_info, etc.) ---
    if let Err(e) = cluster.register_basic_handlers() {
        warn!("Failed to register basic RPC handlers: {}", e);
        println!("  WARNING: Failed to register RPC handlers: {}", e);
    } else {
        info!("Basic RPC handlers registered");
    }

    // --- Periodic tasks ---
    let mut rpc_ticker = tokio::time::interval(Duration::from_secs(60));
    let mut sync_interval = tokio::time::interval(Duration::from_secs(300));
    let mut heartbeat_interval = tokio::time::interval(Duration::from_secs(60));

    info!("Daemon loop started. Press Ctrl+C to stop.");
    println!("  Daemon running. Press Ctrl+C to stop.");
    println!();

    loop {
        tokio::select! {
            _ = rpc_ticker.tick() => {
                tick_rpc_hello(&cluster);
            }
            _ = sync_interval.tick() => {
                tick_sync_to_disk(&cluster);
            }
            _ = heartbeat_interval.tick() => {
                log_heartbeat(&cluster);
            }
            _ = shutdown_rx.recv() => {
                info!("Shutdown signal received");
                break;
            }
        }
    }

    // --- Graceful shutdown with timeout ---
    println!("  Shutting down cluster...");
    let shutdown_result = tokio::time::timeout(
        Duration::from_secs(30),
        async {
            cluster.stop();
            // Give a moment for cleanup
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
    ).await;

    if shutdown_result.is_err() {
        warn!("Shutdown timed out after 30s, forcing exit");
        println!("  WARNING: Shutdown timed out, some resources may not have been cleaned up.");
    }

    info!("Cluster daemon stopped.");
    println!("  Cluster daemon stopped.");

    Ok(())
}

/// Call "hello" on all online peers via RPC (ticker callback).
///
/// Mirrors Go's RPC ticker that calls all online nodes every 60 seconds.
fn tick_rpc_hello(cluster: &Cluster) {
    let online_peers = cluster.get_online_peers();
    if online_peers.is_empty() {
        return;
    }

    let node_id = cluster.node_id().to_string();
    info!("RPC ticker: calling {} online peer(s)", online_peers.len());

    for peer in &online_peers {
        let peer_id = peer.base.id.clone();
        let payload = serde_json::json!({
            "from": node_id,
            "timestamp": chrono::Utc::now().to_rfc3339(),
        });

        match cluster.call_with_context(&peer_id, "hello", payload) {
            Ok(response) => {
                let resp_str = String::from_utf8_lossy(&response);
                info!("RPC hello response from {}: {}", peer_id, resp_str);
            }
            Err(e) => {
                warn!("RPC hello failed for {}: {}", peer_id, e);
            }
        }
    }
}

/// Periodically sync cluster state to disk (every 5 minutes).
fn tick_sync_to_disk(cluster: &Cluster) {
    match cluster.sync_to_disk() {
        Ok(()) => {
            info!("Cluster state synced to disk");
        }
        Err(e) => {
            warn!("Failed to sync cluster state to disk: {}", e);
        }
    }
}

/// Log a periodic heartbeat with cluster health info.
fn log_heartbeat(cluster: &Cluster) {
    let online_count = cluster.get_online_peers().len();
    let running = cluster.is_running();
    info!(
        "Heartbeat: running={}, online_peers={}, node_id={}",
        running,
        online_count,
        cluster.node_id(),
    );
}

/// Log a summary of cluster status with more detail.
#[allow(dead_code)] // Utility function for cluster status reporting, used by gateway lifecycle
fn log_cluster_status(home: &Path, cluster: &Cluster) -> Result<()> {
    let peers_path = common::cluster_dir(home).join("peers.toml");
    let static_peer_count = if peers_path.exists() {
        match cluster_config::load_static_config(&peers_path) {
            Ok(config) => config.peers.len(),
            Err(_) => 0,
        }
    } else {
        0
    };

    let online_peers = cluster.get_online_peers();
    let online_count = online_peers.len();
    let tasks = cluster.list_tasks();

    info!(
        "Cluster status: {} static peer(s), {} online, {} active task(s)",
        static_peer_count,
        online_count,
        tasks.len(),
    );

    // Log online peer details at debug level
    for peer in &online_peers {
        info!(
            "  Peer: {} ({}) - {} at {}",
            peer.base.name,
            peer.base.id,
            if peer.is_online() { "online" } else { "offline" },
            peer.base.address,
        );
    }

    Ok(())
}

/// Load peer addresses from the static peers.toml config.
fn load_peer_addresses(peers_path: &Path) -> Vec<String> {
    if !peers_path.exists() {
        return Vec::new();
    }

    match cluster_config::load_static_config(peers_path) {
        Ok(config) => {
            config.peers.iter()
                .filter(|p| p.enabled && !p.address.is_empty())
                .map(|p| p.address.clone())
                .collect()
        }
        Err(e) => {
            warn!("Failed to load peers.toml: {}", e);
            Vec::new()
        }
    }
}

/// Generate a deterministic node ID from hostname and timestamp.
fn generate_node_id() -> String {
    let hostname = std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "node".to_string());
    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    format!("node-{}-{}", hostname.to_lowercase(), timestamp)
}
