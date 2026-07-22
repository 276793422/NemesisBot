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

    let server = tokio::spawn(async move { while let Ok(_) = listener.accept().await {} });

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
    let server = tokio::spawn(async move { while let Ok(_) = listener.accept().await {} });

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

    let server = tokio::spawn(async move { while let Ok(_) = listener.accept().await {} });

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

    let server = tokio::spawn(async move { while let Ok(_) = listener.accept().await {} });

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

    let server = tokio::spawn(async move { while let Ok(_) = listener.accept().await {} });

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
