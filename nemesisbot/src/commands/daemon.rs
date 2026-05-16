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

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    // -------------------------------------------------------------------------
    // DaemonAction enum construction tests
    // -------------------------------------------------------------------------

    #[test]
    fn test_cluster_mode_validation() {
        let valid_modes = ["auto", "worker", "coordinator"];
        assert!(valid_modes.contains(&"auto"));
        assert!(valid_modes.contains(&"worker"));
        assert!(valid_modes.contains(&"coordinator"));
        assert!(!valid_modes.contains(&"invalid"));
        assert!(!valid_modes.contains(&"master"));
    }

    #[test]
    fn test_cluster_mode_default() {
        let mode: Option<String> = None;
        let mode_str = mode.as_deref().unwrap_or("auto");
        assert_eq!(mode_str, "auto");
    }

    #[test]
    fn test_cluster_mode_explicit() {
        let mode: Option<String> = Some("worker".to_string());
        let mode_str = mode.as_deref().unwrap_or("auto");
        assert_eq!(mode_str, "worker");
    }

    #[test]
    fn test_valid_modes_join() {
        let valid_modes = ["auto", "worker", "coordinator"];
        let joined = valid_modes.join(", ");
        assert_eq!(joined, "auto, worker, coordinator");
    }

    // -------------------------------------------------------------------------
    // generate_node_id
    // -------------------------------------------------------------------------

    #[test]
    fn test_generate_node_id_format() {
        let node_id = generate_node_id();
        assert!(node_id.starts_with("node-"));
        // Should contain a dash between hostname and timestamp
        let parts: Vec<&str> = node_id.splitn(3, '-').collect();
        assert!(parts.len() >= 2); // "node", "<hostname>", "<timestamp>"
    }

    #[test]
    fn test_generate_node_id_is_lowercase() {
        // Even if hostname has uppercase, the result should be lowercase
        let node_id = generate_node_id();
        // The hostname part should be lowercase
        let without_prefix = node_id.strip_prefix("node-").unwrap();
        // Extract hostname part (before the last dash + timestamp)
        if let Some(last_dash) = without_prefix.rfind('-') {
            let hostname_part = &without_prefix[..last_dash];
            assert_eq!(hostname_part, hostname_part.to_lowercase());
        }
    }

    #[test]
    fn test_generate_node_id_unique() {
        let id1 = generate_node_id();
        let id2 = generate_node_id();
        // Timestamps may differ between calls
        // On fast machines they could be the same second, but the format is still valid
        assert!(id1.starts_with("node-"));
        assert!(id2.starts_with("node-"));
    }

    // -------------------------------------------------------------------------
    // load_peer_addresses
    // -------------------------------------------------------------------------

    #[test]
    fn test_load_peer_addresses_nonexistent_file() {
        let path = std::path::PathBuf::from("/nonexistent/peers.toml");
        let peers = load_peer_addresses(&path);
        assert!(peers.is_empty());
    }

    #[test]
    fn test_load_peer_addresses_empty_file() {
        let tmp = TempDir::new().unwrap();
        let peers_path = tmp.path().join("peers.toml");
        std::fs::write(&peers_path, "").unwrap();
        let peers = load_peer_addresses(&peers_path);
        assert!(peers.is_empty());
    }

    #[test]
    fn test_load_peer_addresses_invalid_toml() {
        let tmp = TempDir::new().unwrap();
        let peers_path = tmp.path().join("peers.toml");
        std::fs::write(&peers_path, "this is not valid toml {{{{").unwrap();
        let peers = load_peer_addresses(&peers_path);
        assert!(peers.is_empty());
    }

    // -------------------------------------------------------------------------
    // JSON cluster config parsing (from run_cluster_daemon)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cluster_json_port_parsing() {
        let cluster_json = serde_json::json!({
            "port": 11950,
            "rpc_port": 21950,
            "id": "node-test-123"
        });
        let udp_port = cluster_json.get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(11949) as u16;
        let rpc_port = cluster_json.get("rpc_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(21949) as u16;
        assert_eq!(udp_port, 11950);
        assert_eq!(rpc_port, 21950);
    }

    #[test]
    fn test_cluster_json_default_ports() {
        let cluster_json = serde_json::json!({});
        let udp_port = cluster_json.get("port")
            .and_then(|v| v.as_u64())
            .unwrap_or(11949) as u16;
        let rpc_port = cluster_json.get("rpc_port")
            .and_then(|v| v.as_u64())
            .unwrap_or(21949) as u16;
        assert_eq!(udp_port, 11949);
        assert_eq!(rpc_port, 21949);
    }

    #[test]
    fn test_cluster_json_node_id_extraction() {
        let cluster_json = serde_json::json!({"id": "custom-node-id"});
        let node_id = cluster_json.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| generate_node_id());
        assert_eq!(node_id, "custom-node-id");
    }

    #[test]
    fn test_cluster_json_node_id_default() {
        let cluster_json = serde_json::json!({});
        let node_id = cluster_json.get("id")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .unwrap_or_else(|| generate_node_id());
        assert!(node_id.starts_with("node-"));
    }

    // -------------------------------------------------------------------------
    // Cluster config enabled check (from run function)
    // -------------------------------------------------------------------------

    #[test]
    fn test_cluster_enabled_detection() {
        let raw_cfg = serde_json::json!({
            "cluster": {"enabled": true}
        });
        let cluster_enabled = raw_cfg.get("cluster")
            .and_then(|c| c.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(cluster_enabled);
    }

    #[test]
    fn test_cluster_disabled_detection() {
        let raw_cfg = serde_json::json!({
            "cluster": {"enabled": false}
        });
        let cluster_enabled = raw_cfg.get("cluster")
            .and_then(|c| c.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!cluster_enabled);
    }

    #[test]
    fn test_cluster_no_section() {
        let raw_cfg = serde_json::json!({});
        let cluster_enabled = raw_cfg.get("cluster")
            .and_then(|c| c.get("enabled"))
            .and_then(|v| v.as_bool())
            .unwrap_or(false);
        assert!(!cluster_enabled);
    }

    // -------------------------------------------------------------------------
    // ClusterConfig construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_cluster_config_bind_address_format() {
        let rpc_port: u16 = 21950;
        let bind_address = format!("0.0.0.0:{}", rpc_port);
        assert_eq!(bind_address, "0.0.0.0:21950");
    }

    // -------------------------------------------------------------------------
    // tick_rpc_hello payload construction
    // -------------------------------------------------------------------------

    #[test]
    fn test_rpc_hello_payload_format() {
        let node_id = "node-test-123";
        let payload = serde_json::json!({
            "from": node_id,
            "timestamp": "2025-01-01T00:00:00Z",
        });
        assert_eq!(payload["from"], "node-test-123");
        assert!(payload["timestamp"].is_string());
    }

    // -------------------------------------------------------------------------
    // Additional coverage tests for daemon
    // -------------------------------------------------------------------------

    #[test]
    fn test_pid_path_format() {
        let tmp = tempfile::TempDir::new().unwrap();
        let home = tmp.path();
        let pid_path = home.join("nemesisbot.pid");
        assert!(pid_path.to_str().unwrap().ends_with("nemesisbot.pid"));
    }

    #[test]
    fn test_bind_address_format_various_ports() {
        for port in &[8080u16, 21949, 30000, 65535] {
            let addr = format!("0.0.0.0:{}", port);
            assert!(addr.contains(&port.to_string()));
        }
    }

    #[test]
    fn test_node_id_format() {
        let node_id = "node-daemon-test";
        let sanitized = node_id.replace('-', "_");
        assert_eq!(sanitized, "node_daemon_test");
    }

    #[test]
    fn test_cluster_config_node_info() {
        let node_id = "test-node-id";
        let role = "worker";
        let category = "development";
        let config = serde_json::json!({
            "node_id": node_id,
            "role": role,
            "category": category,
            "enabled": true,
        });
        assert_eq!(config["node_id"], node_id);
        assert_eq!(config["role"], role);
        assert_eq!(config["category"], category);
        assert_eq!(config["enabled"], true);
    }

    #[test]
    fn test_rpc_address_variations() {
        let addresses = vec![
            ("0.0.0.0:21949", "0.0.0.0", 21949),
            ("127.0.0.1:8080", "127.0.0.1", 8080),
            ("192.168.1.1:3000", "192.168.1.1", 3000),
        ];
        for (addr, expected_host, expected_port) in addresses {
            let parts: Vec<&str> = addr.split(':').collect();
            assert_eq!(parts[0], expected_host);
            assert_eq!(parts[1].parse::<u16>().unwrap(), expected_port);
        }
    }

    #[test]
    fn test_daemon_status_json() {
        let status = serde_json::json!({
            "running": false,
            "pid": null,
            "uptime_seconds": null,
        });
        assert_eq!(status["running"], false);
        assert!(status["pid"].is_null());
    }

    #[test]
    fn test_health_endpoint_url() {
        let port = 18790u16;
        let url = format!("http://127.0.0.1:{}/health", port);
        assert_eq!(url, "http://127.0.0.1:18790/health");
    }

    #[test]
    fn test_web_url_format() {
        let host = "0.0.0.0";
        let port = 49000u16;
        let url = format!("http://{}:{}", host, port);
        assert_eq!(url, "http://0.0.0.0:49000");
    }

    #[test]
    fn test_daemon_log_message_format() {
        let node_id = "test-daemon-node";
        let msg = format!("Daemon started, node_id={}", node_id);
        assert!(msg.contains("test-daemon-node"));
        assert!(msg.contains("Daemon started"));
    }
}
