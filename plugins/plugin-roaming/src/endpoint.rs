//! 客户端连接面（M3）：iroh Endpoint 承载整体搬插件——dial 臂（宿主
//! `roaming_dial` 消费）与 accept 臂（插件 serve → `notify_incoming`）共用
//! 一个端点。本模块是项目内**唯一 iroh 客户端依赖点**（插件 standalone
//! project，iroh 不进宿主二进制——体积纪律）。
//!
//! 语义与原宿主 `nemesis-cluster/src/roaming/endpoint.rs` 逐行对齐（迁移即
//! 平移，对端字节兼容）：
//!
//! - **身份**：roaming.key 由 `identity.rs` 唯一读写；本模块经 32B seed 桥接
//!   iroh（`iroh::SecretKey::from_bytes`）。secret 不出插件（M3 收缩 M2 的
//!   过渡面——`identity_payload` 不再带 `secret_key_hex`）。
//! - **通路模型**：ALPN `nemesis-cluster/1`；一条 bi-stream = 一次帧交换
//!   会话（4B 长度前缀 + AEAD 载荷——帧协议在宿主，插件是协议无关字节泵）。
//! - **交接（拍板 9 流粒度）**：dial 臂开一次性 loopback TCP 监听回
//!   `{port, conn_id}`，宿主 `TcpStream::connect` 后帧协议照跑；accept 臂对
//!   每条入站 bi-stream 开一次性 loopback TCP 并 `notify_incoming(conn_id,
//!   port, remote_hex)` 通知宿主受理。TCP EOF = 终止信号（splice 收尾）。
//! - **trust 闸先行**：统一 accept handler 在读首字节**之前**按 trust.toml
//!   （存在即启用）裁决——per-connection 重读文件（编辑即生效），revoked
//!   一票否决 → trusted 放行 → 默认拒；读/解析失败 fail-closed（安全闸不
//!   放行于损坏）。PING 探针也受闸（比旧宿主 echo 更严格且正确——探针与
//!   数据面同一道门）。
//! - **PING 判别**：首 7 字节 == `NBROAM1` = 探针回显；否则 7 字节先转发进
//!   loopback 再双向泵。RPC 帧首 4B BE 长度前缀 ≈1.3e9 不可能撞 ASCII
//!   `NBROAM1`（判别无歧义）。
//! - **拨号路径观测**：每次 dial spawn 观察任务（`Connection::paths` 快照
//!   周期写槽），宿主 status 快照批量读——Dashboard「打洞直连 / 中继转发」
//!   可观测语义原样保留。

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;

use iroh::endpoint::{Connection, ReadError, RecvStream, RelayMode, SendStream, VarInt};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{
    Endpoint, EndpointAddr, EndpointId, RelayConfig, RelayMap, RelayUrl, SecretKey,
    endpoint::presets,
};

use crate::plugin_log;
use crate::roaming::{ROAMING_LOG_DEBUG, ROAMING_LOG_INFO, ROAMING_LOG_WARN};

/// iroh 叠加层 ALPN（与原宿主实现同值——对端字节兼容）。
pub const ROAMING_ALPN: &str = "nemesis-cluster/1";

/// ping/echo 探针载荷（bi-stream 直写直读；回声一致 = 通路成立）。
pub const ROAMING_PING: &[u8] = b"NBROAM1";

/// iroh 地址前缀（`iroh:<64位小写hex>`；宿主 lib.rs `IROH_ADDR_PREFIX` 同值）。
const IROH_ADDR_PREFIX: &str = "iroh:";

/// 宿主受理 loopback TCP 的等待上限（宿主收到 notify 后立即 connect；超限
/// = 宿主侧异常/acceptor 未挂，诚实弃流）。
const HOST_ACCEPT_TIMEOUT: Duration = Duration::from_secs(10);

/// 拨号（connect + open_bi）墙钟上限（对齐旧宿主 dial 臂 10s 档）。
const DIAL_TIMEOUT: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// EndpointId hex 编解码（`iroh:<node_id_hex>` 地址语法的一半；另一半在
// parse_roaming_address）
// ---------------------------------------------------------------------------

/// EndpointId → 64 字符小写 hex（地址串 `iroh:` 后缀形态）。
pub fn node_id_to_hex(node_id: &EndpointId) -> String {
    hex::encode(node_id.as_bytes())
}

/// 64 字符 hex → EndpointId（大小写不敏感；长度/字符非法 loud 报错）。
pub fn node_id_from_hex(s: &str) -> Result<EndpointId, String> {
    let bytes = hex::decode(s.trim())
        .map_err(|e| format!("node_id hex 非法（应为 64 位十六进制）: {e}"))?;
    let arr: [u8; 32] = bytes
        .try_into()
        .map_err(|v: Vec<u8>| format!("node_id 长度 {} 字节 ≠ 32", v.len()))?;
    EndpointId::from_bytes(&arr).map_err(|e| format!("node_id 非法: {e}"))
}

// ---------------------------------------------------------------------------
// 地址解析（`iroh:<hex>[@<socket|relay_url>]` 三形态）
// ---------------------------------------------------------------------------

/// `iroh:<hex>[@<socket|relay_url>]` → [`iroh::EndpointAddr`]（剥前缀 + hex
/// 解码；非法 loud 报错）。
///
/// 三种形态（自宿主 bridge.rs 平移）：
/// - `iroh:<hex>`：仅 node_id——依赖发现层供址（N0 预设的 DNS/relay）。
/// - `iroh:<hex>@<ip:port>`：钉直连地址——Minimal 预设 / 受限网络 / loopback
///   测试的诚实出口（`with_ip_addr`）。
/// - `iroh:<hex>@http(s)://host:port`：钉中继回落地址——自建中继形态
///   （`with_relay_url`；rendezvous `member_dial_address` 产出此形态）。
pub fn parse_roaming_address(addr: &str) -> Result<EndpointAddr, String> {
    let s = addr.trim();
    let rest = s
        .strip_prefix(IROH_ADDR_PREFIX)
        .ok_or_else(|| format!("非 iroh 形态地址（缺少 `{IROH_ADDR_PREFIX}` 前缀）: {addr}"))?;
    let (hex_part, socket_part) = match rest.split_once('@') {
        Some((h, sock)) => {
            if sock.trim().is_empty() {
                return Err(format!("iroh 地址 `@` 后缺地址: {addr}"));
            }
            (h, Some(sock.trim()))
        }
        None => (rest, None),
    };
    let node_id = node_id_from_hex(hex_part)?;
    let base = EndpointAddr::from(node_id);
    match socket_part {
        Some(sock) if sock.contains("://") => {
            // 中继回落形态（http:// / https://）——`with_relay_url`。
            let url: RelayUrl = sock.parse().map_err(|e| {
                format!("iroh 中继地址非法（应为 http(s)://host:port）: {sock}: {e}")
            })?;
            Ok(base.with_relay_url(url))
        }
        Some(sock) => {
            let sa: SocketAddr = sock
                .parse()
                .map_err(|e| format!("iroh 直连地址非法（应为 ip:port）: {sock}: {e}"))?;
            Ok(base.with_ip_addr(sa))
        }
        None => Ok(base),
    }
}

// ---------------------------------------------------------------------------
// trust 闸（trust.toml 语义镜像宿主 TrustBook——同文件同格式同裁决）
// ---------------------------------------------------------------------------

/// trust.toml 数据形态（与宿主 `TrustBookData` 同构：`trusted = [...]` /
/// `revoked = [...]`，serde default 容忍缺段）。trust 导出面（lib.rs）与
/// accept 闸共用同一读写。
#[derive(Debug, Default, serde::Deserialize, serde::Serialize)]
pub(crate) struct TrustBookData {
    #[serde(default)]
    pub(crate) trusted: Vec<String>,
    #[serde(default)]
    pub(crate) revoked: Vec<String>,
}

/// trust.toml 路径（`<workspace>/cluster/trust.toml`；与宿主旧 TrustBook
/// 同一物理文件——迁移零成本）。
pub(crate) fn trust_book_path(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("trust.toml")
}

/// 读信任簿：文件不存在 = 空簿（闸未启用语义由 trust_allows 的 exists 判定
/// 承担——空簿写入即启用闸）；读/解析失败 = Err（fail-closed 调用方裁决）。
pub(crate) fn load_trust_book(workspace: &Path) -> Result<TrustBookData, String> {
    let path = trust_book_path(workspace);
    if !path.exists() {
        return Ok(TrustBookData::default());
    }
    let text =
        std::fs::read_to_string(&path).map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
    toml::from_str(&text).map_err(|e| format!("trust.toml 解析失败: {e}"))
}

/// 写信任簿（`<workspace>/cluster/` 目录不存在则先建）。
pub(crate) fn save_trust_book(workspace: &Path, data: &TrustBookData) -> Result<(), String> {
    let path = trust_book_path(workspace);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    let text = toml::to_string_pretty(data).map_err(|e| format!("trust.toml 序列化失败: {e}"))?;
    std::fs::write(&path, text).map_err(|e| format!("写入 {} 失败: {e}", path.display()))
}

/// 身份归一（trim + 剥 `iroh:` 前缀 + 小写——镜像宿主 normalize_id）。
pub(crate) fn normalize_id(s: &str) -> String {
    s.trim().trim_start_matches(IROH_ADDR_PREFIX).to_lowercase()
}

/// trust 闸裁决（trust.toml 存在即启用；文件缺失 = 未启用放行——token 单层
/// 语义）。per-connection 重读（编辑即生效）；revoked 一票否决 → trusted
/// 放行 → 默认拒；**读/解析失败 fail-closed 拒绝**（安全闸不放行于损坏）。
fn trust_allows(workspace: &Path, remote_hex: &str) -> bool {
    if !trust_book_path(workspace).exists() {
        return true;
    }
    match load_trust_book(workspace) {
        Ok(data) => {
            let id = normalize_id(remote_hex);
            !data.revoked.iter().any(|s| normalize_id(s) == id)
                && data.trusted.iter().any(|s| normalize_id(s) == id)
        }
        Err(e) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("trust.toml 读取失败 fail-closed 拒绝 iroh:{remote_hex}: {e}"),
            );
            false
        }
    }
}

// ---------------------------------------------------------------------------
// 连接注册表（conn_id → Connection；close_conn 强关 + splice 收尾自摘）
// ---------------------------------------------------------------------------

/// 连接注册表：`conn_id`（AtomicU64 计数）→ `Connection`（Clone 句柄）。
///
/// **粒度文档化**：conn_id 按流分配（dial 一次 = 一条连接一条 id；accept 每
/// 条 bi-stream 一个 id），但一条 QUIC 连接上的多条流共享底层连接——
/// [`Self::close_conn`] 强关的是**整条连接**（该连接上的所有流一起死）。这是
/// 半开兜底语义的合理粒度：宿主帧层判死一条流，同连接其他流多半也已是僵尸。
pub struct ConnsRegistry {
    next: AtomicU64,
    map: parking_lot::Mutex<HashMap<u64, Connection>>,
}

impl ConnsRegistry {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(1),
            map: parking_lot::Mutex::new(HashMap::new()),
        }
    }

    /// 注册连接，返回新 conn_id（从 1 起，0 恒不分配）。
    fn insert(&self, conn: Connection) -> u64 {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        self.map.lock().insert(id, conn);
        id
    }

    /// splice 收尾自摘（连接自然结束——非错误路径，静默）。
    fn remove(&self, id: u64) {
        self.map.lock().remove(&id);
    }

    /// 按连接 id 显式关闭（半开兜底；宿主 `roaming_close_conn` 消费）。
    /// 不存在的 id = 幂等 ok:false（`closed:false`）。
    pub fn close_conn(&self, id: u64) -> bool {
        match self.map.lock().remove(&id) {
            Some(conn) => {
                // error_code 0 = 应用层正常关闭（语义：宿主主动断，非协议错）。
                conn.close(VarInt::from(u32::from(0u8)), b"close_conn");
                true
            }
            None => false,
        }
    }

    /// 端点停机收尾：强关全部注册连接（shutdown 链——不留悬挂 splice）。
    fn close_all(&self) {
        let mut map = self.map.lock();
        for (_, conn) in map.drain() {
            conn.close(VarInt::from(u32::from(0u8)), b"endpoint_stop");
        }
    }
}

// ---------------------------------------------------------------------------
// BiStream 组合器（QUIC 收/发两独立类型 → 单一 AsyncRead + AsyncWrite）
// ---------------------------------------------------------------------------

/// iroh bi-stream 组合器（自宿主 bridge.rs `IrohBiStream` 平移）：本插件的
/// 双向泵（`copy_bidirectional`）吃单一 `AsyncRead + AsyncWrite` 流。
struct BiStream {
    send: SendStream,
    recv: RecvStream,
}

impl tokio::io::AsyncRead for BiStream {
    fn poll_read(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &mut tokio::io::ReadBuf<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        std::pin::Pin::new(&mut self.recv).poll_read(cx, buf)
    }
}

impl tokio::io::AsyncWrite for BiStream {
    fn poll_write(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
        buf: &[u8],
    ) -> std::task::Poll<std::io::Result<usize>> {
        // SendStream 的固有 poll_write 返回自家 WriteError（Rust 方法解析固有
        // 优先于 trait）——与 noq 自身 AsyncWrite impl 同款 `Into::into` 转发。
        std::pin::Pin::new(&mut self.send)
            .poll_write(cx, buf)
            .map(|r| r.map_err(std::convert::Into::into))
    }

    fn poll_flush(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // QUIC 流没有 flush 概念（写即发）。
        std::task::Poll::Ready(Ok(()))
    }

    fn poll_shutdown(
        mut self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<std::io::Result<()>> {
        // 终止信号：本端不再发数据，对端读到 EOF——与 TCP shutdown 语义对齐
        // （宿主帧层 TcpConn 写循环收尾 → shutdown → 这里 finish）。
        let _ = self.send.finish();
        std::task::Poll::Ready(Ok(()))
    }
}

// ---------------------------------------------------------------------------
// 端点句柄（endpoint_start 产物）
// ---------------------------------------------------------------------------

/// endpoint_start 配置（宿主传 JSON）。`relay_url` 空 = n0 公共中继（N0
/// 预设）；非空 = Minimal 预设 + 自建中继 RelayMap + 可选 auth token（照搬
/// 宿主 `bind_custom_relay` 语义：Minimal 基座是刻意的——自建中继服务于 n0
/// 不可达的受限网络，DNS/pkarr 发现在此形同虚设）。`idle_timeout_secs` 为
/// 保留位（宿主侧 TcpConn 空闲监测语义的镜像声明，插件不消费——空闲闸在
/// 宿主帧层照常生效）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct EndpointConfig {
    #[serde(default)]
    pub relay_url: String,
    #[serde(default)]
    pub relay_token: String,
    /// 保留位（宿主侧 TcpConn 空闲监测语义的镜像声明，插件不消费——空闲闸
    /// 在宿主帧层照常生效；反序列化目标必须存在否则宿主 config 含此键即解析失败）。
    #[allow(dead_code)]
    #[serde(default)]
    pub idle_timeout_secs: u64,
}

/// 装配好的 iroh 端点 + 插件侧连接注册表（endpoint_start 产物；经
/// `Arc` 共享——dial/close_conn/accept 多路并发访问）。
pub struct EndpointHandle {
    endpoint: Endpoint,
    conns: Arc<ConnsRegistry>,
    /// 最近拨号路径观测（node_id hex → "direct"/"relay"/"unknown"）。
    ///
    /// 每次拨号成功 spawn 一个观察任务：周期读 `Connection::paths` 快照写入
    /// 此表（打洞成功出现 direct 路径后 10s 早退；120s 上限防长命连接挂
    /// 任务；连接关闭即退）。**退出不清槽**：槽语义是「最近一次拨号的路径
    /// 观测」（探针连接用完即关是常态，清掉会让查询恒撞空窗）。
    dial_paths: Arc<parking_lot::Mutex<HashMap<String, String>>>,
    // Mutex 而非 Option：serve/shutdown 都走 `&self`（MutexGuard 非 Send，
    // 不能跨 await——先 take 再 await）。
    router: parking_lot::Mutex<Option<Router>>,
}

impl EndpointHandle {
    /// 本端 EndpointId（= 公钥；传输身份）。
    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// 本端 EndpointId hex（`iroh:` 地址后缀 / endpoint_start 回执）。
    pub fn node_id_hex(&self) -> String {
        node_id_to_hex(&self.node_id())
    }

    /// 本端绑定地址（QUIC socket 实际监听的 IP:port；直连/测试用）。
    pub fn direct_addrs(&self) -> Vec<SocketAddr> {
        self.endpoint.bound_sockets()
    }

    /// 最近一次观测的拨号路径（`"direct"` = 打洞直连，`"relay"` = 经中继
    /// 转发，`None` = 无观测）。批量读入口（宿主 status/nodes.list 消费）。
    pub fn dial_paths_snapshot(&self) -> HashMap<String, String> {
        self.dial_paths.lock().clone()
    }

    /// 按连接 id 显式关闭（半开兜底；`roaming_close_conn` 导出消费）。
    /// 不存在 = 幂等 false。注意粒度：conn_id 按流分配但 close 关整条
    /// QUIC 连接（同连接多流同亡——见 `ConnsRegistry` 粒度注释）。
    pub fn close_conn(&self, id: u64) -> bool {
        self.conns.close_conn(id)
    }

    /// accept 侧：把 ALPN 入站连接交给统一 handler（Router 常驻）。
    /// 重复 serve = 停旧 Router 起新（endpoint_start 热更路径先 stop 再
    /// start，此处防御兜底）。模块内唯一调用者是 [`spawn_endpoint`]。
    async fn serve(&self, handler: BridgeAcceptor) -> Result<(), String> {
        let router = Router::builder(self.endpoint.clone())
            .accept(ROAMING_ALPN, handler)
            .spawn();
        *self.router.lock() = Some(router);
        Ok(())
    }

    /// 连接对端并打开 bi-stream（返回 Connection 供注册表强关用——dial 一次
    /// 连接一条，宿主 close_conn(conn_id) 强关整条）。
    async fn connect_stream_addr(
        &self,
        addr: EndpointAddr,
    ) -> Result<(Connection, SendStream, RecvStream), String> {
        let peer = addr.id; // Copy：先取出，connect(addr) 消费整个 addr
        let conn: Connection = self
            .endpoint
            .connect(addr, ROAMING_ALPN.as_bytes())
            .await
            .map_err(|e| format!("iroh connect 失败: {e}"))?;
        self.spawn_path_watcher(&peer, &conn);
        let (send, recv) = conn
            .open_bi()
            .await
            .map_err(|e| format!("iroh bi-stream 打开失败: {e}"))?;
        Ok((conn, send, recv))
    }

    /// dial 臂：连接对端 + 开一次性 loopback TCP 监听，回 `(port, conn_id)`。
    ///
    /// 宿主拿到端口后 `TcpStream::connect(127.0.0.1:port)`，帧协议照跑；TCP
    /// EOF = 终止信号（splice 任务收尾并摘注册表）。宿主超时未拨入 = 连接
    /// 强关 + 摘注册（半开兜底）。
    pub async fn dial(self: &Arc<Self>, addr: &str) -> Result<(u16, u64), String> {
        let target = parse_roaming_address(addr)?;
        let (conn, send, recv) =
            match tokio::time::timeout(DIAL_TIMEOUT, self.connect_stream_addr(target)).await {
                Ok(r) => r?,
                Err(_) => return Err(format!("iroh dial 超时（{DIAL_TIMEOUT:?}）: {addr}")),
            };
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .map_err(|e| format!("loopback 监听绑定失败: {e}"))?;
        let port = listener
            .local_addr()
            .map_err(|e| format!("loopback 监听地址读取失败: {e}"))?
            .port();
        let conn_id = self.conns.insert(conn.clone());
        tokio::task::spawn(splice_dial(
            self.conns.clone(),
            conn,
            conn_id,
            BiStream { send, recv },
            listener,
        ));
        Ok((port, conn_id))
    }

    /// P1 通路探针：dial + ping/echo 往返，返回 RTT 毫秒。对端统一 handler
    /// 的 PING 分支回显（trust 闸同样适用——探针不是旁路）。生产通路验证由
    /// 宿主 dial 臂的帧层承担；本方法是 iroh 层探针（e2e 测试消费，未来
    /// 诊断导出可用）。
    #[allow(dead_code)]
    pub async fn ping_addr(&self, addr: EndpointAddr, timeout: Duration) -> Result<u128, String> {
        let start = std::time::Instant::now();
        let (_, mut send, mut recv) = tokio::time::timeout(timeout, self.connect_stream_addr(addr))
            .await
            .map_err(|_| "iroh ping 超时（连接阶段）".to_string())??;
        send.write_all(ROAMING_PING)
            .await
            .map_err(|e| format!("iroh ping 写失败: {e}"))?;
        send.finish()
            .map_err(|e| format!("iroh ping finish 失败: {e}"))?;
        let mut buf = vec![0u8; ROAMING_PING.len() + 16];
        let n = tokio::time::timeout(timeout, recv.read(&mut buf))
            .await
            .map_err(|_| "iroh ping 超时（读阶段）".to_string())?
            .map_err(|e| format!("iroh ping 读失败: {e}"))?
            .unwrap_or(0);
        if &buf[..n] != ROAMING_PING {
            return Err(format!(
                "iroh ping 回声不符（读到 {n} 字节，期望 {}）",
                ROAMING_PING.len()
            ));
        }
        Ok(start.elapsed().as_millis())
    }

    /// 停机（router 收流 + 注册连接强关 + endpoint 关闭；幂等——router 槽
    /// 取空即无操作）。
    pub async fn shutdown(&self) {
        // 先取再 await（MutexGuard 非 Send，不能跨 await）。
        let router = self.router.lock().take();
        if let Some(router) = router
            && let Err(e) = router.shutdown().await
        {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("iroh router shutdown 失败（best-effort 忽略）: {e}"),
            );
        }
        self.conns.close_all();
        self.endpoint.close().await;
    }

    /// 拨号成功后挂路径观察任务（fire-and-forget；自宿主平移——iroh 语义：
    /// 连接通常先经 relay 打通，holepunch 成功后出现 direct 路径）。
    fn spawn_path_watcher(&self, peer: &EndpointId, conn: &Connection) {
        let key = node_id_to_hex(peer);
        let conn = conn.clone();
        let slot = self.dial_paths.clone();
        tokio::spawn(async move {
            let mut direct_since: Option<tokio::time::Instant> = None;
            let deadline = tokio::time::Instant::now() + Duration::from_secs(120);
            loop {
                let kind = classify_paths(&conn.paths());
                *slot.lock().entry(key.clone()).or_default() = kind.to_string();
                match kind {
                    "direct" if direct_since.is_none() => {
                        direct_since = Some(tokio::time::Instant::now());
                    }
                    "direct"
                        if direct_since.is_some_and(|t| t.elapsed() >= Duration::from_secs(10)) =>
                    {
                        break; // direct 稳定 10s：停止追踪（结论留在槽里）
                    }
                    _ => {}
                }
                let tick = tokio::time::sleep(Duration::from_secs(2));
                tokio::select! {
                    _ = tick => {}
                    _ = conn.closed() => break, // 连接关闭：最后一次快照已写入
                }
                if tokio::time::Instant::now() >= deadline {
                    break;
                }
            }
        });
    }
}

/// 路径快照分类：任一直连 IP 路径 = "direct"（打洞成功才出现 direct 路径）；
/// 否则任一中继路径 = "relay"；空快照 = "unknown"（连接刚建立路径尚未协商）。
fn classify_paths(list: &iroh::endpoint::PathList<'_>) -> &'static str {
    if list.iter().any(|p| p.is_ip()) {
        "direct"
    } else if list.iter().any(|p| p.is_relay()) {
        "relay"
    } else {
        "unknown"
    }
}

// ---------------------------------------------------------------------------
// 统一 accept handler（trust 闸 → PING 判别 → loopback 交接）
// ---------------------------------------------------------------------------

/// ALPN accept 侧统一协议处理器（M3）：trust 闸先行 → 逐流 accept_bi →
/// per-stream spawn 判别/交接。
struct BridgeAcceptor {
    workspace: PathBuf,
    conns: Arc<ConnsRegistry>,
}

// ProtocolHandler 要求 Debug；字段皆可 derive——但保持与宿主同款手写姿态的
// 简单性，直接 derive（无不可 Debug 字段）。
impl std::fmt::Debug for BridgeAcceptor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("BridgeAcceptor")
            .field("workspace", &self.workspace)
            .finish_non_exhaustive()
    }
}

impl ProtocolHandler for BridgeAcceptor {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        let remote_hex = node_id_to_hex(&conn.remote_id());
        // trust 闸先行（读首字节之前；trust.toml 存在即查——PING 也受闸）。
        if !trust_allows(&self.workspace, &remote_hex) {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("iroh 连接被信任簿拒绝（未列入 trusted 或已吊销）: iroh:{remote_hex}"),
            );
            // 丢 handle = 连接关闭（对端显式失败非黑洞——拒绝信息不外泄，
            // 与宿主旧行为一致：拒绝原因不回对端）。
            return Ok(());
        }
        loop {
            match conn.accept_bi().await {
                Ok((send, recv)) => {
                    let conns = self.conns.clone();
                    let remote = remote_hex.clone();
                    let conn_handle = conn.clone();
                    tokio::task::spawn(handle_inbound_stream(
                        conns,
                        remote,
                        conn_handle,
                        send,
                        recv,
                    ));
                }
                Err(e) => {
                    // 连接结束（对端关闭/协议错）——常规退出路径，debug 级。
                    plugin_log(
                        ROAMING_LOG_DEBUG,
                        "endpoint",
                        &format!("iroh 连接流循环退出 ({remote_hex}): {e}"),
                    );
                    break;
                }
            }
        }
        Ok(())
    }
}

/// 读满 `buf.len()` 或 EOF（QUIC 字节流 `read()` 可能返回不足——必须循环）。
/// 返回实读字节数（0 = 首读即 EOF）。
async fn read_up_to(recv: &mut RecvStream, buf: &mut [u8]) -> Result<usize, ReadError> {
    let mut filled = 0;
    while filled < buf.len() {
        match recv.read(&mut buf[filled..]).await {
            Ok(Some(0)) => break,
            Ok(Some(n)) => filled += n,
            Ok(None) => break, // 流已结束（对端 finish）
            Err(e) => return Err(e),
        }
    }
    Ok(filled)
}

/// 单条入站 bi-stream 的判别与交接：
/// 1. 读首 7 字节判 PING（不足 7 = 短包/EOF：按帧流转发，帧层诚实报文法错）；
/// 2. PING = 探针回显（写回 + finish + 等对端 ack——accept() 返回即 drop 连接，
///    与旧宿主 echo 同款「等 stopped 防 connection lost 竞争」实录教训）；
/// 3. 非 PING = 开一次性 loopback 端口 + `notify_incoming` + 等宿主拨入 +
///    首 7 字节转进 TCP + 双向泵（splice）。
async fn handle_inbound_stream(
    conns: Arc<ConnsRegistry>,
    remote_hex: String,
    conn: Connection,
    mut send: SendStream,
    mut recv: RecvStream,
) {
    let mut head = [0u8; ROAMING_PING.len()];
    let n = match read_up_to(&mut recv, &mut head).await {
        Ok(n) => n,
        Err(e) => {
            plugin_log(
                ROAMING_LOG_DEBUG,
                "endpoint",
                &format!("iroh 入站流读首字节失败 ({remote_hex}): {e}"),
            );
            return;
        }
    };
    if n == 0 {
        return; // 首读即 EOF：空流，无事可做
    }
    if n == ROAMING_PING.len() && head[..n] == *ROAMING_PING {
        // 探针回显（trust 闸已在 accept() 先行——此处无条件回声）。
        if send.write_all(&head[..n]).await.is_ok() {
            let _ = send.finish();
            let _ = send.stopped().await;
        }
        return;
    }
    // 非 PING：开一次性 loopback 端口，通知宿主受理。
    let listener = match tokio::net::TcpListener::bind("127.0.0.1:0").await {
        Ok(l) => l,
        Err(e) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("loopback 监听绑定失败 ({remote_hex}): {e}"),
            );
            return;
        }
    };
    let port = match listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("loopback 监听地址读取失败 ({remote_hex}): {e}"),
            );
            return;
        }
    };
    let conn_id = conns.insert(conn);
    notify_incoming(conn_id, port, &remote_hex);
    let tcp = match tokio::time::timeout(HOST_ACCEPT_TIMEOUT, listener.accept()).await {
        Ok(Ok((tcp, _))) => tcp,
        Ok(Err(e)) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("宿主 loopback 拨入失败 conn_id={conn_id}: {e}"),
            );
            conns.remove(conn_id);
            return;
        }
        Err(_) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!(
                    "宿主 loopback 拨入超时（{HOST_ACCEPT_TIMEOUT:?}）conn_id={conn_id}——acceptor 未挂载或宿主异常"
                ),
            );
            conns.remove(conn_id);
            return;
        }
    };
    let mut tcp = tcp;
    // 首 7 字节先转进 loopback（已消费不能丢——RPC 帧头的头 4 字节就在里面）。
    if let Err(e) = tokio::io::AsyncWriteExt::write_all(&mut tcp, &head[..n]).await {
        plugin_log(
            ROAMING_LOG_WARN,
            "endpoint",
            &format!("首字节转写 loopback 失败 conn_id={conn_id}: {e}"),
        );
        conns.remove(conn_id);
        return;
    }
    // 双向泵：TCP EOF → BiStream::poll_shutdown → send.finish()（终止信号）；
    // iroh EOF → tcp 写半 shutdown → 宿主 reader 见 EOF。错误收尾 = debug
    // （对端/网络常规断连不是告警面）。
    let mut bi = BiStream { send, recv };
    match tokio::io::copy_bidirectional(&mut bi, &mut tcp).await {
        Ok((up, down)) => plugin_log(
            ROAMING_LOG_DEBUG,
            "endpoint",
            &format!("入站 splice 收尾 conn_id={conn_id} ({remote_hex}) up={up} down={down}"),
        ),
        Err(e) => plugin_log(
            ROAMING_LOG_DEBUG,
            "endpoint",
            &format!("入站 splice 错误收尾 conn_id={conn_id} ({remote_hex}): {e}"),
        ),
    }
    conns.remove(conn_id);
}

/// notify_incoming 转发（vtable；宿主缺席/字符串含 NUL = 静默弃——宿主拨入
/// 超时兜底已覆盖此路径的清理）。
fn notify_incoming(conn_id: u64, port: u16, remote_hex: &str) {
    if let Some(vt) = crate::HOST.get()
        && let Some(f) = vt.notify_incoming
        && let Ok(hex_c) = std::ffi::CString::new(remote_hex)
    {
        f(conn_id, port, hex_c.as_ptr());
    }
}

/// dial 臂 splice：等宿主 loopback 拨入 → 双向泵 → 摘注册。宿主超时未拨入 =
/// 连接强关（不留半开）。
async fn splice_dial(
    conns: Arc<ConnsRegistry>,
    conn: Connection,
    conn_id: u64,
    mut bi: BiStream,
    listener: tokio::net::TcpListener,
) {
    let tcp = match tokio::time::timeout(HOST_ACCEPT_TIMEOUT, listener.accept()).await {
        Ok(Ok((tcp, _))) => tcp,
        Ok(Err(e)) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!("宿主 loopback 拨入失败 conn_id={conn_id}: {e}"),
            );
            conn.close(VarInt::from(u32::from(0u8)), b"dial handoff failed");
            conns.remove(conn_id);
            return;
        }
        Err(_) => {
            plugin_log(
                ROAMING_LOG_WARN,
                "endpoint",
                &format!(
                    "宿主 loopback 拨入超时（{HOST_ACCEPT_TIMEOUT:?}）conn_id={conn_id}——连接强关"
                ),
            );
            conn.close(VarInt::from(u32::from(0u8)), b"host dial timeout");
            conns.remove(conn_id);
            return;
        }
    };
    let mut tcp = tcp;
    match tokio::io::copy_bidirectional(&mut bi, &mut tcp).await {
        Ok((up, down)) => plugin_log(
            ROAMING_LOG_DEBUG,
            "endpoint",
            &format!("dial splice 收尾 conn_id={conn_id} up={up} down={down}"),
        ),
        Err(e) => plugin_log(
            ROAMING_LOG_DEBUG,
            "endpoint",
            &format!("dial splice 错误收尾 conn_id={conn_id}: {e}"),
        ),
    }
    conns.remove(conn_id);
}

// ---------------------------------------------------------------------------
// spawn（endpoint_start 消费）
// ---------------------------------------------------------------------------

/// 拉起客户端连接面端点（endpoint_start；幂等热更由调用方先 stop 旧句柄）。
/// accept 恒 true：dial 臂与 accept 臂共用一个端点（身份一致——纯 dial 节点
/// 也能被 ping/连接，语义与旧宿主 setup_roaming 的 accept=true 全同）。
pub async fn spawn_endpoint(
    cfg: &EndpointConfig,
    seed: &[u8; 32],
    workspace: &std::path::Path,
) -> Result<Arc<EndpointHandle>, String> {
    // 身份桥接：插件 ed25519 seed → iroh SecretKey（同数学，iroh 端点吃
    // 自家类型）。
    let secret = SecretKey::from_bytes(seed);
    let mut builder = Endpoint::builder(presets::N0).secret_key(secret);
    let relay_url = cfg.relay_url.trim();
    if !relay_url.is_empty() {
        // 自建中继回落形态（Minimal 基座 + RelayMode::Custom——照搬宿主
        // bind_custom_relay；quic=None 显式关客户端 QUIC relay transport，
        // `From<RelayUrl>` 缺省会带上默认 QUIC 配置——零证书负担走 HTTP）。
        let url: RelayUrl = relay_url
            .parse()
            .map_err(|e| format!("relay_url 非法（{relay_url:?}）: {e}"))?;
        let mut relay = RelayConfig::new(url, None);
        let token = cfg.relay_token.trim();
        if !token.is_empty() {
            relay = relay.with_auth_token(token);
        }
        let map = RelayMap::from_iter([relay]);
        builder = builder
            .relay_mode(RelayMode::Custom(map))
            .preset(presets::Minimal);
    }
    // accept 侧 ALPN 注册（dial 侧无需预注册）。
    builder = builder.alpns(vec![ROAMING_ALPN.as_bytes().to_vec()]);
    let endpoint = builder
        .bind()
        .await
        .map_err(|e| format!("iroh endpoint 绑定失败: {e}"))?;
    let handle = Arc::new(EndpointHandle {
        endpoint,
        conns: Arc::new(ConnsRegistry::new()),
        dial_paths: Arc::new(parking_lot::Mutex::new(HashMap::new())),
        router: parking_lot::Mutex::new(None),
    });
    handle
        .serve(BridgeAcceptor {
            workspace: workspace.to_path_buf(),
            conns: handle.conns.clone(),
        })
        .await?;
    plugin_log(
        ROAMING_LOG_INFO,
        "endpoint",
        &format!(
            "iroh 端点已启动 node_id={} relay={}",
            handle.node_id_hex(),
            if relay_url.is_empty() {
                "n0"
            } else {
                relay_url
            }
        ),
    );
    Ok(handle)
}
