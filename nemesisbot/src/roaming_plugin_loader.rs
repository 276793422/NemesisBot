//! plugin_roaming 宿主装载器（C-ABI 解冻第二例：新式交互插件样板；与
//! [`crate::vm_display_loader`] 同一套装载纪律，差异在交互形态——见下）。
//!
//! 与 plugin-vm（老式功能插件）的三点差异：
//! 1. **常驻服务**：插件自持 tokio runtime，中继/名册常驻监听——宿主侧
//!    句柄落 [`HANDLE`] 槽复用（不是装载即完事）；
//! 2. **同步 FFI**：插件导出内部 `block_on` 投 runtime（拍板 10），宿主
//!    调用侧一律 [`call_roaming`]（`spawn_blocking` 包装，不卡 executor 线程）；
//! 3. **vtable 必须static**：契约要求 vtable 所指内存进程生命周期有效
//!    （[`HOST_VTABLE`]——插件侧虽只拷字段，宿主侧照契约给稳态）。
//!
//! 装载链（任一步失败 = 诚实拒绝 + warn/error 留痕，绝不静默降级）：
//! 1. 在场探测：[`crate::plugin_lib`]（单一真相源，VM 同款查找链）——
//!    缺席 = `Absent`（常态；服务端面诚实降级，装载失败才是错误）；
//! 2. v4 签名闸：Valid 放行 / Unsigned dev warn 放行 release 拒 / 其余拒；
//! 3. dlopen → `Box::leak`（进程生命周期——插件持 runtime/线程，unload
//!    活着的东西是 UB，vm-display 同款）；
//! 4. `roaming_abi_version()` 先查（双向闸 host 半边；init 内插件侧再校验）；
//! 5. 19 符号解析（ABI v2 冻结面完整——缺一即拒，不做部分装载）；
//! 6. `roaming_init(vtable, config_json)`（配置装载 + runtime 创建）；
//! 7. 句柄落 [`HANDLE`] 槽，后续 start/stop/status/... 全走槽内句柄。
//!
//! 停机：[`shutdown_plugin_roaming`]（gateway 进程终态链）——新句柄同库
//! OS 引用计数调 `roaming_shutdown`（幂等）；**不 unload 库**。
//!
//! estop 不触达传输面（中继是传输设施不是 agent 活动面——契约注释同文）。

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use nemesis_plugin::roaming::{
    self, HOST_ROAMING_ABI_VERSION, HostRoamingVTable, ROAMING_ABI_VERSION_SYMBOL,
    ROAMING_CLOSE_CONN_SYMBOL, ROAMING_DIAL_SYMBOL, ROAMING_ENDPOINT_START_SYMBOL,
    ROAMING_ENDPOINT_STOP_SYMBOL, ROAMING_GET_IROH_ID_SYMBOL, ROAMING_IDENTITY_KEYGEN_SYMBOL,
    ROAMING_IDENTITY_LOAD_SYMBOL, ROAMING_INIT_SYMBOL, ROAMING_LOG_DEBUG, ROAMING_LOG_INFO,
    ROAMING_LOG_TRACE, ROAMING_LOG_WARN, ROAMING_REGISTRY_START_SYMBOL,
    ROAMING_REGISTRY_STOP_SYMBOL, ROAMING_SET_IDENTITY_SYMBOL, ROAMING_SHUTDOWN_SYMBOL,
    ROAMING_START_SYMBOL, ROAMING_STATUS_SYMBOL, ROAMING_STOP_SYMBOL, ROAMING_TRUST_LIST_SYMBOL,
    ROAMING_TRUST_REVOKE_SYMBOL, ROAMING_TRUST_TRUST_SYMBOL, ROAMING_TRUST_UNREVOKE_SYMBOL,
    RoamingPluginHandle,
};

use crate::plugin_lib::{PluginLibSpec, SignatureVerdict, unsigned_dev_allow, verify_signature};

/// 装载结果：`Absent` = 插件文件缺席（常态，静默）；`Loaded` = 装载成功；
/// `Rejected` = 在场但装载被拒（签名/ABI/初始化失败——原因已留痕）。
#[derive(Debug)]
pub enum RoamingLoadOutcome {
    Absent,
    Loaded,
    Rejected(String),
}

// ---------------------------------------------------------------------------
// 宿主静态槽（extern "C" fn 不能捕获——workspace 走 OnceLock；句柄槽复用）
// ---------------------------------------------------------------------------

static WORKSPACE_ROOT: OnceLock<PathBuf> = OnceLock::new();
static HANDLE: OnceLock<RoamingPluginHandle> = OnceLock::new();

// ---------------------------------------------------------------------------
// vtable 函数实现（extern "C"，读模块级静态）
// ---------------------------------------------------------------------------

extern "C" fn host_log(level: i32, tag: *const c_char, msg: *const c_char) {
    // tracing target 必须字面量——tag 并入消息体（拍板 5：转发不落盘，
    // 插件不落自己的日志文件）。
    let tag = unsafe { CStr::from_ptr(tag) }.to_string_lossy();
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    match level {
        ROAMING_LOG_TRACE => tracing::trace!(target: "plugin_roaming", "[{tag}] {msg}"),
        ROAMING_LOG_DEBUG => tracing::debug!(target: "plugin_roaming", "[{tag}] {msg}"),
        ROAMING_LOG_INFO => tracing::info!(target: "plugin_roaming", "[{tag}] {msg}"),
        ROAMING_LOG_WARN => tracing::warn!(target: "plugin_roaming", "[{tag}] {msg}"),
        _ => tracing::error!(target: "plugin_roaming", "[{tag}] {msg}"),
    }
}

extern "C" fn host_get_workspace_dir(buf: *mut c_char, buf_len: usize) -> i32 {
    let Some(ws) = WORKSPACE_ROOT.get() else {
        return -1;
    };
    let Some(s) = ws.to_str() else {
        return -1;
    };
    write_cstr(buf, buf_len, s)
}

/// C 字符串写入（截断 = -1，vm-display 同款约定）。
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
    // M1：审计事件透传 tracing（审计链接线随 M2/M3 拨号/入站面一起进）。
    let event = unsafe { CStr::from_ptr(event) }.to_string_lossy();
    let detail = unsafe { CStr::from_ptr(detail) }.to_string_lossy();
    tracing::info!(target: "plugin_roaming", "[audit] {event} {detail}");
}

extern "C" fn host_notify_incoming(conn_id: u64, port: u16, remote_hex: *const c_char) {
    // M3 接线：插件 accept iroh 连接后开 loopback 端口 → 宿主受理
    // （TcpStream::connect + 帧协议照跑）。v2 追加 remote_hex（对端
    // EndpointId hex）——经桥分发给受理器漏斗（server 臂消费），日志留痕。
    let remote = if remote_hex.is_null() {
        String::new()
    } else {
        unsafe { CStr::from_ptr(remote_hex) }
            .to_string_lossy()
            .into_owned()
    };
    nemesis_cluster::roaming::plugin_bridge::dispatch_incoming(conn_id, port, &remote);
    tracing::info!(
        target: "plugin_roaming",
        "[incoming] conn={conn_id} port={port} remote=iroh:{remote}"
    );
}

extern "C" fn host_notify_members(json: *const c_char) {
    // QUIC 插件化 M2 接线：名册循环拉到成员 → 桥漏斗（集群
    // merge_rendezvous_members）。漏斗未挂载（集群装配前/已停机）时桥内
    // debug 丢弃——插件下 tick 重推全量包，自愈语义无需补发。
    let json = unsafe { CStr::from_ptr(json) }.to_string_lossy();
    nemesis_cluster::roaming::plugin_bridge::dispatch_members_json(&json);
    tracing::debug!(target: "plugin_roaming", "[members] {json}");
}

/// 契约要求 vtable 进程生命周期有效（宿主用 static）——static 构造，
/// 函数指针全 const 可建。
static HOST_VTABLE: HostRoamingVTable = HostRoamingVTable {
    abi_version: HOST_ROAMING_ABI_VERSION,
    log: Some(host_log),
    get_workspace_dir: Some(host_get_workspace_dir),
    audit_event: Some(host_audit_event),
    notify_incoming: Some(host_notify_incoming),
    notify_members: Some(host_notify_members),
};

// ---------------------------------------------------------------------------
// 装载主流程
// ---------------------------------------------------------------------------

/// 插件文件在场探测（纯文件面，不 dlopen——CLI 动态隐藏用这个口径）。
pub fn roaming_plugin_present() -> bool {
    PluginLibSpec::ROAMING.find_path().is_some()
}

/// 装载 plugin_roaming（gateway 装配点 / CLI `quic relay` 入口调用）。
///
/// `workspace`：插件侧自管文件（roaming.key / trust.toml / relay 证书）
/// 路径解析根——与 gateway setup_roaming 同口径（workspace 根，非 home）。
/// `config_json`：init 配置，`{"relay": <config.quic.json relay 原节>}`——
/// raw 直传不走 typed 往返（插件 parse_relay_settings 与宿主 quic_config
/// 逐字段同源宽容解析）。幂等：已装载直接回 `Loaded`（init 重入 = 刷新
/// 配置，插件侧语义承接；重复 load 不重复 dlopen）。
/// init 配置构造单点：读 config.quic.json 取 `relay` 段 **raw 直传**，
/// 包成 `{"relay": <原节>}` 字符串（[`load_roaming_plugin`] 的 `config_json`
/// 形参口径）。文件缺席 / 无 relay 段 / 非对象 = `{}`（空节——插件侧宽容
/// 解析落默认档）。gateway assemble 与 CLI `quic relay` 两入口同源。
pub fn relay_init_json(quic_config_path: &Path) -> String {
    let relay_section = std::fs::read_to_string(quic_config_path)
        .ok()
        .and_then(|s| serde_json::from_str::<serde_json::Value>(&s).ok())
        .and_then(|v| v.get("relay").cloned())
        .filter(|v| v.is_object())
        .unwrap_or_else(|| serde_json::json!({}));
    serde_json::json!({ "relay": relay_section }).to_string()
}

pub fn load_roaming_plugin(workspace: &Path, config_json: &str) -> RoamingLoadOutcome {
    if HANDLE.get().is_some() {
        return RoamingLoadOutcome::Loaded;
    }
    let Some(path) = PluginLibSpec::ROAMING.find_path() else {
        return RoamingLoadOutcome::Absent;
    };

    // ---- 签名闸（vm-display 同款四分支） ----
    match verify_signature(&path) {
        SignatureVerdict::Valid => {}
        SignatureVerdict::Unsigned => {
            let msg = "plugin-roaming 无 v4 签名（或 verify feature 关/锚缺席，无从验证）";
            if unsigned_dev_allow() {
                tracing::warn!(
                    "[plugin-roaming] {msg}——开发形态 warn 放行（release 拒绝）：{}",
                    path.display()
                );
            } else {
                tracing::error!(
                    "[plugin-roaming] {msg}，release 形态拒绝装载：{}",
                    path.display()
                );
                return RoamingLoadOutcome::Rejected(format!("{msg}，release 形态拒绝装载"));
            }
        }
        SignatureVerdict::HardReject(reason) => {
            tracing::error!(
                "[plugin-roaming] v4 签名验证未过，拒绝装载（{}）：{reason}",
                path.display()
            );
            return RoamingLoadOutcome::Rejected(format!("签名验证未过：{reason}"));
        }
    }

    // ---- 宿主静态槽（vtable 函数读） ----
    let _ = WORKSPACE_ROOT.set(workspace.to_path_buf());

    // ---- dlopen（进程生命周期，Box::leak 刻意泄漏） ----
    let lib = match unsafe { libloading::Library::new(&path) } {
        Ok(l) => l,
        Err(e) => {
            let msg = format!("库加载失败（{}）：{e}", path.display());
            tracing::error!("[plugin-roaming] {msg}");
            return RoamingLoadOutcome::Rejected(msg);
        }
    };
    let lib: &'static libloading::Library = Box::leak(Box::new(lib));

    // ---- 双向闸 host 半边：abi_version 先查 ----
    macro_rules! need_sym {
        ($sym:expr, $ty:ty) => {
            match unsafe { lib.get::<$ty>($sym.as_bytes()) } {
                Ok(s) => *s,
                Err(e) => {
                    let msg = format!(
                        "插件缺 {} 导出（ABI v2 冻结面不完整）：{e}",
                        $sym
                    );
                    tracing::error!("[plugin-roaming] {msg}");
                    return RoamingLoadOutcome::Rejected(msg);
                }
            }
        };
    }
    let abi_version: roaming::RoamingAbiVersionFn =
        need_sym!(ROAMING_ABI_VERSION_SYMBOL, roaming::RoamingAbiVersionFn);
    let plugin_abi = abi_version();
    if plugin_abi != HOST_ROAMING_ABI_VERSION {
        let msg = format!(
            "插件 ABI 版本不匹配：插件 v{plugin_abi}，宿主 v{HOST_ROAMING_ABI_VERSION}\
             （升级或降级 plugin-roaming 到匹配版本）"
        );
        tracing::error!("[plugin-roaming] {msg}");
        return RoamingLoadOutcome::Rejected(msg);
    }

    // ---- 19 符号解析（冻结面完整——缺一即拒） ----
    let handle = RoamingPluginHandle {
        init: need_sym!(ROAMING_INIT_SYMBOL, roaming::RoamingInitFn),
        start: need_sym!(ROAMING_START_SYMBOL, roaming::RoamingStrFn),
        stop: need_sym!(ROAMING_STOP_SYMBOL, roaming::RoamingStrFn),
        status: need_sym!(ROAMING_STATUS_SYMBOL, roaming::RoamingStrFn),
        shutdown: need_sym!(ROAMING_SHUTDOWN_SYMBOL, roaming::RoamingShutdownFn),
        dial: need_sym!(ROAMING_DIAL_SYMBOL, roaming::RoamingDialFn),
        close_conn: need_sym!(ROAMING_CLOSE_CONN_SYMBOL, roaming::RoamingCloseConnFn),
        identity_keygen: need_sym!(ROAMING_IDENTITY_KEYGEN_SYMBOL, roaming::RoamingJsonFn),
        identity_load: need_sym!(ROAMING_IDENTITY_LOAD_SYMBOL, roaming::RoamingJsonFn),
        get_iroh_id: need_sym!(ROAMING_GET_IROH_ID_SYMBOL, roaming::RoamingJsonFn),
        identity_set: need_sym!(ROAMING_SET_IDENTITY_SYMBOL, roaming::RoamingJsonFn),
        trust_trust: need_sym!(ROAMING_TRUST_TRUST_SYMBOL, roaming::RoamingJsonFn),
        trust_revoke: need_sym!(ROAMING_TRUST_REVOKE_SYMBOL, roaming::RoamingJsonFn),
        trust_unrevoke: need_sym!(ROAMING_TRUST_UNREVOKE_SYMBOL, roaming::RoamingJsonFn),
        trust_list: need_sym!(ROAMING_TRUST_LIST_SYMBOL, roaming::RoamingJsonFn),
        registry_start: need_sym!(
            ROAMING_REGISTRY_START_SYMBOL,
            roaming::RoamingRegistryStartFn
        ),
        registry_stop: need_sym!(ROAMING_REGISTRY_STOP_SYMBOL, roaming::RoamingStrFn),
        endpoint_start: need_sym!(
            ROAMING_ENDPOINT_START_SYMBOL,
            roaming::RoamingEndpointStartFn
        ),
        endpoint_stop: need_sym!(ROAMING_ENDPOINT_STOP_SYMBOL, roaming::RoamingStrFn),
    };

    // ---- init（配置装载 + runtime 创建；插件侧校验宿主 abi_version——双向） ----
    let cfg_c = match CString::new(config_json) {
        Ok(c) => c,
        Err(_) => {
            return RoamingLoadOutcome::Rejected("config_json 含 NUL 字节".to_string());
        }
    };
    let rc = unsafe { (handle.init)(&HOST_VTABLE, cfg_c.as_ptr()) };
    if rc != 0 {
        let msg = format!(
            "roaming_init 失败（rc={rc}）：-1 空指针 / -2 ABI 失配 / -3 配置解析失败 / \
             -4 workspace 不可得 / -5 runtime 创建失败"
        );
        tracing::error!("[plugin-roaming] {msg}");
        return RoamingLoadOutcome::Rejected(msg);
    }

    match HANDLE.set(handle) {
        Ok(()) => {
            // QUIC 插件化 M2：装载成功即注入 nemesis-cluster 桥——集群/WSAPI/
            // CLI 消费面统一走桥（`HANDLE.get()` 的引用即 'static）。
            if let Some(h) = HANDLE.get() {
                nemesis_cluster::roaming::plugin_bridge::set_plugin_handle(h);
            }
            tracing::info!(
                "[plugin-roaming] 装载成功（ABI v{plugin_abi}）：{}",
                path.display()
            );
            RoamingLoadOutcome::Loaded
        }
        Err(_) => RoamingLoadOutcome::Loaded, // 并发装载竞态：先到者已落槽，等价成功
    }
}

/// 已装载句柄（`None` = 插件未装载——装载失败/缺席/尚未装配）。
pub fn plugin_handle() -> Option<&'static RoamingPluginHandle> {
    HANDLE.get()
}

// ---------------------------------------------------------------------------
// 调用辅助（拍板 10：同步 FFI 内 block_on 插件 runtime——宿主侧 spawn_blocking）
// ---------------------------------------------------------------------------

/// 插件调用统一入口：`spawn_blocking` 包装 + JSON 解析 + `ok:false` 展开。
/// 插件缺席 = 诚实 Err（调用方决定报错形态）。
pub async fn call_roaming(
    what: &'static str,
    f: impl FnOnce(RoamingPluginHandle) -> Result<String, String> + Send + 'static,
) -> Result<serde_json::Value, String> {
    let handle = *plugin_handle().ok_or_else(|| format!("{what}: roaming 插件未装载"))?;
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

/// 启动常驻服务（中继 + 注册表）。回执含 http_addr/quic_addr/registry_addr。
pub async fn roaming_start() -> Result<serde_json::Value, String> {
    call_roaming("roaming_start", RoamingPluginHandle::start).await
}

/// 优雅停中继（运行时开关/停机链）。
pub async fn roaming_stop() -> Result<serde_json::Value, String> {
    call_roaming("roaming_stop", RoamingPluginHandle::stop).await
}

/// 运行态快照（running/http_addr/quic_addr/registry_addr/registry_members）。
/// 静态槽形态（读当前进程装载的插件）——M1 消费面全走 handle 参数化
/// （nemesis-web plugin_call / web 面持 AppState 句柄），本入口供 M2 客户端
/// 控制面与未来无句柄上下文消费；现无 bin 调用点，保留对称 API 面。
#[allow(dead_code)]
pub async fn roaming_status() -> Result<serde_json::Value, String> {
    call_roaming("roaming_status", RoamingPluginHandle::status).await
}

// ---------------------------------------------------------------------------
// 停机（gateway 进程终态链）
// ---------------------------------------------------------------------------

/// 调 `roaming_shutdown`（若插件在位）。幂等；插件缺席 = no-op。
/// **不 unload 库**（unload 活线程是 UB——新句柄同库 OS 引用计数，
/// vm-display 同款）。
pub fn shutdown_plugin_roaming() {
    let Some(path) = PluginLibSpec::ROAMING.find_path() else {
        return;
    };
    unsafe {
        let Ok(lib) = libloading::Library::new(&path) else {
            return;
        };
        // 新句柄与 init 时句柄同指一个库（OS 引用计数）——shutdown 幂等
        // 语义由此安全。
        if let Ok(shutdown) =
            lib.get::<roaming::RoamingShutdownFn>(ROAMING_SHUTDOWN_SYMBOL.as_bytes())
        {
            shutdown();
        }
    }
}

#[cfg(test)]
mod tests;
