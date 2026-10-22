//! plugin-vm — NemesisBot VM 显示插件（P5-R2 ⑤，cdylib）
//!
//! **VNC 语义全部住在插件里**：RFB 握手/像素/键鼠字节只是透传，本插件
//! 唯一「懂 VNC」的地方是 **readonly 过滤**——mode=ro 会话丢弃 RFB
//! client→server 写类消息（4=KeyEvent / 5=PointerEvent / 6=ClientCutText，
//! RFB 6.3 协议消息类型）。QEMU 层 `-vnc ...,readonly` 不用（锁死所有
//! viewer 且改档要重启 VM；per-session 过滤在插件侧天然按会话分档）。
//!
//! 架构（网关 shim ⇄ 本插件 ⇄ QEMU VNC）：
//!
//! ```text
//! 浏览器 WS ⇄ 网关 shim（VNC 无知帧中继）
//!     ⇄ TCP 127.0.0.1:<listen_port>（本插件 listener，init 时自选端口
//!       经 vtable init 上报宿主 → set_endpoint 落网关槽）
//!     ⇄ TCP 127.0.0.1:<vnc_port>（QEMU -vnc 回环，按 agent 经宿主
//!       vtable display_port_query 现查——插件不持 VM 状态）
//! ```
//!
//! 线协议（与网关 shim 约定，见 nemesis-web vm_display 模块文档）：
//! ① meta 握手 = 一行 JSON `{"session_id","agent_id","mode"}` + `\n`；
//! ② 此后双向均为 4 字节大端长度前缀帧（网关→插件 = RFB client→server
//!    字节；插件→网关 = 从 QEMU VNC 口读到的原始字节分块重帧）。
//!
//! 自带独立 tokio runtime（宿主进程可能没有tokio 线程可借——宿主 runtime
//! 属于 gateway，插件 runtime 自持自灭；`plugin_vm_display_free` 停监听 +
//! 收 runtime，幂等）。
//!
//! Build: `cargo build --release` → plugin_vm.dll / libplugin_vm.so
//! Install: 拷到 nemesisbot.exe 旁 `plugins/` 子目录（宿主 loader 探测名见下；
//! exe 同目录仅旧部署回落）。**单文件部署**——镜像构建脚本与 guest 双架构
//! ELF 全部内嵌在库本体（`resources` 模块 + `plugin_vm_resource_get` 导出），
//! 宿主用时释放到 workspace；**没有资源目录**（2026-10-22 拍板）。

// 同步副本（源：crates/nemesis-plugin/src/vm_display.rs，字节一致纪律）。
// 插件侧只消费部分常量/类型——未消费面 dead_code 允许（副本不因插件
// 视角裁剪，diff 可比对）。
#[allow(dead_code)]
mod vm_display;

mod resources;

#[cfg(test)]
mod resource_tests;

use std::ffi::{CStr, CString};
use std::io;
use std::os::raw::{c_char, c_int};
use std::sync::{Mutex, OnceLock};

use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::tcp::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::TcpStream;
use tokio::sync::watch;

use vm_display::{HostVmDisplayVTable, PluginVmInfo, HOST_VM_DISPLAY_ABI_VERSION,
    INIT_ERR_ABI_MISMATCH, INIT_ERR_INTERNAL, INIT_ERR_INVALID_ARG};

// ---------------------------------------------------------------------------
// 帧协议常量（与网关 shim 同数值：上行 1 MiB / 下行 16 MiB）
// ---------------------------------------------------------------------------

/// 单帧上限（双向同闸——网关侧上行 1 MiB 是浏览器语义，插件侧对宿主
/// 网关不设 1 MiB 硬闸只设 16 MiB 防失常：本插件与网关同信任域）。
const MAX_FRAME: u32 = 16 * 1024 * 1024;
/// meta 握手行上限（4 KiB——三个字段的 JSON 足够）。
const MAX_META_LINE: usize = 4 * 1024;
/// VNC→插件 下行读块（重帧粒度；64 KiB = 带宽与帧开销的平衡）。
const DOWN_CHUNK: usize = 64 * 1024;
/// QEMU VNC 连接超时。
const VNC_CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(3);

/// RFB client→server 写类消息类型（readonly 会话丢弃）：
/// 4=KeyEvent, 5=PointerEvent, 6=ClientCutText（RFB 6.3）。
fn is_write_message(t: u8) -> bool {
    matches!(t, 4..=6)
}

// ---------------------------------------------------------------------------
// 宿主 vtable 槽（init 时落，进程生命周期）
// ---------------------------------------------------------------------------

#[derive(Clone, Copy)]
struct HostFns {
    log: Option<extern "C" fn(level: i32, tag: *const c_char, msg: *const c_char)>,
    display_port_query: Option<extern "C" fn(agent_id: *const c_char) -> i32>,
    audit_event: Option<extern "C" fn(event: *const c_char, detail: *const c_char)>,
}

static HOST: OnceLock<HostFns> = OnceLock::new();

fn log(level: i32, msg: &str) {
    if let Some(f) = HOST.get().and_then(|h| h.log) {
        if let (Ok(t), Ok(m)) = (CString::new("plugin-vm"), CString::new(msg)) {
            f(level, t.as_ptr(), m.as_ptr());
        }
    }
}

fn audit(event: &str, detail: &str) {
    if let Some(f) = HOST.get().and_then(|h| h.audit_event) {
        if let (Ok(e), Ok(d)) = (CString::new(event), CString::new(detail)) {
            f(e.as_ptr(), d.as_ptr());
        }
    }
}

fn query_vnc_port(agent_id: &str) -> Option<u16> {
    let q = HOST.get()?.display_port_query?;
    let c = CString::new(agent_id).ok()?;
    let rc = q(c.as_ptr());
    if rc <= 0 {
        log(
            3,
            &format!(
                "display_port_query({agent_id}) = {rc}（-1 未知 / -2 未运行 / -3 无显示）"
            ),
        );
        return None;
    }
    // i32 端口 → u16 安全（宿主侧源自 u16）。
    u16::try_from(rc).ok()
}

// ---------------------------------------------------------------------------
// 插件状态（init/free 单写者——C ABI 侧 Mutex 守护）
// ---------------------------------------------------------------------------

struct PluginState {
    shutdown_tx: watch::Sender<bool>,
    thread: Option<std::thread::JoinHandle<()>>,
}

static STATE: Mutex<Option<PluginState>> = Mutex::new(None);

// ---------------------------------------------------------------------------
// C ABI 导出
// ---------------------------------------------------------------------------

/// `plugin_vm_display_init(host, out) -> i32`：校验 host ABI（双向闸
/// plugin 侧半边）→ 起独立 runtime + 回环 listener → 填 out。0 = 成功。
///
/// # Safety
/// `host` 必须指向有效 `HostVmDisplayVTable`（宿主保证进程生命周期）；
/// `out` 必须指向可写的 `PluginVmInfo`。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plugin_vm_display_init(
    host: *const HostVmDisplayVTable,
    out: *mut PluginVmInfo,
) -> c_int {
    if host.is_null() || out.is_null() {
        return INIT_ERR_INVALID_ARG;
    }
    let host_ref = unsafe { &*host };
    // ---- 双向 ABI 闸（plugin 侧半边） ----
    if host_ref.abi_version != HOST_VM_DISPLAY_ABI_VERSION {
        log(
            4,
            &format!(
                "host ABI v{} ≠ 插件 ABI v{HOST_VM_DISPLAY_ABI_VERSION}——拒绝初始化",
                host_ref.abi_version
            ),
        );
        return INIT_ERR_ABI_MISMATCH;
    }
    let fns = HostFns {
        log: host_ref.log,
        display_port_query: host_ref.display_port_query,
        audit_event: host_ref.audit_event,
    };
    if HOST.set(fns).is_err() {
        // 重复 init：幂等成功的前提是已初始化态还在（free 后再 init 走
        // 这里会撞旧 vtable 槽——函数指针同源无害，放行）。
        log(3, "重复 init（HOST 槽已占）——按幂等处理");
    }

    // ---- 回环 listener（std bind 后转 tokio，避免在 runtime 外建异步句柄） ----
    let std_listener = match std::net::TcpListener::bind(("127.0.0.1", 0)) {
        Ok(l) => l,
        Err(e) => {
            log(4, &format!("回环监听绑定失败: {e}"));
            return INIT_ERR_INTERNAL;
        }
    };
    let listen_port = match std_listener.local_addr() {
        Ok(a) => a.port(),
        Err(e) => {
            log(4, &format!("监听地址读取失败: {e}"));
            return INIT_ERR_INTERNAL;
        }
    };
    let _ = std_listener.set_nonblocking(true);

    // ---- 独立 tokio runtime（多线程小 runtime：accept + 会话泵） ----
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log(4, &format!("tokio runtime 启动失败: {e}"));
            return INIT_ERR_INTERNAL;
        }
    };
    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let thread = std::thread::Builder::new()
        .name("plugin-vm".into())
        .spawn(move || {
            runtime.block_on(async move {
                let listener = match tokio::net::TcpListener::from_std(std_listener) {
                    Ok(l) => l,
                    Err(e) => {
                        log(4, &format!("listener 转 tokio 失败: {e}"));
                        return;
                    }
                };
                let mut shutdown_rx = shutdown_rx;
                loop {
                    tokio::select! {
                        _ = shutdown_rx.changed() => break,
                        accepted = listener.accept() => {
                            match accepted {
                                Ok((stream, _addr)) => {
                                    tokio::spawn(handle_session(stream));
                                }
                                Err(e) => {
                                    log(3, &format!("accept 失败: {e}"));
                                    break;
                                }
                            }
                        }
                    }
                }
                // block_on 返回 → runtime drop（thread 闭包收尾）→ 残留
                // 会话任务被取消（free 语义 = 停机，不等待优雅收尾）。
            });
        });
    let thread = match thread {
        Ok(t) => Some(t),
        Err(e) => {
            log(4, &format!("插件线程启动失败: {e}"));
            return INIT_ERR_INTERNAL;
        }
    };

    if let Ok(mut st) = STATE.lock() {
        *st = Some(PluginState {
            shutdown_tx,
            thread,
        });
    }
    unsafe {
        (*out) = PluginVmInfo {
            plugin_abi_version: HOST_VM_DISPLAY_ABI_VERSION,
            listen_port,
        };
    }
    log(2, &format!("初始化完成：回环监听 127.0.0.1:{listen_port}"));
    0
}

/// `plugin_vm_display_free()`：停 listener + 收 runtime。幂等；插件未
/// 初始化 = no-op。
///
/// # Safety
/// 无（纯入口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plugin_vm_display_free() {
    let taken = STATE
        .lock()
        .ok()
        .and_then(|mut st| st.take());
    let Some(state) = taken else {
        return; // 未初始化 / 已释放：幂等 no-op
    };
    let _ = state.shutdown_tx.send(true);
    // shutdown_tx drop（State 解构）→ changed() Err 分支兜底。
    drop(state.shutdown_tx);
    if let Some(t) = state.thread {
        let _ = t.join();
    }
    log(2, "已释放（listener 停 + runtime 收）");
}

/// `plugin_vm_resource_get` 符号名（宿主 dlsym 用；宿主侧 vm_resources
/// 模块持同名字符串，两侧注释互指防漂移）。
pub const RESOURCE_GET_SYMBOL: &str = "plugin_vm_resource_get";

/// 返回码：资源在（out 已填）。
pub const RESOURCE_GET_OK: c_int = 0;
/// 返回码：该资源构建时未内嵌（guest 产物不在场——换全量构建的包）。
pub const RESOURCE_GET_ABSENT: c_int = 2;
/// 返回码：资源名未知 / 参数非法。
pub const RESOURCE_GET_UNKNOWN: c_int = 1;

/// `plugin_vm_resource_get(name, out_ptr, out_len) -> i32`：内嵌资源按名
/// 取字节指针（资源是 include 进 .rodata 的静态数据，库生命周期 =
/// 进程生命周期——宿主永不 unload 本插件，指针恒有效）。
///
/// 返回码：0 = ok；1 = 名未知；2 = 资源未内嵌（构建时产物缺席）；
/// 1 同时覆盖参数非法（宿主不会传 null——传了按未知处理，不 panic）。
///
/// # Safety
/// `name` 必须指向合法 C 字符串；`out_ptr`/`out_len` 必须指向可写内存。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn plugin_vm_resource_get(
    name: *const c_char,
    out_ptr: *mut *const u8,
    out_len: *mut usize,
) -> c_int {
    if name.is_null() || out_ptr.is_null() || out_len.is_null() {
        return RESOURCE_GET_UNKNOWN;
    }
    let name = unsafe { CStr::from_ptr(name) }.to_string_lossy();
    match crate::resources::resource(&name) {
        Some(bytes) => {
            unsafe {
                *out_ptr = bytes.as_ptr();
                *out_len = bytes.len();
            }
            RESOURCE_GET_OK
        }
        // guest 名在值域内但未内嵌 = ABSENT（区别于拼错名 UNKNOWN——
        // 宿主对 ABSENT 诚实报「插件缺该架构资源」，对 UNKNOWN 报 ABI 不符）。
        None if name == resources::RESOURCE_GUEST_X86_64
            || name == resources::RESOURCE_GUEST_AARCH64 =>
        {
            RESOURCE_GET_ABSENT
        }
        None => RESOURCE_GET_UNKNOWN,
    }
}

// ---------------------------------------------------------------------------
// 会话处理
// ---------------------------------------------------------------------------

#[derive(serde::Deserialize)]
struct SessionMeta {
    #[serde(default)]
    session_id: String,
    #[serde(default)]
    agent_id: String,
    /// `ro`（缺省）/ `rw`——未知值按 ro 处理（fail-safe：读多写少，
    /// readonly 是安全缺省）。
    #[serde(default)]
    mode: String,
}

/// 单显示会话：meta 握手 → 查 VNC 端口 → 连 QEMU → 双向泵。
async fn handle_session(stream: TcpStream) {
    let (mut sock_read, mut sock_write) = stream.into_split();

    // ---- meta 握手（一行 JSON + \n） ----
    let meta_line = match read_meta_line(&mut sock_read).await {
        Ok(l) => l,
        Err(e) => {
            log(3, &format!("meta 握手读失败: {e}"));
            return;
        }
    };
    let meta: SessionMeta = match serde_json::from_str(&meta_line) {
        Ok(m) => m,
        Err(e) => {
            log(3, &format!("meta 解析失败（{meta_line:?}）: {e}"));
            return;
        }
    };
    let readonly = meta.mode != "rw";
    audit(
        "session_open",
        &format!(
            "{{\"session_id\":{:?},\"agent_id\":{:?},\"mode\":{:?}}}",
            meta.session_id, meta.agent_id, meta.mode
        ),
    );

    // ---- VNC 端口解析（宿主 vtable 现查——插件不持 VM 状态） ----
    let Some(vnc_port) = query_vnc_port(&meta.agent_id) else {
        // 查询失败 = 断连（网关 down 泵见 EOF → 会话结束；不给浏览器
        // 伪造 RFB 数据）。
        log(3, &format!("agent {} 无可用 VNC 端口，断连", meta.agent_id));
        audit("session_reject", "{\"reason\":\"no_vnc_port\"}");
        return;
    };

    // ---- 连 QEMU VNC ----
    let vnc = match tokio::time::timeout(
        VNC_CONNECT_TIMEOUT,
        TcpStream::connect(("127.0.0.1", vnc_port)),
    )
    .await
    {
        Ok(Ok(s)) => s,
        _ => {
            log(3, &format!("QEMU VNC 127.0.0.1:{vnc_port} 连接失败/超时"));
            audit("session_reject", "{\"reason\":\"vnc_connect_failed\"}");
            return;
        }
    };
    let (mut vnc_read, mut vnc_write) = vnc.into_split();

    // ---- 双向泵 ----
    // 下行：VNC 原始字节 → 64 KiB 分块重帧 → 插件 socket → 网关 → 浏览器。
    let down_sock = &mut sock_write;
    let down = async move {
        let mut buf = vec![0u8; DOWN_CHUNK];
        loop {
            match vnc_read.read(&mut buf).await {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if write_frame(down_sock, &buf[..n]).await.is_err() {
                        break;
                    }
                }
            }
        }
    };

    // 上行：帧解析 → ro 模式丢弃写类消息（4/5/6）→ 原始字节写 VNC。
    let up_sock = &mut sock_read;
    let up = async move {
        while let Ok(frame) = read_frame(up_sock).await {
            if readonly && frame.first().is_some_and(|t| is_write_message(*t)) {
                continue; // readonly：丢弃，连接保持
            }
            if vnc_write.write_all(&frame).await.is_err() {
                break;
            }
        }
    };

    tokio::join!(down, up);
    audit("session_close", &format!("{{\"session_id\":{:?}}}", meta.session_id));
}

/// 读 meta 行（\n 结尾；超限 = Err）。
async fn read_meta_line(
    reader: &mut OwnedReadHalf,
) -> Result<String, io::Error> {
    let mut line = Vec::with_capacity(128);
    let mut byte = [0u8; 1];
    loop {
        let n = reader.read(&mut byte).await?;
        if n == 0 {
            return Err(io::Error::new(
                io::ErrorKind::UnexpectedEof,
                "meta line: eof before newline",
            ));
        }
        if byte[0] == b'\n' {
            break;
        }
        line.push(byte[0]);
        if line.len() > MAX_META_LINE {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "meta line exceeds cap",
            ));
        }
    }
    String::from_utf8(line)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, format!("meta 非 UTF-8: {e}")))
}

/// 长度前缀帧读（4 字节大端长度 + 载荷；超限 = Err）。
async fn read_frame(reader: &mut OwnedReadHalf) -> Result<Vec<u8>, io::Error> {
    let mut len_buf = [0u8; 4];
    reader.read_exact(&mut len_buf).await?;
    let len = u32::from_be_bytes(len_buf);
    if len > MAX_FRAME {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            format!("frame {len} exceeds cap {MAX_FRAME}"),
        ));
    }
    let mut buf = vec![0u8; len as usize];
    reader.read_exact(&mut buf).await?;
    Ok(buf)
}

/// 长度前缀帧写。
async fn write_frame(
    writer: &mut OwnedWriteHalf,
    data: &[u8],
) -> Result<(), io::Error> {
    writer.write_all(&(data.len() as u32).to_be_bytes()).await?;
    writer.write_all(data).await
}

/// CStr 辅助（vtable 回调侧的镜像；本文件 HostFns 存的是裸函数指针，
/// 参数侧CString 由调用点构造）。
#[allow(dead_code)]
fn cstr_lossy(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned()
}
