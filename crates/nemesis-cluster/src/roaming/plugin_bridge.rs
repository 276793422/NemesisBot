//! 宿主侧 C-ABI 桥（QUIC 插件化 M2，拍板 6「导出一个对象，对象里面导出不同的
//! 子对象」的宿主消费面）：nemesis-cluster / nemesis-web / nemesisbot 统一经
//! 本桥调 plugin_roaming，不摸 dlopen / 句柄细节。
//!
//! 分层与方向：
//! - dlopen 与 vtable 装配住在 nemesisbot loader（`roaming_plugin_loader`）——
//!   loader 装载成功即把 `&'static RoamingPluginHandle` 注入本桥（gateway
//!   「文件在场即加载」→ 桥槽恒有插件；CLI 进程需先显式装载再走桥）；
//! - 本桥不依赖 nemesisbot（依赖方向 nemesisbot → nemesis-cluster），所以
//!   ok:false 错误展开与 loader `call_roaming` 同文案**两处镜像**（改文案两边同步）；
//! - 名册循环（拍板 7）的 register/members HTTP 往返在插件内跑，宿主只经
//!   [`set_members_sink`] 挂成员回流漏斗、经 [`registry_start`]/
//!   [`registry_stop_fire_and_forget`] 控循环启停；
//! - M3 客户端连接面：拨号 [`dial`]/[`close_conn`]、端点启停
//!   [`endpoint_start`]/[`endpoint_stop`]、快照 [`status_snapshot`]；入站
//!   交接经 [`set_incoming_sink`] 挂 server 臂受理漏斗。rpc/client 拨号臂
//!   面向 [`RoamingDialer`] 抽象（生产 [`BridgeDialer`]，测试注入假插件）。
//!
//! 线程模型：[`dispatch_members_json`] 在插件循环线程（vtable `notify_members`
//! 反向调用）同步执行，成员漏斗必须是纯内存入册（集群注册表 parking_lot 锁
//! 跨线程安全，无回呼插件，无死锁面）；其余桥调用走 `spawn_blocking`（插件
//! FFI 自持 runtime block_on，不能在 tokio worker 上同步等）。

use std::sync::{Arc, OnceLock, RwLock};

use serde::{Deserialize, de::DeserializeOwned};

use nemesis_plugin::roaming::RoamingPluginHandle;

/// 已装载插件句柄（loader 装载成功注入一次；进程内单例——插件与桥都是
/// 进程级全局，gateway 每进程一个 Cluster，多实例互不干扰）。
static PLUGIN: OnceLock<&'static RoamingPluginHandle> = OnceLock::new();

/// 名册成员回调漏斗（`{"members":[...]}` 全量包 → 集群 merge_rendezvous_members）。
/// RwLock 而非 OnceLock：热更/重装时 loader 重新注入可整体替换。
static MEMBERS_SINK: RwLock<Option<Arc<dyn Fn(&str) + Send + Sync>>> = RwLock::new(None);

/// 入站连接漏斗（M3：notify_incoming → server 臂受理循环）。
static INCOMING_SINK: RwLock<Option<Arc<dyn Fn(&IncomingConn) + Send + Sync>>> = RwLock::new(None);

/// 注入已装载的插件句柄（loader 专属——`HANDLE.get()` 得到的引用即
/// `'static` 生命周期）。重复注入同值幂等；不同值=装配 bug，warn 后忽略。
pub fn set_plugin_handle(h: &'static RoamingPluginHandle) {
    if let Err(existing) = PLUGIN.set(h)
        && !std::ptr::eq(existing, h)
    {
        tracing::warn!("[roaming-bridge] 插件句柄重复注入且指针不同——保持首注入");
    }
}

/// 插件是否已装载（桥面判断；CLI 诚实报错 / gateway 装配闸同读此口径）。
pub fn is_loaded() -> bool {
    PLUGIN.get().is_some()
}

/// 挂成员回流漏斗（集群 setup_roaming 专属；热替换覆盖旧漏斗）。
pub fn set_members_sink(sink: Arc<dyn Fn(&str) + Send + Sync>) {
    *MEMBERS_SINK.write().expect("members sink 锁中毒") = Some(sink);
}

/// 插件循环推成员包的入口（loader `host_notify_members` 转发）。
/// 漏斗未挂 = debug 丢弃（插件下 tick 重推全量包——自愈语义，无需补发）。
pub fn dispatch_members_json(json: &str) {
    let sink = MEMBERS_SINK.read().expect("members sink 锁中毒").clone();
    match sink {
        Some(sink) => sink(json),
        None => tracing::debug!(
            "[roaming-bridge] 成员包抵达但漏斗未挂载（集群装配前/已停机）——丢弃等下 tick 重推"
        ),
    }
}

// ---------------------------------------------------------------------------
// 入站连接交接面（M3：accept 臂——插件开 loopback 端口通知宿主受理）
// ---------------------------------------------------------------------------

/// 入站连接交接载荷（`notify_incoming` v2 三参投影；server 臂消费——
/// `TcpStream::connect(127.0.0.1:port)` 后帧协议照跑）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IncomingConn {
    pub conn_id: u64,
    pub port: u16,
    /// 对端 EndpointId hex（v2 追加；受理器日志/审计带对端身份）。
    pub remote_hex: String,
}

/// 挂入站连接漏斗（集群 setup_roaming 专属；热替换覆盖旧漏斗；停机链置回
/// None 由 [`clear_incoming_sink`] 承担）。
pub fn set_incoming_sink(sink: Arc<dyn Fn(&IncomingConn) + Send + Sync>) {
    *INCOMING_SINK.write().expect("incoming sink 锁中毒") = Some(sink);
}

/// 摘入站漏斗（停机链；摘后抵达的交接诚实丢弃——插件 HOST_ACCEPT_TIMEOUT
/// 兜底强关 loopback 监听，无资源泄漏）。
pub fn clear_incoming_sink() {
    *INCOMING_SINK.write().expect("incoming sink 锁中毒") = None;
}

/// 插件 accept 臂推交接的入口（loader `host_notify_incoming` 转发）。
/// 漏斗未挂 = warn 丢弃（server 臂未装配/已停机；插件侧宿主拨入超时兜底）。
pub fn dispatch_incoming(conn_id: u64, port: u16, remote_hex: &str) {
    let sink = INCOMING_SINK.read().expect("incoming sink 锁中毒").clone();
    match sink {
        Some(sink) => sink(&IncomingConn {
            conn_id,
            port,
            remote_hex: remote_hex.to_string(),
        }),
        None => tracing::warn!(
            "[roaming-bridge] 入站交接抵达但漏斗未挂载（server 臂未装配/已停机）\
             conn={conn_id} port={port} remote=iroh:{remote_hex}——丢弃（插件拨入超时兜底强关）"
        ),
    }
}

/// 桥调用三态：`Absent` = 插件未装载（调用方裁决降级/诚实报错）；
/// `Ok` = 插件 ok:true 且响应解析成功；`Failed` = 插件报错或 FFI/解析异常。
#[derive(Debug)]
pub enum BridgeOutcome<T> {
    Absent,
    Ok(T),
    Failed(String),
}

impl<T> BridgeOutcome<T> {
    pub fn is_absent(&self) -> bool {
        matches!(self, BridgeOutcome::Absent)
    }

    pub fn ok(self) -> Option<T> {
        match self {
            BridgeOutcome::Ok(v) => Some(v),
            _ => None,
        }
    }

    pub fn map<U>(self, f: impl FnOnce(T) -> U) -> BridgeOutcome<U> {
        match self {
            BridgeOutcome::Absent => BridgeOutcome::Absent,
            BridgeOutcome::Ok(v) => BridgeOutcome::Ok(f(v)),
            BridgeOutcome::Failed(e) => BridgeOutcome::Failed(e),
        }
    }
}

/// spawn_blocking 包 FFI + `ok:false` 展开 + 响应 JSON 解析（文案与 loader
/// `call_roaming` 同源镜像）。
async fn call_bridge<T: DeserializeOwned>(
    what: &'static str,
    f: impl Fn(RoamingPluginHandle) -> Result<String, String> + Send + 'static,
) -> BridgeOutcome<T> {
    let Some(h) = PLUGIN.get().copied() else {
        return BridgeOutcome::Absent;
    };
    let res = match tokio::task::spawn_blocking(move || f(*h)).await {
        Ok(r) => r,
        Err(e) => Err(format!("{what}: spawn_blocking join 失败: {e}")),
    };
    // ok:false 展开（镜像 loader call_roaming）：`{what}: [{code}] {msg}`；
    // 缺 code/非对象形态 = 通用 `{what}: {msg}`（裸错误串同款）。
    let raw = match res {
        Ok(r) => r,
        Err(e) => return BridgeOutcome::Failed(e),
    };
    #[derive(Deserialize)]
    struct ErrShape {
        #[serde(default)]
        code: Option<String>,
        #[serde(default)]
        error: Option<String>,
    }
    match serde_json::from_str::<serde_json::Value>(&raw) {
        Ok(v) if v.get("ok") == Some(&serde_json::Value::Bool(false)) => {
            let err: ErrShape = serde_json::from_value(v).unwrap_or(ErrShape {
                code: None,
                error: None,
            });
            match (err.code, err.error) {
                (Some(code), Some(msg)) => BridgeOutcome::Failed(format!("{what}: [{code}] {msg}")),
                (_, Some(msg)) => BridgeOutcome::Failed(format!("{what}: {msg}")),
                (Some(code), None) => BridgeOutcome::Failed(format!("{what}: [{code}]")),
                (None, None) => BridgeOutcome::Failed(format!("{what}: ok=false（无错误详情）")),
            }
        }
        Ok(_) => match serde_json::from_str::<T>(&raw) {
            Ok(v) => BridgeOutcome::Ok(v),
            Err(e) => BridgeOutcome::Failed(format!("{what}: 响应 JSON 解析失败: {e}")),
        },
        Err(e) => BridgeOutcome::Failed(format!("{what}: 响应非 JSON: {e}")),
    }
}

// ---------------------------------------------------------------------------
// 身份面（M2：宿主控制面身份操作全部改走桥）
// ---------------------------------------------------------------------------

/// 身份材料（`identity_keygen`/`identity_ensure` 响应投影）。M3 起 seed 不出
/// 插件（插件是 endpoint 唯一消费者——identity_payload 一律不带 secret）。
#[derive(Debug, Clone, Deserialize)]
pub struct IdentityInfo {
    pub created: bool,
    pub iroh_id: String,
}

/// 身份确保存在（无文件生成 + 落盘 + 返回身份）。等价旧宿主
/// `RoamingIdentity::load_or_create`（M3 起 secret 不再经 ABI 回宿主）。
pub async fn identity_ensure() -> BridgeOutcome<IdentityInfo> {
    call_bridge("roaming_identity_keygen", |h| h.identity_keygen(false)).await
}

/// 身份强制重生成（CLI keygen --force；盘上旧身份作废）。
pub async fn identity_keygen_force() -> BridgeOutcome<IdentityInfo> {
    call_bridge("roaming_identity_keygen", |h| h.identity_keygen(true)).await
}

/// 读当前 iroh 身份 hex（状态面展示用；无身份文件 = Failed(identity_load_failed)）。
pub async fn get_iroh_id() -> BridgeOutcome<String> {
    #[derive(Deserialize)]
    struct Shape {
        iroh_id: String,
    }
    call_bridge("roaming_get_iroh_id", |h| h.get_iroh_id())
        .await
        .map(|s: Shape| s.iroh_id)
}

// ---------------------------------------------------------------------------
// 名册循环控制面（M2：发现循环搬插件，宿主只控启停）
// ---------------------------------------------------------------------------

/// 拉起/热更名册循环（幂等：插件侧停旧起新）。`config_json` 形态 =
/// `{"registry_url":..,"token":..,"member":{RendezvousMember},..}`（宿主组装，
/// 解析单一真相源在插件 registry_client）。响应 `{"ok":true,"running":bool}`。
pub async fn registry_start(config_json: String) -> BridgeOutcome<bool> {
    #[derive(Deserialize)]
    struct Shape {
        #[serde(default)]
        running: bool,
    }
    call_bridge("roaming_registry_start", move |h| {
        h.registry_start(&config_json)
    })
    .await
    .map(|s: Shape| s.running)
}

/// 停名册循环（fire-and-forget：宿主停机链/无 runtime 上下文用——有 runtime
/// 则 spawn 后台执行，结果仅日志；幂等）。
pub fn registry_stop_fire_and_forget() {
    if !is_loaded() {
        return;
    }
    match tokio::runtime::Handle::try_current() {
        Ok(handle) => {
            handle.spawn(async move {
                if let BridgeOutcome::Failed(e) =
                    call_bridge::<serde_json::Value>("roaming_registry_stop", |h| h.registry_stop())
                        .await
                {
                    tracing::debug!("[roaming-bridge] registry_stop 失败（幂等无副作用）: {e}");
                }
            });
        }
        Err(_) => {
            // 无 tokio 上下文（极端停机路径）：插件侧 shutdown 兜底会收循环。
            tracing::debug!(
                "[roaming-bridge] 无 runtime 上下文，registry_stop 跳过（插件 shutdown 兜底）"
            );
        }
    }
}

// ---------------------------------------------------------------------------
// 客户端连接面（M3：dial/close/endpoint 启停/状态快照）
// ---------------------------------------------------------------------------

/// 拨号回执（`roaming_dial` 响应投影）。
#[derive(Debug, Clone, Deserialize)]
pub struct DialInfo {
    /// 插件在本机开的一次性 loopback TCP 监听端口——宿主
    /// `TcpStream::connect(127.0.0.1:port)` 后帧协议照跑。
    pub port: u16,
    /// 连接注册表 id（`close_conn` 半开兜底用）。
    pub conn_id: u64,
}

/// 拨号（addr 形如 `iroh:<hex>` 或 `iroh:<hex>@<relay_url>`——announce 收割
/// 形态原样透传，解析单一真相源在插件 endpoint::parse_roaming_address）。
pub async fn dial(addr: &str) -> BridgeOutcome<DialInfo> {
    let addr = addr.to_string();
    call_bridge("roaming_dial", move |h| h.dial(&addr)).await
}

/// 按连接 id 显式关闭（半开兜底；关整条 QUIC 连接）。幂等（`closed:false`）。
pub async fn close_conn(conn_id: u64) -> BridgeOutcome<bool> {
    #[derive(Deserialize)]
    struct Shape {
        #[serde(default)]
        closed: bool,
    }
    call_bridge("roaming_close_conn", move |h| h.close_conn(conn_id))
        .await
        .map(|s: Shape| s.closed)
}

/// 客户端连接面端点拉起/热更（幂等：插件侧停旧起新）。`config_json` 形态 =
/// `{"relay_url":"","relay_token":"","idle_timeout_secs":0}`（relay_url 空 =
/// n0 公共中继）。响应 `{"ok":true,"node_id":hex}`。
pub async fn endpoint_start(config_json: String) -> BridgeOutcome<String> {
    #[derive(Deserialize)]
    struct Shape {
        node_id: String,
    }
    call_bridge("roaming_endpoint_start", move |h| {
        h.endpoint_start(&config_json)
    })
    .await
    .map(|s: Shape| s.node_id)
}

/// 停客户端连接面端点（幂等；未运行 = ok）。
pub async fn endpoint_stop() -> BridgeOutcome<()> {
    call_bridge::<serde_json::Value>("roaming_endpoint_stop", |h| h.endpoint_stop())
        .await
        .map(|_| ())
}

/// 客户端连接面快照（`roaming_status` 的 endpoint 段投影；插件/端点缺席 =
/// `Absent`——宿主按「无观测」语义渲染）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct EndpointStatus {
    #[serde(default)]
    pub running: bool,
    #[serde(default)]
    pub node_id: String,
    /// 最近拨号路径观测（node_id hex → "direct"/"relay"；插件侧逐连接
    /// 观察任务维护，退出不清槽——「最近一次」语义）。
    #[serde(default)]
    pub dial_paths: std::collections::HashMap<String, String>,
}

pub async fn status_snapshot() -> BridgeOutcome<EndpointStatus> {
    #[derive(Deserialize)]
    struct Full {
        #[serde(default)]
        endpoint: Option<EndpointStatus>,
    }
    call_bridge::<Full>("roaming_status", |h| h.status())
        .await
        .map(|f| f.endpoint.unwrap_or_default())
}

// ---------------------------------------------------------------------------
// 名册循环运行状态（Phase 2 观测性：插件 registry_sync 段 → 宿主 rendezvous
// 段最终形态。此前失败态只进插件日志，宿主快照只在成功路径覆写——前端
// 「同步失败」徽标是死分支）
// ---------------------------------------------------------------------------

/// 插件名册循环运行状态（`roaming_status` 的 `registry_sync` 段投影；
/// 缺段 = 旧插件二进制，宿主按 None 回落旧行为）。
#[derive(Debug, Clone, Default, Deserialize)]
pub struct RegistrySyncStatus {
    #[serde(default)]
    pub registry_url: String,
    #[serde(default)]
    pub fail_streak: u64,
    #[serde(default)]
    pub last_error: String,
    /// 最近一次成功 tick（epoch ms；0 = 从未成功）。
    #[serde(default)]
    pub last_ok_ms: u64,
    /// 最近一次失败 tick（epoch ms；0 = 从未失败）。
    #[serde(default)]
    pub last_fail_ms: u64,
    #[serde(default)]
    pub last_member_count: usize,
}

/// 读插件名册循环运行状态（循环未挂/旧插件无段/桥缺席或失败 = `None`）。
pub async fn registry_sync_snapshot() -> BridgeOutcome<Option<RegistrySyncStatus>> {
    #[derive(Deserialize)]
    struct Full {
        #[serde(default)]
        registry_sync: Option<RegistrySyncStatus>,
    }
    call_bridge::<Full>("roaming_status", |h| h.status())
        .await
        .map(|f| f.registry_sync)
}

/// rendezvous 状态最终形态裁决（纯函数，无 IO——宿主快照 × 插件运行态）。
///
/// 规则：
/// - 插件段缺席（循环未挂/旧插件/桥失败）= 宿主快照原样（旧行为）；
/// - 插件段在但循环从未跑过任何 tick（`last_ok_ms == 0 && last_fail_ms == 0`，
///   刚 start 未到首拍）= 无失败可报，宿主快照原样；
/// - **最后动作是失败**（`last_fail_ms > last_ok_ms`，epoch ms 单调故无需看
///   streak）= 失败形态：`last_sync_ok:false` + 插件 `last_error`/`last_fail_ms`；
///   registry_url 插件自带（失败从出生时宿主无快照可兜底），member_count 用
///   宿主最后已知值（无宿主快照则插件 `last_member_count`——从未成功即 0）；
/// - 最后动作是成功 = 宿主快照原样（members_sink 与成功 tick 同拍写入，
///   成功形态语义未变）。
pub fn effective_rendezvous_state(
    host: Option<crate::roaming::rendezvous::RendezvousClientState>,
    plugin: Option<RegistrySyncStatus>,
) -> Option<crate::roaming::rendezvous::RendezvousClientState> {
    use crate::roaming::rendezvous::RendezvousClientState;
    let Some(p) = plugin else {
        return host;
    };
    if p.last_ok_ms == 0 && p.last_fail_ms == 0 {
        return host;
    }
    if p.last_fail_ms > p.last_ok_ms {
        let (host_url, host_count) = match host {
            Some(h) => (Some(h.registry_url), h.member_count),
            None => (None, p.last_member_count),
        };
        return Some(RendezvousClientState {
            registry_url: if p.registry_url.is_empty() {
                host_url.unwrap_or_default()
            } else {
                p.registry_url
            },
            last_sync_ok: false,
            last_sync_ms: p.last_fail_ms,
            member_count: host_count,
            last_error: p.last_error,
        });
    }
    host
}

// ---------------------------------------------------------------------------
// 拨号抽象（rpc/client 臂消费；测试注入假插件——真插件走 loopback 交接）
// ---------------------------------------------------------------------------

/// 拨号抽象：rpc/client 的 roaming 发送臂经此拨号，不依赖插件在位（测试用
/// `LocalDialer` 直连本地 TCP；生产 `BridgeDialer` 经桥 FFI 插件 loopback）。
pub trait RoamingDialer: Send + Sync + 'static {
    /// 拨号远端：返回 `(loopback 端口, conn_id)`——宿主 connect 127.0.0.1:port
    /// 后帧协议照跑。失败 = 诚实 Err（调用方标 ⛔ 重试语义照常）。
    fn dial(
        &self,
        addr: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, u64), String>> + Send>>;

    /// 按连接 id 显式关闭（半开兜底；best-effort fire-and-forget——失败仅日志）。
    fn close(&self, conn_id: u64);
}

/// 生产实现：经桥打真插件。
#[derive(Debug, Clone, Copy, Default)]
pub struct BridgeDialer;

impl RoamingDialer for BridgeDialer {
    fn dial(
        &self,
        addr: &str,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<(u16, u64), String>> + Send>>
    {
        let addr = addr.to_string();
        Box::pin(async move {
            match dial(&addr).await {
                BridgeOutcome::Ok(info) => Ok((info.port, info.conn_id)),
                BridgeOutcome::Absent => Err("roaming 插件未装载（无法拨号）".to_string()),
                BridgeOutcome::Failed(e) => Err(e),
            }
        })
    }

    fn close(&self, conn_id: u64) {
        if !is_loaded() {
            return;
        }
        tokio::spawn(async move {
            if let BridgeOutcome::Failed(e) = close_conn(conn_id).await {
                tracing::debug!("[roaming-bridge] close_conn({conn_id}) 失败（幂等）: {e}");
            }
        });
    }
}

#[cfg(test)]
mod tests;
