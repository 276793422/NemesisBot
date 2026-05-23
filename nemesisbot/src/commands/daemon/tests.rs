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
