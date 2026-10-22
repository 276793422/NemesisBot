//! P5-R2 ③（网关守门面，VNC 无知 shim）：`/ws/vm-display` WS 端点。
//!
//! **本模块对 VNC/RFB 无知**：它是 dashboard 浏览器与 plugin-vm 插件之间的
//! 帧中继——不解析 RFB、不碰 QEMU、不持有 VM 状态。字节面 =
//! 浏览器 WS Binary 帧 ⇄ 插件回环 TCP（长度前缀帧协议，见下方「与插件的
//! 线协议」）。VNC 语义（RFB 握手/像素格式/键鼠事件、per-session readonly
//! 过滤）全部住在插件里；主二进制增量只有本 shim ≈ 数十 KB（体积纪律：
//! 可选显示面不进主二进制——noVNC 前端动态 import、plugin-vm 独立 cdylib、
//! 本模块 feature 门控三线一致）。
//!
//! **安全模型（与 L8 PTY 同款信任级 = dashboard 登录用户）**：
//! 1. `vm-display` cargo feature（编译闸，IoT 裁剪可关）；
//! 2. `config.json` `executor.vm.display` 运行闸：非 `"vnc"` → 404（端点
//!    关闭不暴露探测面，PTY enabled=false 同款）；
//! 3. token 闸：镜像主 WS 语义（未配 token = 放行）；
//! 4. 插件闸：plugin-vm 未装载/未握手 → **诚实 501「显示插件未安装」**
//!    （display=vnc 是显式开启，用户值得知道差在哪一环，而不是 404 混过去）；
//! 5. estop 闸：连接时刻已触发 → 503 拒升级；会话中触发 → 立即断连
//!    （急停语义 = 冻结一切执行体，显示通道含其中——屏幕看到的内容也是
//!    执行体状态的延伸）；
//! 6. 并发上限：[`MAX_DISPLAY_SESSIONS`]（PTY max_sessions 模式的定数版）；
//! 7. **readonly 是插件侧 per-session 语义**：mode=ro 时插件丢弃 RFB
//!    client→server 写类消息（KeyEvent/PointerEvent/ClientCutText）。QEMU
//!    层的 `-vnc ...,readonly` 不用（会锁死所有 viewer 且改档要重启 VM）。
//!
//! **与插件的线协议**（网关是唯一 WS 演讲者）：网关作为 TCP 客户端连插件
//! 回环监听（[`VmDisplayEndpoint::listen_port`]，插件 init 时经 vtable 上报
//! 并经本模块 [`set_endpoint`] 落槽）。①握手：一行 JSON meta（`\n` 结尾）：
//! `{"session_id","agent_id","mode"}`；②此后双向均为 4 字节大端长度前缀帧
//! （网关→插件 = RFB client→server 字节；插件→网关 = RFB server→client
//! 字节，插件侧从 QEMU VNC 口泵出后重帧）。长度前缀给 meta 定界 + 双向
//! 审计/限额钩子一个自然帧界；上限见 [`MAX_FRAME_BYTES`]。

use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use serde::Deserialize;

use crate::api_handlers::AppState;

/// 并发显示会话上限（PTY max_sessions 模式的定数版：显示通道是单人观察面
/// ——dashboard 用户盯一块屏，4 路已覆盖「多 agent 并行 + 对比观察」；
/// 不新增 config 键，真需求出现再配置化）。
const MAX_DISPLAY_SESSIONS: usize = 4;

/// 单帧字节上限：上行（浏览器→插件）1 MiB——RFB client→server 消息只有
/// 键鼠/剪贴板/像素格式，剪贴板贴大文本是最坏情形；下行 16 MiB——RFB
/// 首帧全屏更新理论可到 宽×高×4 字节（1920x1080 ≈ 8.3 MB），上限兜住
/// 失常/恶意插件，正常增量 tile 远小于此。
const MAX_UPSTREAM_FRAME: usize = 1024 * 1024;
const MAX_DOWNSTREAM_FRAME: usize = 16 * 1024 * 1024;

// ---------------------------------------------------------------------------
// 插件端点槽 + 会话管理器（模块级 OnceLock，PTY 同款——不进 AppState）
// ---------------------------------------------------------------------------

/// plugin-vm 握手产物（宿主 loader 填 [`set_endpoint`]）。
#[derive(Debug, Clone)]
pub struct VmDisplayEndpoint {
    /// 插件回环监听端口（插件自选空闲端口后上报；网关只连 127.0.0.1）。
    pub listen_port: u16,
    /// 插件上报的 ABI 版本（双向版本闸的插件侧半边；host loader 已闸过
    /// 相等性，这里留档可观测）。
    pub plugin_abi_version: u32,
}

static DISPLAY_ENDPOINT: OnceLock<VmDisplayEndpoint> = OnceLock::new();

/// 宿主 loader 装载 plugin-vm 成功后调用（first-wins：同端口重报 = 幂等
/// Ok（同一插件重复握手）；不同端口 = 拒绝——插件热重载走 gateway 重启，
/// 不做运行时换端）。
pub fn set_endpoint(ep: VmDisplayEndpoint) -> Result<(), String> {
    match DISPLAY_ENDPOINT.get_or_init(|| ep.clone()) {
        existing if existing.listen_port == ep.listen_port => Ok(()),
        existing => Err(format!(
            "vm-display 端点已被占用（已有插件监听 127.0.0.1:{}，新上报端口 {}）",
            existing.listen_port, ep.listen_port
        )),
    }
}

/// 插件端点读取（`vm.display.status` 能力探针消费——None = 插件未装载）。
pub(crate) fn endpoint() -> Option<&'static VmDisplayEndpoint> {
    DISPLAY_ENDPOINT.get()
}

/// 显示会话管理器：注册表（并发上限 + estop 断连挂点）。kill 语义 =
/// per-session kill 通道（watch）通知会话循环退出（连接随 future 终止
/// 关闭）；estop 主断连路径同一通道——注册发生在会话循环起跑前，无
/// 「kill 晚到漏杀」窗口（watch 种子值再兜一层，PTY 同款纪律）。
pub struct VmDisplayManager {
    inner: Mutex<HashMap<u64, tokio::sync::watch::Sender<bool>>>,
    next_id: AtomicU64,
}

impl VmDisplayManager {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
        })
    }

    /// 注册会话；满员返回 None。
    fn register(&self, kill_tx: tokio::sync::watch::Sender<bool>, max: usize) -> Option<u64> {
        let mut map = self.inner.lock().ok()?;
        if map.len() >= max {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        map.insert(id, kill_tx);
        Some(id)
    }

    fn unregister(&self, id: u64) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(&id);
        }
    }

    pub fn session_count(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// kill 全部会话（estop / 停机路径）。drain 后逐个 send——接收端已
    /// 全部 drop 的会话（本就在退出中）send Err 静默跳过。
    pub fn kill_all(&self) {
        let txs: Vec<_> = match self.inner.lock() {
            Ok(mut m) => std::mem::take(&mut *m).into_values().collect(),
            Err(_) => return,
        };
        for tx in txs {
            let _ = tx.send(true);
        }
    }
}

static DISPLAY_MANAGER: OnceLock<Arc<VmDisplayManager>> = OnceLock::new();

fn manager() -> &'static Arc<VmDisplayManager> {
    DISPLAY_MANAGER.get_or_init(VmDisplayManager::new)
}

/// estop / 停机路径断连入口（nemesisbot 宿主侧 estop watcher 调用；
/// manager OnceLock 惰性建，未用过显示通道时是 4 会话空表，零成本）。
pub fn disconnect_all_for_estop() {
    manager().kill_all();
}

// ---------------------------------------------------------------------------
// 配置闸（fresh-read，改盘即生效）
// ---------------------------------------------------------------------------

/// 运行闸：`executor.vm.display == "vnc"` 才开端点；否则 None = 404。
fn read_display_config() -> Option<nemesis_config::ExecutorVmConfig> {
    let cfg = nemesis_config::load_live()?.executor.unwrap_or_default().vm;
    (cfg.display == "vnc").then_some(cfg)
}

// ---------------------------------------------------------------------------
// WS 升级 + 帧中继
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct VmDisplayQuery {
    token: Option<String>,
    /// 目标 agent id（宿主注册表里的 VM agent；插件经 vtable 反查 QEMU
    /// display 端口——本 shim 不持 VM 状态）。
    agent: Option<String>,
    /// 观察档 `ro`（缺省）/ 操作档 `rw`；readonly 过滤在插件侧执行。
    mode: Option<String>,
}

/// `/ws/vm-display` 升级入口：运行闸（config display=vnc）→ token 闸 →
/// 插件闸（未装载 501）→ estop 闸 → upgrade。
pub(crate) async fn handle_vm_display_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<VmDisplayQuery>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    // 运行闸：config.json executor.vm.display（默认 none）。
    if read_display_config().is_none() {
        return (axum::http::StatusCode::NOT_FOUND, "vm display disabled").into_response();
    }
    // token 闸（镜像主 WS 语义：未配 token = 放行）。
    if !state.auth_token.is_empty() {
        let token = query.token.unwrap_or_default();
        if token != state.auth_token {
            tracing::warn!("[VmDisplay] WebSocket authentication failed");
            return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    // 插件闸：plugin-vm 未装载 → 诚实 501（display=vnc 已开、差的就是插件）。
    let Some(ep) = endpoint().cloned() else {
        return (
            axum::http::StatusCode::NOT_IMPLEMENTED,
            "显示插件未安装（plugin-vm）：display=vnc 需 plugin-vm 插件在位（exe 旁 plugins/ 目录）",
        )
            .into_response();
    };
    // estop 闸：连接时刻已处急停 → 拒绝。
    if state.estop.as_ref().is_some_and(|e| e.is_engaged()) {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "estop engaged").into_response();
    }
    // 档位：ro（缺省）/ rw；未知值诚实 400（不静默纠偏）。
    let mode = query.mode.as_deref().unwrap_or("ro");
    let mode = match mode {
        "ro" => "ro",
        "rw" => "rw",
        other => {
            return (
                axum::http::StatusCode::BAD_REQUEST,
                format!("未知 mode {other:?}（合法值：ro | rw）"),
            )
                .into_response();
        }
    };
    let agent = query.agent.unwrap_or_default();
    ws.on_upgrade(move |socket| {
        handle_vm_display_socket(socket, state, ep, agent, mode.to_string())
    })
}

/// 长度前缀帧读：4 字节大端长度 + 载荷（超限 = Err，防失常/恶意对端撑内存）。
async fn read_frame(
    reader: &mut (impl tokio::io::AsyncRead + Unpin),
    cap: usize,
) -> std::io::Result<Vec<u8>> {
    use tokio::io::AsyncReadExt;
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > cap {
        return Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            format!("frame {len} bytes exceeds cap {cap}"),
        ));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// 长度前缀帧写。
async fn write_frame(
    writer: &mut (impl tokio::io::AsyncWrite + Unpin),
    data: &[u8],
) -> std::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    writer.write_all(&(data.len() as u32).to_be_bytes()).await?;
    writer.write_all(data).await?;
    Ok(())
}

/// 会话守卫：任何退出路径都保证摘牌。
struct SessionGuard {
    mgr: &'static Arc<VmDisplayManager>,
    id: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.mgr.unregister(self.id);
    }
}

async fn handle_vm_display_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    ep: VmDisplayEndpoint,
    agent: String,
    mode: String,
) {
    // 连插件回环监听（connect 有超时；插件不在 = 诚实报错断开）。
    let addr = format!("127.0.0.1:{}", ep.listen_port);
    let tcp = match tokio::time::timeout(
        std::time::Duration::from_secs(3),
        tokio::net::TcpStream::connect(&addr),
    )
    .await
    {
        Ok(Ok(s)) => s,
        _ => {
            let _ = socket
                .send(Message::Text(
                    format!("显示插件连接失败（{addr}）：插件可能已退出").into(),
                ))
                .await;
            return;
        }
    };
    let (mut tcp_read, mut tcp_write) = tcp.into_split();

    // meta 握手（一行 JSON + \n——插件的会话定界；agent_id 缺省 = 主实例）。
    let session_id = format!("d-{}", chrono::Utc::now().timestamp_millis());
    let meta = serde_json::json!({
        "session_id": session_id,
        "agent_id": agent,
        "mode": mode,
    });
    use tokio::io::AsyncWriteExt;
    if let Err(e) = tcp_write.write_all(format!("{}\n", meta).as_bytes()).await {
        let _ = socket
            .send(Message::Text(format!("meta 握手写失败: {e}").into()))
            .await;
        return;
    }

    // 下行泵任务：插件 TCP → 帧解析 → channel（16 MiB cap 兜失常插件）。
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let down = tokio::spawn(async move {
        while let Ok(frame) = read_frame(&mut tcp_read, MAX_DOWNSTREAM_FRAME).await {
            if out_tx.send(frame).is_err() {
                break;
            }
        }
    });

    // 上行泵任务：channel → 帧写（WS 侧只投递，写串行化在独立任务）。
    let (in_tx, mut in_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    let up = tokio::spawn(async move {
        while let Some(chunk) = in_rx.recv().await {
            if write_frame(&mut tcp_write, &chunk).await.is_err() {
                break;
            }
        }
    });

    // 满员闸（注册即 enforce；拒绝经 WS Text 通知对端后关闭）。
    let (kill_tx, mut kill_rx) = tokio::sync::watch::channel(false);
    let Some(conn_id) = manager().register(kill_tx, MAX_DISPLAY_SESSIONS) else {
        let _ = socket
            .send(Message::Text(
                format!("显示会话数已达上限（{MAX_DISPLAY_SESSIONS}），拒绝新连接。").into(),
            ))
            .await;
        down.abort();
        up.abort();
        return;
    };
    let _guard = SessionGuard {
        mgr: manager(),
        id: conn_id,
    };
    tracing::info!("[VmDisplay] session {conn_id} opened (agent={agent}, mode={mode})");

    // 断连归并：管理器 kill 通道（estop/停机路径）+ estop watch（会话订阅）
    // 归并为一个 dying 布尔——任一置 true 即断。种子值兜「注册与订阅落定
    // 之间触发」的竞态窗口（PTY 同款纪律）。
    let mut estop_rx: Option<tokio::sync::watch::Receiver<bool>> =
        state.estop.as_ref().map(|e| e.subscribe());
    let mut dying = *kill_rx.borrow() || estop_rx.as_ref().is_some_and(|rx| *rx.borrow());

    loop {
        if dying {
            tracing::info!("[VmDisplay] estop/kill — disconnecting session {conn_id}");
            break;
        }
        tokio::select! {
            biased;
            maybe = async {
                tokio::select! {
                    r = kill_rx.changed() => r.ok(),
                    r = async {
                        match estop_rx.as_mut() {
                            Some(rx) => rx.changed().await.ok(),
                            None => std::future::pending().await,
                        }
                    } => r,
                }
            } => {
                let engaged = *kill_rx.borrow()
                    || estop_rx.as_ref().map(|rx| *rx.borrow()).unwrap_or(false);
                if maybe.is_some() && engaged {
                    dying = true;
                    continue;
                }
            }
            frame = out_rx.recv() => {
                match frame {
                    Some(data) => {
                        if socket.send(Message::Binary(data.into())).await.is_err() {
                            break;
                        }
                    }
                    // 插件侧关闭（VM 停止/插件退出）→ 会话结束。
                    None => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        if b.len() > MAX_UPSTREAM_FRAME {
                            break;
                        }
                        let _ = in_tx.send(b.to_vec());
                    }
                    // Text 帧：ping 保活（穿越代理/负载均衡）；其余忽略。
                    Some(Ok(Message::Text(t))) => {
                        if t.trim() == "ping" {
                            let _ = socket.send(Message::Text("pong".into())).await;
                        }
                    }
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // 清理：摘牌（guard Drop）+ 断两个泵任务（channel 关闭也会自然终止，
    // abort 兜秒退）。
    down.abort();
    up.abort();
    tracing::info!("[VmDisplay] session {conn_id} closed");
}

#[cfg(all(test, feature = "vm-display"))]
mod tests;
