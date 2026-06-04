use super::*;

use crate::registry::HealthConfig;

fn make_announce(node_id: &str, addresses: &[&str], rpc_port: u16) -> DiscoveryMessage {
    DiscoveryMessage::new_announce(
        node_id,
        "TestNode",
        addresses.iter().map(|s| (*s).to_string()).collect(),
        rpc_port,
        "worker",
        "development",
        vec![],
        vec!["llm".into()],
        "agent",
    )
}

// -----------------------------------------------------------------------
// handle_discovery_message tests
// -----------------------------------------------------------------------

#[test]
fn test_handle_announce_registers_peer() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let msg = make_announce("remote-node-1", &["10.0.0.5"], 9000);

    let action = handle_discovery_message(&msg, "local-node", &registry);
    assert_eq!(action, DiscoveryAction::None);
    assert!(registry.get("remote-node-1").is_some());
}

#[test]
fn test_handle_bye_removes_peer() {
    let registry = PeerRegistry::new(HealthConfig::default());
    registry.upsert(message_to_node_info(&make_announce("remote-3", &["10.0.0.7"], 9000)));
    assert!(registry.get("remote-3").is_some());

    let bye = DiscoveryMessage::new_bye("remote-3");

    let action = handle_discovery_message(&bye, "local", &registry);
    assert_eq!(action, DiscoveryAction::None);
    assert!(registry.get("remote-3").is_none());
}

#[test]
fn test_ignore_own_messages() {
    let registry = PeerRegistry::new(HealthConfig::default());
    let msg = make_announce("local-node", &["10.0.0.1"], 9000);

    let action = handle_discovery_message(&msg, "local-node", &registry);
    assert_eq!(action, DiscoveryAction::Ignore);
    assert!(registry.is_empty());
}

#[test]
fn test_message_to_node_info_uses_first_address() {
    let msg = make_announce("node-abc", &["10.0.0.1", "192.168.1.1"], 8080);
    let info = message_to_node_info(&msg);
    assert_eq!(info.base.address, "10.0.0.1:8080");
    assert_eq!(info.base.name, "TestNode");
    assert_eq!(info.base.role, NodeRole::Worker);
    assert_eq!(info.base.category, "development");
}

#[test]
fn test_message_to_node_info_manager_role() {
    let msg = DiscoveryMessage::new_announce(
        "mgr-1", "ManagerNode", vec!["10.0.0.1".into()], 9000, "manager", "production", vec![], vec![], "agent",
    );
    let info = message_to_node_info(&msg);
    assert_eq!(info.base.role, NodeRole::Master);
}

// -----------------------------------------------------------------------
// UdpListener tests
// -----------------------------------------------------------------------

#[test]
fn test_udp_listener_bind_port_zero() {
    // Port 0 = OS assigns a free port
    let listener = UdpListener::new(0, None).unwrap();
    assert_ne!(listener.port(), 0);
    assert!(!listener.is_running());
}

#[test]
fn test_udp_listener_bind_specific_port() {
    // Bind to a random available port first to get a port number,
    // then try to bind UdpListener to that same port.
    let temp = UdpSocket::bind("0.0.0.0:0").unwrap();
    let port = temp.local_addr().unwrap().port();
    drop(temp); // Free the port

    let listener = UdpListener::new(port, None).unwrap();
    assert_eq!(listener.port(), port);
}

#[test]
fn test_udp_listener_start_stop_lifecycle() {
    let listener = UdpListener::new(0, None).unwrap();

    listener.start().unwrap();
    assert!(listener.is_running());

    // Small delay to let the receive thread start
    std::thread::sleep(Duration::from_millis(50));

    listener.stop().unwrap();
    assert!(!listener.is_running());
}

#[test]
fn test_udp_listener_double_start_fails() {
    let listener = UdpListener::new(0, None).unwrap();
    listener.start().unwrap();
    let result = listener.start();
    assert!(result.is_err());
    listener.stop().unwrap();
}

#[test]
fn test_udp_listener_stop_when_not_started_fails() {
    let listener = UdpListener::new(0, None).unwrap();
    let result = listener.stop();
    assert!(result.is_err());
}

#[test]
fn test_udp_listener_send_receive_roundtrip() {
    // Create a listener on an OS-assigned port
    let listener = UdpListener::new(0, None).unwrap();
    let port = listener.port();

    // Received messages stored here
    let received = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let received_clone = Arc::clone(&received);

    listener.set_message_handler(Box::new(move |msg, _addr| {
        received_clone.lock().push(msg.node_id.clone());
    }));

    listener.start().unwrap();
    std::thread::sleep(Duration::from_millis(50));

    // Send a message to the listener
    let sender = UdpSocket::bind("0.0.0.0:0").unwrap();
    sender.set_broadcast(true).unwrap();
    let msg = DiscoveryMessage::new_announce(
        "test-sender", "SenderNode",
        vec!["10.0.0.1".into()], 9000,
        "worker", "dev", vec![], vec![], "agent",
    );
    let data = msg.to_bytes().unwrap();
    sender.send_to(&data, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).unwrap();

    // Wait for receive
    std::thread::sleep(Duration::from_millis(200));

    listener.stop().unwrap();

    let msgs = received.lock();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0], "test-sender");
}

#[test]
fn test_udp_listener_encrypted_roundtrip() {
    let key = crate::discovery::crypto::derive_key("test-cluster-secret");

    let listener = UdpListener::new(0, Some(key)).unwrap();
    let _port = listener.port();

    let received = Arc::new(parking_lot::Mutex::new(Vec::<String>::new()));
    let received_clone = Arc::clone(&received);

    listener.set_message_handler(Box::new(move |msg, _addr| {
        received_clone.lock().push(msg.node_id.clone());
    }));

    listener.start().unwrap();
    std::thread::sleep(Duration::from_millis(50));

    // Send an encrypted broadcast
    listener.broadcast(&DiscoveryMessage::new_announce(
        "encrypted-node", "EncNode",
        vec!["10.0.0.1".into()], 9000,
        "worker", "dev", vec![], vec![], "agent",
    )).unwrap();

    std::thread::sleep(Duration::from_millis(200));
    listener.stop().unwrap();

    let msgs = received.lock();
    assert_eq!(msgs.len(), 1);
    assert_eq!(msgs[0], "encrypted-node");
}

#[test]
fn test_udp_listener_ignores_invalid_json() {
    let listener = UdpListener::new(0, None).unwrap();
    let port = listener.port();

    let received = Arc::new(parking_lot::Mutex::new(0u32));
    let received_clone = Arc::clone(&received);

    listener.set_message_handler(Box::new(move |_msg, _addr| {
        *received_clone.lock() += 1;
    }));

    listener.start().unwrap();
    std::thread::sleep(Duration::from_millis(50));

    // Send invalid data
    let sender = UdpSocket::bind("0.0.0.0:0").unwrap();
    sender.send_to(b"not json at all", SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).unwrap();

    std::thread::sleep(Duration::from_millis(200));
    listener.stop().unwrap();

    assert_eq!(*received.lock(), 0);
}

#[test]
fn test_udp_listener_ignores_wrong_encryption_key() {
    let key1 = crate::discovery::crypto::derive_key("correct-key");
    let key2 = crate::discovery::crypto::derive_key("wrong-key");

    let listener = UdpListener::new(0, Some(key1)).unwrap();
    let port = listener.port();

    let received = Arc::new(parking_lot::Mutex::new(0u32));
    let received_clone = Arc::clone(&received);

    listener.set_message_handler(Box::new(move |_msg, _addr| {
        *received_clone.lock() += 1;
    }));

    listener.start().unwrap();
    std::thread::sleep(Duration::from_millis(50));

    // Send data encrypted with the WRONG key
    let sender = UdpSocket::bind("0.0.0.0:0").unwrap();
    let msg = DiscoveryMessage::new_announce(
        "attacker", "BadNode", vec!["10.0.0.1".into()], 9000,
        "worker", "dev", vec![], vec![], "agent",
    );
    let plaintext = msg.to_bytes().unwrap();
    let encrypted = crate::discovery::crypto::encrypt_data(&key2, &plaintext).unwrap();
    sender.send_to(&encrypted, SocketAddrV4::new(Ipv4Addr::LOCALHOST, port)).unwrap();

    std::thread::sleep(Duration::from_millis(200));
    listener.stop().unwrap();

    // Should have been silently discarded
    assert_eq!(*received.lock(), 0);
}

// -----------------------------------------------------------------------
// Broadcast address tests
// -----------------------------------------------------------------------

#[test]
fn test_get_broadcast_addresses_includes_global() {
    let addrs = get_broadcast_addresses();
    assert!(addrs.contains(&Ipv4Addr::BROADCAST));
}

#[test]
fn test_compute_broadcast() {
    let ip = Ipv4Addr::new(192, 168, 1, 100);
    let mask = [255, 255, 255, 0];
    let broadcast = compute_broadcast(ip, mask);
    assert_eq!(broadcast, Ipv4Addr::new(192, 168, 1, 255));
}

#[test]
fn test_compute_broadcast_class_b() {
    let ip = Ipv4Addr::new(172, 16, 5, 100);
    let mask = [255, 255, 0, 0];
    let broadcast = compute_broadcast(ip, mask);
    assert_eq!(broadcast, Ipv4Addr::new(172, 16, 255, 255));
}

#[test]
fn test_compute_broadcast_class_a() {
    let ip = Ipv4Addr::new(10, 0, 0, 1);
    let mask = [255, 0, 0, 0];
    let broadcast = compute_broadcast(ip, mask);
    assert_eq!(broadcast, Ipv4Addr::new(10, 255, 255, 255));
}

// -----------------------------------------------------------------------
// get_all_local_ips
// -----------------------------------------------------------------------

#[test]
fn test_get_all_local_ips_returns_something() {
    let _ips = get_all_local_ips();
    // On a machine with network connectivity, we should find at least one IP
    // (but in CI this may be empty, so we just test it doesn't panic)
}

// ============================================================
// Coverage improvement: more listener edge cases
// ============================================================

#[test]
fn test_format_timestamp_returns_string() {
    let ts = format_timestamp(1700000000);
    // Should be a valid date string
    assert!(!ts.is_empty());
}

#[test]
fn test_local_ip_addresses_returns_result() {
    let result = local_ip_addresses();
    // Just verify it doesn't panic
    let _ = result;
}

#[test]
fn test_get_broadcast_addresses_returns_vec() {
    let addrs = get_broadcast_addresses();
    // Just verify it doesn't panic
    let _ = addrs;
}

#[test]
fn test_message_to_node_info_from_message() {
    use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
    let msg = DiscoveryMessage {
        version: "1.0".into(),
        msg_type: DiscoveryMessageType::Announce,
        node_id: "node-1".into(),
        name: "Test Node".into(),
        addresses: vec!["192.168.1.10".into()],
        rpc_port: 21949,
        role: "worker".into(),
        category: "development".into(),
        tags: vec!["test".into()],
        capabilities: vec!["cluster".into()],
        node_type: "agent".into(),
        timestamp: 1700000000,
    };
    let info = message_to_node_info(&msg);
    assert_eq!(info.base.id, "node-1");
    assert_eq!(info.base.name, "Test Node");
}

#[test]
fn test_handle_discovery_message_own_node_ignored() {
    use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
    use crate::registry::{PeerRegistry, HealthConfig};
    let registry = PeerRegistry::new(HealthConfig::default());
    let msg = DiscoveryMessage {
        version: "1.0".into(),
        msg_type: DiscoveryMessageType::Announce,
        node_id: "local-node".into(),
        name: "Local".into(),
        addresses: vec![],
        rpc_port: 9000,
        role: "worker".into(),
        category: "general".into(),
        tags: vec![],
        capabilities: vec![],
        node_type: "agent".into(),
        timestamp: 1700000000,
    };
    let action = handle_discovery_message(&msg, "local-node", &registry);
    assert!(matches!(action, DiscoveryAction::Ignore));
}

#[test]
fn test_handle_discovery_message_remote_announce() {
    use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
    use crate::registry::{PeerRegistry, HealthConfig};
    let registry = PeerRegistry::new(HealthConfig::default());
    let msg = DiscoveryMessage {
        version: "1.0".into(),
        msg_type: DiscoveryMessageType::Announce,
        node_id: "remote-node".into(),
        name: "Remote".into(),
        addresses: vec!["10.0.0.1".into()],
        rpc_port: 9000,
        role: "worker".into(),
        category: "general".into(),
        tags: vec![],
        capabilities: vec![],
        node_type: "agent".into(),
        timestamp: 1700000000,
    };
    let action = handle_discovery_message(&msg, "local-node", &registry);
    assert!(matches!(action, DiscoveryAction::None));
}

#[test]
fn test_handle_discovery_message_bye() {
    use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
    use crate::registry::{PeerRegistry, HealthConfig};
    let registry = PeerRegistry::new(HealthConfig::default());
    // First add the node
    let announce = DiscoveryMessage {
        version: "1.0".into(),
        msg_type: DiscoveryMessageType::Announce,
        node_id: "bye-node".into(),
        name: "Bye".into(),
        addresses: vec!["10.0.0.2".into()],
        rpc_port: 9000,
        role: "worker".into(),
        category: "general".into(),
        tags: vec![],
        capabilities: vec![],
        node_type: "agent".into(),
        timestamp: 1700000000,
    };
    handle_discovery_message(&announce, "local-node", &registry);

    // Now send bye
    let bye = DiscoveryMessage {
        version: "1.0".into(),
        msg_type: DiscoveryMessageType::Bye,
        node_id: "bye-node".into(),
        name: "Bye".into(),
        addresses: vec![],
        rpc_port: 9000,
        role: "worker".into(),
        category: "general".into(),
        tags: vec![],
        capabilities: vec![],
        node_type: "agent".into(),
        timestamp: 1700000001,
    };
    let action = handle_discovery_message(&bye, "local-node", &registry);
    assert!(matches!(action, DiscoveryAction::None));
}

#[test]
fn test_compute_broadcast_class_c() {
    let ip = Ipv4Addr::new(192, 168, 1, 100);
    let mask = [255, 255, 255, 0];
    let broadcast = compute_broadcast(ip, mask);
    assert_eq!(broadcast, Ipv4Addr::new(192, 168, 1, 255));
}
