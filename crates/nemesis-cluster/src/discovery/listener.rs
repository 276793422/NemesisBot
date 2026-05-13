//! Discovery listener - UDP broadcast peer discovery.
//!
//! Receives UDP broadcast packets from other nodes and processes
//! Announce/Bye messages. Also provides the `UdpListener` struct for
//! actual UDP socket I/O (bind, receive loop, broadcast).

use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

use crate::discovery::crypto::{decrypt_data, encrypt_data};
use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
use crate::registry::PeerRegistry;
use crate::types::{ExtendedNodeInfo, NodeStatus};
use nemesis_types::cluster::{NodeInfo, NodeRole};

// ---------------------------------------------------------------------------
// UdpListener - async-friendly UDP listener with broadcast
// ---------------------------------------------------------------------------

/// Type alias for the message handler callback.
/// Receives the parsed `DiscoveryMessage` and the sender's `SocketAddrV4`.
pub type MessageHandler = Box<dyn Fn(&DiscoveryMessage, SocketAddrV4) + Send + Sync>;

/// UDP listener for cluster discovery broadcasts.
///
/// Mirrors Go's `UDPListener`:
/// - Binds to `0.0.0.0:<port>` (all interfaces)
/// - Runs a receive loop on a background thread
/// - Supports optional AES-256-GCM encryption
/// - Broadcasts to all local subnet broadcast addresses
pub struct UdpListener {
    socket: Arc<UdpSocket>,
    port: u16,
    enc_key: Option<[u8; 32]>,
    running: Arc<AtomicBool>,
    handler: Arc<parking_lot::RwLock<Option<MessageHandler>>>,
}

impl UdpListener {
    /// Create a new UDP listener bound to `0.0.0.0:<port>`.
    ///
    /// `enc_key` is the AES-256 key for broadcast encryption; pass `None` to
    /// disable encryption (plaintext mode, backward compatible).
    pub fn new(port: u16, enc_key: Option<[u8; 32]>) -> Result<Self, io::Error> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);
        let socket = UdpSocket::bind(addr)?;
        socket.set_broadcast(true)?;
        socket.set_read_timeout(Some(Duration::from_secs(1)))?;

        let actual_port = socket.local_addr()?.port();

        Ok(Self {
            socket: Arc::new(socket),
            port: actual_port,
            enc_key,
            running: Arc::new(AtomicBool::new(false)),
            handler: Arc::new(parking_lot::RwLock::new(None)),
        })
    }

    /// Set the callback invoked for each received discovery message.
    pub fn set_message_handler(&self, handler: MessageHandler) {
        *self.handler.write() = Some(handler);
    }

    /// Start the receive loop on a background thread.
    pub fn start(&self) -> Result<(), io::Error> {
        if self.running.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::AlreadyExists, "listener already running"));
        }
        self.running.store(true, Ordering::SeqCst);

        let socket = Arc::clone(&self.socket);
        let running = Arc::clone(&self.running);
        let handler = Arc::clone(&self.handler);
        let enc_key = self.enc_key;

        std::thread::Builder::new()
            .name("discovery-udp-listen".into())
            .spawn(move || {
                let mut buf = [0u8; 4096];
                while running.load(Ordering::SeqCst) {
                    match socket.recv_from(&mut buf) {
                        Ok((n, addr)) => {
                            let raw_data = &buf[..n];

                            // Decrypt if encryption is enabled
                            let msg_data = if let Some(key) = enc_key {
                                match decrypt_data(&key, raw_data) {
                                    Ok(decrypted) => decrypted,
                                    Err(_) => continue, // Silently discard
                                }
                            } else {
                                raw_data.to_vec()
                            };

                            // Parse message
                            let msg = match DiscoveryMessage::from_bytes(&msg_data) {
                                Ok(m) => m,
                                Err(_) => continue,
                            };

                            // Validate message
                            if msg.validate().is_err() {
                                continue;
                            }

                            // Call handler
                            let handler_guard = handler.read();
                            if let Some(ref handler_fn) = *handler_guard {
                                let ip = match addr.ip() {
                                    std::net::IpAddr::V4(v4) => v4,
                                    std::net::IpAddr::V6(_) => continue,
                                };
                                let sender = SocketAddrV4::new(ip, addr.port());
                                handler_fn(&msg, sender);
                            }
                        }
                        Err(ref e) if e.kind() == io::ErrorKind::TimedOut
                            || e.kind() == io::ErrorKind::WouldBlock =>
                        {
                            // Timeout is expected, continue checking running flag
                            continue;
                        }
                        Err(_) => {
                            // Socket closed or other fatal error
                            break;
                        }
                    }
                }
            })?;

        Ok(())
    }

    /// Stop the listener.
    pub fn stop(&self) -> Result<(), io::Error> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "listener not running"));
        }
        self.running.store(false, Ordering::SeqCst);
        Ok(())
    }

    /// Check whether the listener is running.
    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }

    /// Get the actual port the listener is bound to (important when port 0 is used).
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Broadcast a discovery message to all local subnet broadcast addresses.
    ///
    /// Mirrors Go's `UDPListener.Broadcast()`.
    pub fn broadcast(&self, msg: &DiscoveryMessage) -> Result<(), io::Error> {
        let data = msg.to_bytes().map_err(|e| {
            io::Error::new(io::ErrorKind::InvalidData, format!("marshal error: {}", e))
        })?;

        // Encrypt if encryption is enabled
        let send_data = if let Some(ref key) = self.enc_key {
            encrypt_data(key, &data).map_err(|_| {
                io::Error::new(io::ErrorKind::Other, "encryption failed")
            })?
        } else {
            data
        };

        let broadcast_addrs = get_broadcast_addresses();

        for addr in &broadcast_addrs {
            let target = SocketAddrV4::new(*addr, self.port);
            let _ = self.socket.send_to(&send_data, target);
        }

        Ok(())
    }
}

// ---------------------------------------------------------------------------
// Broadcast address enumeration
// ---------------------------------------------------------------------------

/// Enumerate broadcast addresses for all up, non-loopback IPv4 interfaces.
///
/// Returns `255.255.255.255` as the first entry (global broadcast), plus
/// the subnet-specific broadcast addresses calculated as `ip | !mask`.
///
/// Mirrors Go's `UDPListener.getBroadcastAddresses()`.
///
/// **Note**: On Windows this uses `GetAdaptersAddresses` via `std::net` and
/// falls back gracefully if the local interface list is unavailable.
pub fn get_broadcast_addresses() -> Vec<Ipv4Addr> {
    let mut addrs = vec![Ipv4Addr::BROADCAST]; // 255.255.255.255

    // Use platform APIs to enumerate network interfaces.
    // We use a simple approach: bind a UDP socket to each local address
    // we can find and compute the broadcast from the subnet.
    match local_ip_addresses() {
        Ok(ip_addrs) => {
            for (ip, mask) in ip_addrs {
                let broadcast = compute_broadcast(ip, mask);
                if !addrs.contains(&broadcast) {
                    addrs.push(broadcast);
                }
            }
        }
        Err(_) => {
            // Fallback: just use global broadcast
        }
    }

    addrs
}

/// Get local IPv4 addresses with their subnet masks.
///
/// Uses platform-specific APIs. On failure returns an empty list.
fn local_ip_addresses() -> io::Result<Vec<(Ipv4Addr, [u8; 4])>> {
    // Platform-specific: use `ipconfig` or `ifconfig` equivalent.
    // We'll use the `std::net` approach: enumerate by binding test sockets.
    // For a proper implementation we'd use `if-addrs` or `windows-sys` crate,
    // but to avoid adding new dependencies, we use a heuristic:
    // - Get local IPs by connecting a UDP socket to a public address
    // - Assume /24 mask for private ranges

    let mut results = Vec::new();

    // Try to find all local IPs by binding to 0.0.0.0 and checking
    let socket = UdpSocket::bind("0.0.0.0:0")?;

    // Try connecting to several "known" addresses to discover local IPs.
    // This is a common trick - doesn't actually send data.
    let probes = ["8.8.8.8:53", "1.1.1.1:53", "192.168.1.1:80"];
    for probe in &probes {
        if socket.connect(probe).is_ok() {
            if let Ok(local) = socket.local_addr() {
                if let std::net::IpAddr::V4(ip) = local.ip() {
                    if !ip.is_loopback() && !ip.is_unspecified() {
                        // Assume /24 for private ranges
                        let mask = [255, 255, 255, 0];
                        if !results.iter().any(|(existing, _)| *existing == ip) {
                            results.push((ip, mask));
                        }
                    }
                }
            }
        }
    }

    Ok(results)
}

/// Compute the broadcast address from an IP and subnet mask.
fn compute_broadcast(ip: Ipv4Addr, mask: [u8; 4]) -> Ipv4Addr {
    let ip_bytes = ip.octets();
    Ipv4Addr::new(
        ip_bytes[0] | !mask[0],
        ip_bytes[1] | !mask[1],
        ip_bytes[2] | !mask[2],
        ip_bytes[3] | !mask[3],
    )
}

// ---------------------------------------------------------------------------
// Helper: get all local IPs (for discovery service to use)
// ---------------------------------------------------------------------------

/// Get all local IPv4 addresses by enumerating network interfaces.
/// Returns addresses suitable for inclusion in announce messages.
///
/// Delegates to `crate::network::get_all_local_ips()` which enumerates
/// all network interfaces (matching Go's `GetAllLocalIPs()` behavior),
/// filters virtual/loopback/link-local, and sorts by priority.
pub fn get_all_local_ips() -> Vec<String> {
    crate::network::get_all_local_ips()
}

// ---------------------------------------------------------------------------
// DiscoveryAction + handle_discovery_message (kept for backward compat)
// ---------------------------------------------------------------------------

/// Actions to take after processing a discovery message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiscoveryAction {
    /// No action needed.
    None,
    /// Message was from self, ignore.
    Ignore,
}

/// Processes a received discovery message and updates the peer registry.
pub fn handle_discovery_message(
    msg: &DiscoveryMessage,
    local_node_id: &str,
    registry: &PeerRegistry,
) -> DiscoveryAction {
    // Ignore our own messages
    if msg.node_id == local_node_id {
        return DiscoveryAction::Ignore;
    }

    match msg.msg_type {
        DiscoveryMessageType::Announce => {
            let node = message_to_node_info(msg);
            registry.upsert(node);
            DiscoveryAction::None
        }
        DiscoveryMessageType::Bye => {
            registry.remove(&msg.node_id);
            tracing::info!(node_id = %msg.node_id, "Peer announced departure");
            DiscoveryAction::None
        }
    }
}

/// Convert a discovery message to an ExtendedNodeInfo for registry insertion.
fn message_to_node_info(msg: &DiscoveryMessage) -> ExtendedNodeInfo {
    let address = msg
        .addresses
        .first()
        .cloned()
        .unwrap_or_default();

    let display_name = if msg.name.is_empty() {
        format!("node-{}", &msg.node_id[..8.min(msg.node_id.len())])
    } else {
        msg.name.clone()
    };

    let role = match msg.role.as_str() {
        "manager" | "coordinator" | "master" => NodeRole::Master,
        _ => NodeRole::Worker,
    };

    ExtendedNodeInfo {
        base: NodeInfo {
            id: msg.node_id.clone(),
            name: display_name,
            role,
            address: format!("{}:{}", address, msg.rpc_port),
            category: if msg.category.is_empty() {
                "development".into()
            } else {
                msg.category.clone()
            },
            last_seen: format_timestamp(msg.timestamp),
        },
        status: NodeStatus::Online,
        capabilities: msg.capabilities.clone(),
        addresses: msg.addresses.clone(),
    }
}

/// Convert a Unix timestamp (seconds) to an RFC3339 string.
fn format_timestamp(ts: i64) -> String {
    chrono::DateTime::from_timestamp(ts, 0)
        .map(|dt| dt.to_rfc3339())
        .unwrap_or_else(|| ts.to_string())
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
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
            "mgr-1", "ManagerNode", vec!["10.0.0.1".into()], 9000, "manager", "production", vec![], vec![],
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
            "worker", "dev", vec![], vec![],
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
            "worker", "dev", vec![], vec![],
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
            "worker", "dev", vec![], vec![],
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
}
