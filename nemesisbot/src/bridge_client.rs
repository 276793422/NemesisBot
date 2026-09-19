//! 反向桥客户端（goal：节点显示名 + 反向桥与多设备汇聚，一期批次二）。
//!
//! 职责：出站连远端中继服务端（`relay_url` + `/bridge`），hello 握手（token
//! 走 hello 帧不走 URL query——避免进访问日志）、断线指数退避重连（5s→60s
//! 封顶）、30s 心跳 + 90s 无任何下行帧判服务端假死主动断开重连、conn 泵
//! （`ConnOpen` → dial 本机 web server → 纯字节流搬运）、`access_check`
//! 比对（面板访问密码只存本机，比对 `SHA-256` 哈希，服务端零存储）。
//!
//! **桥为旁路**：桥客户端任何失败（连不上/被拒/重连循环）都不影响本机
//! dashboard 与 Bot 主功能——失败只打日志 + 退避重试。
//!
//! 帧类型与 conn 语义复用 `nemesis_web::relay::protocol`（与服务端同一
//! 套定义，两端天然对齐）：
//! - 短 conn（HTTP）：服务端整段请求 `ConnData{fin:true}` 下发（fin 仅为
//!   信息性提示，请求完整性由 content-length 界定——写半**不** shutdown，
//!   2026-09-19 真机实测 shutdown 使 hyper 于处理前断连）→ 读响应回传
//!   （`fin:false` 逐包）→ 本机 EOF 发**空载荷 + fin=true** 收口（EOF
//!   约定；keep-alive 下服务端按响应长度收口后回发 `ConnClose` 通知）→
//!   dial 失败/读错误发 `ConnClose`
//! - 长 conn（WS 隧道）：同泵，双向 `fin:false` 逐帧，任一端关闭走
//!   `ConnClose` / EOF 约定
//!
//! **不做端到端加密**（goal 裁决：防线边界已知情；ws over 公网明文——
//! 中继是 HTTP 时浏览器侧也必然是 ws；部署方上 HTTPS/wss 即全链路加密）。

use std::collections::HashMap;
use std::time::{Duration, Instant};

use futures::{SinkExt, StreamExt};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio_tungstenite::{MaybeTlsStream, WebSocketStream, tungstenite::Message};

use nemesis_web::relay::protocol::{BridgeFrame, decode_frame, encode_frame};

use sha2::{Digest, Sha256};

// ---------------------------------------------------------------------------
// 常量（goal 钉死：30s 心跳、90s 假死、5s→60s 退避）
// ---------------------------------------------------------------------------

/// 心跳间隔（客户端 30s 一跳；服务端 3× 容错判离线）。
pub(crate) const HEARTBEAT_INTERVAL_SECS: u64 = 30;
/// 假死判定：这么久没收到中继**任何**下行帧（Pong/帧都算）→ 主动断开重连
/// （TCP 半开/中间设备静默丢连接的解药）。
pub(crate) const DEAD_AFTER_SECS: u64 = 90;
/// 退避下限（首次/成功重置后）。
pub(crate) const BACKOFF_MIN_SECS: u64 = 5;
/// 退避上限（指数翻倍封顶）。
pub(crate) const BACKOFF_MAX_SECS: u64 = 60;
/// hello 后等 welcome 的超时（服务端收到 hello 立即回；超时 = 中继假死/网络断）。
pub(crate) const WELCOME_TIMEOUT_SECS: u64 = 10;

/// 单 conn 本机读取缓冲（响应/WS 帧逐块搬运，16KB 平衡内存与帧数）。
const CONN_READ_BUF: usize = 16 * 1024;
/// 单 conn 下行写队列深度（写本机 tcp 慢于 ws 下发时的背压面）。
const CONN_WRITE_CAPACITY: usize = 16;

// ---------------------------------------------------------------------------
// 参数与时序（时序参数化 = 测试可毫秒级跑完假死/退避路径）
// ---------------------------------------------------------------------------

/// 桥 RPC 枢纽句柄（二期批次六）。cluster 构建 = [`crate::bridge_rpc::
/// DeviceBridgeRpc`]；非 cluster 构建无此子系统，别名退化 `()`——消除
/// `BridgeClientParams` 字段类型的 cfg 门重复书写。
#[cfg(feature = "cluster")]
pub type BridgeRpcHandle = std::sync::Arc<crate::bridge_rpc::DeviceBridgeRpc>;
#[cfg(not(feature = "cluster"))]
pub type BridgeRpcHandle = ();

/// 二期批次六 helper（cfg 双实现）：挂载桥 RPC 上行出口。
/// 非 cluster 构建无此子系统，no-op。
#[cfg(feature = "cluster")]
fn bridge_rpc_attach(handle: &BridgeRpcHandle, tx: mpsc::UnboundedSender<BridgeFrame>) {
    handle.attach_uplink(tx);
}
#[cfg(not(feature = "cluster"))]
fn bridge_rpc_attach(handle: &BridgeRpcHandle, _tx: mpsc::UnboundedSender<BridgeFrame>) {
    let _ = handle;
}

/// 二期批次六 helper（cfg 双实现）：摘除桥 RPC 上行出口 + 清 pending。
#[cfg(feature = "cluster")]
fn bridge_rpc_detach(handle: &BridgeRpcHandle) {
    handle.detach_uplink();
}
#[cfg(not(feature = "cluster"))]
fn bridge_rpc_detach(handle: &BridgeRpcHandle) {
    let _ = handle;
}

/// 二期批次六 helper（cfg 双实现）：下行 `cluster_rpc` 帧分流。
#[cfg(feature = "cluster")]
async fn bridge_rpc_downstream(
    handle: &BridgeRpcHandle,
    payload: serde_json::Value,
) -> Option<BridgeFrame> {
    handle.handle_downstream(payload).await
}
#[cfg(not(feature = "cluster"))]
async fn bridge_rpc_downstream(
    handle: &BridgeRpcHandle,
    _payload: serde_json::Value,
) -> Option<BridgeFrame> {
    let _ = handle;
    None
}

/// 三期批次八 helper（cfg 双实现）：下行 `member_sync` 帧合并。
/// 非 cluster 构建无集群 registry，no-op（维持一期忽略语义）。
#[cfg(feature = "cluster")]
fn bridge_rpc_member_sync(handle: &BridgeRpcHandle, payload: &serde_json::Value) {
    handle.handle_member_sync(payload);
}
#[cfg(not(feature = "cluster"))]
fn bridge_rpc_member_sync(handle: &BridgeRpcHandle, _payload: &serde_json::Value) {
    let _ = handle;
}

/// 桥客户端启动参数（gateway 装配处构造）。
pub struct BridgeClientParams {
    /// 中继服务端地址（如 `ws://vps.example.com:60600`；自动拼 `/bridge`）。
    pub relay_url: String,
    /// 接入门 token（与远端服务端 `bridge.server.token` 同值）。
    pub token: String,
    /// 本机桥身份（子路径前缀 `/d/<node_id>/` 的锚点；稳定 = 浏览器收藏可用）。
    pub node_id: String,
    /// 设备显示名（一期仅中继状态页展示）。
    pub name: String,
    /// 设备版本（hello 帧携带）。
    pub version: String,
    /// 本机 web server 端口（conn 泵 dial `127.0.0.1:<port>`；real_port）。
    pub web_port: u16,
    /// 面板访问密码（只存本机；`AccessCheck` 时比对 SHA-256；空 = 拒绝一切远程访问）。
    pub access_token: String,
    /// 二期：本机集群身份快照（Some = hello 携带，hub 注册进集群 registry
    /// 同权组网；None = 纯隧道设备语义）。gateway 装配处按 cluster 存在
    /// 且 rpc_port>0 构造。
    pub cluster_identity: Option<nemesis_web::relay::BridgeClusterIdentity>,
    /// 二期批次六：桥帧 RPC 枢纽（Some = 集群构建——下行 `cluster_rpc`
    /// 帧分流到本地 RPC 链 / pending 表；None = 一期语义，下行帧 WARN 忽略）。
    /// gateway 装配处与 RpcClient::set_bridge_transport 同源注入。
    pub bridge_rpc: Option<BridgeRpcHandle>,
}

/// 可调时序（默认值 = 上方常量；测试传小值毫秒级跑完）。
#[derive(Clone, Copy, Debug)]
pub(crate) struct LoopTiming {
    pub heartbeat: Duration,
    pub dead_after: Duration,
    pub welcome_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for LoopTiming {
    fn default() -> Self {
        Self {
            heartbeat: Duration::from_secs(HEARTBEAT_INTERVAL_SECS),
            dead_after: Duration::from_secs(DEAD_AFTER_SECS),
            welcome_timeout: Duration::from_secs(WELCOME_TIMEOUT_SECS),
            backoff_min: Duration::from_secs(BACKOFF_MIN_SECS),
            backoff_max: Duration::from_secs(BACKOFF_MAX_SECS),
        }
    }
}

/// gateway 装配入口：spawn 桥客户端主循环（旁路，不阻断调用方）。
pub fn spawn(params: BridgeClientParams) -> tokio::task::JoinHandle<()> {
    tokio::spawn(run_loop(params, LoopTiming::default()))
}

// ---------------------------------------------------------------------------
// 纯函数（可单测）
// ---------------------------------------------------------------------------

/// relay_url → 中继 ws 端点（强制 `/bridge` 路径；忽略用户误带的 path）。
pub(crate) fn bridge_endpoint(relay_url: &str) -> Result<String, String> {
    let trimmed = relay_url.trim();
    let Some((scheme, rest)) = trimmed.split_once("://") else {
        return Err(format!(
            "relay_url 缺少 scheme（需 ws:// 或 wss://）：{relay_url}"
        ));
    };
    if scheme != "ws" && scheme != "wss" {
        return Err(format!(
            "relay_url scheme 必须是 ws:// 或 wss://（当前 {scheme}://）"
        ));
    }
    let authority = rest.split('/').next().unwrap_or_default();
    if authority.is_empty() {
        return Err(format!("relay_url 缺少 host：{relay_url}"));
    }
    Ok(format!("{scheme}://{authority}/bridge"))
}

/// 主机名链（COMPUTERNAME → HOSTNAME → "unknown"；与 cluster
/// generate_node_id、diagnostics 同款），原样大小写——作设备显示名。
pub(crate) fn hostname() -> String {
    std::env::var("COMPUTERNAME")
        .or_else(|_| std::env::var("HOSTNAME"))
        .unwrap_or_else(|_| "unknown".to_string())
}

/// 桥身份默认值：hostname 链 + `bridge-` 前缀（区分集群 node id），小写化
/// （URL 路径友好）。**稳定**：重启不变——子路径收藏/书签依赖这一点
/// （集群 generate_node_id 带随机 uuid，正是不稳定才不能复用）。
pub(crate) fn hostname_node_id() -> String {
    format!("bridge-{}", hostname().to_lowercase())
}

/// 常数时间字节比较（比对哈希用；长度不同直接 false——长度不是秘密）。
pub(crate) fn constant_time_eq(a: &[u8], b: &[u8]) -> bool {
    if a.len() != b.len() {
        return false;
    }
    let mut diff = 0u8;
    for (x, y) in a.iter().zip(b.iter()) {
        diff |= x ^ y;
    }
    diff == 0
}

/// access_check 比对：`SHA-256(本机 access_token)` hex == 上报 hash_hex。
/// 空 access_token = fail-closed（拒绝一切远程面板访问）。
pub(crate) fn check_access(access_token: &str, hash_hex: &str) -> bool {
    if access_token.is_empty() {
        return false;
    }
    let mut hasher = Sha256::new();
    hasher.update(access_token.as_bytes());
    let expect = format!("{:x}", hasher.finalize());
    constant_time_eq(expect.as_bytes(), hash_hex.to_lowercase().as_bytes())
}

/// 退避步进：×2 翻倍，封顶 `max`。
pub(crate) fn next_backoff(cur: Duration, max: Duration) -> Duration {
    let doubled = cur.mul_f64(2.0);
    if doubled > max || doubled.is_zero() {
        max
    } else {
        doubled
    }
}

// ---------------------------------------------------------------------------
// conn 泵（一条 conn = 浏览器 ↔ 本机 web server 的一次字节流搬运）
// ---------------------------------------------------------------------------

/// 下行（服务端 → 本机 web server）写命令。
enum ConnWriteCmd {
    Data(Vec<u8>),
    /// 整条关闭（服务端发来 `ConnClose`）。
    CloseAll,
}

struct ConnHandle {
    write_tx: mpsc::Sender<ConnWriteCmd>,
    read_task: tokio::task::JoinHandle<()>,
}

/// `ConnOpen` 处理：dial 本机 web server，起读/写双泵。
/// 失败诚实回 `ConnClose`（服务端据此给浏览器回 502）。
async fn open_conn(
    conn_id: u64,
    web_port: u16,
    conns: &mut HashMap<u64, ConnHandle>,
    up_tx: &mpsc::UnboundedSender<BridgeFrame>,
) {
    match tokio::net::TcpStream::connect(("127.0.0.1", web_port)).await {
        Err(e) => {
            tracing::warn!("[Bridge] conn#{conn_id} dial 127.0.0.1:{web_port} 失败：{e}");
            let _ = up_tx.send(BridgeFrame::ConnClose {
                conn_id,
                reason: format!("dial failed: {e}"),
            });
        }
        Ok(stream) => {
            let (mut rd, mut wr) = stream.into_split();
            let (write_tx, mut write_rx) = mpsc::channel::<ConnWriteCmd>(CONN_WRITE_CAPACITY);
            // 写泵：下行帧字节落本机 tcp。写失败 = 本机断了，静默退出——
            // 读泵随即读到 EOF/错误，经 EOF 约定/ConnClose 收口整条 conn。
            tokio::spawn(async move {
                while let Some(cmd) = write_rx.recv().await {
                    match cmd {
                        ConnWriteCmd::Data(bytes) => {
                            if wr.write_all(&bytes).await.is_err() {
                                break;
                            }
                        }
                        ConnWriteCmd::CloseAll => break,
                    }
                }
                // wr drop = 写半关闭；rd 仍由读泵持有直至收口。
            });
            // 读泵：本机响应 → 上行 ConnData（seq 逐包递增）。
            // EOF（Ok(0)）→ 空载荷 + fin=true（EOF 约定，服务端据此收口）。
            // 读错误 → ConnClose。cmd_tx 断（ws 出口已死）→ 退出。
            let up_tx = up_tx.clone();
            let read_task = tokio::spawn(async move {
                let mut seq: u64 = 0;
                let mut buf = vec![0u8; CONN_READ_BUF];
                loop {
                    match rd.read(&mut buf).await {
                        Ok(0) => {
                            let _ = up_tx.send(BridgeFrame::ConnData {
                                conn_id,
                                seq,
                                data_b64: String::new(),
                                fin: true,
                            });
                            break;
                        }
                        Ok(n) => {
                            use base64::Engine as _;
                            let data_b64 =
                                base64::engine::general_purpose::STANDARD.encode(&buf[..n]);
                            if up_tx
                                .send(BridgeFrame::ConnData {
                                    conn_id,
                                    seq,
                                    data_b64,
                                    fin: false,
                                })
                                .is_err()
                            {
                                break;
                            }
                            seq += 1;
                        }
                        Err(e) => {
                            let _ = up_tx.send(BridgeFrame::ConnClose {
                                conn_id,
                                reason: format!("local read error: {e}"),
                            });
                            break;
                        }
                    }
                }
            });
            tracing::debug!("[Bridge] conn#{conn_id} 已建立（127.0.0.1:{web_port}）");
            conns.insert(
                conn_id,
                ConnHandle {
                    write_tx,
                    read_task,
                },
            );
        }
    }
}

/// 关一条 conn：abort 读泵 + 关写泵（CloseAll）+ 移出表。
fn drop_conn(conn_id: u64, conns: &mut HashMap<u64, ConnHandle>) {
    if let Some(h) = conns.remove(&conn_id) {
        h.read_task.abort();
        // try_send：写泵队列满也无所谓——反正整条要关。
        let _ = h.write_tx.try_send(ConnWriteCmd::CloseAll);
        tracing::debug!("[Bridge] conn#{conn_id} 已关闭");
    }
}

// ---------------------------------------------------------------------------
// 主循环
// ---------------------------------------------------------------------------

type WsStream = WebSocketStream<MaybeTlsStream<tokio::net::TcpStream>>;

/// session 结束原因（run_loop 据此区分「配对失败」与「网络断」打日志，
/// 并决定退避是否重置）。
enum SessionEnd {
    /// welcome ok=false——token 错/门关/node_id 冲突，**不是**网络问题。
    Rejected(String),
    /// 连接层断开。`welcomed` = 本次会话是否曾配对成功（true = 重置退避：
    /// 长连接后断开从 5s 重新爬；false = 握手期就断，退避继续爬升）。
    Disconnected { welcomed: bool },
    /// 通道页手动重连按钮踢断（批次三）。用户明确意图 = 立即以最小退避
    /// 重连；**跳过**本轮退避等待直接 connect（kick permit 已在 session
    /// 内消费，循环尾的 select 等不到它了，必须显式 continue）。
    Kicked,
}

/// 桥客户端主循环：连接 → 握手 → session，失败退避重试，永续。
/// 旁路纪律：本函数 panic/卡死都不允许波及主功能（全部路径有界等待）。
///
/// 状态上报（批次三）：状态迁移点写 [`nemesis_web::relay::report_client_status`]
/// 全局槽——通道页 `GET /api/relay/overview` 读取；手动重连经
/// `reconnect_notify()` 的 Notify 踢醒（session 主 select 与循环尾退避
/// 两处监听）。
pub(crate) async fn run_loop(params: BridgeClientParams, timing: LoopTiming) {
    let endpoint = match bridge_endpoint(&params.relay_url) {
        Ok(ep) => ep,
        Err(e) => {
            tracing::error!("[Bridge] 桥客户端未启动：{e}");
            return;
        }
    };
    // 首报：槽从 None → Connecting（通道页「未启用」→「连接中」的翻转点）。
    nemesis_web::relay::report_client_status(nemesis_web::relay::BridgeClientStatus {
        enabled: true,
        state: nemesis_web::relay::BridgeClientState::Connecting,
        relay_url: params.relay_url.clone(),
        node_id: params.node_id.clone(),
        last_error: None,
        updated_at: unix_now(),
    });
    let mut backoff = timing.backoff_min;
    loop {
        match tokio_tungstenite::connect_async(&endpoint).await {
            Err(e) => {
                tracing::info!("[Bridge] 连接中继失败：{e}（{endpoint}；网络断或中继未启动）");
                nemesis_web::relay::report_client_state(
                    nemesis_web::relay::BridgeClientState::Disconnected,
                    Some(format!("连接失败：{e}")),
                );
            }
            Ok((ws, _)) => {
                tracing::info!(
                    "[Bridge] 已连接中继 {endpoint}，发送 hello（node_id={}）",
                    params.node_id
                );
                nemesis_web::relay::report_client_state(
                    nemesis_web::relay::BridgeClientState::Connecting,
                    None,
                );
                match session(ws, &params, timing).await {
                    SessionEnd::Kicked => {
                        // 手动重连：退避重置 + 跳过等待立即重连。
                        tracing::info!("[Bridge] 手动重连：立即重连中继");
                        backoff = timing.backoff_min;
                        continue;
                    }
                    SessionEnd::Rejected(reason) => {
                        // 配对失败 ≠ 网络断：诚实 ERROR（用户第一排查项是
                        // 两端 token 是否一致 / 中继门是否开着）。退避**不**
                        // 重置——持续错 token 不会被高频重试打爆中继日志；
                        // 中继侧改配置后自然在下个退避点恢复。
                        tracing::error!(
                            "[Bridge] 中继拒绝接入（配对失败，检查两端 bridge token 是否一致）：{reason}"
                        );
                        nemesis_web::relay::report_client_state(
                            nemesis_web::relay::BridgeClientState::Rejected,
                            Some(reason),
                        );
                    }
                    SessionEnd::Disconnected { welcomed } => {
                        if welcomed {
                            // 曾完整建联：退避重置（下次断开从最小值重爬）。
                            tracing::warn!("[Bridge] 桥连接断开，{}s 后重连", backoff.as_secs());
                            backoff = timing.backoff_min;
                        } else {
                            tracing::warn!(
                                "[Bridge] 握手未完成即断开，{}s 后重试",
                                backoff.as_secs()
                            );
                        }
                        nemesis_web::relay::report_client_state(
                            nemesis_web::relay::BridgeClientState::Disconnected,
                            Some("连接断开".to_string()),
                        );
                    }
                }
            }
        }
        // 退避等待：手动重连可打断（跳过剩余等待，立即重连 + 退避重置）。
        let kicked = tokio::select! {
            _ = tokio::time::sleep(backoff) => false,
            _ = nemesis_web::relay::reconnect_notify().notified() => true,
        };
        if kicked {
            tracing::info!("[Bridge] 手动重连：跳过退避等待，立即重连");
            backoff = timing.backoff_min;
        } else {
            backoff = next_backoff(backoff, timing.backoff_max);
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// 单次桥会话：hello → 等 welcome → 主循环（下行帧 / conn 泵上行 / 心跳与假死）。
/// 返回 = 连接已死（调用方重连）。
async fn session(ws: WsStream, params: &BridgeClientParams, timing: LoopTiming) -> SessionEnd {
    let (mut ws_write, mut ws_read) = ws.split();
    // welcome ok 后置 true（run_loop 据此重置退避）。
    let mut welcomed = false;

    // ---- 阶段 1：hello + 等 welcome（有界等待，服务端收到即回）----
    // 二期：集群身份快照展开进 hello（None = 纯隧道设备，字段省略——
    // 服务端按缺省 None 处理，老版本互连兼容）。
    let identity = params.cluster_identity.clone();
    let hello = match encode_frame(&BridgeFrame::BridgeHello {
        token: params.token.clone(),
        node_id: params.node_id.clone(),
        name: params.name.clone(),
        version: params.version.clone(),
        cluster_node_id: identity.as_ref().map(|i| i.node_id.clone()),
        cluster_name: identity.as_ref().map(|i| i.name.clone()),
        role: identity.as_ref().map(|i| i.role.clone()),
        category: identity.as_ref().map(|i| i.category.clone()),
        tags: identity.as_ref().map(|i| i.tags.clone()),
        capabilities: identity.as_ref().map(|i| i.capabilities.clone()),
        node_type: identity.as_ref().map(|i| i.node_type.clone()),
        rpc_port: identity.as_ref().map(|i| i.rpc_port),
        addresses: identity.as_ref().map(|i| i.addresses.clone()),
    }) {
        Ok(s) => s,
        Err(e) => {
            tracing::error!("[Bridge] hello 序列化失败（不可能路径）：{e}");
            return SessionEnd::Disconnected { welcomed };
        }
    };
    if let Err(e) = ws_write.send(Message::Text(hello.into())).await {
        tracing::warn!("[Bridge] hello 发送失败：{e}");
        return SessionEnd::Disconnected { welcomed };
    }
    match tokio::time::timeout(timing.welcome_timeout, ws_read.next()).await {
        Err(_) => {
            tracing::warn!(
                "[Bridge] 等待 welcome 超时（{}s）",
                timing.welcome_timeout.as_secs()
            );
            return SessionEnd::Disconnected { welcomed };
        }
        Ok(None) => return SessionEnd::Disconnected { welcomed },
        Ok(Some(Err(e))) => {
            tracing::warn!("[Bridge] 握手期间 ws 错误：{e}");
            return SessionEnd::Disconnected { welcomed };
        }
        Ok(Some(Ok(msg))) => {
            let text = match msg {
                Message::Text(t) => t,
                _ => {
                    tracing::warn!("[Bridge] 握手首帧非 Text，协议错乱，断开");
                    return SessionEnd::Disconnected { welcomed };
                }
            };
            match decode_frame(&text) {
                Ok(BridgeFrame::BridgeWelcome {
                    ok: true,
                    hub_node_id,
                    ..
                }) => {
                    welcomed = true;
                    // 二期：hub 侧集群身份（正常模式非空；`--relay` 纯中继
                    // 为空——设备据此感知 hub 是否集群节点，三期寻址用）。
                    if hub_node_id.is_empty() {
                        tracing::info!(
                            "[Bridge] 配对成功，桥通道已建立（心跳 {}s；hub 为纯中继，无集群身份）",
                            timing.heartbeat.as_secs()
                        );
                    } else {
                        tracing::info!(
                            "[Bridge] 配对成功，桥通道已建立（心跳 {}s；hub 集群节点 {hub_node_id}）",
                            timing.heartbeat.as_secs()
                        );
                    }
                    nemesis_web::relay::report_client_state(
                        nemesis_web::relay::BridgeClientState::Connected,
                        None,
                    );
                }
                Ok(BridgeFrame::BridgeWelcome {
                    ok: false, reason, ..
                }) => {
                    return SessionEnd::Rejected(reason);
                }
                _ => {
                    tracing::warn!("[Bridge] 握手首帧非 welcome，协议错乱，断开");
                    return SessionEnd::Disconnected { welcomed };
                }
            }
        }
    }

    // ---- 阶段 2：主循环 ----
    // conn 泵上行帧统一出口（泵不持有 ws 写半——主循环单点写，串行化）。
    let (cmd_tx, mut cmd_rx) = mpsc::unbounded_channel::<BridgeFrame>();
    // 二期批次六：挂载桥 RPC 上行出口（BridgeSend 发帧 + hub 请求的响应
    // 回上行都经此）；会话收尾 detach（在途桥请求按链路死亡收口）。
    if let Some(bridge_rpc) = &params.bridge_rpc {
        bridge_rpc_attach(bridge_rpc, cmd_tx.clone());
    }
    let mut conns: HashMap<u64, ConnHandle> = HashMap::new();
    let mut last_rx = Instant::now();
    let mut heartbeat_tick = tokio::time::interval(timing.heartbeat);
    heartbeat_tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    heartbeat_tick.tick().await; // 首个 tick 立即返回，跳过

    let end = loop {
        tokio::select! {
            // 下行：中继 → 设备
            incoming = ws_read.next() => {
                match incoming {
                    Some(Ok(Message::Text(text))) => {
                        last_rx = Instant::now();
                        let Ok(frame) = decode_frame(&text) else {
                            tracing::warn!("[Bridge] 无法解码下行帧，忽略");
                            continue;
                        };
                        match frame {
                            BridgeFrame::Pong => {} // 心跳回执：last_rx 已刷新
                            BridgeFrame::Heartbeat => {} // 服务端不该发；防御忽略
                            BridgeFrame::AccessCheck { request_id, node_id: _, hash_hex } => {
                                let ok = check_access(&params.access_token, &hash_hex);
                                if ok {
                                    tracing::info!("[Bridge] access_check#{request_id} 通过");
                                } else {
                                    tracing::warn!("[Bridge] access_check#{request_id} 拒绝（哈希不匹配或本机未配置访问密码）");
                                }
                                if send_frame(&mut ws_write, BridgeFrame::AccessResult { request_id, ok }).await.is_err() {
                                    break SessionEnd::Disconnected { welcomed };
                                }
                            }
                            BridgeFrame::ConnOpen { conn_id, .. } => {
                                open_conn(conn_id, params.web_port, &mut conns, &cmd_tx).await;
                            }
                            BridgeFrame::ConnData { conn_id, seq: _, data_b64, fin: _ } => {
                                use base64::Engine as _;
                                let data = match base64::engine::general_purpose::STANDARD.decode(&data_b64) {
                                    Ok(d) => d,
                                    Err(e) => {
                                        tracing::warn!("[Bridge] conn#{conn_id} 下行 base64 解码失败：{e}");
                                        drop_conn(conn_id, &mut conns);
                                        if send_frame(&mut ws_write, BridgeFrame::ConnClose { conn_id, reason: "bad base64".into() }).await.is_err() {
                                            break SessionEnd::Disconnected { welcomed };
                                        }
                                        continue;
                                    }
                                };
                                let Some(h) = conns.get(&conn_id) else {
                                    // conn 已收口/未登记——通知服务端停发。
                                    if send_frame(&mut ws_write, BridgeFrame::ConnClose { conn_id, reason: "conn not found".into() }).await.is_err() {
                                        break SessionEnd::Disconnected { welcomed };
                                    }
                                    continue;
                                };
                                // fin=true = 请求方向最后一包数据（短 conn：
                                // 整段请求单帧 fin=true）。**不半关闭写半**
                                // ——2026-09-19 真机彩排实测：写半 shutdown
                                // 会让 hyper 服务端在处理请求前读到 EOF 直接
                                // 断连（零字节响应）；请求完整性由
                                // content-length 界定（serialize_http_request
                                // 恒写该头），fin 下行仅是信息性提示，设备侧
                                // 无需动作。空载荷 + fin = 纯 EOF 语义（防御，
                                // 同样无需动作）。
                                let mut cmds: Vec<ConnWriteCmd> = Vec::with_capacity(1);
                                if !data.is_empty() {
                                    cmds.push(ConnWriteCmd::Data(data));
                                }
                                let mut backpressured = false;
                                for cmd in cmds {
                                    if h.write_tx.try_send(cmd).is_err() {
                                        // 下行写队列满 = 本机消费不过来，连接不健康
                                        // ——诚实收口（对齐服务端 DEVICE_OUTBOUND_CAPACITY 语义）。
                                        backpressured = true;
                                        break;
                                    }
                                }
                                if backpressured {
                                    tracing::warn!("[Bridge] conn#{conn_id} 下行背压超限，关闭该 conn");
                                    drop_conn(conn_id, &mut conns);
                                    if send_frame(&mut ws_write, BridgeFrame::ConnClose { conn_id, reason: "backpressure".into() }).await.is_err() {
                                        break SessionEnd::Disconnected { welcomed };
                                    }
                                }
                            }
                            BridgeFrame::ConnClose { conn_id, reason } => {
                                // 服务端主动关（浏览器断开/conn 泄漏清理/停发通知）。
                                tracing::debug!("[Bridge] conn#{conn_id} 服务端关闭：{reason}");
                                drop_conn(conn_id, &mut conns);
                            }
                            BridgeFrame::BridgeHello { .. } | BridgeFrame::BridgeClose { .. }
                            | BridgeFrame::BridgeWelcome { .. } | BridgeFrame::AccessResult { .. } => {
                                tracing::warn!("[Bridge] 收到不该由服务端发的帧，忽略");
                            }
                            // 二期批次六：下行 cluster_rpc 帧 → 桥 RPC 枢纽分流
                            // （response → 唤醒本地 pending；request → 喂本地
                            // RPC 链，响应回上行）。未装配枢纽（一期形态）维持
                            // WARN 忽略。
                            BridgeFrame::ClusterRpc { payload } => match &params.bridge_rpc {
                                Some(bridge_rpc) => {
                                    if let Some(up_frame) =
                                        bridge_rpc_downstream(bridge_rpc, payload).await
                                        && send_frame(&mut ws_write, up_frame).await.is_err()
                                    {
                                        break SessionEnd::Disconnected { welcomed };
                                    }
                                }
                                None => {
                                    tracing::warn!(
                                        "[Bridge] 收到 cluster_rpc 帧但未装配桥 RPC 枢纽，忽略"
                                    );
                                }
                            },
                            BridgeFrame::MemberSync { payload } => {
                                // 三期批次八：成员表合并（集群装配的设备才
                                // 消费；纯设备/`--relay` 链路维持忽略语义）。
                                if let Some(bridge_rpc) = &params.bridge_rpc {
                                    bridge_rpc_member_sync(bridge_rpc, &payload);
                                } else {
                                    tracing::debug!(
                                        "[Bridge] 收到 member_sync 但未装配集群，忽略"
                                    );
                                }
                            }
                        }
                    }
                    Some(Ok(_)) => { last_rx = Instant::now(); } // Binary/Ping/Pong 底层处理
                    Some(Err(e)) => {
                        tracing::warn!("[Bridge] 桥连接 ws 错误：{e}");
                        break SessionEnd::Disconnected { welcomed };
                    }
                    None => break SessionEnd::Disconnected { welcomed }, // 服务端关连接
                }
            }
            // 上行：conn 泵 → 中继
            maybe_frame = cmd_rx.recv() => {
                match maybe_frame {
                    Some(frame) => {
                        if send_frame(&mut ws_write, frame).await.is_err() {
                            break SessionEnd::Disconnected { welcomed };
                        }
                    }
                    // 全部泵退出且主循环不再 spawn：分支禁用（ws_read 分支仍在）。
                    None => continue,
                }
            }
            // 心跳 + 假死判定
            _ = heartbeat_tick.tick() => {
                if last_rx.elapsed() > timing.dead_after {
                    tracing::warn!(
                        "[Bridge] {}s 内未收到中继任何帧，判定服务端假死，主动断开重连",
                        timing.dead_after.as_secs()
                    );
                    break SessionEnd::Disconnected { welcomed };
                }
                if send_frame(&mut ws_write, BridgeFrame::Heartbeat).await.is_err() {
                    break SessionEnd::Disconnected { welcomed };
                }
            }
            // 批次三：手动重连踢——断开当前会话立即重连（permits 存续语义
            // 见 Notify 文档；握手中的会话不响应，最长 welcome_timeout 后
            // 由循环尾的退避 select 立即消费）。
            _ = nemesis_web::relay::reconnect_notify().notified() => {
                break SessionEnd::Kicked;
            }
        }
    };
    // 收尾：全部 conn 泵终止（abort 读泵；写泵随 CloseAll/Channel 关闭退出）。
    for conn_id in conns.keys().copied().collect::<Vec<_>>() {
        drop_conn(conn_id, &mut conns);
    }
    // 二期批次六：摘除桥 RPC 上行出口 + 清 pending（在途桥请求按链路死亡收口
    // ——RpcClient 仲裁按「桥路径失败」走兜底/诚实报错，不悬挂）。
    if let Some(bridge_rpc) = &params.bridge_rpc {
        bridge_rpc_detach(bridge_rpc);
    }
    end
}

/// 发一帧（Text）。写半死亡 = 会话该结束了。
async fn send_frame(
    ws_write: &mut futures::stream::SplitSink<WsStream, Message>,
    frame: BridgeFrame,
) -> Result<(), ()> {
    match encode_frame(&frame) {
        Ok(text) => ws_write
            .send(Message::Text(text.into()))
            .await
            .map_err(|_| ()),
        Err(e) => {
            tracing::error!("[Bridge] 帧序列化失败（不可能路径）：{e}");
            Err(())
        }
    }
}
