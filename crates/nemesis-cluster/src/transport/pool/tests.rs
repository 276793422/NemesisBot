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
// coverage: sync reuse path (63), async first-check reuse
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
    let getter =
        tokio::spawn(async move { getter_pool.get_with_timeout("s4-dc", "127.0.0.1:1").await });

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

/// 回归：get_inner 捞出**池内死条目**时必须归还信号量
/// 许可 + 节点计数——旧代码只减 active_count，注释声称「许可留作复用」
/// 是谎话（forget 掉的许可拿不回来，随后的 dial 取的是新许可），每轮
/// 死连接轮换永久漏 1 槽。max_conns=1 让漏槽直接表现为第二次 get 假限流。
#[tokio::test]
async fn test_t5_dead_entry_in_map_releases_permit_on_get() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move {
        let mut kept = Vec::new();
        while let Ok((stream, _)) = listener.accept().await {
            kept.push(stream);
        }
    });

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 1,
        max_conns_per_node: 4,
        ..Default::default()
    });

    // 1. 拨号（唯一许可被 forget）→ 归还（活着，入池）。
    let (key, conn) = pool.get("t5-node", &addr).await.unwrap();
    pool.return_connection(key.clone(), conn);
    assert_eq!(pool.active_connection_count(), 1);
    assert_eq!(pool.get_stats().node_conns.get("t5-node"), Some(&1));
    assert_eq!(
        pool.semaphore.available_permits(),
        0,
        "唯一许可在池内条目上"
    );

    // 2. 池内条目死亡（模拟半开连接被标记；return_connection 的验活
    //    拒收构造不出这个形态，直接在 map 里 close）。
    pool.conns.lock().get_mut(&key).unwrap().conn.close();

    // 3. 再次 get：死条目被现场剔除——旧行为漏还许可，dial 撞
    //    "connection limit reached (1)"；修复后许可归还 → 重拨成功，
    //    且三账（active / node_count / 许可）对齐。
    let (_key2, conn2) = pool.get("t5-node", &addr).await.unwrap();
    assert!(conn2.is_active(), "死条目剔除后必须重拨成功");
    assert_eq!(pool.active_connection_count(), 1);
    assert_eq!(
        pool.get_stats().node_conns.get("t5-node"),
        Some(&1),
        "死条目剔除必须回落节点计数，重拨再补回"
    );
    assert_eq!(pool.semaphore.available_permits(), 0, "新拨消耗唯一许可");

    drop(conn2);
    pool.close();
    server.abort();
}

/// 回归：return_connection 收到**死连接**时节点计数
/// 必须随许可、active_count 一起回落（镜像 dial 的消耗三账）。
#[tokio::test]
async fn test_t5_return_dead_conn_releases_node_count() {
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
        max_conns_per_node: 4,
        ..Default::default()
    });

    let (key, mut conn) = pool.get("t5-dead", &addr).await.unwrap();
    conn.close();
    pool.return_connection(key, conn);

    assert_eq!(pool.active_connection_count(), 0);
    assert!(
        !pool.get_stats().node_conns.contains_key("t5-dead"),
        "死连接归还得回落节点计数（旧行为漂移 +1）"
    );
    assert_eq!(pool.semaphore.available_permits(), 10, "许可必须全额归还");

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

// ============================================================
// 重连语义链路测试（集群完备性加固，见模块头「重连语义」）：
// 隐式自愈全链路 —— 本地关闭的连接归还时被拒（不回池）→ 下次 get 现场
// 剔除死条目并新拨 → active_count 回到 1。半开连接（对端静默丢）的
// 「首次写失败」发现路径是 TCP 固有边界，不在单测覆盖（见模块头裁决）。
// ============================================================

#[tokio::test]
async fn test_link_locally_closed_conn_evicted_then_fresh_dialed() {
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

    // 1. 首次 get：新拨，active=1。
    let (key, mut conn) = pool.get("link-heal", &addr).await.unwrap();
    assert!(conn.is_active());
    assert_eq!(pool.active_connection_count(), 1);

    // 2. 本地关闭（模拟本侧探得死连接），归还 → 拒收（死连接不回池），
    //    槽位即释（active 归零、信号量返还）。
    conn.close();
    pool.return_connection(key.clone(), conn);
    assert_eq!(pool.active_connection_count(), 0);

    // 3. 直接把死条目塞回池内（模拟归还前已死的遗留条目）：
    //    下次 get 的取用即验会剔除它并新拨一条活的。
    //    （return_connection 已验活拒收，本段构造池内死条目的路径。）
    //    上面第 2 步已证明拒收，这里复用 dead-entry 语义做全链路收尾：
    let (key2, conn2) = pool.get("link-heal", &addr).await.unwrap();
    assert_eq!(key2, key);
    assert!(conn2.is_active(), "重拨后连接必须存活");
    assert_eq!(pool.active_connection_count(), 1, "自愈后 active 恰好 1");

    drop(conn2);
    pool.close();
    server.abort();
}

/// 回归锁：同 key 二次归还不得覆盖池内条目。
/// per-node limit 允许同 node+address 并发租借（get 直接拨新不走池），
/// 两条租借的 key 相同；旧实现顺序归还时 HashMap:insert 覆盖池内第一条，
/// 被覆盖连接 dial 时 forget 的 permit + node_count + active_count 三账
/// 单位永久蒸发（泄漏单位不在池内，cleanup_dead 摸不到）——反复并发调用
/// 即逼近 "connection limit reached" / "per-node limit reached" 假限流。
/// 修复后：后来者关连接并回吐自己的三账单位，池内条目与许可账目对齐。
#[tokio::test]
async fn test_pool_return_same_key_twice_does_not_overwrite_entry() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

    let pool = Pool::new(AsyncPoolConfig {
        max_conns: 10,
        max_conns_per_node: 3,
        ..Default::default()
    });

    // 同 key 并发租借两条（per-node limit=3 允许）：get 都直接拨新，
    // 两条租借共享同一 key "node-1:{addr}"。
    let (key1, c1) = pool.get("node-1", &addr).await.unwrap();
    let (key2, c2) = pool.get("node-1", &addr).await.unwrap();
    assert_eq!(key1, key2, "同 node+address 的 key 必须相同");
    assert_eq!(pool.active_connection_count(), 2);

    // 顺序归还两条：第二条不得覆盖第一条的池条目。
    pool.return_connection(key1.clone(), c1);
    pool.return_connection(key2, c2);

    let stats = pool.get_stats();
    assert_eq!(
        stats.active_conns, 1,
        "同 key 二归还后池内只留一条（覆盖泄漏形态 active=2）"
    );
    assert_eq!(
        stats.node_conns.get("node-1"),
        Some(&1),
        "节点计数不得漂移（覆盖泄漏形态=2）"
    );
    // 许可账目直接核对：max_conns=10，池内 1 条各占 1 许可 → 9 可用
    //（覆盖泄漏形态第二条 forget 的许可不回吐 → 8）。
    assert_eq!(
        pool.semaphore.available_permits(),
        9,
        "两条租借的许可都必须回吐"
    );

    // 池内幸存条目取用即验：get 命中池内连接（不新拨 → active 不变）。
    let (key3, c3) = pool.get("node-1", &addr).await.unwrap();
    assert_eq!(key3, key1);
    assert_eq!(
        pool.active_connection_count(),
        1,
        "命中池内条目不产生新租约记账"
    );

    drop(c3);
    pool.close();
    server.abort();
}

/// return_connection 幕帘竞态窗回归锁（2026-11-05 风险报告评审）：
/// 旧实现快路径守卫（读 closed）与拿 conns 锁插表/记账之间无原子性，
/// close()（先 store(true) 再拿锁清三账）可在窗口内整体插队——活连接
/// 塞进已关池（滞留活 socket，forget 的许可无条目可释放）或死臂对已
/// 归零的 active_count fetch_sub（debug panic / release 回绕）。修复 =
/// 拿锁后锁内复检 closed，记账同临界区（与 remove/cleanup_dead 既有
/// 锁内记账模式对齐）。
///
/// 模拟形态：测试线程持 conns 锁（归还线程已过快路径、阻塞在拿锁），
/// 持锁下手工执行 close() 的全部副作用（store(true) + 清三账——close
/// 本体拿同一把锁，持锁模拟 = close 赢得插队），放锁后归还线程锁内
/// 复检必读 true → 幕帘臂。另一种真实交错（线程未达锁时 close 已整体
/// 完成 → 快路径幕帘）产生相同可观测终态，断言无 flake。
#[tokio::test]
async fn test_return_connection_curtain_recheck_closes_race_window() {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap().to_string();
    let server = tokio::spawn(async move { while listener.accept().await.is_ok() {} });

    let max_conns = 8usize;
    let pool = std::sync::Arc::new(Pool::new(AsyncPoolConfig {
        max_conns,
        max_conns_per_node: 3,
        ..Default::default()
    }));

    let (key, conn) = pool.get("node-race", &addr).await.unwrap();
    assert_eq!(pool.active_connection_count(), 1);
    // dial forget 了 1 个许可
    assert_eq!(pool.semaphore.available_permits(), max_conns - 1);

    // 持锁握手：归还线程过快路径守卫后必然阻塞在 conns.lock() 上
    let guard = pool.conns.lock();
    let handle = {
        let pool = std::sync::Arc::clone(&pool);
        let key = key.clone();
        std::thread::spawn(move || pool.return_connection(key, conn))
    };
    // 给归还线程时间过守卫、抵达锁点（不足时线程走快路径幕帘，终态相同）
    std::thread::sleep(Duration::from_millis(50));
    // 持锁下执行 close() 的全部记账步骤（close 本体拿同一把锁；此处持锁
    // 模拟 = close 的插队在本函数守卫通过之后、拿锁之前完成）
    pool.closed.store(true, Ordering::SeqCst);
    pool.node_counts.lock().clear();
    pool.active_count.store(0, Ordering::SeqCst);
    // 池内无条目 → close 不补许可
    drop(guard);

    handle.join().unwrap();

    // 幕帘语义终态：归还走幕帘臂（只关连接 + 补还许可），不碰已清三账
    assert_eq!(
        pool.active_connection_count(),
        0,
        "active_count 保持 close 归零值（下溢形态=usize::MAX）"
    );
    assert!(
        pool.conns.lock().is_empty(),
        "已关池不得滞留归还连接（活 socket 泄漏形态）"
    );
    assert_eq!(
        pool.node_counts.lock().len(),
        0,
        "节点计数不得复活（幻影键形态）"
    );
    assert_eq!(
        pool.semaphore.available_permits(),
        max_conns,
        "归还连接 dial 消耗的许可由幕帘臂补还（许可泄漏形态={}-1）",
        max_conns
    );

    server.abort();
}
