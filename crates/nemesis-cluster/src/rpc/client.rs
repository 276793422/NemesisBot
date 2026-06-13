//! RPC client - sends requests to remote cluster nodes.
//!
//! Connects to a remote node via TCP using the transport layer, sends a framed
//! RPC request, and waits for the corresponding response. Supports:
//! - Rate limiting per peer (token bucket + sliding window)
//! - Subnet-aware address selection
//! - Connection pooling via `ConnectionPool`
//! - 60-minute default timeout (outermost timeout for RPC calls)

use std::net::IpAddr;
use std::sync::Arc;
use std::time::Duration;

use parking_lot::Mutex;
use tokio::net::TcpStream;
use tokio::time;

use crate::rpc_types::{Frame, RPCRequest, RPCResponse};
use crate::transport::conn::Connection;
use crate::transport::pool::ConnectionPool;

/// Error type for RPC client operations.
#[derive(Debug, thiserror::Error)]
pub enum RpcClientError {
    #[error("Connection error: {0}")]
    Connection(String),
    #[error("Timeout waiting for response")]
    Timeout,
    #[error("Serialization error: {0}")]
    Serialization(String),
    #[error("Response error: {0}")]
    RemoteError(String),
    #[error("Rate limited: {0}")]
    RateLimited(String),
    #[error("Context cancelled")]
    Cancelled,
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
}

// ---------------------------------------------------------------------------
// Rate limiter (mirrors Go's RateLimiter)
// ---------------------------------------------------------------------------

/// Per-peer token-bucket rate limiter with sliding-window burst detection.
pub struct RateLimiter {
    max_tokens: usize,
    refill_interval: Duration,
    max_requests_per_window: usize,
    window: Duration,
    state: Mutex<RateLimiterState>,
}

struct RateLimiterState {
    tokens: std::collections::HashMap<String, usize>,
    last_refill: std::time::Instant,
    requests: std::collections::HashMap<String, Vec<std::time::Instant>>,
}

impl RateLimiter {
    /// Create a new rate limiter.
    ///
    /// * `max_tokens` - tokens per refill per peer
    /// * `refill_interval` - how often tokens are replenished
    /// * `max_requests_per_window` - max requests per peer in `window`
    /// * `window` - sliding window duration
    pub fn new(
        max_tokens: usize,
        refill_interval: Duration,
        max_requests_per_window: usize,
        window: Duration,
    ) -> Self {
        Self {
            max_tokens,
            refill_interval,
            max_requests_per_window,
            window,
            state: Mutex::new(RateLimiterState {
                tokens: std::collections::HashMap::new(),
                last_refill: std::time::Instant::now(),
                requests: std::collections::HashMap::new(),
            }),
        }
    }

    /// Acquire a token for `peer_id`. Returns `Err` if rate-limited.
    /// Synchronous version for backward compatibility.
    pub fn acquire(&self, peer_id: &str) -> Result<(), RpcClientError> {
        let mut state = self.state.lock();

        // Refill tokens if interval elapsed
        if state.last_refill.elapsed() > self.refill_interval {
            state.last_refill = std::time::Instant::now();
            for tokens in state.tokens.values_mut() {
                *tokens = self.max_tokens;
            }
        }

        // Initialise peer state
        state.tokens.entry(peer_id.to_string()).or_insert(self.max_tokens);
        state.requests.entry(peer_id.to_string()).or_insert_with(Vec::new);

        // Prune old timestamps in the sliding window
        let now = std::time::Instant::now();
        let window_start = now - self.window;
        if let Some(timestamps) = state.requests.get_mut(peer_id) {
            timestamps.retain(|ts| *ts > window_start);
            if timestamps.len() >= self.max_requests_per_window {
                tracing::warn!(
                    peer_id = peer_id,
                    window_requests = timestamps.len(),
                    max_per_window = self.max_requests_per_window,
                    "[RpcClient] Rate limited: peer exceeded window limit",
                );
                return Err(RpcClientError::RateLimited(format!(
                    "peer {} exceeded {} requests per {:?}",
                    peer_id, self.max_requests_per_window, self.window
                )));
            }
        }

        // Check token availability
        if let Some(tokens) = state.tokens.get_mut(peer_id) {
            if *tokens > 0 {
                *tokens -= 1;
                state
                    .requests
                    .get_mut(peer_id)
                    .unwrap()
                    .push(std::time::Instant::now());
                return Ok(());
            }
        }

        Err(RpcClientError::RateLimited(format!(
            "peer {} has no tokens available",
            peer_id
        )))
    }

    /// Async acquire that retries with 100ms intervals (matching Go's blocking retry).
    ///
    /// Retries up to 600 times (60 seconds total wait). If no token becomes
    /// available, returns `RateLimited` error. This matches Go's indefinite
    /// Acquire loop which sleeps 100ms between retries.
    pub async fn acquire_async(&self, peer_id: &str) -> Result<(), RpcClientError> {
        const MAX_RETRIES: usize = 600;
        const RETRY_INTERVAL: Duration = Duration::from_millis(100);

        for attempt in 0..MAX_RETRIES {
            if let Ok(()) = self.acquire(peer_id) {
                if attempt > 0 {
                    tracing::debug!(
                        peer_id = peer_id,
                        attempts = attempt,
                        "[RpcClient] Rate limit acquire succeeded after retries",
                    );
                }
                return Ok(());
            }
            tokio::time::sleep(RETRY_INTERVAL).await;
        }

        tracing::warn!(
            peer_id = peer_id,
            max_retries = MAX_RETRIES,
            "[RpcClient] Rate limited, exhausted all retries",
        );
        Err(RpcClientError::RateLimited(format!(
            "peer {} rate limited after {} retries",
            peer_id, MAX_RETRIES
        )))
    }

    /// Release a token back to the bucket.
    pub fn release(&self, peer_id: &str) {
        let mut state = self.state.lock();
        if let Some(tokens) = state.tokens.get_mut(peer_id) {
            *tokens += 1;
        }
    }
}

// ---------------------------------------------------------------------------
// Local network interface (for subnet matching)
// ---------------------------------------------------------------------------

/// A local network interface used for address selection.
#[derive(Debug, Clone)]
pub struct LocalNetworkInterface {
    pub ip: String,
    pub mask: String,
}

/// Trait for resolving peer information (decoupled from Cluster to avoid
/// circular dependencies).
pub trait PeerResolver: Send + Sync {
    /// Get a peer's addresses by ID. Returns `(addresses, rpc_port, is_online)`.
    fn get_peer_info(&self, peer_id: &str) -> Option<(Vec<String>, u16, bool)>;

    /// Get local network interfaces for subnet matching.
    fn get_local_interfaces(&self) -> Vec<LocalNetworkInterface>;

    /// Get the local node ID.
    fn get_node_id(&self) -> String;
}

// ---------------------------------------------------------------------------
// RPC Client
// ---------------------------------------------------------------------------

/// Default RPC timeout: 60 minutes (outermost timeout, matching Go implementation).
pub const DEFAULT_RPC_TIMEOUT: Duration = Duration::from_secs(60 * 60);

/// RPC client for communicating with remote cluster nodes.
pub struct RpcClient {
    /// Connection pool for TCP connections.
    pool: Arc<ConnectionPool>,
    /// Per-peer rate limiter.
    rate_limiter: RateLimiter,
    /// Default timeout for RPC calls.
    timeout: Duration,
    /// Authentication token.
    auth_token: Mutex<Option<String>>,
    /// Peer resolver for looking up addresses.
    resolver: Option<Arc<dyn PeerResolver>>,
}

impl RpcClient {
    /// Create a new RPC client with the default 60-minute timeout.
    pub fn new() -> Self {
        Self {
            pool: Arc::new(ConnectionPool::default()),
            rate_limiter: RateLimiter::new(
                10,                            // max_tokens
                Duration::from_secs(1),        // refill_interval
                30,                            // max_requests_per_window
                Duration::from_secs(10),       // window
            ),
            timeout: DEFAULT_RPC_TIMEOUT,
            auth_token: Mutex::new(None),
            resolver: None,
        }
    }

    /// Create a client with a custom timeout.
    pub fn with_timeout(timeout: Duration) -> Self {
        Self {
            timeout,
            ..Self::new()
        }
    }

    /// Create a client with a peer resolver.
    pub fn with_resolver(resolver: Arc<dyn PeerResolver>) -> Self {
        Self {
            resolver: Some(resolver),
            ..Self::new()
        }
    }

    /// Set the authentication token.
    pub fn set_auth_token(&self, token: String) {
        *self.auth_token.lock() = Some(token);
    }

    /// Return the configured timeout.
    pub fn timeout(&self) -> Duration {
        self.timeout
    }

    // -- High-level API -------------------------------------------------------

    /// Send an RPC request to `peer_id` and wait for the response.
    ///
    /// Resolves the peer's address via the configured `PeerResolver`, applies
    /// rate limiting, establishes a TCP connection, sends the framed request,
    /// and waits for the matching response.
    pub async fn call(
        &self,
        peer_id: &str,
        request: RPCRequest,
    ) -> Result<RPCResponse, RpcClientError> {
        self.call_with_timeout(peer_id, request, self.timeout).await
    }

    /// Send an RPC request with a custom timeout.
    pub async fn call_with_timeout(
        &self,
        peer_id: &str,
        request: RPCRequest,
        timeout: Duration,
    ) -> Result<RPCResponse, RpcClientError> {
        let start = std::time::Instant::now();

        // 1. Rate limiting (async retry matching Go's blocking Acquire)
        if let Err(e) = self.rate_limiter.acquire_async(peer_id).await {
            tracing::warn!(
                peer_id = peer_id,
                error = %e,
                "[RpcClient] Rate limited, request rejected",
            );
            return Err(e);
        }
        let needs_release = true;

        let result = async {
        // 2. Resolve peer addresses
        let (addresses, rpc_port, is_online) = self
            .resolver
            .as_ref()
            .and_then(|r| r.get_peer_info(peer_id))
            .ok_or_else(|| {
                RpcClientError::Connection(format!("peer not found: {}", peer_id))
            })?;

        if !is_online {
            tracing::warn!(
                peer_id = peer_id,
                "[RpcClient] Peer is offline",
            );
            return Err(RpcClientError::Connection(format!(
                "peer is offline: {}",
                peer_id
            )));
        }

        // 3. Build full addresses (IP:Port)
        let full_addresses: Vec<String> = addresses
            .iter()
            .map(|addr| {
                if addr.contains(':') {
                    addr.clone()
                } else {
                    format!("{}:{}", addr, rpc_port)
                }
            })
            .collect();

        // 4. Select best address and connect
        let best_addr = self.select_best_address(&full_addresses);

        tracing::debug!(
            peer_id = peer_id,
            addr = %best_addr,
            action = ?request.action,
            request_id = %request.id,
            "[RpcClient] Connecting to peer",
        );

        // 5. Execute with timeout
        time::timeout(timeout, async {
            self.send_and_receive(&best_addr, &full_addresses, &request).await
        })
        .await
        .map_err(|_| {
            tracing::error!(
                peer_id = peer_id,
                action = ?request.action,
                timeout_secs = timeout.as_secs(),
                "[RpcClient] Call timed out",
            );
            RpcClientError::Timeout
        })?
        }.await;

        if needs_release {
            self.rate_limiter.release(peer_id);
        }

        let elapsed = start.elapsed();
        match &result {
            Ok(_) => {
                tracing::info!(
                    peer_id = peer_id,
                    action = ?request.action,
                    duration_ms = elapsed.as_millis() as u64,
                    "[RpcClient] Call completed successfully",
                );
            }
            Err(e) => {
                tracing::warn!(
                    peer_id = peer_id,
                    action = ?request.action,
                    duration_ms = elapsed.as_millis() as u64,
                    error = %e,
                    "[RpcClient] Call failed",
                );
            }
        }

        result
    }

    // -- Internal helpers -----------------------------------------------------

    /// Try connecting to the best address first, then fall back to others.
    async fn send_and_receive(
        &self,
        best_addr: &str,
        all_addresses: &[String],
        request: &RPCRequest,
    ) -> Result<RPCResponse, RpcClientError> {
        // Try best address first
        match self.try_connect_and_send(best_addr, request).await {
            Ok(resp) => return Ok(resp),
            Err(e) => {
                tracing::debug!(
                    addr = best_addr,
                    error = %e,
                    "[RpcClient] Failed to connect to best address, trying fallbacks"
                );
            }
        }

        // Fallback to other addresses (limit to 3 additional attempts)
        let mut attempts = 0;
        for addr in all_addresses {
            if addr == best_addr {
                continue;
            }
            if attempts >= 3 {
                break;
            }
            attempts += 1;

            match self.try_connect_and_send(addr, request).await {
                Ok(resp) => {
                    tracing::info!(addr = addr, "[RpcClient] Connected to fallback address");
                    return Ok(resp);
                }
                Err(e) => {
                    tracing::debug!(addr = addr, error = %e, "[RpcClient] Fallback address failed");
                }
            }
        }

        Err(RpcClientError::Connection(format!(
            "all connection attempts failed for peer",
        )))
    }

    /// Connect to a single address, send the request, receive the response.
    ///
    /// The synchronous TCP I/O (frame send/recv) is offloaded to a blocking
    /// thread via `tokio::task::spawn_blocking` so that it does not stall
    /// the tokio runtime (matching Go's goroutine-per-call model).
    async fn try_connect_and_send(
        &self,
        addr: &str,
        request: &RPCRequest,
    ) -> Result<RPCResponse, RpcClientError> {
        // Dial TCP with 10-second timeout (matching Go's net.DialTimeout)
        let stream = time::timeout(
            Duration::from_secs(10),
            TcpStream::connect(addr),
        )
        .await
        .map_err(|_| RpcClientError::Connection(format!("dial timeout to {}", addr)))?
        .map_err(|e| RpcClientError::Connection(format!("connect to {}: {}", addr, e)))?;

        // Convert to std::net::TcpStream for sync frame I/O
        let std_stream = stream.into_std().map_err(|e| {
            RpcClientError::Connection(format!("stream conversion: {}", e))
        })?;
        std_stream.set_nonblocking(false).map_err(|e| {
            RpcClientError::Connection(format!("set blocking: {}", e))
        })?;

        tracing::debug!(
            addr = addr,
            "[RpcClient] Connected to {}",
            addr,
        );

        // Derive AES-256 key from auth_token if set. Both sides of the
        // connection derive the same key from the shared token; the server's
        // TcpConn decrypts inbound frames and encrypts outbound responses
        // using this key. Empty token → plaintext frames.
        let cipher_key = self
            .auth_token
            .lock()
            .clone()
            .filter(|t| !t.is_empty())
            .map(|t| crate::transport::frame::derive_key(&t));

        // Clone request data for the blocking closure
        let request_clone = request.clone();
        let addr_owned = addr.to_string();

        // Run synchronous frame I/O on the blocking thread pool
        tokio::task::spawn_blocking(move || {
            Self::sync_send_and_recv(std_stream, &request_clone, &addr_owned, cipher_key)
        })
        .await
        .map_err(|e| RpcClientError::Connection(format!("blocking task join: {}", e)))?
    }

    /// Synchronous send-and-receive on a blocking thread.
    ///
    /// If `cipher_key` is set, the outgoing JSON frame is AEAD-encrypted and
    /// the incoming response frame is decrypted. Plaintext path is used when
    /// no auth token is configured (preserves the legacy no-auth mode).
    fn sync_send_and_recv(
        std_stream: std::net::TcpStream,
        request: &RPCRequest,
        addr: &str,
        cipher_key: Option<[u8; crate::transport::frame::AES_KEY_SIZE]>,
    ) -> Result<RPCResponse, RpcClientError> {
        use crate::transport::frame::{decrypt_frame, encrypt_frame};

        let mut conn = Connection::new(std_stream);

        // Encode request as WireMessage JSON and send with single length prefix.
        // Connection::send adds [4-byte length][data] framing.
        // We send the (possibly encrypted) JSON bytes so the server's
        // AsyncFrameReader reads [4-byte length][payload] — single framing only.
        let wire = crate::transport::conn::WireMessage {
            version: "1.0".into(),
            id: request.id.clone(),
            msg_type: "request".into(),
            from: request.source.clone(),
            to: request.target.clone().unwrap_or_default(),
            action: match &request.action {
                crate::rpc_types::ActionType::Known(k) => match k {
                    crate::rpc_types::KnownAction::PeerChat => "peer_chat",
                    crate::rpc_types::KnownAction::PeerChatCallback => "peer_chat_callback",
                    crate::rpc_types::KnownAction::ForgeShare => "forge_share",
                    crate::rpc_types::KnownAction::Ping => "ping",
                    crate::rpc_types::KnownAction::Status => "status",
                },
                crate::rpc_types::ActionType::Custom(s) => s.as_str(),
            }.into(),
            payload: request.payload.clone(),
            timestamp: chrono::Local::now().timestamp(),
            error: String::new(),
        };
        let json_bytes = serde_json::to_vec(&wire).map_err(|e| {
            RpcClientError::Serialization(e.to_string())
        })?;
        let wire_bytes = if let Some(ref key) = cipher_key {
            encrypt_frame(&json_bytes, key).map_err(|e| {
                RpcClientError::Serialization(format!("encrypt request: {}", e))
            })?
        } else {
            json_bytes
        };
        conn.send(&wire_bytes).map_err(|e| {
            RpcClientError::Connection(format!("send to {}: {}", addr, e))
        })?;

        tracing::debug!(
            addr = addr,
            request_id = %request.id,
            "[RpcClient] RPC request sent, waiting for response"
        );

        // Receive response frame: [4-byte length][payload]
        let resp_data = conn.recv().map_err(|e| {
            RpcClientError::Connection(format!("recv from {}: {}", addr, e))
        })?;
        let resp_plaintext = if let Some(ref key) = cipher_key {
            decrypt_frame(&resp_data, key).map_err(|e| {
                RpcClientError::Connection(format!("decrypt response from {}: {}", addr, e))
            })?
        } else {
            resp_data
        };

        let response: RPCResponse = Frame::decode_response(&resp_plaintext).map_err(|e| {
            RpcClientError::Serialization(format!("decode response: {}", e))
        })?;

        // Check for remote error
        if let Some(ref err) = response.error {
            tracing::error!(
                addr = addr,
                request_id = %request.id,
                error = %err,
                "[RpcClient] Remote error response",
            );
            return Err(RpcClientError::RemoteError(err.clone()));
        }

        Ok(response)
    }

    /// Select the best address from a list using subnet matching.
    fn select_best_address(&self, addresses: &[String]) -> String {
        if addresses.len() <= 1 {
            return addresses.first().cloned().unwrap_or_default();
        }

        // Try subnet matching if resolver is available
        if let Some(ref resolver) = self.resolver {
            let local_interfaces = resolver.get_local_interfaces();
            if !local_interfaces.is_empty() {
                for addr in addresses {
                    if let Some(remote_ip) = extract_ip_from_addr(addr) {
                        let remote_ip_str = remote_ip.to_string();
                        for local_iface in &local_interfaces {
                            if is_same_subnet(&remote_ip_str, &local_iface.ip, &local_iface.mask) {
                                tracing::debug!(
                                    addr = addr,
                                    local_ip = %local_iface.ip,
                                    "[RpcClient] Selected address in same subnet"
                                );
                                return addr.clone();
                            }
                        }
                    }
                }
            }
        }

        // Fallback: return first address
        addresses.first().cloned().unwrap_or_default()
    }

    /// Close all pooled connections.
    pub fn close(&self) {
        self.pool.close_all();
    }
}

impl Default for RpcClient {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Subnet helpers
// ---------------------------------------------------------------------------

/// Extract the IP portion from an "IP:Port" string.
fn extract_ip_from_addr(addr: &str) -> Option<IpAddr> {
    // Handle [IPv6]:Port
    if let Some(idx) = addr.rfind("]:") {
        let ip_str = &addr[1..idx]; // strip '[' and ']:port'
        return ip_str.parse().ok();
    }
    // Handle IPv4:Port
    let parts: Vec<&str> = addr.rsplitn(2, ':').collect();
    if parts.len() == 2 {
        return parts[1].parse().ok();
    }
    addr.parse().ok()
}

/// Check if two IPv4 addresses are in the same subnet given a mask string.
fn is_same_subnet(ip1: &str, ip2: &str, mask: &str) -> bool {
    let a: IpAddr = match ip1.parse() {
        Ok(a) => a,
        Err(_) => return false,
    };
    let b: IpAddr = match ip2.parse() {
        Ok(b) => b,
        Err(_) => return false,
    };
    let m: IpAddr = match mask.parse() {
        Ok(m) => m,
        Err(_) => return false,
    };

    match (a, b, m) {
        (IpAddr::V4(a4), IpAddr::V4(b4), IpAddr::V4(m4)) => {
            let a_bytes = a4.octets();
            let b_bytes = b4.octets();
            let m_bytes = m4.octets();
            for i in 0..4 {
                if (a_bytes[i] & m_bytes[i]) != (b_bytes[i] & m_bytes[i]) {
                    return false;
                }
            }
            true
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests;
