use super::*;

/// Helper to create a base `ExtendedNodeInfo` for tests with empty addresses.
fn make_test_node(
    id: &str,
    status: NodeStatus,
    capabilities: Vec<&str>,
    last_seen: &str,
) -> ExtendedNodeInfo {
    ExtendedNodeInfo {
        base: NodeInfo {
            id: id.into(),
            name: format!("{}-name", id),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: last_seen.into(),
        },
        status,
        capabilities: capabilities.into_iter().map(String::from).collect(),
        addresses: vec![],
        node_type: "agent".into(),
    }
}

#[test]
fn test_cluster_config_default() {
    let config = ClusterConfig::default();
    assert!(config.node_id.is_empty());
    assert_eq!(config.bind_address, "0.0.0.0:9000");
    assert!(config.peers.is_empty());
}

#[test]
fn test_extended_node_info_serialization() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "worker-1".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: "2026-04-29T00:00:00Z".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "tools".into()],
        addresses: vec![],
        node_type: "agent".into(),
    };
    let json = serde_json::to_string(&node).unwrap();
    let back: ExtendedNodeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.status, NodeStatus::Online);
    assert_eq!(back.capabilities.len(), 2);
    assert!(back.addresses.is_empty());
}

#[test]
fn test_node_status_roundtrip() {
    let status = NodeStatus::Connecting;
    let json = serde_json::to_string(&status).unwrap();
    let back: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, NodeStatus::Connecting);
}

#[test]
fn test_extended_node_info_get_uptime() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "worker-1".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: chrono::Local::now()
                .format("%Y-%m-%dT%H:%M:%SZ")
                .to_string(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        addresses: vec![],
        node_type: "agent".into(),
    };
    let uptime = node.get_uptime();
    // Should be very small since we just set it
    assert!(uptime.as_secs() < 10);
}

#[test]
fn test_extended_node_info_get_uptime_empty() {
    let node = make_test_node("node-1", NodeStatus::Online, vec![], "");
    assert_eq!(node.get_uptime(), std::time::Duration::ZERO);
}

#[test]
fn test_extended_node_info_get_uptime_invalid() {
    let node = make_test_node("node-1", NodeStatus::Online, vec![], "not-a-date");
    assert_eq!(node.get_uptime(), std::time::Duration::ZERO);
}

#[test]
fn test_is_online() {
    let mut node = make_test_node("node-1", NodeStatus::Online, vec![], "");
    assert!(node.is_online());
    node.status = NodeStatus::Offline;
    assert!(!node.is_online());
}

#[test]
fn test_set_status() {
    let mut node = make_test_node("node-1", NodeStatus::Offline, vec![], "");
    assert!(!node.is_online());
    node.set_status(NodeStatus::Online);
    assert!(node.is_online());
    assert!(!node.base.last_seen.is_empty());
}

#[test]
fn test_update_last_seen() {
    let mut node = make_test_node("node-1", NodeStatus::Offline, vec![], "");
    node.update_last_seen();
    assert!(node.is_online());
    assert!(!node.base.last_seen.is_empty());
}

#[test]
fn test_mark_offline() {
    let mut node = make_test_node("node-1", NodeStatus::Online, vec![], "");
    node.mark_offline("connection lost");
    assert!(!node.is_online());
    assert_eq!(node.status, NodeStatus::Offline);
}

#[test]
fn test_has_capability() {
    let node = make_test_node("node-1", NodeStatus::Online, vec!["llm", "tools"], "");
    assert!(node.has_capability("llm"));
    assert!(node.has_capability("LLM")); // case-insensitive
    assert!(node.has_capability("tools"));
    assert!(!node.has_capability("webhook"));
}

#[test]
fn test_to_peer_config() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "worker-1".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: "2026-04-29T00:00:00Z".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        addresses: vec!["10.0.0.1".into(), "192.168.1.1".into()],
        node_type: "agent".into(),
    };
    let config = node.to_peer_config();
    assert_eq!(config.id, "node-1");
    assert_eq!(config.name, "worker-1");
    assert_eq!(config.address, "10.0.0.1:9000");
    assert_eq!(config.role, "worker");
    assert_eq!(config.category, "development");
    assert_eq!(config.status.state, "online");
}

#[test]
fn test_display() {
    let node = make_test_node("node-1", NodeStatus::Online, vec![], "");
    let s = format!("{}", node);
    assert!(s.contains("node-1"));
    assert!(s.contains("10.0.0.1:9000"));
    assert!(s.contains("online"));
}

#[test]
fn test_addresses_field_default() {
    // Verify that deserializing without addresses field gives empty vec
    let json = r#"{"id":"n1","name":"n1","role":"Worker","address":"10.0.0.1:9000","category":"dev","last_seen":"","status":"Online","capabilities":[]}"#;
    let node: ExtendedNodeInfo = serde_json::from_str(json).unwrap();
    assert!(node.addresses.is_empty());
}

#[test]
fn test_addresses_field_preserved() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "worker-1".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: "".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec!["10.0.0.1".into(), "192.168.1.1".into()],
        node_type: "agent".into(),
    };
    let json = serde_json::to_string(&node).unwrap();
    let back: ExtendedNodeInfo = serde_json::from_str(&json).unwrap();
    assert_eq!(back.addresses.len(), 2);
    assert_eq!(back.addresses[0], "10.0.0.1");
    assert_eq!(back.addresses[1], "192.168.1.1");
}

// -- Additional tests: role capability checking, config validation, edge cases --

#[test]
fn test_cluster_config_custom_values() {
    let config = ClusterConfig {
        node_id: "bot-42".into(),
        bind_address: "192.168.1.100:9100".into(),
        peers: vec!["10.0.0.1:9000".into(), "10.0.0.2:9000".into()],
    };
    assert_eq!(config.node_id, "bot-42");
    assert_eq!(config.bind_address, "192.168.1.100:9100");
    assert_eq!(config.peers.len(), 2);
}

#[test]
fn test_node_status_variants() {
    assert_ne!(NodeStatus::Online, NodeStatus::Offline);
    assert_ne!(NodeStatus::Online, NodeStatus::Connecting);
    assert_ne!(NodeStatus::Offline, NodeStatus::Connecting);
}

#[test]
fn test_get_status_string_all_variants() {
    let mut node = make_test_node("n1", NodeStatus::Online, vec![], "");
    assert_eq!(node.get_status_string(), "online");

    node.status = NodeStatus::Offline;
    assert_eq!(node.get_status_string(), "offline");

    node.status = NodeStatus::Connecting;
    assert_eq!(node.get_status_string(), "connecting");
}

#[test]
fn test_extended_node_info_getters() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-42".into(),
            name: "TestBot".into(),
            role: NodeRole::Master,
            address: "10.0.0.1:9000".into(),
            category: "testing".into(),
            last_seen: "".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into(), "tools".into()],
        addresses: vec!["10.0.0.1".into()],
        node_type: "agent".into(),
    };
    assert_eq!(node.get_id(), "node-42");
    assert_eq!(node.get_name(), "TestBot");
    assert_eq!(node.get_address(), "10.0.0.1:9000");
    assert_eq!(node.get_capabilities().len(), 2);
}

#[test]
fn test_to_peer_config_master_role() {
    let node = ExtendedNodeInfo {
        base: NodeInfo {
            id: "master-1".into(),
            name: "MasterNode".into(),
            role: NodeRole::Master,
            address: "10.0.0.1:9000".into(),
            category: "production".into(),
            last_seen: "2026-04-29T00:00:00Z".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec!["llm".into()],
        addresses: vec![],
        node_type: "agent".into(),
    };
    let config = node.to_peer_config();
    assert_eq!(config.role, "master");
    assert_eq!(config.category, "production");
}

#[test]
fn test_has_capability_empty_capabilities() {
    let node = make_test_node("n1", NodeStatus::Online, vec![], "");
    assert!(!node.has_capability("llm"));
    assert!(!node.has_capability("anything"));
}

#[test]
fn test_cluster_config_serialization_roundtrip() {
    let config = ClusterConfig {
        node_id: "node-abc".into(),
        bind_address: "0.0.0.0:8080".into(),
        peers: vec!["host1:9000".into(), "host2:9000".into()],
    };
    let json = serde_json::to_string(&config).unwrap();
    let back: ClusterConfig = serde_json::from_str(&json).unwrap();
    assert_eq!(back.node_id, "node-abc");
    assert_eq!(back.peers.len(), 2);
}

// -- Additional tests: get_uptime future timestamp, Connecting serialization --

#[test]
fn test_get_uptime_future_timestamp() {
    // Set last_seen to 1 hour in the future
    let future_ts = (chrono::Local::now() + chrono::Duration::hours(1)).to_rfc3339();
    let node = make_test_node("future-node", NodeStatus::Online, vec![], &future_ts);
    let uptime = node.get_uptime();
    // Future timestamps should return Duration::ZERO (negative duration case)
    assert_eq!(uptime, std::time::Duration::ZERO);
}

#[test]
fn test_node_status_connecting_serialization_roundtrip() {
    let status = NodeStatus::Connecting;
    let json = serde_json::to_string(&status).unwrap();
    assert!(
        json.contains("Connecting"),
        "expected Connecting in JSON, got: {}",
        json
    );
    let back: NodeStatus = serde_json::from_str(&json).unwrap();
    assert_eq!(back, NodeStatus::Connecting);
}

#[test]
fn test_node_status_connecting_string_representation() {
    let node = make_test_node("n-conn", NodeStatus::Connecting, vec![], "");
    assert_eq!(node.get_status_string(), "connecting");
    // Display should show "connecting"
    let display = format!("{}", node);
    assert!(
        display.contains("connecting"),
        "expected 'connecting' in display: {}",
        display
    );
}

// -- content_eq tests --

#[test]
fn test_content_eq_identical_nodes() {
    let node = make_test_node("node-1", NodeStatus::Online, vec!["llm"], "");
    let same = make_test_node("node-1", NodeStatus::Online, vec!["llm"], "");
    assert!(node.content_eq(&same));
}

#[test]
fn test_content_eq_ignores_last_seen() {
    let a = make_test_node("node-1", NodeStatus::Online, vec![], "2026-01-01T00:00:00Z");
    let b = make_test_node("node-1", NodeStatus::Online, vec![], "2026-06-01T00:00:00Z");
    assert!(a.content_eq(&b));
}

#[test]
fn test_content_eq_different_id() {
    let a = make_test_node("node-1", NodeStatus::Online, vec![], "");
    let b = make_test_node("node-2", NodeStatus::Online, vec![], "");
    assert!(!a.content_eq(&b));
}

#[test]
fn test_content_eq_different_capabilities() {
    let a = make_test_node("node-1", NodeStatus::Online, vec!["llm"], "");
    let b = make_test_node("node-1", NodeStatus::Online, vec!["llm", "tools"], "");
    assert!(!a.content_eq(&b));
}

#[test]
fn test_content_eq_different_status() {
    let a = make_test_node("node-1", NodeStatus::Online, vec![], "");
    let b = make_test_node("node-1", NodeStatus::Offline, vec![], "");
    assert!(!a.content_eq(&b));
}

#[test]
fn test_content_eq_different_addresses() {
    let a = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "node-1-name".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: "".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec!["10.0.0.1".into()],
        node_type: "agent".into(),
    };
    let b = ExtendedNodeInfo {
        base: NodeInfo {
            id: "node-1".into(),
            name: "node-1-name".into(),
            role: NodeRole::Worker,
            address: "10.0.0.1:9000".into(),
            category: "development".into(),
            last_seen: "".into(),
        },
        status: NodeStatus::Online,
        capabilities: vec![],
        addresses: vec!["10.0.0.1".into(), "192.168.1.1".into()],
        node_type: "agent".into(),
    };
    assert!(!a.content_eq(&b));
}

#[test]
fn test_content_eq_different_node_type() {
    let a = make_test_node("node-1", NodeStatus::Online, vec![], "");
    let mut b = make_test_node("node-1", NodeStatus::Online, vec![], "");
    b.node_type = "node".into();
    assert!(!a.content_eq(&b));
}
