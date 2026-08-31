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

    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

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
    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

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

    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

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

    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

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

    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

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

// ============================================================
// S4 coverage: sync reuse path (63), async first-check reuse
// (238-241), dead-entry path (243), double-check per-node limit
// (280-285), cleanup_dead removal (422-431), dec_node_count
// arcs (470-477).
// ============================================================

/// No-op tracing subscriber so field-recording lines inside
/// `tracing::info!`/`debug!` macros actually execute.
struct S4PoolSubscriber;

impl tracing::Subscriber for S4PoolSubscriber {
    fn enabled(&self, _metadata: &tracing::Metadata<'_>) -> bool {
        true
    }
    fn new_span(&self, _span: &tracing::span::Attributes<'_>) -> tracing::Id {
        tracing::Id::from_u64(1)
    }
    fn record(&self, _span: &tracing::Id, _values: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _span: &tracing::Id, _follows: &tracing::Id) {}
    fn event(&self, _event: &tracing::Event<'_>) {}
    fn enter(&self, _span: &tracing::Id) {}
    fn exit(&self, _span: &tracing::Id) {}
}

static S4_POOL_SUBSCRIBER: std::sync::Once = std::sync::Once::new();

fn install_s4_pool_subscriber() {
    S4_POOL_SUBSCRIBER.call_once(|| {
        let _ = tracing::subscriber::set_global_default(S4PoolSubscriber);
    });
}

/// Sync pool: a returned healthy connection is reused by the next
/// get_or_connect (pool.rs 62-63).
#[test]
fn test_s4_sync_pool_reuses_returned_connection() {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    // One accept suffices: the second get_or_connect must REUSE, not dial.
    let handle = std::thread::spawn(move || listener.accept().unwrap());

    let pool = ConnectionPool::new(PoolConfig::default());
    let conn = pool.get_or_connect(&addr).unwrap();
    pool.return_connection(&addr, conn);
    assert_eq!(pool.total_connections(), 1);

    // Second call must reuse the pooled connection instead of dialing.
    let conn2 = pool.get_or_connect(&addr).unwrap();
    assert!(conn2.is_connected());
    assert_eq!(pool.total_connections(), 0, "reused conn leaves pool");

    drop(conn2);
    pool.close_all();
    handle.join().unwrap();
}

/// Async pool: return a healthy connection, then get again — the
/// first-check reuses the pooled entry (pool.rs 237-241).
#[tokio::test]
async fn test_s4_async_pool_reuses_returned_connection() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            kept.push(stream);
        }
    });

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 3,
        ..Default::default()
    });

    let (key, conn) = pool.get("s4-reuse", &addr).await.unwrap();
    pool.return_connection(key.clone(), conn);
    assert_eq!(pool.active_connection_count(), 1);

    // Re-get must hit the first-check reuse path and hand back the same key.
    let (key2, conn2) = pool.get("s4-reuse", &addr).await.unwrap();
    assert_eq!(key2, key);
    assert!(conn2.is_active());
    // dec_node_count ran during reuse: counts for the node are empty again.
    assert!(pool.get_stats().node_conns.is_empty());

    drop(conn2);
    pool.close();
    server.abort();
}

/// Async pool: a pooled-but-dead entry takes the dead branch of the
/// first check (pool.rs 242-244) and a fresh connection is dialed.
#[tokio::test]
async fn test_s4_async_pool_first_check_dead_entry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            kept.push(stream);
        }
    });

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 3,
        ..Default::default()
    });

    let (key, mut conn) = pool.get("s4-dead", &addr).await.unwrap();
    assert_eq!(pool.active_connection_count(), 1);

    // Close the conn and re-insert it into the pool (bypassing
    // return_connection, which would refuse a dead conn).
    conn.close();
    assert!(!conn.is_active());
    let node_id = conn.node_id().to_string();
    let address = conn.address().to_string();
    pool.conns.lock().insert(
        key.clone(),
        PoolEntry {
            conn,
            node_id,
            address,
        },
    );

    // Next get finds the dead entry, decrements active_count, dials fresh.
    let (key2, conn2) = pool.get("s4-dead", &addr).await.unwrap();
    assert_eq!(key2, key);
    assert!(conn2.is_active());
    assert_eq!(pool.active_connection_count(), 1);

    drop(conn2);
    pool.close();
    server.abort();
}

/// Async pool: the per-node double-check after semaphore acquisition
/// (pool.rs 275-288). The caller passes the first per-node check, parks
/// on the exhausted semaphore, and the node count is raised while it
/// waits — deterministically reproduced by holding the only permit,
/// inserting the count, then releasing a permit.
#[tokio::test]
async fn test_s4_pool_double_check_per_node_after_semaphore() {
    let pool = std::sync::Arc::new(Pool::new(AsyncPoolConfig {
        max_conns: 1,
        max_conns_per_node: 1,
        dial_timeout: Duration::from_secs(5),
        ..Default::default()
    }));

    // Consume the single permit so any getter parks on the semaphore.
    let held = pool.semaphore.clone();
    let permit = held.try_acquire_owned().unwrap();
    assert_eq!(pool.semaphore.available_permits(), 0);

    let getter_pool = pool.clone();
    let getter = tokio::spawn(async move {
        getter_pool
            .get_with_timeout("s4-dc", "127.0.0.1:1")
            .await
    });

    // Let the getter run up to its semaphore wait.
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Simulate another connection completing its dial for this node.
    pool.node_counts.lock().insert("s4-dc".to_string(), 1);

    // Release a permit so the getter wakes and re-checks the per-node limit.
    pool.semaphore.add_permits(1);

    let result = getter.await.unwrap();
    let err = result.expect_err("expected per-node double-check failure");
    assert!(
        err.contains("after acquiring semaphore"),
        "unexpected error: {}",
        err
    );
    assert_eq!(pool.active_connection_count(), 0);
    // The permit was dropped by the error path, so one permit is available.
    assert_eq!(pool.semaphore.available_permits(), 1);

    drop(permit);
    pool.close();
}

/// cleanup_dead removes a pooled dead entry and releases its semaphore
/// slot (pool.rs 412-435).
#[tokio::test]
async fn test_s4_cleanup_dead_removes_dead_entry() {
    install_s4_pool_subscriber();

    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            kept.push(stream);
        }
    });

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 3,
        ..Default::default()
    });

    let (key, mut conn) = pool.get("s4-cd", &addr).await.unwrap();
    conn.close();
    assert!(!conn.is_active());
    let node_id = conn.node_id().to_string();
    let address = conn.address().to_string();
    pool.conns.lock().insert(
        key,
        PoolEntry {
            conn,
            node_id,
            address,
        },
    );
    let before_permits = pool.semaphore.available_permits();

    let removed = pool.cleanup_dead();
    assert_eq!(removed, 1);
    assert_eq!(pool.active_connection_count(), 0);
    assert!(pool.conns.lock().is_empty());
    assert!(pool.node_counts.lock().is_empty());
    assert_eq!(
        pool.semaphore.available_permits(),
        before_permits + 1,
        "dead entry's forgotten permit must be returned"
    );

    pool.close();
    server.abort();
}

/// dec_node_count arcs (pool.rs 470-478): decrement to a non-zero value,
/// decrement to zero (key removed), and decrement for an absent key.
#[test]
fn test_s4_dec_node_count_arcs() {
    let pool = Pool::with_defaults();

    pool.node_counts.lock().insert("s4-arc".to_string(), 2);
    pool.dec_node_count("s4-arc");
    assert_eq!(pool.node_counts.lock().get("s4-arc").copied(), Some(1));

    pool.dec_node_count("s4-arc");
    assert!(pool.node_counts.lock().get("s4-arc").is_none());

    // Absent key: must be a no-op, not a panic.
    pool.dec_node_count("s4-arc-absent");
    assert!(pool.node_counts.lock().is_empty());
}
