//! iroh 漫游会话池（2026-11-14 审查发现 #2 根修：iroh 长连接 + 心跳保活）。
//!
//! 旧行为：每条 RPC 请求独立拨号（插件 iroh connect + loopback 一次性
//! 监听 + splice），请求完成即关——每次请求付出完整 QUIC 握手 + 打洞
//! 代价，且打洞映射无流量保活时自然失效（下一条请求重新打洞）。
//!
//! 新行为（方案 A，宿主侧改造、插件零改动）：
//! - **会话缓存**：per-addr 缓存 [`RoamingConn`]（loopback TCP 已 split
//!   的读写半），同目标请求在同一连接上串行帧交换（async Mutex——请求/
//!   响应成对原子，不串台）。服务端 `handle_connection` 与插件 splice
//!   均为内容无关的顺序多请求循环，连接级多路复用天然成立。
//! - **心跳保活**（默认 30s，`roaming_heartbeat_interval_secs` 可配，
//!   0=关）：对闲置 ≥ interval 的会话发应用层 ping（服务端内置
//!   handler）——同时刷新 QUIC 会话与 NAT/打洞映射；连接忙（帧交换
//!   进行中）自动跳过。
//! - **断线检测 + 后台重连**：心跳 ping 失败（或帧交换失败）→ 摘会话
//!   → 后台带退避重连（成功后新会话入池；对端不可达按退避上限放弃，
//!   期间真实请求按池缺失路径按需重拨）。send 失败可安全重拨（帧未写
//!   出 = 对端不可能已执行）；recv 失败归 ResponseLost（帧已写出，
//!   禁止同请求重发——BUG-20261109-004 语义延续）。
//! - **闲置 TTL**（600s）：心跳关闭形态下会话不无限堆积；心跳开启时
//!   ping 成功即刷新，TTL 自然不触发。TTL 逐出是刻意关闭（不重连）。
//!
//! 并发拨号去重：同 addr 并发池缺失只拨一次（per-addr 拨号互斥 +
//! 双重检查），输者复用赢者会话。

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::Duration;

use tokio::sync::Mutex;
use tokio::time::timeout;

use crate::roaming::bridge::RoamingConn;
use crate::roaming::plugin_bridge::RoamingDialer;
use crate::transport::conn::WireMessage;

/// 会话闲置 TTL（秒）：超过即逐出（刻意关闭，不重连——真实请求按需重拨）。
/// 心跳开启且健康时 ping 持续刷新 last_used，此闸自然不触发。
const SESSION_IDLE_TTL_SECS: u64 = 600;

/// 心跳 ping 超时（秒）——远小于业务 io_timeout：ping 只回答「链路活不活」。
const PING_TIMEOUT: Duration = Duration::from_secs(10);

/// 重连拨号预算（秒）：插件内拨号限时 10s + loopback 交接余量。
const RECONNECT_DIAL_BUDGET: Duration = Duration::from_secs(15);

/// 心跳 ping 默认间隔（秒）。config.cluster.json
/// `roaming_heartbeat_interval_secs` 缺省同值；0 = 关。
pub(crate) const DEFAULT_HEARTBEAT_INTERVAL_SECS: u64 = 30;

/// 重连退避阶梯（秒）：断线后 5 次尝试（约 7.7 分钟窗口），全败放弃——
/// 期间与之后的真实请求走池缺失按需重拨（重连是保活优化不是正确性依赖）。
const RECONNECT_DELAY_SECS: &[u64] = &[2, 8, 30, 120, 300];

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}

// ---------------------------------------------------------------------------
// 会话
// ---------------------------------------------------------------------------

/// 池内单会话：一条 iroh 连接的 loopback 交接 TCP（已 split），conn 背后
/// 的 async Mutex 把「发请求帧 + 收响应帧」绑成原子对——同目标并发请求
/// 在同一连接上排队，帧不串台。
pub(crate) struct PooledSession {
    conn: Mutex<RoamingConn<tokio::net::tcp::OwnedWriteHalf, tokio::net::tcp::OwnedReadHalf>>,
    /// 插件连接 id（`dialer.close` 收整条 QUIC 连接用；建会话后不变）。
    conn_id: u64,
    /// 最近使用时刻（ms）。帧交换成功 / 心跳 ping 成功时刷新。
    last_used_ms: AtomicU64,
}

/// 单次帧交换结果（连接级三态——调用方据此决定重发语义）。
pub(crate) enum SessionExchange {
    /// 收到响应帧（明文字节）。
    Ok(Vec<u8>),
    /// 发送阶段失败（帧可能未完整写出——连接已坏；同请求重发的判定
    /// 与旧按请求拨号形态一致，归上层）。
    SendBroken(String),
    /// 发送成功但收响应失败（帧已写出——ResponseLost，禁止同请求重发）。
    ResponseLost(String),
}

impl PooledSession {
    /// 在会话上做一次帧交换（send → recv 原子对；外层超时由调用方包）。
    async fn exchange(&self, wire_bytes: &[u8], io_timeout: Duration) -> SessionExchange {
        let mut conn = self.conn.lock().await;
        match timeout(io_timeout, conn.send_frame(wire_bytes)).await {
            Err(_) => return SessionExchange::SendBroken(format!("send timeout({io_timeout:?})")),
            Ok(Err(e)) => return SessionExchange::SendBroken(e),
            Ok(Ok(())) => {}
        }
        match timeout(io_timeout, conn.recv_frame()).await {
            Err(_) => SessionExchange::ResponseLost(format!("recv timeout({io_timeout:?})")),
            Ok(Err(e)) => SessionExchange::ResponseLost(e),
            Ok(Ok(bytes)) => {
                self.last_used_ms.store(now_ms(), Ordering::Relaxed);
                SessionExchange::Ok(bytes)
            }
        }
    }

    /// 会话是否可立即接管（未在帧交换中）。忙 = 有在途请求，跳过。
    fn try_lock(
        &self,
    ) -> Option<
        tokio::sync::MutexGuard<
            '_,
            RoamingConn<tokio::net::tcp::OwnedWriteHalf, tokio::net::tcp::OwnedReadHalf>,
        >,
    > {
        self.conn.try_lock().ok()
    }

    fn idle_secs(&self) -> u64 {
        now_ms().saturating_sub(self.last_used_ms.load(Ordering::Relaxed)) / 1000
    }

    fn touch(&self) {
        self.last_used_ms.store(now_ms(), Ordering::Relaxed);
    }
}

// ---------------------------------------------------------------------------
// 池
// ---------------------------------------------------------------------------

/// 单次池交换结果（调用方 RpcClient 映射到 `RpcClientError`）。
pub(crate) enum PoolExchange {
    /// 收到响应帧。
    Ok(Vec<u8>),
    /// 连接失败（拨号失败 / 发送失败——同请求可安全重拨重发，与旧
    /// 按请求拨号形态的 Connection 语义一致）。
    Broken(String),
    /// 帧已写出但响应丢失（ResponseLost——禁止同请求重发）。
    Lost(String),
}

/// iroh 漫游会话池（宿主侧；RpcClient 持有）。
pub(crate) struct RoamingSessionPool {
    /// per-addr 会话（addr = 完整 `iroh:<hex>[@<relay>]` 拨号形态）。
    sessions: std::sync::Mutex<HashMap<String, Arc<PooledSession>>>,
    /// per-addr 拨号互斥（并发池缺失只拨一次；锁内做双重检查）。
    dial_locks: std::sync::Mutex<HashMap<String, Arc<Mutex<()>>>>,
    /// 后台任务（心跳/重连）用的拨号出口——与 RpcClient 本体字段同源
    /// 注入（`set_dialer` 转发）。None = 尚未装配（后台退让）。
    backend_dialer: std::sync::Mutex<Option<Arc<dyn RoamingDialer>>>,
    /// 后台任务用的 token 快照（`set_token` 转发；装配序 token 先于
    /// dialer 到达，独立存放免合并丢失）。
    backend_token: std::sync::Mutex<String>,
    /// 心跳间隔（秒；0=关）。config 注入；测试注入小值驱动。
    heartbeat_interval: AtomicU64,
    /// 会话闲置 TTL（秒）。常量缺省；测试注入小值驱动逐出。
    idle_ttl: AtomicU64,
    /// 重连退避阶梯（秒序列）。常量缺省；测试注入短阶梯驱动。
    reconnect_delays: std::sync::Mutex<Vec<u64>>,
    /// 心跳循环已启动（幂等；`set_dialer` 时拉起）。
    hb_started: AtomicBool,
}

impl Default for RoamingSessionPool {
    fn default() -> Self {
        Self::new()
    }
}

impl RoamingSessionPool {
    pub(crate) fn new() -> Self {
        Self {
            sessions: std::sync::Mutex::new(HashMap::new()),
            dial_locks: std::sync::Mutex::new(HashMap::new()),
            backend_dialer: std::sync::Mutex::new(None),
            backend_token: std::sync::Mutex::new(String::new()),
            heartbeat_interval: AtomicU64::new(DEFAULT_HEARTBEAT_INTERVAL_SECS),
            idle_ttl: AtomicU64::new(SESSION_IDLE_TTL_SECS),
            reconnect_delays: std::sync::Mutex::new(RECONNECT_DELAY_SECS.to_vec()),
            hb_started: AtomicBool::new(false),
        }
    }

    /// 注入后台拨号出口（RpcClient::set_roaming_dialer 转发；幂等覆盖）。
    /// 首次注入且有运行时上下文时拉起心跳循环。
    pub(crate) fn set_dialer(self: &Arc<Self>, dialer: Arc<dyn RoamingDialer>) {
        *self.backend_dialer.lock().unwrap() = Some(dialer);
        self.maybe_start_heartbeat();
    }

    /// 注入后台 token 快照（RpcClient::set_auth_token 转发；幂等覆盖）。
    pub(crate) fn set_token(&self, token: &str) {
        *self.backend_token.lock().unwrap() = token.to_string();
    }

    /// 配置心跳间隔（秒；0=关）。装配期在 `set_dialer` 前后调用皆可
    /// （循环每 tick 现读，热生效）。
    pub fn set_heartbeat_interval(&self, secs: u64) {
        self.heartbeat_interval.store(secs, Ordering::Relaxed);
    }

    /// 测试注入口：会话闲置 TTL（秒）。
    #[cfg(test)]
    pub(crate) fn set_idle_ttl_for_tests(&self, secs: u64) {
        self.idle_ttl.store(secs, Ordering::Relaxed);
    }

    /// 测试注入口：重连退避阶梯（秒序列）。
    #[cfg(test)]
    pub(crate) fn set_reconnect_delays_for_tests(&self, secs: Vec<u64>) {
        *self.reconnect_delays.lock().unwrap() = secs;
    }

    /// 当前池内会话数（测试/观测面）。
    #[cfg(test)]
    pub(crate) async fn session_count(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    /// 心跳循环拉起（幂等；需 tokio 运行时上下文——测试外调用点都在
    /// gateway 装配路径上）。无运行时 = 跳过（池按需拨号语义照常，
    /// 只是失去保活）。
    fn maybe_start_heartbeat(self: &Arc<Self>) {
        if self.hb_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        let spawned = tokio::runtime::Handle::try_current().map(|h| h.spawn(this.heartbeat_loop()));
        if spawned.is_err() {
            // 无运行时上下文：诚实降级（保活缺席，拨号语义不变）。
            tracing::debug!("[RoamingPool] 心跳循环未拉起（无 tokio 运行时上下文）");
        }
    }

    /// 心跳主循环：1s 粒度 tick；每 tick 做闲置 TTL 清扫（忙会话跳过），
    /// 每 `heartbeat_interval` 做一轮心跳 ping 扫描（0=关）。
    async fn heartbeat_loop(self: Arc<Self>) {
        let mut last_ping_sweep = now_ms();
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let interval = self.heartbeat_interval.load(Ordering::Relaxed);
            let due = now_ms().saturating_sub(last_ping_sweep) >= interval.saturating_mul(1000);
            if interval > 0 && due {
                last_ping_sweep = now_ms();
            }
            self.sweep_once(interval > 0 && due).await;
        }
    }

    /// 一轮清扫。`do_ping` = 本 tick 是否做心跳 ping（false = 只做 TTL
    /// 清扫——心跳关闭形态）。
    async fn sweep_once(self: &Arc<Self>, do_ping: bool) {
        let interval = self.heartbeat_interval.load(Ordering::Relaxed);
        let ttl = self.idle_ttl.load(Ordering::Relaxed);
        let entries: Vec<(String, Arc<PooledSession>)> = {
            let map = self.sessions.lock().unwrap();
            map.iter().map(|(k, v)| (k.clone(), v.clone())).collect()
        };
        for (addr, sess) in entries {
            // 忙（帧交换在途）= 活跃会话，ping 与 TTL 都跳过——在途
            // 请求不该被保活逻辑打断或逐出。
            let Some(mut conn) = sess.try_lock() else {
                continue;
            };
            let idle = sess.idle_secs();
            if idle >= ttl {
                self.evict(&addr, &sess, false);
                tracing::debug!(addr = %addr, idle_secs = idle, "[RoamingPool] 会话闲置超 TTL 逐出");
                continue;
            }
            if !do_ping || idle < interval {
                continue;
            }
            let ping = heartbeat_ping_frame();
            let r = timeout(PING_TIMEOUT, async {
                conn.send_frame(&ping).await?;
                conn.recv_frame().await
            })
            .await;
            match r {
                Ok(Ok(_)) => {
                    // 链路活：刷新 last_used（TTL 不触发，QUIC/NAT 保活）。
                    sess.touch();
                }
                _ => {
                    // 断线：摘会话 + 后台带退避重连。
                    tracing::info!(addr = %addr, "[RoamingPool] 心跳失败——会话摘除，后台重连");
                    self.evict(&addr, &sess, true);
                }
            }
        }
    }

    /// 摘会话（ptr 相等守卫——只摘自己看见的那条，不误伤刚被真实请求
    /// 重拨替换的新会话）+ 显式关插件连接 + 可选后台重连。
    fn evict(self: &Arc<Self>, addr: &str, sess: &Arc<PooledSession>, reconnect: bool) {
        let replaced = {
            let mut map = self.sessions.lock().unwrap();
            match map.get(addr) {
                Some(cur) if Arc::ptr_eq(cur, sess) => {
                    map.remove(addr);
                    false
                }
                _ => true, // 已被替换/已不在——新会话顶上，勿动
            }
        };
        if replaced {
            return;
        }
        let dialer = self.backend_dialer.lock().unwrap().clone();
        if let Some(d) = dialer {
            d.close(sess.conn_id);
        }
        if reconnect {
            self.spawn_reconnect(addr.to_string());
        }
    }

    /// 后台重连 worker：按退避阶梯重拨；成功则新会话入池（若真实请求
    /// 已抢先拨好则关掉多余连接）；阶梯耗尽放弃（诚实 debug 留痕——
    /// 池缺失路径仍会在真实请求时按需重拨）。
    fn spawn_reconnect(self: &Arc<Self>, addr: String) {
        let this = Arc::clone(self);
        tokio::spawn(async move {
            let delays = this.reconnect_delays.lock().unwrap().clone();
            for delay in delays {
                tokio::time::sleep(Duration::from_secs(delay)).await;
                // 真实请求已重拨 / 心跳已另起新会话 → 收工。
                if this.sessions.lock().unwrap().contains_key(&addr) {
                    return;
                }
                let Some(dialer) = this.backend_dialer.lock().unwrap().clone() else {
                    return;
                };
                let token = this.backend_token.lock().unwrap().clone();
                match this
                    .dial_new_session(&addr, dialer.as_ref(), &token, RECONNECT_DIAL_BUDGET)
                    .await
                {
                    Ok(sess) => {
                        let mut map = this.sessions.lock().unwrap();
                        if map.contains_key(&addr) {
                            // 竞态败者：关掉自己这条，复用在场会话。
                            drop(map);
                            dialer.close(sess.conn_id);
                        } else {
                            map.insert(addr.clone(), sess);
                            tracing::info!(addr = %addr, "[RoamingPool] 后台重连成功（会话回池）");
                        }
                        return;
                    }
                    Err(e) => {
                        tracing::debug!(addr = %addr, error = %e, "[RoamingPool] 重连失败（按退避续试）");
                    }
                }
            }
            tracing::debug!(addr = %addr, "[RoamingPool] 重连退避耗尽放弃——真实请求将按需重拨");
        });
    }

    /// 拨一条新会话（插件 dial + loopback 交接 + RoamingConn 装配）。
    /// 失败诚实 Err（半开兜底 close 已就地收尾）。
    async fn dial_new_session(
        &self,
        addr: &str,
        dialer: &dyn RoamingDialer,
        token: &str,
        dial_budget: Duration,
    ) -> Result<Arc<PooledSession>, String> {
        let (port, conn_id) = timeout(dial_budget, dialer.dial(addr))
            .await
            .map_err(|_| format!("iroh dial timeout to {addr}"))?
            .map_err(|e| format!("iroh connect to {addr}: {e}"))?;
        let stream = match timeout(
            Duration::from_secs(5),
            tokio::net::TcpStream::connect(("127.0.0.1", port)),
        )
        .await
        {
            Ok(Ok(s)) => s,
            Ok(Err(e)) => {
                dialer.close(conn_id);
                return Err(format!("iroh loopback 交接失败（port {port}）: {e}"));
            }
            Err(_) => {
                dialer.close(conn_id);
                return Err(format!("iroh loopback 交接超时（port {port}）"));
            }
        };
        let (read_half, write_half) = stream.into_split();
        Ok(Arc::new(PooledSession {
            conn: Mutex::new(RoamingConn::new(write_half, read_half, token)),
            conn_id,
            last_used_ms: AtomicU64::new(now_ms()),
        }))
    }

    /// 池交换主入口（RpcClient::roaming_connect_and_send 委托）。
    /// 路径：池命中直接帧交换；池缺失 → per-addr 拨号互斥 + 双重检查
    /// → 拨新会话入池 → 交换。连接级失败（Broken/Lost）都摘会话——
    /// Broken 附带后台重连，Lost 同样（连接已坏，保活义务相同）。
    pub(crate) async fn exchange(
        self: &Arc<Self>,
        addr: &str,
        wire_bytes: Vec<u8>,
        io_timeout: Duration,
        dialer: Arc<dyn RoamingDialer>,
        token: &str,
    ) -> PoolExchange {
        // 快路径：池命中。
        let cached = self.sessions.lock().unwrap().get(addr).cloned();
        if let Some(sess) = cached {
            match sess.exchange(&wire_bytes, io_timeout).await {
                SessionExchange::Ok(bytes) => return PoolExchange::Ok(bytes),
                SessionExchange::SendBroken(e) => {
                    self.evict(addr, &sess, true);
                    return PoolExchange::Broken(e);
                }
                SessionExchange::ResponseLost(e) => {
                    self.evict(addr, &sess, true);
                    return PoolExchange::Lost(e);
                }
            }
        }

        // 池缺失：per-addr 拨号互斥（并发请求只拨一次）+ 双重检查。
        let dial_gate = {
            let mut locks = self.dial_locks.lock().unwrap();
            Arc::clone(locks.entry(addr.to_string()).or_default())
        };
        let _gate = dial_gate.lock().await;
        let raced = self.sessions.lock().unwrap().get(addr).cloned();
        let sess = if let Some(s) = raced {
            s
        } else {
            match self
                .dial_new_session(addr, dialer.as_ref(), token, dial_budget(io_timeout))
                .await
            {
                Ok(s) => {
                    self.sessions
                        .lock()
                        .unwrap()
                        .insert(addr.to_string(), Arc::clone(&s));
                    s
                }
                Err(e) => return PoolExchange::Broken(e),
            }
        };
        drop(_gate);

        match sess.exchange(&wire_bytes, io_timeout).await {
            SessionExchange::Ok(bytes) => PoolExchange::Ok(bytes),
            SessionExchange::SendBroken(e) => {
                self.evict(addr, &sess, true);
                PoolExchange::Broken(e)
            }
            SessionExchange::ResponseLost(e) => {
                self.evict(addr, &sess, true);
                PoolExchange::Lost(e)
            }
        }
    }
}

/// 请求路径拨号预算（与旧形态一致）：插件内拨号限时 10s + loopback 余量，
/// 下限 io_timeout（重 LLM 场景外层放宽时不被内层先卡死）。
fn dial_budget(io_timeout: Duration) -> Duration {
    Duration::from_secs(10).max(io_timeout) + Duration::from_secs(5)
}

/// 心跳 ping 帧（应用层 `ping` action——服务端内置 handler 回 pong）。
fn heartbeat_ping_frame() -> Vec<u8> {
    let msg = WireMessage::new_request("", "", "ping", serde_json::json!({}));
    serde_json::to_vec(&msg).unwrap_or_default()
}

#[cfg(test)]
mod tests;
