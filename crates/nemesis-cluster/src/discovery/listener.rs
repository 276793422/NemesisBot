//! Discovery listener - UDP broadcast peer discovery.
//!
//! Receives UDP broadcast packets from other nodes and processes
//! Announce/Bye messages. Also provides the `UdpListener` struct for
//! actual UDP socket I/O (bind, receive loop, broadcast).

use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;
use std::time::Instant;

use crate::discovery::crypto::{decrypt_data, encrypt_data};
use crate::discovery::message::{DiscoveryMessage, DiscoveryMessageType};
use crate::registry::PeerRegistry;
use crate::types::{ExtendedNodeInfo, NodeStatus};
use nemesis_types::cluster::{NodeInfo, NodeRole};

// ---------------------------------------------------------------------------
// 解密失败来源追踪（L2，2026-09-27 修订）
// ---------------------------------------------------------------------------
//
// token 失配 = 安全边界在正常工作，不是错误——局域网内多个集群各配各的
// token 时互相丢弃是**预期行为**，不应以 WARN 级别周期性刷日志（旧实现
// 按 1/100 计数限频 WARN，多源 × 30s 广播节奏下仍是持续噪音）。
//
// 诊断能力放在两处（日常运行零日志输出）：
// 1. 每个来源**首见**丢弃时一条 DEBUG（翻转沿去重：同源后续失败静默，
//    上界 = 不同来源数，与时间无关）；
// 2. [`DecryptDropSummary`] 供 `cluster.status` WSAPI 按需查询——
//    "为什么互相看不见"变成跑一条命令就能看到"有几个异 token 来源"。

/// 单个解密失败来源的摘要条目（时间以"距现在多少秒"表达，便于直接序列化）。
#[derive(Debug, Clone, serde::Serialize)]
pub struct DecryptDropSource {
    /// 来源地址 `ip:port`（announce 的广播源端口每次可能不同，见
    /// [`DecryptDropTracker::record`] 的按 IP 归并说明）。
    pub addr: String,
    /// 距首次见到该来源多少秒。
    pub first_seen_secs_ago: u64,
    /// 距最近一次丢弃多少秒。
    pub last_seen_secs_ago: u64,
    /// 该来源累计丢弃帧数。
    pub drops: u64,
}

/// 解密失败丢弃摘要（`cluster.status` WSAPI `discovery_drops` 字段）。
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct DecryptDropSummary {
    /// 全部来源累计丢弃帧数。
    pub total_drops: u64,
    /// 已知异 token 来源列表（按首见时间升序，环形保留最近 [`DecryptDropTracker::MAX_SOURCES`] 个）。
    pub sources: Vec<DecryptDropSource>,
}

/// 解密失败来源账本（进程内共享：receive 线程写，status 查询读）。
#[derive(Default)]
pub(crate) struct DecryptDropTracker {
    /// IP → 条目。按 **IP** 归并而非完整 addr：广播源的源端口是随机的
    /// （对端每个 announce 换一个临时端口），按 addr 归并会导致同一节点
    /// 永远"首见"。
    sources: parking_lot::Mutex<HashMap<Ipv4Addr, SourceEntry>>,
    total_drops: AtomicU64,
}

#[derive(Clone, Copy)]
struct SourceEntry {
    first_seen: Instant,
    last_seen: Instant,
    drops: u64,
}

impl DecryptDropTracker {
    /// 环形保留的来源上限。超过后淘汰最旧的——极端场景（>16 个异 token
    /// 源）下被淘汰的来源若再次出现会再记一条首见 DEBUG，量级仍可控。
    const MAX_SOURCES: usize = 16;

    /// 记录一次解密失败。已知来源静默累积；新来源记一条首见 DEBUG。
    fn record(&self, addr: Ipv4Addr) {
        self.total_drops.fetch_add(1, Ordering::Relaxed);
        let mut map = self.sources.lock();
        let now = Instant::now();
        match map.get_mut(&addr) {
            Some(entry) => {
                entry.last_seen = now;
                entry.drops += 1;
            }
            None => {
                if map.len() >= Self::MAX_SOURCES {
                    if let Some(oldest) = map
                        .iter()
                        .min_by_key(|(_, e)| e.first_seen)
                        .map(|(k, _)| *k)
                    {
                        map.remove(&oldest);
                    }
                }
                map.insert(
                    addr,
                    SourceEntry {
                        first_seen: now,
                        last_seen: now,
                        drops: 1,
                    },
                );
                tracing::debug!(
                    peer = %addr,
                    "[Discovery] 解密失败首见（异 token 来源？互不发现是预期安全行为）\
                     —— 同源后续失败不再记录；按需查看：cluster.status 的 discovery_drops"
                );
            }
        }
    }

    /// 供 status 查询的摘要快照（sources 按首见时间升序）。
    pub fn summary(&self) -> DecryptDropSummary {
        let map = self.sources.lock();
        let now = Instant::now();
        let mut sources: Vec<DecryptDropSource> = map
            .iter()
            .map(|(ip, e)| DecryptDropSource {
                addr: ip.to_string(),
                first_seen_secs_ago: now.duration_since(e.first_seen).as_secs(),
                last_seen_secs_ago: now.duration_since(e.last_seen).as_secs(),
                drops: e.drops,
            })
            .collect();
        sources.sort_by_key(|s| std::cmp::Reverse(s.first_seen_secs_ago));
        DecryptDropSummary {
            total_drops: self.total_drops.load(Ordering::Relaxed),
            sources,
        }
    }
}

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
    /// 解密失败来源账本（receive 线程写，`decrypt_drop_summary` 读）。
    drops: Arc<DecryptDropTracker>,
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
            drops: Arc::new(DecryptDropTracker::default()),
        })
    }

    /// Set the callback invoked for each received discovery message.
    pub fn set_message_handler(&self, handler: MessageHandler) {
        *self.handler.write() = Some(handler);
    }

    /// Start the receive loop on a background thread.
    pub fn start(&self) -> Result<(), io::Error> {
        if self.running.load(Ordering::SeqCst) {
            return Err(io::Error::new(
                io::ErrorKind::AlreadyExists,
                "listener already running",
            ));
        }
        self.running.store(true, Ordering::SeqCst);

        let socket = Arc::clone(&self.socket);
        let running = Arc::clone(&self.running);
        let handler = Arc::clone(&self.handler);
        let enc_key = self.enc_key;
        let drops = Arc::clone(&self.drops);

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
                                    // Token 失配 = 安全边界正常工作（多集群共存互不
                                    // 发现是预期行为），不刷 WARN；首见记一条 DEBUG，
                                    // 累计账走 tracker 供 status 摘要查询。
                                    Err(_) => {
                                        if let std::net::IpAddr::V4(v4) = addr.ip() {
                                            drops.record(v4);
                                        }
                                        continue;
                                    }
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
                        Err(ref e)
                            if e.kind() == io::ErrorKind::TimedOut
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
            return Err(io::Error::new(
                io::ErrorKind::NotConnected,
                "listener not running",
            ));
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

    /// 解密失败丢弃摘要（token 失配来源账本快照，供 status 按需查询）。
    pub fn decrypt_drop_summary(&self) -> DecryptDropSummary {
        self.drops.summary()
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
            encrypt_data(key, &data).map_err(|_| io::Error::other("encryption failed"))?
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

    /// Encrypt (if configured) and send a discovery message to a specific
    /// unicast target (`host:port`). Best-effort companion to `broadcast()`
    /// for peers unreachable by subnet broadcast（异端口拓扑定向单播，
    /// 见 [`crate::discovery::ClusterCallbacks::peer_udp_endpoints`]）。
    pub fn send_unicast(&self, target: &str, msg: &DiscoveryMessage) {
        let data = match msg.to_bytes() {
            Ok(d) => d,
            Err(_) => return,
        };
        let send_data = if let Some(key) = self.enc_key {
            match encrypt_data(&key, &data) {
                Ok(encrypted) => encrypted,
                Err(_) => return,
            }
        } else {
            data
        };
        let _ = self.socket.send_to(&send_data, target);
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
///
/// # 部署假设注记（L4/L5，2026-09-27）
///
/// UDP 广播发现依赖**物理层可达**：
/// - **L4（AP 隔离）**：无线 AP 开启客户端隔离（AP isolation / guest 网络）
///   时，同网段客户端之间二层互不可达——广播发出去了但对端收不到，发现层
///   无感知也无解。排查路径：确认两端在同一网段且 AP 未开隔离；仍不通时用
///   `peer_udp_endpoints` 配置定向单播（走已知 host:port，不依赖广播）。
/// - **L5（255.255.255.255 出广域）**：受限网络（部分蜂窝/企业网）会把
///   255.255.255.255 定向到 WAN 或直接吞掉；缓解=子网定向广播（`ip|!mask`，
///   本函数第二类条目）通常仍在本网段内可达。多宿主/异端口拓扑同理，兜底
///   都是一致的：显式配置 `peer_udp_endpoints` 定向单播。
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
/// Uses `get_local_network_interfaces()` which properly enumerates ALL
/// network interfaces via `if_addrs`, including multi-homed hosts.
/// Falls back to the UDP connect trick if interface enumeration fails.
fn local_ip_addresses() -> io::Result<Vec<(Ipv4Addr, [u8; 4])>> {
    let interfaces = crate::network::get_local_network_interfaces();
    if !interfaces.is_empty() {
        let mut results = Vec::new();
        for iface in &interfaces {
            if let Ok(ip) = iface.ip.parse::<Ipv4Addr>()
                && let Ok(mask) = iface.mask.parse::<Ipv4Addr>()
                && !results.iter().any(|(existing, _)| *existing == ip)
            {
                results.push((ip, mask.octets()));
            }
        }
        return Ok(results);
    }

    // Fallback: single IP via UDP connect trick with /24 assumption
    let mut results = Vec::new();
    let socket = UdpSocket::bind("0.0.0.0:0")?;
    if socket.connect("8.8.8.8:53").is_ok()
        && let Ok(local) = socket.local_addr()
        && let std::net::IpAddr::V4(ip) = local.ip()
        && !ip.is_loopback()
        && !ip.is_unspecified()
    {
        results.push((ip, [255, 255, 255, 0]));
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
    let address = msg.addresses.first().cloned().unwrap_or_default();

    let display_name = if msg.name.is_empty() {
        format!("node-{}", &msg.node_id[..8.min(msg.node_id.len())])
    } else {
        msg.name.clone()
    };

    let role = NodeRole::from_role_str(&msg.role);

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
        tags: msg.tags.clone(),
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
mod cov_tests;
#[cfg(test)]
mod tests;
