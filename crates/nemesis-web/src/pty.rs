//! L8（devtool-upgrade 阶段 7）：WS PTY 内嵌终端（dashboard 终端 tab）。
//!
//! **人工通道，与 B4 后台注册表是两回事**：B4（background_registry）是
//! agent 通道——agent 发起 `background_start`/`output`/`kill`，注册表管
//! 生命周期；本模块是**人工通道**——dashboard 用户在浏览器里开交互终端
//! （xterm.js ↔ ConPTY/Unix PTY），不经 agent loop、不进工具注册表、
//! agent 不可见。两者共享的只有 estop（触发即 kill 全部 PTY 会话）与
//! 审计（各自落盘）。
//!
//! **安全模型（双闸 + 审计 + estop）**：
//! 1. `terminal` cargo feature（编译闸，IoT 裁剪可关）；
//! 2. `config.json` `terminal.enabled`（默认 **false**，运行闸）；
//! 3. 全 I/O 审计：`<workspace>/logs/terminal/pty_<ts>_<id>.log` 记录
//!    每帧输入/输出字节（**含回显的密码类输入**——文件头诚实注记）；
//! 4. estop 订阅：急停触发 → 全部 PTY 会话立即 kill。
//!
//! 诚实边界：交互 shell 无法逐命令审批（会话内 keystroke 不可能过安全
//! 8 层）、`restrict_to_workspace` 约不住 `cd`。信任级 = 「dashboard 登录
//! 用户」——远程可达性是 K3 `!cmd` 本地 CLI 没有的差异，靠 token 闸 +
//! 双闸 + 审计补偿。
//!
//! **协议**：独立 WS 端点 `/ws/pty?token=`（token 校验镜像主 WS）。
//! Binary 帧 = PTY 原始字节（免 base64）；Text 帧 = control JSON：
//! `{"type":"resize","cols":N,"rows":N}` / `{"type":"ping"}`（回 pong）。
//! 断连 = kill 会话（v1 无会话驻留/重连附接）。

use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use portable_pty::{CommandBuilder, PtySize, native_pty_system};
use serde::Deserialize;

use crate::api_handlers::AppState;

/// 初始 PTY 尺寸（前端连上后立即 resize 到实际视口）。
const INITIAL_COLS: u16 = 120;
const INITIAL_ROWS: u16 = 30;

// ---------------------------------------------------------------------------
// 会话管理器（estop kill + 满员闸的挂点）
// ---------------------------------------------------------------------------

/// 一条会话的终止句柄（portable-pty `clone_killer()` 的产物，专为跨线程
/// kill 设计——会话任务自己等子进程退出，管理器只管杀）。
struct PtySessionHandle {
    killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
}

/// PTY 会话管理器：注册表 + 满员闸 + estop kill-all 挂点。
///
/// 挂在模块级 [`PTY_MANAGER`]（OnceLock，`ensure_manager` 幂等初始化）——
/// 不进 `AppState`（字面量构造点遍布 69 个测试文件，加字段 = 全库爆破，
/// 同 L1 commands_registry 先例）。
pub struct PtySessionManager {
    inner: Mutex<HashMap<u64, PtySessionHandle>>,
    next_id: AtomicU64,
    /// shell 工作目录（通常是 workspace；None = 继承进程 cwd）。
    workspace: Option<String>,
}

impl PtySessionManager {
    pub fn new(workspace: Option<String>) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(HashMap::new()),
            next_id: AtomicU64::new(1),
            workspace,
        })
    }

    /// 注册会话；满员（`max_sessions`）返回 None。
    fn register(
        &self,
        killer: Box<dyn portable_pty::ChildKiller + Send + Sync>,
        max_sessions: u32,
    ) -> Option<u64> {
        let mut map = self.inner.lock().ok()?;
        if map.len() >= max_sessions as usize {
            return None;
        }
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        map.insert(id, PtySessionHandle { killer });
        Some(id)
    }

    fn unregister(&self, id: u64) {
        if let Ok(mut map) = self.inner.lock() {
            map.remove(&id);
        }
    }

    /// 当前会话数（Dashboard/诊断用）。
    pub fn session_count(&self) -> usize {
        self.inner.lock().map(|m| m.len()).unwrap_or(0)
    }

    /// kill 全部会话（estop 触发路径；单会话断连走 SessionGuard unregister
    /// + 会话任务内 kill，不经这里）。
    pub fn kill_all(&self) {
        let map = match self.inner.lock() {
            Ok(mut m) => std::mem::take(&mut *m),
            Err(_) => return,
        };
        for (id, mut h) in map {
            tracing::info!("[Pty] estop/session-sweep killing session {id}");
            let _ = h.killer.kill();
        }
    }
}

static PTY_MANAGER: OnceLock<Arc<PtySessionManager>> = OnceLock::new();

/// 幂等初始化模块级管理器（server 装配时调用；重复调用不覆盖）。
pub fn ensure_manager(workspace: Option<String>) -> Arc<PtySessionManager> {
    PTY_MANAGER
        .get_or_init(|| PtySessionManager::new(workspace))
        .clone()
}

fn manager() -> Option<&'static Arc<PtySessionManager>> {
    PTY_MANAGER.get()
}

// ---------------------------------------------------------------------------
// 配置解析 + shell 选择
// ---------------------------------------------------------------------------

/// 运行闸 + 参数（每次升级连接时 fresh-read config.json，改盘即生效）。
/// enabled=false（或 config 整个缺席）→ None = 端点 404。
fn read_terminal_config() -> Option<nemesis_config::TerminalConfig> {
    #[cfg(test)]
    {
        // 测试注入点优先——pty 单测**不安装进程级 config store**（原因见
        // [`TEST_TERMINAL_CFG`] 与 tests.rs 顶部说明）。
        if let Some(overridden) = TEST_TERMINAL_CFG.get() {
            return overridden.clone();
        }
    }
    let cfg = nemesis_config::load_live()?.terminal.unwrap_or_default();
    cfg.enabled.then_some(cfg)
}

/// 测试注入点（仅 `terminal` feature 的测试编译进本 crate）：pty 单测把
/// terminal 配置放这里，而不是 `set_global` 装进程级 store——config 全局
/// OnceLock first-wins 首装不可拆卸，装上后同 binary 里 models.rs
/// `load_config` 的全局优先分支（`load_live()` 短路、忽略 home）会把依赖
/// home 隔离的既有测试（handlers::tests 的 stress_* 家族）整体劫持到测试
/// 夹具上（stress_100_* 三连红的根因，2026-09-07 根修）。
#[cfg(test)]
static TEST_TERMINAL_CFG: OnceLock<Option<nemesis_config::TerminalConfig>> = OnceLock::new();

/// shell 选择：config `terminal.shell` 覆盖 > Windows powershell >
/// Unix `$SHELL` > `/bin/sh`。
fn resolve_shell(override_shell: Option<&str>) -> String {
    if let Some(s) = override_shell {
        return s.to_string();
    }
    #[cfg(windows)]
    {
        "powershell.exe".to_string()
    }
    #[cfg(not(windows))]
    {
        std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_string())
    }
}

// ---------------------------------------------------------------------------
// WS 升级 + 会话泵
// ---------------------------------------------------------------------------

#[derive(Deserialize)]
pub(crate) struct PtyQuery {
    token: Option<String>,
}

/// `/ws/pty` 升级入口：编译闸（feature）→ 运行闸（config）→ token 闸 →
/// 满员闸在 spawn 后 `register` 处。
pub(crate) async fn handle_pty_upgrade(
    ws: WebSocketUpgrade,
    Query(query): Query<PtyQuery>,
    State(state): State<Arc<AppState>>,
) -> axum::response::Response {
    // 运行闸：config.json terminal.enabled（默认 false）。端点关闭时
    // 404——不向未启用者暴露探测面。
    let Some(cfg) = read_terminal_config() else {
        return (axum::http::StatusCode::NOT_FOUND, "terminal disabled").into_response();
    };
    // token 闸（镜像 websocket_handler 主 WS 语义：未配 token = 放行）。
    if !state.auth_token.is_empty() {
        let token = query.token.unwrap_or_default();
        if token != state.auth_token {
            tracing::warn!("[Pty] WebSocket authentication failed");
            return (axum::http::StatusCode::UNAUTHORIZED, "unauthorized").into_response();
        }
    }
    let Some(mgr) = manager() else {
        return (
            axum::http::StatusCode::SERVICE_UNAVAILABLE,
            "pty manager not wired",
        )
            .into_response();
    };
    // 连接时刻已处急停 → 拒绝（不 spawn 新执行体）。
    if state.estop.as_ref().is_some_and(|e| e.is_engaged()) {
        return (axum::http::StatusCode::SERVICE_UNAVAILABLE, "estop engaged").into_response();
    }
    ws.on_upgrade(move |socket| handle_pty_socket(socket, state, mgr.clone(), cfg))
}

/// control 帧（Text）解析：resize / ping。
fn parse_control(text: &str) -> Control {
    match serde_json::from_str::<serde_json::Value>(text) {
        Ok(v) => match v.get("type").and_then(|t| t.as_str()) {
            Some("resize") => {
                let cols = v
                    .get("cols")
                    .and_then(|c| c.as_u64())
                    .unwrap_or(INITIAL_COLS as u64) as u16;
                let rows = v
                    .get("rows")
                    .and_then(|r| r.as_u64())
                    .unwrap_or(INITIAL_ROWS as u64) as u16;
                // 钳制到合理范围（防 0 / 疯狂值撑爆缓冲）。
                let cols = cols.clamp(2, 500);
                let rows = rows.clamp(2, 200);
                Control::Resize { cols, rows }
            }
            Some("ping") => Control::Ping,
            _ => Control::Ignore,
        },
        Err(_) => Control::Ignore,
    }
}

enum Control {
    Resize { cols: u16, rows: u16 },
    Ping,
    Ignore,
}

/// 审计文件句柄（None = 无 workspace，审计缺席 + 启动时 warn）。
struct AuditWriter {
    file: Option<std::fs::File>,
}

impl AuditWriter {
    fn open(workspace: Option<&str>, shell: &str, conn_id: u64) -> Self {
        let Some(ws) = workspace else {
            tracing::warn!("[Pty] session {conn_id} opened without workspace — audit disabled");
            return Self { file: None };
        };
        let dir = PathBuf::from(ws).join("logs").join("terminal");
        if let Err(e) = std::fs::create_dir_all(&dir) {
            tracing::warn!("[Pty] audit dir create failed: {e}");
            return Self { file: None };
        }
        let ts = chrono::Local::now().format("%Y%m%d_%H%M%S");
        let path = dir.join(format!("pty_{ts}_{conn_id}.log"));
        let file = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(&path)
            .ok();
        if let Some(mut f) = file {
            // 头部：诚实注记审计语义（std::fs 同步写，无 tokio 写缓冲
            // 家族风险；帧级 append 即落盘）。
            let header = format!(
                "# NemesisBot PTY audit\n# shell: {shell}\n# started: {}\n# 注意：全部输入/输出字节（含回显的密码类输入）都会记录在此。\n",
                chrono::Local::now().to_rfc3339()
            );
            let _ = f.write_all(header.as_bytes());
            Self { file: Some(f) }
        } else {
            tracing::warn!("[Pty] audit file open failed: {}", path.display());
            Self { file: None }
        }
    }

    /// 记一帧：dir = b"I"（客户端输入）/ b"O"（PTY 输出）。
    fn frame(&mut self, dir: &[u8], data: &[u8]) {
        if let Some(f) = self.file.as_mut() {
            let _ = f.write_all(&[b'[', dir[0], b']', b' ']);
            let _ = f.write_all(data);
            let _ = f.write_all(b"\n");
        }
    }
}

/// 会话守卫：任何退出路径（含 panic unwind）都保证摘牌。
struct SessionGuard {
    mgr: Arc<PtySessionManager>,
    id: u64,
}

impl Drop for SessionGuard {
    fn drop(&mut self) {
        self.mgr.unregister(self.id);
    }
}

async fn handle_pty_socket(
    mut socket: WebSocket,
    state: Arc<AppState>,
    mgr: Arc<PtySessionManager>,
    cfg: nemesis_config::TerminalConfig,
) {
    let shell = resolve_shell(cfg.shell.as_deref());
    let cwd = mgr.workspace.clone().map(PathBuf::from);

    // 开 PTY + spawn shell。
    let pty_system = native_pty_system();
    let pair = match pty_system.openpty(PtySize {
        rows: INITIAL_ROWS,
        cols: INITIAL_COLS,
        pixel_width: 0,
        pixel_height: 0,
    }) {
        Ok(p) => p,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("pty open failed: {e}").into()))
                .await;
            return;
        }
    };
    let mut cmd = CommandBuilder::new(&shell);
    if let Some(cwd) = &cwd {
        cmd.cwd(cwd);
    }
    #[cfg(not(windows))]
    cmd.env("TERM", "xterm-256color");
    let mut child = match pair.slave.spawn_command(cmd) {
        Ok(c) => c,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("shell spawn failed: {e}").into()))
                .await;
            return;
        }
    };
    // slave 用完即弃（spawn 后持有会挡 EOF 语义）。
    drop(pair.slave);

    // 满员闸。
    let Some(conn_id) = mgr.register(child.clone_killer(), cfg.max_sessions) else {
        let _ = socket
            .send(Message::Text(
                format!("终端会话数已达上限（{}），拒绝新连接。", cfg.max_sessions).into(),
            ))
            .await;
        let _ = child.kill();
        return;
    };
    let _guard = SessionGuard {
        mgr: mgr.clone(),
        id: conn_id,
    };
    tracing::info!(
        "[Pty] session {conn_id} opened (shell={shell}, cwd={})",
        cwd.as_ref()
            .map(|p| p.display().to_string())
            .unwrap_or_default()
    );

    let mut reader = match pair.master.try_clone_reader() {
        Ok(r) => r,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("pty reader failed: {e}").into()))
                .await;
            return;
        }
    };
    let writer = match pair.master.take_writer() {
        Ok(w) => w,
        Err(e) => {
            let _ = socket
                .send(Message::Text(format!("pty writer failed: {e}").into()))
                .await;
            return;
        }
    };
    let master = pair.master;
    let mut audit = AuditWriter::open(mgr.workspace.as_deref(), &shell, conn_id);

    // 读泵：PTY master → channel（阻塞 Read 放独立线程）。
    let (out_tx, mut out_rx) = tokio::sync::mpsc::unbounded_channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut buf = [0u8; 8192];
        loop {
            match reader.read(&mut buf) {
                Ok(0) => break,
                Ok(n) => {
                    if out_tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    });

    // 写泵：channel → PTY master writer（Write 放独立线程，ws 侧只投递）。
    let (in_tx, in_rx) = std::sync::mpsc::channel::<Vec<u8>>();
    std::thread::spawn(move || {
        let mut writer = writer;
        for chunk in in_rx {
            if writer.write_all(&chunk).is_err() {
                break;
            }
        }
    });

    // estop 订阅：急停触发 → 立即 kill 本会话。
    let mut estop_rx: Option<tokio::sync::watch::Receiver<bool>> =
        state.estop.as_ref().map(|e| e.subscribe());

    loop {
        tokio::select! {
            biased;
            // estop 优先（急停语义 = 冻结一切执行体）。
            maybe = async {
                match estop_rx.as_mut() {
                    Some(rx) => rx.changed().await.ok(),
                    None => std::future::pending().await,
                }
            } => {
                let engaged = estop_rx
                    .as_ref()
                    .map(|rx| *rx.borrow())
                    .unwrap_or(false);
                if maybe.is_some() && engaged {
                    tracing::info!("[Pty] estop engaged — killing session {conn_id}");
                    audit.frame(b"E", b"estop");
                    let _ = child.kill();
                    break;
                }
                // release（engaged=false）不打断会话。
            }
            data = out_rx.recv() => {
                match data {
                    Some(chunk) => {
                        audit.frame(b"O", &chunk);
                        if socket.send(Message::Binary(chunk.into())).await.is_err() {
                            break;
                        }
                    }
                    // PTY 关闭（shell 退出）→ 会话结束。
                    None => break,
                }
            }
            msg = socket.recv() => {
                match msg {
                    Some(Ok(Message::Binary(b))) => {
                        audit.frame(b"I", &b);
                        let _ = in_tx.send(b.to_vec());
                    }
                    Some(Ok(Message::Text(t))) => match parse_control(&t) {
                        Control::Resize { cols, rows } => {
                            let _ = master.resize(PtySize {
                                rows,
                                cols,
                                pixel_width: 0,
                                pixel_height: 0,
                            });
                        }
                        Control::Ping => {
                            let _ = socket.send(Message::Text("{\"type\":\"pong\"}".into())).await;
                        }
                        Control::Ignore => {}
                    },
                    Some(Ok(Message::Close(_))) | None => break,
                    Some(Ok(_)) => {}
                    Some(Err(_)) => break,
                }
            }
        }
    }

    // 清理：摘牌（guard Drop）+ kill + reap（阻塞 wait 放 blocking 任务）。
    let _ = child.kill();
    tokio::task::spawn_blocking(move || {
        let _ = child.wait();
    });
    tracing::info!("[Pty] session {conn_id} closed");
}

#[cfg(all(test, feature = "terminal"))]
mod tests;
