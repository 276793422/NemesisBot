//! plugin_mail 宿主装载器（邮件基础设施子系统；roaming 同款装载纪律第三例）。
//!
//! 双闸语义（顺序刻意）：
//! 1. **总闸**：`config.mail.json` 的 `enabled`（缺省 false）——装了 DLL 也
//!    不自动起（显式开启形态；gateway 装配点判，CLI 未来入口同款）；
//! 2. **名册开关闸**：`config.plugin.native.json` 的 plugin_mail 键（缺省
//!    启用）——vm/onnx/ui/roaming 同款义务（关了就是不装，文件在不在无关）。
//!
//! 装载链（任一步失败 = 诚实拒绝 + error 留痕，绝不静默降级）：
//! 0. 名册开关闸 → 1. 在场探测（[`crate::plugin_lib`]）→ 2. v4 签名四分支
//! 闸（Valid / Unsigned dev warn release 拒 / HardReject）→ 3. dlopen
//! `Box::leak`（进程生命周期——插件持 runtime/线程，unload 活着的东西是 UB）
//! → 4. `mail_abi_version` 双向闸 host 半边 → 5. 16 符号解析（ABI v1 冻结面
//! 完整——缺一即拒）→ 6. `mail_init(vtable, config_json)`（`password_ref`
//! 经宿主 vault 解析成 `password` 明文注入——插件不见 vault）→ 7. 句柄落
//! [`HANDLE`] 槽。
//!
//! notify/audit 面：audit_event 经静态槽分发（[`set_audit_sink`]，gateway
//! 注入 SecurityPlugin 审计链闭包；未注入 = tracing 留痕）；`notify_mail_in`
//! / `notify_mail_sent` 同款静态槽（[`set_notify_in_sink`] /
//! [`set_notify_sent_sink`]，B3 注入消费面——mailbox.updated WS push / 发信
//! 回执）；未注入 = tracing 留痕（事件不丢形态，只丢分发）。
//!
//! 停机：[`shutdown_plugin_mail`]（gateway 进程终态链）——新句柄同库 OS
//! 引用计数调 `mail_shutdown`（幂等）；**不 unload 库**。estop 不触达传输面
//! （SMTP/POP3 是邮件基础设施不是 agent 活动面——契约注释同文）。

use std::ffi::CStr;
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::{Arc, OnceLock};

use parking_lot::RwLock;

use nemesis_plugin::mail::{
    self, HOST_MAIL_ABI_VERSION, HostMailVTable, MAIL_ABI_VERSION_SYMBOL, MAIL_ADDR_ADD_SYMBOL,
    MAIL_ADDR_LIST_SYMBOL, MAIL_ADDR_REMOVE_SYMBOL, MAIL_DELETE_SYMBOL, MAIL_INIT_SYMBOL,
    MAIL_LIST_SYMBOL, MAIL_LOG_DEBUG, MAIL_LOG_INFO, MAIL_LOG_TRACE, MAIL_LOG_WARN,
    MAIL_MARK_READ_SYMBOL, MAIL_READ_SYMBOL, MAIL_RECORD_SYMBOL, MAIL_SEND_SYMBOL,
    MAIL_SERVER_START_SYMBOL, MAIL_SERVER_STATUS_SYMBOL, MAIL_SERVER_STOP_SYMBOL,
    MAIL_SHUTDOWN_SYMBOL, MAIL_STATS_SYMBOL, MailPluginHandle,
};

use crate::plugin_lib::{PluginLibSpec, SignatureVerdict, unsigned_dev_allow, verify_signature};

/// 装载结果（roaming 同款四态）。
#[derive(Debug)]
pub enum MailLoadOutcome {
    /// 插件文件缺席（常态，静默）。
    Absent,
    /// 装载成功。
    Loaded,
    /// 在场但装载被拒（签名/ABI/初始化失败——原因已留痕）。
    Rejected(String),
    /// 本地插件库开关停用（`config.plugin.native.json`）。
    DisabledBySwitch,
}

// ---------------------------------------------------------------------------
// 宿主静态槽（extern "C" fn 不能捕获——home 走 OnceLock；句柄/通知槽复用）
// ---------------------------------------------------------------------------

static HOME_ROOT: OnceLock<PathBuf> = OnceLock::new();
static HANDLE: OnceLock<MailPluginHandle> = OnceLock::new();

/// 通知分发槽（B3 注入；None = tracing 留痕）。
type NotifySink = Arc<dyn Fn(&str) + Send + Sync>;
static NOTIFY_IN_SINK: RwLock<Option<NotifySink>> = RwLock::new(None);
static NOTIFY_SENT_SINK: RwLock<Option<NotifySink>> = RwLock::new(None);

/// 注入收信通知消费面（mailbox.updated WS push / 通道回灌等）。
pub fn set_notify_in_sink(sink: NotifySink) {
    *NOTIFY_IN_SINK.write() = Some(sink);
}

/// 注入发信结果消费面（发信回执）。
pub fn set_notify_sent_sink(sink: NotifySink) {
    *NOTIFY_SENT_SINK.write() = Some(sink);
}

/// 审计分发槽（B3 注入安全审计链；None = tracing 留痕）。参数 = (event, detail)。
type AuditSink = Arc<dyn Fn(&str, &str) + Send + Sync>;
static AUDIT_SINK: RwLock<Option<AuditSink>> = RwLock::new(None);

/// 注入审计消费面（gateway 装配期接 SecurityPlugin `append_direct_audit_event`
/// ——收信/发信/删信/地址变更全事件进审计链；estop audit hook 同款形态）。
pub fn set_audit_sink(sink: AuditSink) {
    *AUDIT_SINK.write() = Some(sink);
}

// ---------------------------------------------------------------------------
// vtable 函数实现（extern "C"，读模块级静态）
// ---------------------------------------------------------------------------

extern "C" fn host_log(level: i32, tag: *const c_char, msg: *const c_char) {
    // tracing target 必须字面量——tag 并入消息体（转发不落盘，
    // 插件不落自己的日志文件）。
    let tag = unsafe { CStr::from_ptr(tag) }.to_string_lossy();
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    match level {
        MAIL_LOG_TRACE => tracing::trace!(target: "plugin_mail", "[{tag}] {msg}"),
        MAIL_LOG_DEBUG => tracing::debug!(target: "plugin_mail", "[{tag}] {msg}"),
        MAIL_LOG_INFO => tracing::info!(target: "plugin_mail", "[{tag}] {msg}"),
        MAIL_LOG_WARN => tracing::warn!(target: "plugin_mail", "[{tag}] {msg}"),
        _ => tracing::error!(target: "plugin_mail", "[{tag}] {msg}"),
    }
}

/// home 根回调（契约：邮件库 = `<home>/mail/`，插件自拼子目录——结构性
/// 隔离的路径锚点）。返回写入字节数（>0 成功 / -1 缓冲不足）。
extern "C" fn host_get_home_dir(buf: *mut c_char, buf_len: usize) -> i32 {
    let Some(home) = HOME_ROOT.get() else {
        return -1;
    };
    let Some(s) = home.to_str() else {
        return -1;
    };
    write_cstr(buf, buf_len, s)
}

/// C 字符串写入（截断 = -1，roaming 同款约定——契约同文长度语义勿改）。
fn write_cstr(buf: *mut c_char, buf_len: usize, s: &str) -> i32 {
    if buf.is_null() || buf_len == 0 {
        return -1;
    }
    let bytes = s.as_bytes();
    if bytes.len() + 1 > buf_len {
        return -1;
    }
    unsafe {
        std::ptr::copy_nonoverlapping(bytes.as_ptr() as *const c_char, buf, bytes.len());
        *buf.add(bytes.len()) = 0;
    }
    bytes.len() as i32
}

extern "C" fn host_audit_event(event: *const c_char, detail: *const c_char) {
    // 审计事件分发：槽在 = 进安全审计链（gateway 注入 append_direct_audit_event
    // 闭包）；槽空 = tracing 留痕（链未启用/测试形态——事件不丢形态）。
    let event = unsafe { CStr::from_ptr(event) }
        .to_string_lossy()
        .into_owned();
    let detail = unsafe { CStr::from_ptr(detail) }
        .to_string_lossy()
        .into_owned();
    let sink = AUDIT_SINK.read().clone();
    match sink {
        Some(f) => f(&event, &detail),
        None => tracing::info!(target: "plugin_mail", "[audit] {event} {detail}"),
    }
}

extern "C" fn host_notify_mail_in(meta_json: *const c_char) {
    let meta = unsafe { CStr::from_ptr(meta_json) }
        .to_string_lossy()
        .into_owned();
    let sink = NOTIFY_IN_SINK.read().clone();
    match sink {
        Some(f) => f(&meta),
        None => tracing::info!(target: "plugin_mail", "[mail_in] {meta}"),
    }
}

extern "C" fn host_notify_mail_sent(result_json: *const c_char) {
    let result = unsafe { CStr::from_ptr(result_json) }
        .to_string_lossy()
        .into_owned();
    let sink = NOTIFY_SENT_SINK.read().clone();
    match sink {
        Some(f) => f(&result),
        None => tracing::info!(target: "plugin_mail", "[mail_sent] {result}"),
    }
}

/// 契约要求 vtable 进程生命周期有效（宿主 static）——static 构造。
static HOST_VTABLE: HostMailVTable = HostMailVTable {
    abi_version: HOST_MAIL_ABI_VERSION,
    log: Some(host_log),
    get_home_dir: Some(host_get_home_dir),
    audit_event: Some(host_audit_event),
    notify_mail_in: Some(host_notify_mail_in),
    notify_mail_sent: Some(host_notify_mail_sent),
};

// ---------------------------------------------------------------------------
// 配置（总闸读取 + init JSON 构造单点）
// ---------------------------------------------------------------------------

/// 总闸：`config.mail.json` 的 `enabled` 键（缺文件/坏 JSON/缺键 = false
/// ——显式开启形态，静默关闭是默认态不是错误）。
pub fn mail_enabled(home: &Path) -> bool {
    read_mail_config_raw(home)
        .and_then(|v| v.get("enabled").and_then(|b| b.as_bool()))
        .unwrap_or(false)
}

/// 读 config.mail.json 原始 JSON（BOM 容忍；缺文件 = 空 `{}`）。
fn read_mail_config_raw(home: &Path) -> Option<serde_json::Value> {
    let path = crate::common::mail_config_path(home);
    let raw = std::fs::read_to_string(&path).ok()?;
    let raw = raw.trim_start_matches('\u{feff}');
    serde_json::from_str(raw).ok()
}

/// notify 通知段解析（[`crate::mail_notify::install`] 安装单点）：
/// `"notify": {"on": ["estop", "board_budget"], "to": "ops@corp.example"}`。
/// 缺段 = `None`（消费点全关，缺省静默）；段在 = (kinds, to)。
pub(crate) fn mail_notify_section(home: &Path) -> Option<(Vec<String>, String)> {
    let n = read_mail_config_raw(home)?.get("notify")?.clone();
    let kinds = n
        .get("on")
        .and_then(|a| a.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|s| s.as_str().map(str::to_string))
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    let to = n
        .get("to")
        .and_then(|s| s.as_str())
        .unwrap_or("")
        .to_string();
    Some((kinds, to))
}

/// init 配置构造单点：config.mail.json **raw 直传**（插件侧宽容解析落默认
/// 档；不走 typed 往返——roaming relay_init_json 同款纪律）+ `password_ref`
/// 经宿主 vault 解析成 `password` 明文注入（pop3/client 两段；插件不见
/// vault）。解析失败 = **fail-closed 拒载**（显式配了引用就解析不出明文，
/// 静默落空密码 = 开放认证面，是安全洞不是降级）。
pub fn mail_init_json(home: &Path) -> Result<String, String> {
    let mut v = read_mail_config_raw(home).unwrap_or_else(|| serde_json::json!({}));
    for (section, ctx) in [
        ("pop3", "mail.pop3.password_ref"),
        ("client", "mail.client.password_ref"),
    ] {
        let Some(obj) = v.get_mut(section).and_then(|s| s.as_object_mut()) else {
            continue;
        };
        let Some(reference) = obj
            .remove("password_ref")
            .and_then(|p| p.as_str().map(str::to_string))
        else {
            continue;
        };
        if reference.is_empty() {
            continue;
        }
        let resolved = nemesis_config::vault_ref::resolve_secret_field(&reference, ctx)
            .map_err(|e| format!("{ctx} 解析失败：{e}"))?;
        obj.insert("password".to_string(), serde_json::Value::String(resolved));
    }
    Ok(v.to_string())
}

// ---------------------------------------------------------------------------
// 装载主流程
// ---------------------------------------------------------------------------

/// 插件文件在场探测（纯文件面，不 dlopen——CLI `mail status` 消费口径）。
pub fn mail_plugin_present() -> bool {
    PluginLibSpec::MAIL.find_path().is_some()
}

/// 装载 plugin_mail（gateway 装配点调用；总闸 `mail_enabled` 由调用方
/// 前置判断——这里只管名册→签名→dlopen→init 链）。
///
/// `home`：插件侧邮件库路径根（契约 get_home_dir 回 home 根，插件拼
/// `mail/` 子目录——结构性隔离）。`config_dir`：名册开关闸
/// （`config.plugin.native.json`）读取根。幂等：已装载直接回 `Loaded`
/// （重复 load 不重复 dlopen；配置刷新走 init 幂等重入面）。
pub fn load_mail_plugin(home: &Path, config_dir: &Path) -> MailLoadOutcome {
    // ---- 名册开关闸（vm/onnx/ui/roaming 同款义务；文件探测之前） ----
    if !nemesis_config::plugins_native::load_enabled(
        config_dir,
        nemesis_config::plugins_native::NativePluginId::PluginMail,
    ) {
        return MailLoadOutcome::DisabledBySwitch;
    }
    if HANDLE.get().is_some() {
        return MailLoadOutcome::Loaded;
    }
    let Some(path) = PluginLibSpec::MAIL.find_path() else {
        return MailLoadOutcome::Absent;
    };

    // ---- 签名闸（四分支，roaming 同款） ----
    match verify_signature(&path) {
        SignatureVerdict::Valid => {}
        SignatureVerdict::Unsigned => {
            let msg = "plugin-mail 无 v4 签名（或 verify feature 关/锚缺席，无从验证）";
            if unsigned_dev_allow() {
                tracing::warn!(
                    "[plugin-mail] {msg}——开发形态 warn 放行（release 拒绝）：{}",
                    path.display()
                );
            } else {
                tracing::error!(
                    "[plugin-mail] {msg}，release 形态拒绝装载：{}",
                    path.display()
                );
                return MailLoadOutcome::Rejected(format!("{msg}，release 形态拒绝装载"));
            }
        }
        SignatureVerdict::HardReject(reason) => {
            tracing::error!(
                "[plugin-mail] v4 签名验证未过，拒绝装载（{}）：{reason}",
                path.display()
            );
            return MailLoadOutcome::Rejected(format!("签名验证未过：{reason}"));
        }
    }

    // ---- 宿主静态槽（vtable 函数读） ----
    let _ = HOME_ROOT.set(home.to_path_buf());

    // ---- dlopen（进程生命周期，Box::leak 刻意泄漏） ----
    let lib = match unsafe { libloading::Library::new(&path) } {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("库加载失败（{}）：{e}", path.display());
            tracing::error!("[plugin-mail] {msg}");
            return MailLoadOutcome::Rejected(msg);
        }
    };
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

    // ---- 双向闸 host 半边：abi_version 先查 ----
    macro_rules! need_sym {
        ($sym:expr, $ty:ty) => {
            match unsafe { lib.get::<$ty>($sym.as_bytes()) } {
                Ok(s) => *s,
                Err(e) => {
                    let msg = format!("插件缺 {} 导出（ABI v1 冻结面不完整）：{e}", $sym);
                    tracing::error!("[plugin-mail] {msg}");
                    return MailLoadOutcome::Rejected(msg);
                }
            }
        };
    }
    let abi_version: mail::MailAbiVersionFn =
        need_sym!(MAIL_ABI_VERSION_SYMBOL, mail::MailAbiVersionFn);
    let plugin_abi = abi_version();
    if plugin_abi != HOST_MAIL_ABI_VERSION {
        let msg = format!(
            "插件 ABI 版本不匹配：插件 v{plugin_abi}，宿主 v{HOST_MAIL_ABI_VERSION}\
             （升级或降级 plugin-mail 到匹配版本）"
        );
        tracing::error!("[plugin-mail] {msg}");
        return MailLoadOutcome::Rejected(msg);
    }

    // ---- 16 符号解析（冻结面完整——缺一即拒） ----
    let handle = MailPluginHandle {
        init: need_sym!(MAIL_INIT_SYMBOL, mail::MailInitFn),
        shutdown: need_sym!(MAIL_SHUTDOWN_SYMBOL, mail::MailShutdownFn),
        server_start: need_sym!(MAIL_SERVER_START_SYMBOL, mail::MailJsonFn),
        server_stop: need_sym!(MAIL_SERVER_STOP_SYMBOL, mail::MailStrFn),
        server_status: need_sym!(MAIL_SERVER_STATUS_SYMBOL, mail::MailStrFn),
        send: need_sym!(MAIL_SEND_SYMBOL, mail::MailSendFn),
        list: need_sym!(MAIL_LIST_SYMBOL, mail::MailJsonFn),
        read: need_sym!(MAIL_READ_SYMBOL, mail::MailJsonFn),
        mark_read: need_sym!(MAIL_MARK_READ_SYMBOL, mail::MailJsonFn),
        delete: need_sym!(MAIL_DELETE_SYMBOL, mail::MailJsonFn),
        stats: need_sym!(MAIL_STATS_SYMBOL, mail::MailStrFn),
        record: need_sym!(MAIL_RECORD_SYMBOL, mail::MailJsonFn),
        addr_add: need_sym!(MAIL_ADDR_ADD_SYMBOL, mail::MailJsonFn),
        addr_remove: need_sym!(MAIL_ADDR_REMOVE_SYMBOL, mail::MailJsonFn),
        addr_list: need_sym!(MAIL_ADDR_LIST_SYMBOL, mail::MailStrFn),
    };

    // ---- init（password_ref 已解析的配置装载 + runtime 创建；插件侧校验
    //      宿主 abi_version——双向闸） ----
    let config_json = match mail_init_json(home) {
        Ok(j) => j,
        Err(e) => {
            let msg = format!("init 配置构造失败（fail-closed）：{e}");
            tracing::error!("[plugin-mail] {msg}");
            return MailLoadOutcome::Rejected(msg);
        }
    };
    if let Err(e) = handle.init(&HOST_VTABLE, &config_json) {
        let msg = format!(
            "mail_init 失败：{e}（-1 空指针 / -2 ABI 失配 / -3 配置解析失败 / \
             -4 home 不可得 / -5 runtime 创建失败）"
        );
        tracing::error!("[plugin-mail] {msg}");
        return MailLoadOutcome::Rejected(msg);
    }

    match HANDLE.set(handle) {
        Ok(()) => {
            tracing::info!(
                "[plugin-mail] 装载成功（ABI v{plugin_abi}）：{}",
                path.display()
            );
            MailLoadOutcome::Loaded
        }
        Err(_) => MailLoadOutcome::Loaded, // 并发装载竞态：先到者已落槽，等价成功
    }
}

/// 已装载句柄（`None` = 插件未装载——缺席/总闸关/名册关/装载失败）。
pub fn plugin_handle() -> Option<&'static MailPluginHandle> {
    HANDLE.get()
}

// ---------------------------------------------------------------------------
// 调用辅助（同步 FFI 内 block_on 插件 runtime——宿主侧 spawn_blocking）
// ---------------------------------------------------------------------------

/// 插件调用统一入口：`spawn_blocking` 包装 + JSON 解析 + `ok:false` 展开。
/// 插件缺席 = 诚实 Err（调用方决定报错形态）。
pub async fn call_mail(
    what: &'static str,
    f: impl FnOnce(MailPluginHandle) -> Result<String, String> + Send + 'static,
) -> Result<serde_json::Value, String> {
    let handle = *plugin_handle().ok_or_else(|| format!("{what}: mail 插件未装载"))?;
    let raw = tokio::task::spawn_blocking(move || f(handle))
        .await
        .map_err(|e| format!("{what}: spawn_blocking join 失败: {e}"))??;
    let v: serde_json::Value =
        serde_json::from_str(&raw).map_err(|e| format!("{what}: 返回非 JSON: {e}（raw={raw}）"))?;
    if v.get("ok").and_then(|b| b.as_bool()) == Some(false) {
        let code = v
            .get("err_code")
            .and_then(|c| c.as_str())
            .unwrap_or("unknown");
        let msg = v.get("err_msg").and_then(|m| m.as_str()).unwrap_or("");
        return Err(format!("{what}: [{code}] {msg}"));
    }
    Ok(v)
}

/// 启动服务端（按 init 配置 smtp/pop3 各自 enabled 起面）。gateway 装配点
/// 装载成功后调用。
pub async fn mail_server_start() -> Result<serde_json::Value, String> {
    call_mail("mail_server_start", |h| h.server_start("{}")).await
}

/// 停服务端（两面全停；幂等）。
pub async fn mail_server_stop() -> Result<serde_json::Value, String> {
    call_mail("mail_server_stop", MailPluginHandle::server_stop).await
}

/// 服务端运行态快照。
#[allow(dead_code)]
pub async fn mail_server_status() -> Result<serde_json::Value, String> {
    call_mail("mail_server_status", MailPluginHandle::server_status).await
}

// ---------------------------------------------------------------------------
// 停机（gateway 进程终态链）
// ---------------------------------------------------------------------------

/// 调 `mail_shutdown`（若插件在位）。幂等；插件缺席 = no-op。**不 unload
/// 库**（unload 活线程是 UB——新句柄同库 OS 引用计数，roaming 同款）。
pub fn shutdown_plugin_mail() {
    let Some(path) = PluginLibSpec::MAIL.find_path() else {
        return;
    };
    unsafe {
        let Ok(lib) = libloading::Library::new(&path) else {
            return;
        };
        // 新句柄与 init 时句柄同指一个库（OS 引用计数）——shutdown 幂等
        // 语义由此安全。
        if let Ok(shutdown) = lib.get::<mail::MailShutdownFn>(MAIL_SHUTDOWN_SYMBOL.as_bytes()) {
            shutdown();
        }
    }
}

#[cfg(test)]
mod tests;
