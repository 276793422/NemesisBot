//! plugin-mail——邮件基础设施插件（C-ABI 解冻第四例，新式交互插件样板）。
//!
//! 模块布局：
//! - [`mail`]：契约逐字节副本（宿主 `crates/nemesis-plugin/src/mail.rs`
//!   双向复制，drift 测试钉死）；
//! - [`store`]：maildir 按地址分库 + 地址注册表；
//! - [`smtp_server`]：SMTP 收信服务端（两级判定 + 拒中继 + 552 + STARTTLS +
//!   反垃圾闸）；
//! - [`pop3_server`]：POP3 取信服务端（USER 地址映射 + 并集 UIDL + POP3S +
//!   启动准入闸）；
//! - [`client`]：出站发信（465 implicit TLS + AUTH LOGIN）。
//!
//! 安全闸（产品层 fail-closed，不靠部署环境兜底）：POP3 非回环免认证拒绝
//! 启动、非回环明文默认拒绝（allow_plaintext_auth 显式逃生）；TLS 装配失败
//! = 启动失败；两面 idle 超时 + SMTP 反垃圾限值默认在位。
//!
//! 线程纪律（契约同文）：全部 JSON 导出内部 `block_on` 投插件自持 runtime
//! 阻塞等结果——宿主调用侧必须 `spawn_blocking`。STATE 串行语义：导出之间
//! 单线程串行（返回 JSON 静态缓冲 per-call 覆盖）。
//!
//! 返回 JSON 约定（契约同文）：成功 `{"ok":true,...}`；失败
//! `{"ok":false,"err_code":"...","err_msg":"..."}`。
//!
//! init 返回码：0 成功 / -1 空指针 / -2 ABI 失配 / -3 配置解析失败 /
//! -4 home 不可得 / -5 runtime 创建失败。

mod client;
mod mail;
mod pop3_server;
mod smtp_server;
mod store;

#[cfg(test)]
mod drift_tests;
#[cfg(test)]
mod tests;

pub use mail::*;

use std::cell::RefCell;
use std::ffi::{CStr, CString};
use std::os::raw::{c_char, c_int};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use parking_lot::{Mutex, RwLock};
use tokio::net::TcpListener;
use tokio::sync::watch;

use store::MailStore;

// ---------------------------------------------------------------------------
// 配置（`config.mail.json` resolved 形态；password_ref 宿主已解析为 password）
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
struct MailConfig {
    /// 总闸（宿主装配判断消费——插件不判，收下即可）。
    enabled: bool,
    smtp: smtp_server::SmtpSettings,
    pop3: pop3_server::Pop3Settings,
    client: client::ClientSettings,
}

// ---------------------------------------------------------------------------
// 面级 TLS + 会话闸基础件（SMTP STARTTLS / POP3S 共用）
// ---------------------------------------------------------------------------

/// 服务端 TLS 配置（SMTP STARTTLS / POP3S 共用形态；enabled 但装配失败 =
/// 拒绝启动，fail-closed）。
#[derive(Debug, Clone, Default, serde::Deserialize)]
#[serde(default)]
pub struct TlsServerSettings {
    pub enabled: bool,
    pub cert_path: String,
    pub key_path: String,
}

/// 面级 TLS 装配（fail-closed：enabled 但路径空/文件读失败/装配失败 = Err，
/// 调用方在 bind 前拒绝启动对应服务面；disabled = None 明文面）。
pub(crate) fn build_face_tls(
    tls: &TlsServerSettings,
    face: &str,
) -> Result<Option<Arc<rustls::ServerConfig>>, String> {
    if !tls.enabled {
        return Ok(None);
    }
    if tls.cert_path.is_empty() || tls.key_path.is_empty() {
        return Err(format!(
            "{face} TLS 已启用但证书/私钥路径为空——配置 {face}.tls.cert_path / key_path，或关闭 {face}.tls.enabled"
        ));
    }
    let cert_pem = std::fs::read(&tls.cert_path)
        .map_err(|e| format!("{face} TLS 证书读取失败 {}: {e}", tls.cert_path))?;
    let key_pem = std::fs::read(&tls.key_path)
        .map_err(|e| format!("{face} TLS 私钥读取失败 {}: {e}", tls.key_path))?;
    let cfg = nemesis_mail_protocol::tls_server_config(&cert_pem, &key_pem)?;
    Ok(Some(Arc::new(cfg)))
}

/// ServerConfig → acceptor（tokio-rustls 0.26 `From<Arc<ServerConfig>>`）。
pub(crate) fn to_acceptor(cfg: Arc<rustls::ServerConfig>) -> Arc<tokio_rustls::TlsAcceptor> {
    Arc::new(tokio_rustls::TlsAcceptor::from(cfg))
}

/// 秒数 → 会话 idle 超时（0 = 不限 → None）。
pub(crate) fn idle_or_max(secs: u64) -> Option<std::time::Duration> {
    if secs == 0 {
        None
    } else {
        Some(std::time::Duration::from_secs(secs))
    }
}

// ---------------------------------------------------------------------------
// 宿主回调桥（vtable 拷贝——契约：插件侧只读拷贝不存指针）
// ---------------------------------------------------------------------------

/// 宿主回调封装（init 时从 vtable 拷出的函数指针集）。日志不走这里——
/// 插件日志统一经模块级 [`log`]（HOST 静态槽），此处只承载事件类回调。
pub struct MailHost {
    audit_fn: Option<MailAuditFn>,
    notify_in_fn: Option<MailNotifyInFn>,
    notify_sent_fn: Option<MailNotifySentFn>,
}

impl MailHost {
    fn audit(&self, event: &str, detail: &str) {
        if let Some(f) = self.audit_fn {
            let (e, d) = (cstr_lossy(event), cstr_lossy(detail));
            f(e.as_ptr(), d.as_ptr());
        }
    }

    fn notify_mail_in(&self, meta_json: &str) {
        if let Some(f) = self.notify_in_fn {
            let j = cstr_lossy(meta_json);
            f(j.as_ptr());
        }
    }

    fn notify_mail_sent(&self, result_json: &str) {
        if let Some(f) = self.notify_sent_fn {
            let j = cstr_lossy(result_json);
            f(j.as_ptr());
        }
    }
}

/// CString 构造（内部含 NUL 截断——回调载荷不可能是合法 C 串，截断诚实）。
fn cstr_lossy(s: &str) -> CString {
    let cut = s.split('\0').next().unwrap_or("");
    CString::new(cut).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// 状态
// ---------------------------------------------------------------------------

static HOST: OnceLock<HostMailVTable> = OnceLock::new();
static STATE: Mutex<Option<Arc<MailState>>> = Mutex::new(None);

struct MailState {
    /// 配置（init 幂等重入 = 整体刷新；RwLock——刷新/读取分离）。
    cfg: RwLock<MailConfig>,
    store: Arc<MailStore>,
    host: Arc<MailHost>,
    runtime: tokio::runtime::Runtime,
    servers: Mutex<ServersState>,
}

struct ServersState {
    smtp: Option<RunningServer>,
    pop3: Option<RunningServer>,
}

struct RunningServer {
    port: u16,
    bind: String,
    /// 本面 TLS 是否在位（STARTTLS / POP3S——status 面投影）。
    tls: bool,
    shutdown: watch::Sender<bool>,
    join: Option<tokio::task::JoinHandle<()>>,
}

/// 模块级日志（经 HOST vtable 转发宿主 tracing；HOST 未挂 = 静默丢弃——
/// init 之前的日志无可转发对象）。
pub(crate) fn log(level: i32, tag: &str, msg: &str) {
    if let Some(vt) = HOST.get()
        && let Some(f) = vt.log
    {
        let (t, m) = (cstr_lossy(tag), cstr_lossy(msg));
        f(level, t.as_ptr(), m.as_ptr());
    }
}

// ---------------------------------------------------------------------------
// 返回 JSON 静态缓冲（per-call 覆盖；单线程串行纪律）
// ---------------------------------------------------------------------------

thread_local! {
    static OUT: RefCell<Vec<u8>> = RefCell::new(Vec::with_capacity(4096));
}

/// 结果写入静态缓冲并返回指针（显式 NUL 结尾——String 堆缓冲不保证 NUL，
/// CStr::from_ptr 会越界扫陈旧字节）。安全函数：只触 thread_local，无裸指针
/// 解引用（edition 2024 unsafe_op_in_unsafe_fn 下导出体保持无 unsafe 调用）。
fn set_out(s: &str) -> *const c_char {
    OUT.with(|buf| {
        let mut buf = buf.borrow_mut();
        buf.clear();
        buf.extend_from_slice(s.as_bytes());
        buf.push(0);
        buf.as_ptr() as *const c_char
    })
}

fn err_json(code: &str, msg: &str) -> String {
    serde_json::json!({"ok": false, "err_code": code, "err_msg": msg}).to_string()
}

/// 统一出口：Ok → 静态缓冲；Err → `{"ok":false,"err_code":"error",...}`。
fn finish(r: Result<String, String>) -> *const c_char {
    match r {
        Ok(json) => set_out(&json),
        Err(e) => set_out(&err_json("error", &e)),
    }
}

/// 取状态（Arc 克隆后立即放锁——导出之间串行语义由调用方保证，锁不跨 IO）。
fn with_state(f: impl FnOnce(&MailState) -> Result<String, String>) -> Result<String, String> {
    let st = STATE.lock().clone();
    match st {
        Some(st) => f(&st),
        None => Err("插件未初始化（mail_init 未调用或已停机）".to_string()),
    }
}

/// C 入参字符串收集（null 容忍）。裸指针解引用在内部 unsafe 块内收口，
/// 调用方（unsafe 导出体）不再需要 unsafe 块。
fn cstr_arg(arg: *const c_char) -> String {
    if arg.is_null() {
        return String::new();
    }
    unsafe { CStr::from_ptr(arg).to_string_lossy().into_owned() }
}

// ---------------------------------------------------------------------------
// 生命周期（3）
// ---------------------------------------------------------------------------

/// 读 ABI 版本（宿主装载闸第一道）。
#[unsafe(no_mangle)]
pub extern "C" fn mail_abi_version() -> u32 {
    HOST_MAIL_ABI_VERSION
}

/// 初始化：宿主回调表 + 配置 JSON。返回 0 = 成功；非 0 = 拒载（码表见
/// 模块头）。重复调用 = 刷新配置（幂等放行；home/运行态不动）。
///
/// # Safety
/// `vtable` 必须指向进程生命周期有效的 `HostMailVTable`（契约：宿主
/// static）；本函数只读拷贝字段，不存指针。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_init(
    vtable: *const HostMailVTable,
    config_json: *const c_char,
) -> c_int {
    if vtable.is_null() || config_json.is_null() {
        return -1;
    }
    let src = unsafe { &*vtable };
    let vt = HostMailVTable {
        abi_version: src.abi_version,
        log: src.log,
        get_home_dir: src.get_home_dir,
        audit_event: src.audit_event,
        notify_mail_in: src.notify_mail_in,
        notify_mail_sent: src.notify_mail_sent,
    };
    if vt.abi_version != HOST_MAIL_ABI_VERSION {
        return -2;
    }
    let cfg_str = unsafe { CStr::from_ptr(config_json) }
        .to_string_lossy()
        .into_owned();
    let cfg: MailConfig = match serde_json::from_str(&cfg_str) {
        Ok(c) => c,
        Err(e) => {
            // 配置坏 = 拒载（不静默落默认档——错配置上线是事故不是特性）。
            log(MAIL_LOG_ERROR, "init", &format!("config 解析失败: {e}"));
            return -3;
        }
    };
    let Some(home_fn) = vt.get_home_dir else {
        return -4;
    };
    let mut buf = [0 as c_char; 4096];
    let rc = home_fn(buf.as_mut_ptr(), buf.len());
    if rc <= 0 {
        return -4;
    }
    let home = unsafe { CStr::from_ptr(buf.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    if home.is_empty() {
        return -4;
    }
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(e) => {
            log(MAIL_LOG_ERROR, "init", &format!("runtime 创建失败: {e}"));
            return -5;
        }
    };

    let _ = HOST.set(vt);
    let stored = HOST.get().expect("HOST 刚刚 set");
    let host = Arc::new(MailHost {
        audit_fn: stored.audit_event,
        notify_in_fn: stored.notify_mail_in,
        notify_sent_fn: stored.notify_mail_sent,
    });
    let store = Arc::new(MailStore::new(PathBuf::from(&home).join("mail")));

    let mut state = STATE.lock();
    match state.as_ref() {
        Some(st) => {
            // 幂等重入 = 刷新配置（runtime/store 不动）。
            *st.cfg.write() = cfg;
            log(MAIL_LOG_INFO, "init", "配置刷新（幂等重入）");
            0
        }
        None => {
            *state = Some(Arc::new(MailState {
                cfg: RwLock::new(cfg),
                store,
                host,
                runtime,
                servers: Mutex::new(ServersState {
                    smtp: None,
                    pop3: None,
                }),
            }));
            log(MAIL_LOG_INFO, "init", "邮件基础设施插件初始化完成");
            0
        }
    }
}

/// 优雅停机（gateway 进程停机链专用）：停服务端 + runtime 后台收尾。
/// **estop 不触达本函数**（契约同文——信箱照常收发，急停冻结的是 agent）。
#[unsafe(no_mangle)]
pub extern "C" fn mail_shutdown() {
    let st = STATE.lock().take();
    let Some(st) = st else { return };
    stop_all_servers(&st);
    // runtime.shutdown_background 消费 self——此处 MailState Arc 是唯一引用
    // （STATE 已 take；服务端任务只持 store/host 克隆不持 state），try_unwrap
    // 成功 = 优雅后台停机；失败（防御臂）= 放弃 Arc 交 drop 收尾，语义不变。
    match Arc::try_unwrap(st) {
        Ok(state) => state.runtime.shutdown_background(),
        Err(_) => log(
            MAIL_LOG_WARN,
            "shutdown",
            "状态 Arc 仍被共享，runtime 交由 drop 收尾",
        ),
    }
    log(MAIL_LOG_INFO, "shutdown", "邮件基础设施插件停机完成");
}

// ---------------------------------------------------------------------------
// 服务端（3）
// ---------------------------------------------------------------------------

/// 启动服务端（override JSON 可覆盖 smtp/pop3 段；空/`{}` = 按 init 配置）。
/// 重复 start = 先停旧再起新（幂等热更；端口 0 = 临时端口带回）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_server_start(arg: *const c_char) -> *const c_char {
    let arg = cstr_arg(arg);
    finish(server_start_impl(&arg))
}

/// 回环绑址判定（127.0.0.1 / ::1 / localhost 之外的形态 = 非回环——
/// 0.0.0.0 与具名 IP 都算，准入闸的判定锚）。
pub(crate) fn is_loopback_bind(bind: &str) -> bool {
    matches!(bind, "127.0.0.1" | "::1" | "localhost")
}

fn server_start_impl(override_json: &str) -> Result<String, String> {
    with_state(|st| {
        let trimmed = override_json.trim();
        let override_cfg: Option<MailConfig> = if trimmed.is_empty() || trimmed == "{}" {
            None
        } else {
            Some(
                serde_json::from_str(trimmed)
                    .map_err(|e| format!("override JSON 解析失败: {e}"))?,
            )
        };
        let smtp_cfg = override_cfg
            .as_ref()
            .map(|c| c.smtp.clone())
            .unwrap_or_else(|| st.cfg.read().smtp.clone());
        let pop3_cfg = override_cfg
            .as_ref()
            .map(|c| c.pop3.clone())
            .unwrap_or_else(|| st.cfg.read().pop3.clone());

        // ---- 准入闸阶段（bind 前零副作用纯判定 + TLS 装配；失败 = 拒绝启动
        // 且旧服务不动——配置校验失败不产生「服务被停」副作用）----
        let smtp_tls = if smtp_cfg.enabled {
            build_face_tls(&smtp_cfg.tls, "SMTP")?.map(crate::to_acceptor)
        } else {
            None
        };
        let pop3_tls = if pop3_cfg.enabled {
            pop3_server::admission(&pop3_cfg)?
        } else {
            None
        };

        // 先停旧（重启语义：配置热更后 start 生效）。
        stop_all_servers(st);

        let mut servers = st.servers.lock();
        let mut smtp_info = serde_json::json!({"enabled": false});
        if smtp_cfg.enabled {
            let addr = format!("{}:{}", smtp_cfg.bind, smtp_cfg.port);
            let listener = st
                .runtime
                .block_on(TcpListener::bind(&addr))
                .map_err(|e| format!("SMTP bind {addr} 失败：{e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("SMTP local_addr 失败：{e}"))?
                .port();
            let (tx, rx) = watch::channel(false);
            let ctx = Arc::new(smtp_server::SmtpCtx {
                store: st.store.clone(),
                host: st.host.clone(),
            });
            let join = st.runtime.spawn(smtp_server::run(
                listener,
                ctx,
                Arc::new(smtp_cfg.clone()),
                smtp_tls.clone(),
                rx,
            ));
            servers.smtp = Some(RunningServer {
                port,
                bind: smtp_cfg.bind.clone(),
                tls: smtp_tls.is_some(),
                shutdown: tx,
                join: Some(join),
            });
            smtp_info = serde_json::json!(
                {"enabled": true, "bind": smtp_cfg.bind, "port": port, "tls": smtp_tls.is_some()}
            );
            if smtp_cfg.local_domains.is_empty() {
                // 计划 §53：空表 = 拒全部 + 启动 warn（诚实暴露「当前收不到
                // 任何信」——运维可感知，不是静默黑洞）。
                log(
                    MAIL_LOG_WARN,
                    "smtp",
                    "local_domains 为空表：将拒绝全部收信（550）——请配置域名后重启",
                );
            }
            if smtp_tls.is_some() {
                log(
                    MAIL_LOG_INFO,
                    "smtp",
                    "STARTTLS 已启用（opportunistic：EHLO 广告 250-STARTTLS，客户端支持即加密）",
                );
            } else if !is_loopback_bind(&smtp_cfg.bind) {
                // 非回环 + 无 TLS：投递通道明文 + 反垃圾闸是限值防护不是内容过滤
                // ——诚实暴露（域外/未注册恒 550 拒绝，无开放中继）。
                log(
                    MAIL_LOG_WARN,
                    "smtp",
                    &format!(
                        "非回环绑址 {bind}:{port} + 无 TLS：投递通道明文，任何人可向已注册地址投递\
                         （反垃圾限值防护已在产品层：单连接信数/RCPT 失败/idle 超时闸）；\
                         公网部署建议启用 smtp.tls（STARTTLS）",
                        bind = smtp_cfg.bind,
                        port = port
                    ),
                );
            }
            log(
                MAIL_LOG_INFO,
                "smtp",
                &format!("服务端已启动 {addr}（实际端口 {port}）"),
            );
        }

        let mut pop3_info = serde_json::json!({"enabled": false});
        if pop3_cfg.enabled {
            let addr = format!("{}:{}", pop3_cfg.bind, pop3_cfg.port);
            let listener = st
                .runtime
                .block_on(TcpListener::bind(&addr))
                .map_err(|e| format!("POP3 bind {addr} 失败：{e}"))?;
            let port = listener
                .local_addr()
                .map_err(|e| format!("POP3 local_addr 失败：{e}"))?
                .port();
            let (tx, rx) = watch::channel(false);
            let ctx = Arc::new(pop3_server::Pop3Ctx {
                store: st.store.clone(),
                host: st.host.clone(),
            });
            let join = st.runtime.spawn(pop3_server::run(
                listener,
                ctx,
                Arc::new(pop3_cfg.clone()),
                pop3_tls.clone(),
                rx,
            ));
            servers.pop3 = Some(RunningServer {
                port,
                bind: pop3_cfg.bind.clone(),
                tls: pop3_tls.is_some(),
                shutdown: tx,
                join: Some(join),
            });
            pop3_info = serde_json::json!(
                {"enabled": true, "bind": pop3_cfg.bind, "port": port, "tls": pop3_tls.is_some()}
            );
            if pop3_tls.is_some() {
                log(MAIL_LOG_INFO, "pop3", "POP3S 已启用（整面 implicit TLS）");
            } else if !is_loopback_bind(&pop3_cfg.bind) {
                // 走到这里 = admission 已放行 = 明文密码 + allow_plaintext_auth
                // 显式逃生——warn 语义随之更新（免认证/默认明文形态已在闸被拒）。
                log(
                    MAIL_LOG_WARN,
                    "pop3",
                    &format!(
                        "非回环绑址 {}:{port} + 明文密码（allow_plaintext_auth 显式放行）——\
                         认证传输未加密，LAN 内网自担风险；公网请启用 pop3.tls（POP3S）",
                        pop3_cfg.bind,
                        port = port
                    ),
                );
            }
            log(
                MAIL_LOG_INFO,
                "pop3",
                &format!("服务端已启动 {addr}（实际端口 {port}）"),
            );
        }

        st.host.audit("mail.server_start", &smtp_info.to_string());
        Ok(serde_json::json!({"ok": true, "smtp": smtp_info, "pop3": pop3_info}).to_string())
    })
}

/// 停服务端（两面全停；未运行 = 幂等 ok）。无入参（契约 `MailStrFn` 形态）。
///
/// # Safety
/// 无裸指针入参；宿主调用侧遵守模块头线程纪律（spawn_blocking）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_server_stop() -> *const c_char {
    finish(server_stop_impl())
}

fn server_stop_impl() -> Result<String, String> {
    with_state(|st| {
        stop_all_servers(st);
        st.host.audit("mail.server_stop", "{}");
        Ok(serde_json::json!({"ok": true}).to_string())
    })
}

/// 服务端运行态（smtp/pop3 各自运行/停机 + 端口）。无入参（契约 `MailStrFn`）。
///
/// # Safety
/// 无裸指针入参；宿主调用侧遵守模块头线程纪律（spawn_blocking）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_server_status() -> *const c_char {
    finish(server_status_impl())
}

fn server_status_impl() -> Result<String, String> {
    with_state(|st| {
        let servers = st.servers.lock();
        let face = |rs: Option<&RunningServer>| match rs {
            Some(s) => {
                serde_json::json!({"running": true, "bind": s.bind, "port": s.port, "tls": s.tls})
            }
            None => serde_json::json!({"running": false}),
        };
        Ok(serde_json::json!({
            "ok": true,
            "smtp": face(servers.smtp.as_ref()),
            "pop3": face(servers.pop3.as_ref()),
        })
        .to_string())
    })
}

fn stop_all_servers(st: &MailState) {
    // 逐面 take（MutexGuard 经 DerefMut 整体重借用——数组里两个 &mut 字段
    // 会撞 E0499，先取出再放锁）。
    let smtp = st.servers.lock().smtp.take();
    let pop3 = st.servers.lock().pop3.take();
    for rs in [smtp, pop3].into_iter().flatten() {
        let _ = rs.shutdown.send(true);
        drop(rs.shutdown); // sender 全 drop → changed() Err → run() 兜底退出
        if let Some(j) = rs.join {
            let _ = st.runtime.block_on(async {
                tokio::time::timeout(std::time::Duration::from_secs(3), j).await
            });
        }
    }
}

// ---------------------------------------------------------------------------
// 客户端（1）
// ---------------------------------------------------------------------------

/// 发信（三 C 字符串参）。
///
/// # Safety
/// 三个入参均须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_send(
    to: *const c_char,
    subject: *const c_char,
    body: *const c_char,
) -> *const c_char {
    let (to, subject, body) = (cstr_arg(to), cstr_arg(subject), cstr_arg(body));
    finish(send_impl(&to, &subject, &body))
}

fn send_impl(to: &str, subject: &str, body: &str) -> Result<String, String> {
    with_state(|st| {
        let client_cfg = st.cfg.read().client.clone();
        if !client_cfg.enabled {
            return Err("client 未启用（config.mail.json client.enabled）".to_string());
        }
        let message_id = st
            .runtime
            .block_on(client::send(&client_cfg, to, subject, body))?;
        st.host.audit(
            "mail.send",
            &serde_json::json!({"to": to, "subject": subject, "message_id": message_id})
                .to_string(),
        );
        st.host.notify_mail_sent(
            &serde_json::json!({
                "ok": true,
                "to": to,
                "subject": subject,
                "message_id": message_id,
                "ts_ms": now_ms(),
            })
            .to_string(),
        );
        Ok(serde_json::json!({"ok": true, "message_id": message_id}).to_string())
    })
}

// ---------------------------------------------------------------------------
// 存储（6）
// ---------------------------------------------------------------------------

/// 列信（query JSON；空 = 默认主库全量）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_list(arg: *const c_char) -> *const c_char {
    let arg = cstr_arg(arg);
    finish(list_impl(&arg))
}

fn list_impl(query_json: &str) -> Result<String, String> {
    with_state(|st| {
        let q: store::MailQuery =
            serde_json::from_str(query_json).map_err(|e| format!("query JSON 解析失败: {e}"))?;
        let items = st.store.list(&q)?;
        Ok(serde_json::json!({"ok": true, "items": items}).to_string())
    })
}

/// 读信全文（入参 = 邮件 id）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_read(arg: *const c_char) -> *const c_char {
    let id = cstr_arg(arg);
    finish(read_impl(&id))
}

fn read_impl(id: &str) -> Result<String, String> {
    with_state(|st| {
        let (meta, body) = st.store.read(id)?;
        Ok(serde_json::json!({"ok": true, "meta": meta, "body": body}).to_string())
    })
}

/// 标记已读（入参 = 邮件 id）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_mark_read(arg: *const c_char) -> *const c_char {
    let id = cstr_arg(arg);
    finish(mark_read_impl(&id))
}

fn mark_read_impl(id: &str) -> Result<String, String> {
    with_state(|st| {
        st.store.mark_read(id)?;
        Ok(serde_json::json!({"ok": true}).to_string())
    })
}

/// 删信（宿主治理面专用——agent 工具 schema 无此动作；入参 = 邮件 id）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_delete(arg: *const c_char) -> *const c_char {
    let id = cstr_arg(arg);
    finish(delete_impl(&id))
}

fn delete_impl(id: &str) -> Result<String, String> {
    with_state(|st| {
        st.store.delete(id)?;
        st.host
            .audit("mail.delete", &serde_json::json!({"id": id}).to_string());
        Ok(serde_json::json!({"ok": true}).to_string())
    })
}

/// 库统计（各注册地址总数/未读/字节）。
///
/// # Safety
/// 无裸指针入参；宿主调用侧遵守模块头线程纪律（spawn_blocking）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_stats() -> *const c_char {
    finish(stats_impl())
}

fn stats_impl() -> Result<String, String> {
    with_state(|st| st.store.stats().map(|v| v.to_string()))
}

/// 收信记录登记（宿主管外部来源元数据统一纳管；入参 = meta JSON）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_record(arg: *const c_char) -> *const c_char {
    let meta = cstr_arg(arg);
    finish(record_impl(&meta))
}

fn record_impl(meta_json: &str) -> Result<String, String> {
    with_state(|st| {
        let meta: serde_json::Value =
            serde_json::from_str(meta_json).map_err(|e| format!("meta JSON 解析失败: {e}"))?;
        let out = st.store.record(&meta)?;
        st.host.audit(
            "mail.record",
            &serde_json::to_string(&out).unwrap_or_default(),
        );
        Ok(out.to_string())
    })
}

// ---------------------------------------------------------------------------
// 地址（3）
// ---------------------------------------------------------------------------

/// 注册地址（入参 = `{"address": "..."}`；域闸在 store）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_addr_add(arg: *const c_char) -> *const c_char {
    let arg = cstr_arg(arg);
    finish(addr_add_impl(&arg))
}

fn addr_add_impl(arg_json: &str) -> Result<String, String> {
    with_state(|st| {
        let v: serde_json::Value =
            serde_json::from_str(arg_json).map_err(|e| format!("JSON 解析失败: {e}"))?;
        let address = v
            .get("address")
            .and_then(|a| a.as_str())
            .ok_or("缺 address 字段")?;
        let domains = st.cfg.read().smtp.local_domains.clone();
        st.store.addr_add(address, &domains)?;
        st.host.audit(
            "mail.addr_add",
            &serde_json::json!({"address": address}).to_string(),
        );
        Ok(serde_json::json!({"ok": true, "address": address}).to_string())
    })
}

/// 注销地址（只停收不删库；入参 = 地址裸串）。
///
/// # Safety
/// `arg` 须为 NUL 结尾合法 C 串指针或 null（内部 `cstr_arg` 收口）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_addr_remove(arg: *const c_char) -> *const c_char {
    let address = cstr_arg(arg);
    finish(addr_remove_impl(&address))
}

fn addr_remove_impl(address: &str) -> Result<String, String> {
    with_state(|st| {
        st.store.addr_remove(address)?;
        st.host.audit(
            "mail.addr_remove",
            &serde_json::json!({"address": address}).to_string(),
        );
        Ok(serde_json::json!({"ok": true, "address": address}).to_string())
    })
}

/// 全部注册地址（含默认主库标记）。
///
/// # Safety
/// 无裸指针入参；宿主调用侧遵守模块头线程纪律（spawn_blocking）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn mail_addr_list() -> *const c_char {
    finish(addr_list_impl())
}

fn addr_list_impl() -> Result<String, String> {
    with_state(|st| {
        let addrs = st.store.list_addresses()?;
        let list: Vec<String> = addrs.iter().map(|a| a.address.clone()).collect();
        let default = list.first().cloned().unwrap_or_default();
        Ok(serde_json::json!({"ok": true, "addresses": list, "default": default}).to_string())
    })
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_millis() as u64)
        .unwrap_or(0)
}
