//! A1 — iroh Endpoint 装配与节点身份密钥（P0 + P1）。
//!
//! 身份模型：每节点一对持久 ed25519 密钥（iroh `SecretKey`），落盘
//! `<workspace>/cluster/roaming.key`（hex 32B seed）。NodeId = 公钥派生，
//! 传输身份自证于 QUIC-TLS 握手——连不上冒名节点（公钥即地址）。
//!
//! 通路模型：ALPN `nemesis-cluster/1`；每连接一条 bi-stream 跑现有帧协议
//! （4B 长度前缀 + AEAD 载荷，见 `transport::frame`）。ping/echo 是 P1 的
//! 通路自检（`roaming ping` CLI 与探针语义共用）。

use std::path::{Path, PathBuf};

use iroh::endpoint::{Connection, RecvStream, SendStream};
use iroh::protocol::{AcceptError, ProtocolHandler, Router};
use iroh::{Endpoint, EndpointAddr, EndpointId, SecretKey, endpoint::presets};

/// iroh 叠加层 ALPN（应用层协议协商标识；版本后缀与集群 RPC 语义对齐）。
pub const ROAMING_ALPN: &str = "nemesis-cluster/1";

/// ping/echo 探针载荷（bi-stream 直写直读；回声一致 = 通路成立）。
pub const ROAMING_PING: &[u8] = b"NBROAM1";

/// 节点密钥文件路径（`<workspace>/cluster/roaming.key`）。
pub fn roaming_key_path(workspace: &Path) -> PathBuf {
    workspace.join("cluster").join("roaming.key")
}

// ---------------------------------------------------------------------------
// EndpointId hex 编解码（`iroh:<node_id_hex>` 地址语法的一半；另一半在
// bridge 的 dial 分派）
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
// 节点身份密钥
// ---------------------------------------------------------------------------

/// 节点漫游身份（ed25519 密钥对 + 派生 EndpointId）。
pub struct RoamingIdentity {
    secret: SecretKey,
}

impl RoamingIdentity {
    /// 生成全新身份（内存态；持久化用 [`persist`]）。
    pub fn generate() -> Self {
        Self {
            secret: SecretKey::generate(),
        }
    }

    /// 从 32B seed 还原（加载 roaming.key 用）。
    pub fn from_seed(seed: &[u8; 32]) -> Self {
        Self {
            secret: SecretKey::from_bytes(seed),
        }
    }

    /// 32B seed（roaming.key 的二进制形态）。
    pub fn seed(&self) -> [u8; 32] {
        self.secret.to_bytes()
    }

    /// 本节点 iroh EndpointId（= 公钥；传输身份）。
    pub fn node_id(&self) -> EndpointId {
        self.secret.public()
    }

    /// EndpointId hex（`iroh:` 地址后缀 / 分享卡主体）。
    pub fn node_id_hex(&self) -> String {
        node_id_to_hex(&self.node_id())
    }

    /// 加载或生成：文件存在 → 读 hex seed 还原；否则生成并落盘。
    ///
    /// 落盘权限 Unix 0600（Windows 无等价语义，依赖目录 ACL）；父目录
    /// 不存在则创建。返回 (身份, 是否新生成)。
    pub fn load_or_create(workspace: &Path) -> Result<(Self, bool), String> {
        let path = roaming_key_path(workspace);
        if path.exists() {
            return Ok((Self::load(&path)?, false));
        }
        let id = Self::generate();
        persist(&id, &path)?;
        Ok((id, true))
    }

    /// 从 roaming.key 还原（hex 32B seed；非法/缺文件 loud 报错）。
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path)
            .map_err(|e| format!("读取 {} 失败: {e}", path.display()))?;
        let bytes = hex::decode(text.trim()).map_err(|e| format!("roaming.key hex 非法: {e}"))?;
        let seed: [u8; 32] = bytes
            .try_into()
            .map_err(|v: Vec<u8>| format!("roaming.key 长度 {} 字节 ≠ 32（seed）", v.len()))?;
        Ok(Self::from_seed(&seed))
    }
}

/// 持久化身份到 roaming.key（hex seed；父目录自动创建；Unix 0600）。
pub fn persist(id: &RoamingIdentity, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .map_err(|e| format!("创建 {} 失败: {e}", parent.display()))?;
    }
    std::fs::write(path, hex::encode(id.seed()))
        .map_err(|e| format!("写 {} 失败: {e}", path.display()))?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
            .map_err(|e| format!("收紧 {} 权限失败: {e}", path.display()))?;
    }
    Ok(())
}

// ---------------------------------------------------------------------------
// Endpoint 装配
// ---------------------------------------------------------------------------

/// 装配好的 iroh 端点（叠加层传输入口）。
pub struct RoamingEndpoint {
    endpoint: Endpoint,
    // Mutex 而非 Option：serve/shutdown 都走 `&self`——端点经 Arc 共享给
    // Cluster（`Cluster::stop` 关停）时无需所有权辗转。
    router: parking_lot::Mutex<Option<Router>>,
}

impl RoamingEndpoint {
    /// 绑定端点（QUIC + n0 默认中继 + pkarr DNS 寻址；accept 侧 ALPN 注册，
    /// dial 侧无需预注册）。跨网打洞的标准形态。
    ///
    /// `identity` 决定 EndpointId；`accept` = 是否接收入站连接（网关侧 true，
    /// 纯客户端场景 false）。绑定失败 loud 报错。
    pub async fn bind(identity: &RoamingIdentity, accept: bool) -> Result<Self, String> {
        Self::bind_with(identity, accept, presets::N0).await
    }

    /// 绑定端点（Minimal 预设：无中继、无 DNS 寻址——纯直连）。
    ///
    /// 受限网络（公网 relay 不可达）的诚实出口：对端地址必须以完整
    /// `EndpointAddr`（带 IP）或已有发现缓存提供。loopback 测试同用此形态。
    pub async fn bind_direct(identity: &RoamingIdentity, accept: bool) -> Result<Self, String> {
        Self::bind_with(identity, accept, presets::Minimal).await
    }

    async fn bind_with(
        identity: &RoamingIdentity,
        accept: bool,
        preset: impl iroh::endpoint::presets::Preset,
    ) -> Result<Self, String> {
        let mut builder = Endpoint::builder(preset).secret_key(identity.secret.clone());
        if accept {
            builder = builder.alpns(vec![ROAMING_ALPN.as_bytes().to_vec()]);
        }
        let endpoint = builder
            .bind()
            .await
            .map_err(|e| format!("iroh endpoint 绑定失败: {e}"))?;
        Ok(Self {
            endpoint,
            router: parking_lot::Mutex::new(None),
        })
    }

    /// 本端 EndpointId。
    pub fn node_id(&self) -> EndpointId {
        self.endpoint.id()
    }

    /// 本端 EndpointId hex（`iroh:` 地址后缀）。
    pub fn node_id_hex(&self) -> String {
        node_id_to_hex(&self.node_id())
    }

    /// 本端绑定地址（QUIC socket 实际监听的 IP:port；直连/测试用）。
    pub fn direct_addrs(&self) -> Vec<std::net::SocketAddr> {
        self.endpoint.bound_sockets()
    }

    /// accept 侧：把 ALPN 入站连接交给 `handler`（P2 帧桥接接线；P1 只挂
    /// echo 探针 handler）。
    pub async fn serve(&self, handler: RoamingProtocolHandler) -> Result<(), String> {
        let router = Router::builder(self.endpoint.clone())
            .accept(ROAMING_ALPN, handler)
            .spawn();
        *self.router.lock() = Some(router);
        Ok(())
    }

    /// dial 对端并打开帧 bi-stream（完整地址形态：node_id + 已知直连地址）。
    ///
    /// 返回的 (send, recv) 交给 bridge 的帧管线（AEAD+framing 照常叠加）。
    /// 地址带 IP 时直连（零发现依赖）；仅 node_id 时走 DNS/relay 发现。
    pub async fn connect_stream_addr(
        &self,
        addr: EndpointAddr,
    ) -> Result<(SendStream, RecvStream), String> {
        let conn: Connection = self
            .endpoint
            .connect(addr, ROAMING_ALPN.as_bytes())
            .await
            .map_err(|e| format!("iroh connect 失败: {e}"))?;
        conn.open_bi()
            .await
            .map_err(|e| format!("iroh bi-stream 打开失败: {e}"))
    }

    /// dial 对端并打开帧 bi-stream（`iroh:<hex>` 地址的连接出口；仅 node_id，
    /// 依赖发现层供址）。
    pub async fn connect_stream(
        &self,
        node_id: EndpointId,
    ) -> Result<(SendStream, RecvStream), String> {
        self.connect_stream_addr(EndpointAddr::from(node_id)).await
    }

    /// P1 通路探针（完整地址形态）：dial + ping/echo 往返，返回 RTT 毫秒。
    ///
    /// 对端必须 `serve(RoamingProtocolHandler::echo())`。语义对齐 G2 帧
    /// 级探针（真实帧往返，非 TCP connect 假成功）。
    pub async fn ping_addr(
        &self,
        addr: EndpointAddr,
        timeout: std::time::Duration,
    ) -> Result<u128, String> {
        let start = std::time::Instant::now();
        let (mut send, mut recv) = tokio::time::timeout(timeout, self.connect_stream_addr(addr))
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
                "iroh ping 回声不符（读到 {} 字节，期望 {}）",
                n,
                ROAMING_PING.len()
            ));
        }
        Ok(start.elapsed().as_millis())
    }

    /// P1 通路探针（仅 node_id 形态，依赖发现层供址）。
    pub async fn ping(
        &self,
        node_id: EndpointId,
        timeout: std::time::Duration,
    ) -> Result<u128, String> {
        self.ping_addr(EndpointAddr::from(node_id), timeout).await
    }

    /// 停机（router 收流 + endpoint 关闭；幂等——router 槽取空即无操作）。
    pub async fn shutdown(&self) {
        // 先取再 await（MutexGuard 非 Send，不能跨 await——会毒化 spawn 方）。
        let router = self.router.lock().take();
        if let Some(router) = router
            && let Err(e) = router.shutdown().await
        {
            tracing::warn!(error = %e, "iroh router shutdown 失败（best-effort 忽略）");
        }
        self.endpoint.close().await;
    }
}

/// 入站连接处理器（bridge 模式）：accept 侧把整个 `Connection` 交给调用方
/// 的任务。spawn 后立即返回是安全的——`SendStream`/`RecvStream` 各持一个
/// `ConnectionRef`（noq 引用计数），streams 活着时 accept 返回丢弃 handle
/// 不会关连接。
pub type RoamingConnHandler = std::sync::Arc<dyn Fn(Connection) + Send + Sync>;

/// ALPN accept 侧协议处理器：echo 探针（P1 通路自检）或 bridge（P2——连接
/// 整体转交上层帧管线，如 RPC server 的 `TcpConn` 接管）。
#[derive(Clone, Default)]
pub struct RoamingProtocolHandler {
    echo: bool,
    conn_handler: Option<RoamingConnHandler>,
}

// 手写 Debug（`ProtocolHandler` 要求；Arc<dyn Fn> 无 Debug，derive 不可用）。
impl std::fmt::Debug for RoamingProtocolHandler {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("RoamingProtocolHandler")
            .field("echo", &self.echo)
            .field("bridge", &self.conn_handler.is_some())
            .finish()
    }
}

impl RoamingProtocolHandler {
    /// echo 探针 handler（P1 通路自检）。
    pub fn echo() -> Self {
        Self {
            echo: true,
            conn_handler: None,
        }
    }

    /// bridge 模式：每个入站连接 spawn 转交给 `handler`（P2 帧管线接管）。
    pub fn bridge(handler: RoamingConnHandler) -> Self {
        Self {
            echo: false,
            conn_handler: Some(handler),
        }
    }
}

impl ProtocolHandler for RoamingProtocolHandler {
    async fn accept(&self, conn: Connection) -> Result<(), AcceptError> {
        if let Some(handler) = &self.conn_handler {
            // bridge：整体转交（handler 内部自行 spawn/管理生命周期）。
            // streams 持有 ConnectionRef，此处丢弃 handle 不影响存活。
            handler(conn);
            return Ok(());
        }
        if !self.echo {
            // 非 echo 非 bridge（未配置）：接受连接但拒绝之，不吞不挂——
            // 对端得到显式错误而非黑洞。
            return Err(AcceptError::from_err(std::io::Error::other(
                "roaming bridge not configured",
            )));
        }
        // echo：逐连接单 stream 收完回写（探针每次 ping 开一条 bi-stream）。
        let Ok((mut send, mut recv)) = conn.accept_bi().await else {
            return Ok(());
        };
        // 8KB 上限：探针帧个位数字节，超限=异常流量，截断不放大。
        let mut buf = vec![0u8; 8192];
        match recv.read(&mut buf).await {
            Ok(Some(n)) => {
                send.write_all(&buf[..n])
                    .await
                    .map_err(|e| AcceptError::from_err(std::io::Error::other(e.to_string())))?;
                send.finish()
                    .map_err(|e| AcceptError::from_err(std::io::Error::other(e.to_string())))?;
                // accept() 返回即 drop 连接——等对端 ack 全部回声数据再返回，
                // 否则 client 读回声与连接关闭竞争（实录：connection lost）。
                let _ = send.stopped().await;
            }
            _ => return Ok(()),
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests;
