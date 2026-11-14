//! 漫游会话池测试（长连接复用 / 心跳保活 / 断线检出重连 / TTL / 并发拨号）。
//!
//! 测试形态：`PeerSim` 假拨号器自含对端模拟——`dial()` 绑 loopback 监听 +
//! spawn 单次 accept 的帧回声服务（`RoamingConn` 同款帧格式 + AEAD token，
//! 镜像插件 `splice_dial` 一监听一连接的合同）；`kill_active()` 模拟对端
//! 断链；`set_accept(false)` 模拟对端不可达（重连拨号必败）。
//!
//! 诚实边界：loopback 测不出 relay RTT / 打洞延迟（真实环境收益更大）与
//! NAT 保活行为（将来真机验证）——这里验证的是池逻辑本身：复用、检出、
//! 重连、退避、逐出、并发拨号去重。

use std::sync::Arc;
use std::sync::atomic::AtomicUsize;

use super::*;

const TEST_TOKEN: &str = "pool-test-token";
const TEST_ADDR: &str = "iroh:testpeer";

// ---------------------------------------------------------------------------
// PeerSim：自含对端模拟的假拨号器
// ---------------------------------------------------------------------------

struct PeerSim {
    /// dial 调用计数（会话复用 / 重连断言核心观测量）。
    dial_calls: Arc<AtomicUsize>,
    /// close 调用计数（best-effort 断连面；不作硬断言）。
    close_calls: Arc<AtomicUsize>,
    /// 对端是否可达（false = dial 诚实 Err——重连必败场景）。
    accept_enabled: Arc<AtomicBool>,
    /// 活跃连接的 kill 手柄（发信号 → 回声任务退出 → 客户端见 FIN/RST）。
    active: Arc<std::sync::Mutex<Vec<tokio::sync::oneshot::Sender<()>>>>,
    token: Arc<String>,
    next_conn_id: Arc<AtomicU64>,
}

impl PeerSim {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            dial_calls: Arc::new(AtomicUsize::new(0)),
            close_calls: Arc::new(AtomicUsize::new(0)),
            accept_enabled: Arc::new(AtomicBool::new(true)),
            active: Arc::new(std::sync::Mutex::new(Vec::new())),
            token: Arc::new(TEST_TOKEN.to_string()),
            next_conn_id: Arc::new(AtomicU64::new(1)),
        })
    }

    fn dial_calls(&self) -> usize {
        self.dial_calls.load(Ordering::SeqCst)
    }

    fn set_accept(&self, on: bool) {
        self.accept_enabled.store(on, Ordering::SeqCst);
    }

    /// 杀掉全部活跃连接（模拟对端断链/进程重启——客户端 recv 必败）。
    async fn kill_active(&self) {
        let handles: Vec<_> = self.active.lock().unwrap().drain(..).collect();
        for tx in handles {
            let _ = tx.send(());
        }
        // 给回声任务一拍退出（客户端侧 FIN/RST 到达）。
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl RoamingDialer for PeerSim {
    fn dial(
        &self,
        _addr: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, u64), String>> + Send>>
    {
        self.dial_calls.fetch_add(1, Ordering::SeqCst);
        if !self.accept_enabled.load(Ordering::SeqCst) {
            return Box::pin(async { Err("peer down（对端不可达模拟）".into()) });
        }
        // &self 不可移动——克隆全部共享句柄进任务。
        let registry = Arc::clone(&self.active);
        let token = Arc::clone(&self.token);
        let next_id = Arc::clone(&self.next_conn_id);
        Box::pin(async move {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
                .await
                .map_err(|e| e.to_string())?;
            let port = listener.local_addr().map_err(|e| e.to_string())?.port();
            let conn_id = next_id.fetch_add(1, Ordering::SeqCst);
            let (kill_tx, mut kill_rx) = tokio::sync::oneshot::channel::<()>();
            registry.lock().unwrap().push(kill_tx);
            tokio::spawn(async move {
                // 单次 accept——镜像插件 splice_dial「一监听一连接」合同。
                let accepted = timeout(Duration::from_secs(10), listener.accept()).await;
                let Ok(Ok((sock, _))) = accepted else {
                    return;
                };
                let (read_half, write_half) = sock.into_split();
                let mut conn = RoamingConn::new(write_half, read_half, &token);
                loop {
                    tokio::select! {
                        _ = &mut kill_rx => break,
                        frame = conn.recv_frame() => match frame {
                            Ok(bytes) => {
                                // 请求 → pong 响应（服务端 ping handler 同形态）。
                                let Ok(req) = serde_json::from_slice::<WireMessage>(&bytes) else {
                                    break;
                                };
                                let resp =
                                    WireMessage::new_response(&req, serde_json::json!({"status": "pong"}));
                                let out = match serde_json::to_vec(&resp) {
                                    Ok(v) => v,
                                    Err(_) => break,
                                };
                                if conn.send_frame(&out).await.is_err() {
                                    break;
                                }
                            }
                            Err(_) => break,
                        }
                    }
                }
            });
            Ok((port, conn_id))
        })
    }

    fn close(&self, _conn_id: u64) {
        self.close_calls.fetch_add(1, Ordering::SeqCst);
    }
}

// ---------------------------------------------------------------------------
// 测试辅助
// ---------------------------------------------------------------------------

fn make_pool(sim: &Arc<PeerSim>, heartbeat_interval: u64) -> Arc<RoamingSessionPool> {
    let pool = Arc::new(RoamingSessionPool::new());
    pool.set_heartbeat_interval(heartbeat_interval);
    pool.set_token(TEST_TOKEN);
    pool.set_dialer(Arc::clone(sim) as Arc<dyn RoamingDialer>);
    pool
}

/// 一次 ping 帧交换。Ok(bool) = pong 合法；Err = Broken/Lost（调用方按
/// 用例语义断言）。
async fn ping_once(pool: &Arc<RoamingSessionPool>, sim: &Arc<PeerSim>) -> Result<bool, String> {
    let wire = heartbeat_ping_frame();
    let bytes = match pool
        .exchange(
            TEST_ADDR,
            wire,
            Duration::from_secs(5),
            Arc::clone(sim) as Arc<dyn RoamingDialer>,
            TEST_TOKEN,
        )
        .await
    {
        PoolExchange::Ok(b) => b,
        PoolExchange::Broken(e) => return Err(format!("broken: {e}")),
        PoolExchange::Lost(e) => return Err(format!("lost: {e}")),
    };
    let resp: WireMessage = serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
    Ok(resp.error.is_empty()
        && resp.action == "ping"
        && resp.payload.get("status").and_then(|s| s.as_str()) == Some("pong"))
}

// ---------------------------------------------------------------------------
// 用例
// ---------------------------------------------------------------------------

/// 同目标多次请求复用同一会话：3 次交换恰好 1 次拨号（长连接核心收益）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exchange_reuses_session_across_requests() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 0);
    for _ in 0..3 {
        assert!(ping_once(&pool, &sim).await.unwrap(), "pong 响应合法");
    }
    assert_eq!(
        sim.dial_calls(),
        1,
        "三次请求必须复用同一会话（恰好一次拨号）"
    );
    assert_eq!(pool.session_count().await, 1);
}

/// 死会话：请求诚实失败（不得假成功）→ 会话被摘 → 下次请求重拨恢复。
/// 失败形态 Broken（发送臂败）或 Lost（帧已写出禁重发）都合法。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn dead_session_rejects_and_next_request_redials() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 0);
    assert!(ping_once(&pool, &sim).await.unwrap());
    assert_eq!(sim.dial_calls(), 1);

    sim.kill_active().await;
    let second = ping_once(&pool, &sim).await;
    assert!(second.is_err(), "死连接上不得假成功（got {second:?}）");

    assert!(ping_once(&pool, &sim).await.unwrap(), "重拨后恢复");
    assert!(sim.dial_calls() >= 2, "必须重新拨号（原会话已死）");
    assert_eq!(pool.session_count().await, 1);
}

/// 心跳三合一：健康闲置会话被 ping 保活（不逐出不重拨）→ 对端断链被
/// 检出 → 后台重连成功回池 → 新会话可用。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn heartbeat_keeps_alive_detects_break_and_reconnects() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 1); // 1s 心跳
    assert!(ping_once(&pool, &sim).await.unwrap());
    assert_eq!(sim.dial_calls(), 1);

    // 不杀连接：闲置超 1s 后心跳 ping 保活——会话不被逐出、不重拨。
    tokio::time::sleep(Duration::from_secs(3)).await;
    assert_eq!(sim.dial_calls(), 1, "健康闲置会话由心跳保活（不重拨）");
    assert_eq!(pool.session_count().await, 1, "保活会话不被 TTL 逐出");

    // 杀连接 → 心跳检出 → 摘会话 → 后台重连（对端活着）→ 回池。
    sim.kill_active().await;
    tokio::time::sleep(Duration::from_secs(6)).await;
    assert!(sim.dial_calls() >= 2, "心跳必须检出断线并触发后台重连");
    assert_eq!(pool.session_count().await, 1, "重连成功后会话回池");
    assert!(ping_once(&pool, &sim).await.unwrap(), "重连后的新会话可用");
}

/// 心跳关闭形态：闲置超 TTL 的会话被清扫，且 TTL 逐出是刻意关闭——
/// 不触发重连（dial 计数不变）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_ttl_evicts_when_heartbeat_off() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 0); // 心跳关
    pool.set_idle_ttl_for_tests(2);
    assert!(ping_once(&pool, &sim).await.unwrap());
    tokio::time::sleep(Duration::from_secs(4)).await;
    assert_eq!(pool.session_count().await, 0, "闲置超 TTL 必须被清扫");
    assert_eq!(sim.dial_calls(), 1, "TTL 逐出不触发重连");
}

/// 对端不可达：重连按退避阶梯试完全败 → 放弃（池保持空）；对端复活后
/// 真实请求按池缺失路径按需重拨成功（重连是保活优化不是正确性依赖）。
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn reconnect_backoff_bounded_when_peer_down() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 1);
    pool.set_reconnect_delays_for_tests(vec![1, 1]);
    assert!(ping_once(&pool, &sim).await.unwrap());

    sim.set_accept(false);
    sim.kill_active().await;
    tokio::time::sleep(Duration::from_secs(5)).await;
    assert_eq!(
        pool.session_count().await,
        0,
        "对端不可达：重连全败后池保持空"
    );
    assert!(
        sim.dial_calls() >= 2,
        "重连尝试必须发生（初始 1 次 + 后台重连拨号）"
    );

    sim.set_accept(true);
    assert!(
        ping_once(&pool, &sim).await.unwrap(),
        "对端恢复后按需重拨成功"
    );
    assert_eq!(pool.session_count().await, 1);
}

/// 同目标并发请求：8 路并发池缺失必须只拨一次（per-addr 拨号互斥 +
/// 双重检查），全部请求成功且收敛到单会话。
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_requests_share_one_dial() {
    let sim = PeerSim::new();
    let pool = make_pool(&sim, 0);
    let mut handles = Vec::new();
    for _ in 0..8 {
        let pool = Arc::clone(&pool);
        let sim = Arc::clone(&sim);
        handles.push(tokio::spawn(async move { ping_once(&pool, &sim).await }));
    }
    for h in handles {
        assert!(h.await.unwrap().unwrap(), "全部并发请求成功");
    }
    assert_eq!(
        sim.dial_calls(),
        1,
        "并发池缺失必须只拨一次（per-addr 拨号互斥）"
    );
    assert_eq!(pool.session_count().await, 1);
}
