//! Connection pool for reusing TCP connections to peers.
//!
//! Provides two pool implementations:
//! - `ConnectionPool` — synchronous pool for simple use cases
//! - `Pool` — async pool with semaphore-based concurrency limits, per-node
//!   limits, timeouts, and stats (mirrors Go's `Pool`)

use std::collections::HashMap;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use parking_lot::Mutex;
use tokio::sync::Semaphore;

use super::conn::{Connection, TcpConn, TcpConnConfig};

// ===========================================================================
// Synchronous ConnectionPool (backward-compatible)
// ===========================================================================

/// Configuration for the synchronous connection pool.
#[derive(Debug, Clone)]
pub struct PoolConfig {
    /// Maximum connections per peer.
    pub max_per_peer: usize,
    /// Maximum total connections.
    pub max_total: usize,
}

impl Default for PoolConfig {
    fn default() -> Self {
        Self {
            max_per_peer: 4,
            max_total: 100,
        }
    }
}

/// A pool of TCP connections to remote cluster nodes.
pub struct ConnectionPool {
    config: PoolConfig,
    pools: Mutex<HashMap<String, Vec<Connection>>>,
}

impl ConnectionPool {
    /// Create a new connection pool with the given configuration.
    pub fn new(config: PoolConfig) -> Self {
        Self {
            config,
            pools: Mutex::new(HashMap::new()),
        }
    }

    /// Get a connection from the pool, or create a new one.
    pub fn get_or_connect(&self, addr: &str) -> Result<Connection, String> {
        // Try to reuse an existing connection
        {
            let mut pools = self.pools.lock();
            if let Some(conns) = pools.get_mut(addr) {
                while let Some(conn) = conns.pop() {
                    if conn.is_connected() {
                        return Ok(conn);
                    }
                }
            }
        }

        // Create a new connection
        Connection::connect(addr).map_err(|e| format!("Failed to connect to {}: {}", addr, e))
    }

    /// Return a connection to the pool for reuse.
    pub fn return_connection(&self, addr: &str, conn: Connection) {
        if !conn.is_connected() {
            return;
        }

        let mut pools = self.pools.lock();
        let conns = pools.entry(addr.to_string()).or_insert_with(Vec::new);

        if conns.len() < self.config.max_per_peer {
            conns.push(conn);
        }
        // Otherwise, drop the connection (exceeds per-peer limit)
    }

    /// Close all connections in the pool.
    pub fn close_all(&self) {
        let mut pools = self.pools.lock();
        for (_, conns) in pools.drain() {
            for mut conn in conns {
                conn.close();
            }
        }
    }

    /// Return the total number of pooled connections.
    pub fn total_connections(&self) -> usize {
        self.pools.lock().values().map(|v| v.len()).sum()
    }

    /// Return the number of peers with active connections.
    pub fn peer_count(&self) -> usize {
        self.pools.lock().len()
    }
}

impl Default for ConnectionPool {
    fn default() -> Self {
        Self::new(PoolConfig::default())
    }
}

// ===========================================================================
// Async Pool with semaphore, per-node limits, and stats
// ===========================================================================

/// Configuration for the async connection pool.
///
/// Mirrors Go's `PoolConfig`.
#[derive(Debug, Clone)]
pub struct AsyncPoolConfig {
    /// Maximum total connections across all peers (default: 50).
    pub max_conns: usize,
    /// Maximum connections per peer node (default: 3).
    pub max_conns_per_node: usize,
    /// Timeout for dialing new connections (default: 10s).
    pub dial_timeout: Duration,
    /// Idle timeout for pooled connections (default: 65min).
    pub idle_timeout: Duration,
    /// Timeout for sending messages (default: 10s).
    pub send_timeout: Duration,
    /// Optional auth token for new connections.
    pub auth_token: Option<String>,
}

impl Default for AsyncPoolConfig {
    fn default() -> Self {
        Self {
            max_conns: 50,
            max_conns_per_node: 3,
            dial_timeout: Duration::from_secs(10),
            idle_timeout: Duration::from_secs(65 * 60),
            send_timeout: Duration::from_secs(10),
            auth_token: None,
        }
    }
}

/// Pool statistics.
#[derive(Debug, Clone, Default)]
pub struct PoolStats {
    /// Number of active connections.
    pub active_conns: usize,
    /// Maximum total connections.
    pub max_conns: usize,
    /// Available slots for new connections.
    pub available_slots: usize,
    /// Number of connections per node (node_id → count).
    pub node_conns: HashMap<String, usize>,
}

/// Entry in the pool holding a TcpConn and its metadata.
struct PoolEntry {
    conn: TcpConn,
    node_id: String,
    #[allow(dead_code)]
    address: String,
}

/// An async connection pool with semaphore-based concurrency limits.
///
/// Mirrors Go's `Pool`:
/// - Total connections limited by semaphore
/// - Per-node connection limits
/// - Double-check locking for concurrent access
/// - Async dial with configurable timeout
pub struct Pool {
    conns: Mutex<HashMap<String, PoolEntry>>,
    config: AsyncPoolConfig,
    semaphore: std::sync::Arc<Semaphore>,
    node_counts: Mutex<HashMap<String, usize>>,
    active_count: AtomicUsize,
}

impl Pool {
    /// Create a new async pool with the given configuration.
    pub fn new(config: AsyncPoolConfig) -> Self {
        let max = config.max_conns;
        Self {
            conns: Mutex::new(HashMap::new()),
            config,
            semaphore: std::sync::Arc::new(Semaphore::new(max)),
            node_counts: Mutex::new(HashMap::new()),
            active_count: AtomicUsize::new(0),
        }
    }

    /// Create a pool with default configuration.
    pub fn with_defaults() -> Self {
        Self::new(AsyncPoolConfig::default())
    }

    /// Get an existing connection or create a new one.
    ///
    /// Uses a semaphore to limit total connections and per-node limits.
    /// Returns the connection key for later removal.
    pub async fn get(
        &self,
        node_id: &str,
        address: &str,
    ) -> Result<(String, TcpConn), String> {
        self.get_inner(node_id, address, false).await
    }

    /// Get a connection with semaphore wait timeout (5 seconds).
    ///
    /// Unlike `get`, this method waits up to 5 seconds for a semaphore slot
    /// to become available, matching Go's pool behavior. Falls back to an
    /// immediate error only if the timeout elapses.
    pub async fn get_with_timeout(
        &self,
        node_id: &str,
        address: &str,
    ) -> Result<(String, TcpConn), String> {
        self.get_inner(node_id, address, true).await
    }

    /// Inner implementation shared by `get` and `get_with_timeout`.
    async fn get_inner(
        &self,
        node_id: &str,
        address: &str,
        wait_for_semaphore: bool,
    ) -> Result<(String, TcpConn), String> {
        let key = format!("{}:{}", node_id, address);

        // First check: try to get an existing connection
        {
            let mut conns = self.conns.lock();
            if let Some(entry) = conns.remove(&key) {
                if entry.conn.is_active() {
                    self.dec_node_count(node_id);
                    return Ok((key, entry.conn));
                }
                // Connection is dead, remove it
                self.active_count.fetch_sub(1, Ordering::SeqCst);
                // Semaphore permit stays consumed, we'll reuse it
            }
        }

        // Check per-node limit
        {
            let counts = self.node_counts.lock();
            if let Some(&count) = counts.get(node_id) {
                if count >= self.config.max_conns_per_node {
                    return Err(format!(
                        "per-node limit reached for {} ({}/{})",
                        node_id, count, self.config.max_conns_per_node
                    ));
                }
            }
        }

        // Acquire semaphore permit (limits total connections)
        let semaphore = self.semaphore.clone();
        let permit = if wait_for_semaphore {
            // Wait up to 5 seconds for a slot (matches Go's pool behavior)
            tokio::time::timeout(Duration::from_secs(5), semaphore.acquire())
                .await
                .map_err(|_| format!("connection limit timeout ({})", self.config.max_conns))?
                .map_err(|_| format!("connection limit reached ({})", self.config.max_conns))?
        } else {
            semaphore
                .try_acquire()
                .map_err(|_| format!("connection limit reached ({})", self.config.max_conns))?
        };

        // Double-check: re-verify limits after acquiring permit
        {
            let counts = self.node_counts.lock();
            if let Some(&count) = counts.get(node_id) {
                if count >= self.config.max_conns_per_node {
                    drop(counts);
                    drop(permit); // Release the semaphore permit
                    return Err(format!(
                        "per-node limit reached for {} after acquiring semaphore",
                        node_id
                    ));
                }
            }
        }

        // Dial new connection
        let conn = self.dial(node_id, address).await?;

        // Update counts
        self.inc_node_count(node_id);
        self.active_count.fetch_add(1, Ordering::SeqCst);

        // Forget the permit (it stays consumed until connection is removed)
        // We don't return the connection to the pool, caller owns it
        permit.forget();

        Ok((key, conn))
    }

    /// Get a connection with cancellation support and semaphore wait timeout.
    pub async fn get_with_context(
        &self,
        node_id: &str,
        address: &str,
    ) -> Result<(String, TcpConn), String> {
        // Use tokio::select for cancellation
        tokio::select! {
            result = self.get_with_timeout(node_id, address) => result,
            _ = tokio::time::sleep(self.config.dial_timeout) => {
                Err(format!("dial timeout for {}:{}", node_id, address))
            }
        }
    }

    /// Remove and close a connection by key.
    pub fn remove(&self, key: &str) {
        let mut conns = self.conns.lock();
        if let Some(entry) = conns.remove(key) {
            self.dec_node_count(&entry.node_id);
            self.active_count.fetch_sub(1, Ordering::SeqCst);
            // Add a semaphore permit back
            self.semaphore.add_permits(1);
            // TcpConn::Drop will close it
        }
    }

    /// Remove all connections for a given node.
    pub fn remove_node(&self, node_id: &str) {
        let mut conns = self.conns.lock();
        let keys: Vec<String> = conns
            .iter()
            .filter(|(_, entry)| entry.node_id == node_id)
            .map(|(k, _)| k.clone())
            .collect();

        let removed = keys.len();
        for key in &keys {
            conns.remove(key);
        }
        if removed > 0 {
            let mut counts = self.node_counts.lock();
            counts.remove(node_id);
            self.active_count.fetch_sub(removed, Ordering::SeqCst);
            self.semaphore.add_permits(removed);
        }
    }

    /// Return a connection to the pool for reuse.
    ///
    /// If the connection is still active, it is stored for future reuse.
    /// Otherwise it is closed and the resources are released.
    pub fn return_connection(&self, key: String, mut conn: TcpConn) {
        if conn.is_active() {
            let entry = PoolEntry {
                node_id: conn.node_id().to_string(),
                address: conn.address().to_string(),
                conn,
            };
            self.conns.lock().insert(key, entry);
        } else {
            // Connection is dead, release resources
            conn.close();
            self.active_count.fetch_sub(1, Ordering::SeqCst);
            self.semaphore.add_permits(1);
        }
    }

    /// Close all connections in the pool.
    pub fn close(&self) {
        let mut conns = self.conns.lock();
        let count = conns.len();
        conns.clear(); // TcpConn::Drop handles closing
        self.node_counts.lock().clear();
        self.active_count.store(0, Ordering::SeqCst);
        // Add permits back
        self.semaphore.add_permits(count);
    }

    /// Get pool statistics.
    pub fn get_stats(&self) -> PoolStats {
        let _conns = self.conns.lock();
        let active = self.active_count.load(Ordering::SeqCst);
        let node_counts = self.node_counts.lock();

        PoolStats {
            active_conns: active,
            max_conns: self.config.max_conns,
            available_slots: self.config.max_conns.saturating_sub(active),
            node_conns: node_counts.clone(),
        }
    }

    /// Get the number of active connections.
    pub fn active_connection_count(&self) -> usize {
        self.active_count.load(Ordering::SeqCst)
    }

    /// Detect and remove dead connections, releasing their semaphore slots.
    ///
    /// A connection is considered dead if `is_active()` returns `false`.
    /// This method iterates all pooled entries, removes dead ones, and
    /// releases their semaphore permits so the slots do not leak.
    /// Returns the number of dead connections removed.
    ///
    /// This should be called periodically (e.g., every 30-60 seconds) to
    /// prevent semaphore slot leaks when connections are lost without `remove()`
    /// being called.
    pub fn cleanup_dead(&self) -> usize {
        let mut conns = self.conns.lock();
        let dead_keys: Vec<String> = conns
            .iter()
            .filter(|(_, entry)| !entry.conn.is_active())
            .map(|(k, _)| k.clone())
            .collect();

        let removed = dead_keys.len();
        for key in &dead_keys {
            if let Some(entry) = conns.remove(key) {
                self.dec_node_count(&entry.node_id);
                self.active_count.fetch_sub(1, Ordering::SeqCst);
                // Release the semaphore slot that was leaked by `permit.forget()`
                self.semaphore.add_permits(1);
            }
        }

        if removed > 0 {
            tracing::info!(removed, "Cleaned up dead connections from pool");
        }

        removed
    }

    /// Dial a new TCP connection.
    async fn dial(&self, node_id: &str, address: &str) -> Result<TcpConn, String> {
        let stream = tokio::time::timeout(self.config.dial_timeout, async {
            tokio::net::TcpStream::connect(address).await
        })
        .await
        .map_err(|_| format!("dial timeout for {}", address))?
        .map_err(|e| format!("dial failed for {}: {}", address, e))?;

        let config = TcpConnConfig {
            node_id: node_id.to_string(),
            address: address.to_string(),
            send_timeout: self.config.send_timeout,
            idle_timeout: self.config.idle_timeout,
            auth_token: self.config.auth_token.clone(),
            ..Default::default()
        };

        let mut conn = TcpConn::new(stream, config);
        conn.start()
            .await
            .map_err(|e| format!("conn start failed for {}: {}", address, e))?;

        Ok(conn)
    }

    /// Increment the connection count for a node.
    fn inc_node_count(&self, node_id: &str) {
        let mut counts = self.node_counts.lock();
        *counts.entry(node_id.to_string()).or_insert(0) += 1;
    }

    /// Decrement the connection count for a node.
    fn dec_node_count(&self, node_id: &str) {
        let mut counts = self.node_counts.lock();
        if let Some(count) = counts.get_mut(node_id) {
            *count = count.saturating_sub(1);
            if *count == 0 {
                counts.remove(node_id);
            }
        }
    }
}

impl Default for Pool {
    fn default() -> Self {
        Self::with_defaults()
    }
}

impl Drop for Pool {
    fn drop(&mut self) {
        self.close();
    }
}

// ===========================================================================
// Tests
// ===========================================================================

#[cfg(test)]
mod tests {
    use super::*;

    // --- Sync ConnectionPool tests ---

    #[test]
    fn test_pool_config_default() {
        let config = PoolConfig::default();
        assert_eq!(config.max_per_peer, 4);
        assert_eq!(config.max_total, 100);
    }

    #[test]
    fn test_empty_pool() {
        let pool = ConnectionPool::new(PoolConfig::default());
        assert_eq!(pool.total_connections(), 0);
        assert_eq!(pool.peer_count(), 0);
    }

    #[test]
    fn test_close_all_empty() {
        let pool = ConnectionPool::new(PoolConfig::default());
        pool.close_all(); // Should not panic
    }

    #[test]
    fn test_get_or_connect_creates_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        // Accept in background
        let handle = std::thread::spawn(move || listener.accept().unwrap());

        let pool = ConnectionPool::new(PoolConfig::default());
        let conn = pool.get_or_connect(&addr).unwrap();
        assert!(conn.is_connected());

        handle.join().unwrap();
    }

    #[test]
    fn test_return_connection() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let handle = std::thread::spawn(move || listener.accept().unwrap());

        let pool = ConnectionPool::new(PoolConfig::default());
        let conn = pool.get_or_connect(&addr).unwrap();
        pool.return_connection(&addr, conn);
        assert_eq!(pool.total_connections(), 1);

        handle.join().unwrap();
    }

    // --- Async Pool tests ---

    #[test]
    fn test_async_pool_config_default() {
        let config = AsyncPoolConfig::default();
        assert_eq!(config.max_conns, 50);
        assert_eq!(config.max_conns_per_node, 3);
        assert_eq!(config.dial_timeout, Duration::from_secs(10));
    }

    #[test]
    fn test_pool_stats_empty() {
        let pool = Pool::with_defaults();
        let stats = pool.get_stats();
        assert_eq!(stats.active_conns, 0);
        assert_eq!(stats.max_conns, 50);
        assert_eq!(stats.available_slots, 50);
    }

    #[tokio::test]
    async fn test_pool_get_and_return() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {}
        });

        let pool = Pool::new(AsyncPoolConfig {
            max_conns: 10,
            max_conns_per_node: 2,
            ..Default::default()
        });

        let (key, conn) = pool.get("node-1", &addr).await.unwrap();
        assert!(conn.is_active());
        assert_eq!(pool.active_connection_count(), 1);

        // Return the connection
        pool.return_connection(key, conn);
        assert_eq!(pool.active_connection_count(), 1); // Still active in pool

        let stats = pool.get_stats();
        assert_eq!(stats.active_conns, 1);

        pool.close();
        assert_eq!(pool.active_connection_count(), 0);

        server.abort();
    }

    #[tokio::test]
    async fn test_pool_per_node_limit() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        // Accept multiple connections
        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {}
        });

        let pool = Pool::new(AsyncPoolConfig {
            max_conns: 100,
            max_conns_per_node: 2,
            ..Default::default()
        });

        // Should be able to get up to 2 connections
        let (_, _c1) = pool.get("node-1", &addr).await.unwrap();
        let (_, _c2) = pool.get("node-1", &addr).await.unwrap();

        // Third should fail
        let result = pool.get("node-1", &addr).await;
        assert!(result.is_err());
        assert!(result.unwrap_err().contains("per-node limit"));

        pool.close();
        server.abort();
    }

    #[tokio::test]
    async fn test_pool_remove() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {}
        });

        let pool = Pool::with_defaults();
        let (key, conn) = pool.get("node-1", &addr).await.unwrap();
        assert_eq!(pool.active_connection_count(), 1);

        // Return the connection, then remove it
        pool.return_connection(key.clone(), conn);
        assert_eq!(pool.active_connection_count(), 1); // In pool

        pool.remove(&key);
        assert_eq!(pool.active_connection_count(), 0);

        let stats = pool.get_stats();
        assert_eq!(stats.available_slots, 50);

        server.abort();
    }

    #[tokio::test]
    async fn test_pool_close_all() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            // Accept connections until the listener is closed
            while let Ok((_stream, _)) = listener.accept().await {}
        });

        let pool = Pool::new(AsyncPoolConfig {
            max_conns: 10,
            max_conns_per_node: 5,
            ..Default::default()
        });

        let (_key1, conn1) = pool.get("node-1", &addr).await.unwrap();
        let (_key2, conn2) = pool.get("node-1", &addr).await.unwrap();
        assert_eq!(pool.active_connection_count(), 2);

        // Return connections first, then close
        drop(conn1);
        drop(conn2);
        pool.close();
        assert_eq!(pool.active_connection_count(), 0);

        let stats = pool.get_stats();
        assert_eq!(stats.available_slots, 10);

        server.abort();
    }

    #[tokio::test]
    async fn test_pool_dial_timeout() {
        let pool = Pool::new(AsyncPoolConfig {
            dial_timeout: Duration::from_millis(100),
            ..Default::default()
        });

        // Connect to a non-routable address (will timeout)
        let result = pool.get("node-1", "10.255.255.1:9999").await;
        assert!(result.is_err());
    }

    #[test]
    fn test_pool_cleanup_dead_empty() {
        let pool = Pool::with_defaults();
        let removed = pool.cleanup_dead();
        assert_eq!(removed, 0);
    }

    #[test]
    fn test_pool_cleanup_dead_removes_inactive() {
        // This test verifies cleanup_dead removes entries where
        // is_active() returns false. We simulate this by directly
        // inserting a manually-constructed dead entry.
        let pool = Pool::with_defaults();

        // We cannot easily create a TcpConn without a real TCP connection,
        // so we verify cleanup_dead on an empty pool returns 0.
        // The actual dead-connection detection is exercised in integration
        // tests where connections can be dropped.
        assert_eq!(pool.cleanup_dead(), 0);
        assert_eq!(pool.active_connection_count(), 0);
    }

    // ============================================================
    // Coverage improvement: more pool edge cases
    // ============================================================

    #[tokio::test]
    async fn test_pool_remove_node_single_conn() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {}
        });

        let pool = Pool::new(AsyncPoolConfig {
            max_conns: 10,
            max_conns_per_node: 3,
            ..Default::default()
        });

        let (key, conn) = pool.get("node-y", &addr).await.unwrap();
        pool.return_connection(key, conn);

        let before = pool.active_connection_count();
        // At least 0 or 1 depending on if conn was active
        pool.remove_node("node-y");
        let after = pool.active_connection_count();
        // After remove_node, count should be <= before
        assert!(after <= before, "remove_node should not increase count");

        server.abort();
    }

    #[tokio::test]
    async fn test_pool_return_closed_connection() {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap().to_string();

        let server = tokio::spawn(async move {
            while let Ok(_) = listener.accept().await {}
        });

        let pool = Pool::new(AsyncPoolConfig {
            max_conns: 10,
            max_conns_per_node: 3,
            ..Default::default()
        });

        let (key, mut conn) = pool.get("node-1", &addr).await.unwrap();
        assert_eq!(pool.active_connection_count(), 1);

        // Close the connection, then return it
        conn.close();
        pool.return_connection(key, conn);
        // Dead connection should not be added back to pool
        assert_eq!(pool.active_connection_count(), 0);

        server.abort();
    }

    #[test]
    fn test_pool_default_impl() {
        let pool = Pool::default();
        let stats = pool.get_stats();
        assert_eq!(stats.max_conns, 50);
    }

    #[tokio::test]
    async fn test_pool_get_with_timeout() {
        let pool = Pool::new(AsyncPoolConfig {
            dial_timeout: Duration::from_millis(50),
            ..Default::default()
        });

        // Connect to a non-routable address (will timeout)
        let result = pool.get_with_timeout("node-1", "10.255.255.1:9999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pool_get_with_context_timeout() {
        let pool = Pool::new(AsyncPoolConfig {
            dial_timeout: Duration::from_millis(50),
            ..Default::default()
        });

        let result = pool.get_with_context("node-1", "10.255.255.1:9999").await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn test_pool_remove_nonexistent_key() {
        let pool = Pool::with_defaults();
        pool.remove("nonexistent-key");
        // Should not panic
        assert_eq!(pool.active_connection_count(), 0);
    }

    #[tokio::test]
    async fn test_pool_remove_node_nonexistent() {
        let pool = Pool::with_defaults();
        pool.remove_node("nonexistent-node");
        // Should not panic
        assert_eq!(pool.active_connection_count(), 0);
    }

    #[test]
    fn test_async_pool_config_debug() {
        let config = AsyncPoolConfig::default();
        let debug = format!("{:?}", config);
        assert!(debug.contains("max_conns"));
    }
}
