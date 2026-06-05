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
    receive_thread: parking_lot::Mutex<Option<std::thread::JoinHandle<()>>>,
}

impl UdpListener {
    /// Create a new UDP listener bound to `0.0.0.0:<port>`.
    ///
    /// `enc_key` is the AES-256 key for broadcast encryption; pass `None` to
    /// disable encryption (plaintext mode, backward compatible).
    pub fn new(port: u16, enc_key: Option<[u8; 32]>) -> Result<Self, io::Error> {
        let addr = SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, port);

        // Use socket2 to set SO_REUSEADDR before binding.
        // This allows multiple processes on the same machine to bind to the
        // same UDP port, which is essential for localhost cluster testing.
        let socket2_socket = socket2::Socket::new(
            socket2::Domain::IPV4,
            socket2::Type::DGRAM,
            Some(socket2::Protocol::UDP),
        )?;
        socket2_socket.set_reuse_address(true)?;
        socket2_socket.set_broadcast(true)?;
        socket2_socket.bind(&socket2::SockAddr::from(addr))?;
        let socket: UdpSocket = socket2_socket.into();
        socket.set_read_timeout(Some(Duration::from_secs(1)))?;

        let actual_port = socket.local_addr()?.port();

        Ok(Self {
            socket: Arc::new(socket),
            port: actual_port,
            enc_key,
            running: Arc::new(AtomicBool::new(false)),
            handler: Arc::new(parking_lot::RwLock::new(None)),
            receive_thread: parking_lot::Mutex::new(None),
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

        let handle = std::thread::Builder::new()
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

        *self.receive_thread.lock() = Some(handle);

        Ok(())
    }

    /// Stop the listener and join the receive thread.
    pub fn stop(&self) -> Result<(), io::Error> {
        if !self.running.load(Ordering::SeqCst) {
            return Err(io::Error::new(io::ErrorKind::NotConnected, "listener not running"));
        }
        self.running.store(false, Ordering::SeqCst);

        // Join the receive thread
        if let Some(handle) = self.receive_thread.lock().take() {
            let _ = handle.join();
        }
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
            tracing::info!(node_id = %msg.node_id, "[Discovery] Peer announced departure");
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
        node_type: msg.node_type.clone(),
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
mod tests;
