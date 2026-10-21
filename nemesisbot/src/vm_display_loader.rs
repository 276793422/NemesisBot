//! P5-R2 ④：plugin-vm 宿主装载器（`vm-display` feature 门控；implies
//! `vm`——display_port_query 要读 VmSupervisor，没有 VM 后端的显示插件
//! 无意义）。
//!
//! 装载链（任一步失败 = 诚实拒绝 + warn 留痕，绝不静默降级成无插件）：
//! 1. **在场探测**：宿主 exe 同目录找 `plugin_vm.dll` / `libplugin_vm.so`
//!    / `libplugin_vm.dylib`；缺席 = 静默 Ok（display=vnc 时 shim 端会
//!    诚实 501「显示插件未安装」——缺席是常态不是错误，装载失败才是）。
//! 2. **v4 签名闸**：`verify::verify_bytes` + 编译期锚（verify feature）。
//!    Tampered/SignatureInvalid/Untrusted/Revoked/Expired = 拒载 + 留痕；
//!    NoSignature = dev（debug_assertions）warn 放行 / release 拒绝；
//!    verify feature 关或锚缺席 = 无从验证，同 NoSignature 分支语义
//!    （dev warn / release 拒——一方插件在 release 形态必须可验证）。
//! 3. **vtable 构建**：workspace + display_port_query（读 VmSupervisor
//!    registry，只在 VM 运行且 display=vnc 时报端口）。
//! 4. **init + 双向 ABI 闸**：`plugin_vm_display_init` 返回的
//!    `plugin_abi_version` 必须等于 [`HOST_VM_DISPLAY_ABI_VERSION`]
//!    （插件侧在 init 内也校验 host 下发的 abi_version——双向）。
//! 5. **set_endpoint**：端口落 nemesis-web vm_display 槽（first-wins，
//!    重复装载同端口幂等）。
//!
//! estop 断连挂点在 gateway 装配处（[`disconnect_all_for_estop`] 经
//! estop watcher 调用；本模块只管装载）。

use std::ffi::CStr;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::Arc;

use nemesis_plugin::vm_display::{
    HOST_VM_DISPLAY_ABI_VERSION, HOST_VM_DISPLAY_ABI_VERSION as ABI, HostVmDisplayVTable,
    INIT_SYMBOL, PORT_QUERY_ERR_INTERNAL, PORT_QUERY_ERR_NO_DISPLAY, PORT_QUERY_ERR_NOT_RUNNING,
    PORT_QUERY_ERR_UNKNOWN, PluginVmInfo,
};

/// 装载结果：Ok(None) = 插件文件缺席（常态）；Ok(Some) = 装载成功；
/// Err = 在场但装载被拒（签名/ABI/初始化失败——必须留痕）。
pub enum LoadOutcome {
    /// 插件未安装（exe 同目录无 plugin-vm 库文件）——常态，静默。
    Absent,
    /// 装载成功（端口已 set_endpoint）。
    Loaded { listen_port: u16 },
    /// 装载被拒（原因已留痕）。
    Rejected(String),
}

// ---------------------------------------------------------------------------
// 宿主静态槽（extern "C" fn 不能捕获——supervisor/workspace 走 OnceLock）
// ---------------------------------------------------------------------------

static SUPERVISOR: std::sync::OnceLock<Arc<crate::agents::vm_runtime::VmSupervisor>> =
    std::sync::OnceLock::new();
static WORKSPACE_ROOT: std::sync::OnceLock<PathBuf> = std::sync::OnceLock::new();

/// 在场库文件名（平台分派）。
fn plugin_filenames() -> &'static [&'static str] {
    if cfg!(windows) {
        &["plugin_vm.dll"]
    } else if cfg!(target_os = "macos") {
        &["libplugin_vm.dylib"]
    } else {
        &["libplugin_vm.so"]
    }
}

fn find_plugin_path() -> Option<PathBuf> {
    let exe_dir = std::env::current_exe().ok()?.parent()?.to_path_buf();
    plugin_filenames()
        .iter()
        .map(|name| exe_dir.join(name))
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// vtable 函数实现（extern "C"，读模块级静态）
// ---------------------------------------------------------------------------

extern "C" fn host_log(level: i32, tag: *const c_char, msg: *const c_char) {
    // tracing target 必须字面量——tag 并入消息体（插件 tag 保留可观测）。
    let tag = unsafe { CStr::from_ptr(tag) }.to_string_lossy();
    let msg = unsafe { CStr::from_ptr(msg) }.to_string_lossy();
    match level {
        0 => tracing::trace!(target: "plugin_vm", "[{tag}] {msg}"),
        1 => tracing::debug!(target: "plugin_vm", "[{tag}] {msg}"),
        2 => tracing::info!(target: "plugin_vm", "[{tag}] {msg}"),
        3 => tracing::warn!(target: "plugin_vm", "[{tag}] {msg}"),
        _ => tracing::error!(target: "plugin_vm", "[{tag}] {msg}"),
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

/// C 字符串写入（截断 = -1，与 HostServices.get_workspace_dir 同款约定）。
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

extern "C" fn host_display_port_query(agent_id: *const c_char) -> i32 {
    let Some(sup) = SUPERVISOR.get() else {
        return PORT_QUERY_ERR_INTERNAL;
    };
    if agent_id.is_null() {
        return PORT_QUERY_ERR_UNKNOWN;
    }
    let agent = unsafe { CStr::from_ptr(agent_id) }.to_string_lossy();
    // registry 记录是显示端口的落盘真相源（spawn 时写入）；运行闸 = 槽内
    // 在册（VM 停了端口就没了，不在槽内一律报未运行）。
    let running: std::collections::HashSet<String> = sup.running_agents().into_iter().collect();
    if !running.contains(agent.as_ref()) {
        return PORT_QUERY_ERR_NOT_RUNNING;
    }
    for (record, is_running) in sup.instance_status_all() {
        if record.agent_id == agent && is_running {
            return match record.display_port {
                Some(port) => port as i32,
                None => PORT_QUERY_ERR_NO_DISPLAY,
            };
        }
    }
    PORT_QUERY_ERR_UNKNOWN
}

extern "C" fn host_audit_event(event: *const c_char, detail: *const c_char) {
    let event = unsafe { CStr::from_ptr(event) }.to_string_lossy();
    let detail = unsafe { CStr::from_ptr(detail) }.to_string_lossy();
    tracing::info!(target: "plugin_vm", "[audit] {event} {detail}");
}

fn build_vtable() -> HostVmDisplayVTable {
    HostVmDisplayVTable {
        abi_version: ABI,
        log: Some(host_log),
        get_workspace_dir: Some(host_get_workspace_dir),
        display_port_query: Some(host_display_port_query),
        audit_event: Some(host_audit_event),
    }
}

// ---------------------------------------------------------------------------
// v4 签名闸
// ---------------------------------------------------------------------------

/// 签名闸裁决（留痕由调用方做——把裁决与原因一次说清）。
enum SignatureVerdict {
    Valid,
    /// 无签名或无从验证（verify feature 关 / 锚缺席）：dev warn 放行 /
    /// release 拒绝。
    Unsigned,
    /// 五类硬拒：签名在但不可信/被改/被吊销/过期。
    HardReject(String),
}

#[cfg(feature = "verify")]
fn verify_signature(path: &Path) -> SignatureVerdict {
    use nemesis_verify::verify;
    let run = || -> Result<SignatureVerdict, String> {
        let Some(anchor_hex) = crate::verify_policy::ROOT_ANCHOR else {
            return Ok(SignatureVerdict::Unsigned);
        };
        let anchor = nemesis_verify::hex_util::hex_decode_32(anchor_hex)
            .map_err(|e| format!("锚指纹解码失败: {e}"))?;
        let bytes = std::fs::read(path)
            .map_err(|e| format!("读取插件库失败（{}）：{e}", path.display()))?;
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        Ok(match verify::verify_bytes(&bytes, &[anchor], now) {
            nemesis_verify::verify::VerifyOutcome::Valid { .. } => SignatureVerdict::Valid,
            nemesis_verify::verify::VerifyOutcome::NoSignature => SignatureVerdict::Unsigned,
            other => SignatureVerdict::HardReject(format!("{other:?}")),
        })
    };
    match run() {
        Ok(v) => v,
        Err(e) => SignatureVerdict::HardReject(e),
    }
}

#[cfg(not(feature = "verify"))]
fn verify_signature(_path: &Path) -> SignatureVerdict {
    // verify feature 关：无从验证 = 无签名同分支（dev warn / release 拒）。
    SignatureVerdict::Unsigned
}

// ---------------------------------------------------------------------------
// 装载主流程
// ---------------------------------------------------------------------------

/// 装载 plugin-vm（gateway 装配点调用一次）。
///
/// `supervisor`：VM 运行时（display_port_query 数据源）；`workspace`：
/// 插件侧路径解析用。
pub fn load_plugin_vm(
    supervisor: Arc<crate::agents::vm_runtime::VmSupervisor>,
    workspace: &Path,
) -> LoadOutcome {
    let Some(path) = find_plugin_path() else {
        return LoadOutcome::Absent;
    };

    // ---- 签名闸 ----
    match verify_signature(&path) {
        SignatureVerdict::Valid => {}
        SignatureVerdict::Unsigned => {
            let msg = "plugin-vm 无 v4 签名（或 verify feature 关/锚缺席，无从验证）";
            if cfg!(debug_assertions) {
                tracing::warn!(
                    "[plugin-vm] {msg}——开发形态 warn 放行（release 拒绝）：{}",
                    path.display()
                );
            } else {
                return LoadOutcome::Rejected(format!("{msg}，release 形态拒绝装载"));
            }
        }
        SignatureVerdict::HardReject(reason) => {
            tracing::error!(
                "[plugin-vm] v4 签名验证未过，拒绝装载（{}）：{reason}",
                path.display()
            );
            return LoadOutcome::Rejected(format!("签名验证未过：{reason}"));
        }
    }

    // ---- 宿主静态槽（vtable 函数读） ----
    let _ = WORKSPACE_ROOT.set(workspace.to_path_buf());
    let _ = SUPERVISOR.set(supervisor);

    // ---- dlopen + init ----
    unsafe {
        let lib = match libloading::Library::new(&path) {
            Ok(l) => l,
            Err(e) => {
                return LoadOutcome::Rejected(format!("库加载失败（{}）：{e}", path.display()));
            }
        };
        // Library 生命周期 = 进程（init 后永不 unload——插件持有线程/
        // runtime，unload 一个活着的东西是 UB）。Box::leak 刻意泄漏。
        let lib: &'static libloading::Library = Box::leak(Box::new(lib));
        let init: libloading::Symbol<
            unsafe extern "C" fn(*const HostVmDisplayVTable, *mut PluginVmInfo) -> c_int,
        > = match lib.get(INIT_SYMBOL.as_bytes()) {
            Ok(s) => s,
            Err(e) => {
                return LoadOutcome::Rejected(format!("符号 {INIT_SYMBOL} 缺失：{e}"));
            }
        };
        let vtable = build_vtable();
        let mut info = PluginVmInfo {
            plugin_abi_version: 0,
            listen_port: 0,
        };
        let rc = init(&vtable, &mut info);
        if rc != 0 {
            return LoadOutcome::Rejected(format!(
                "plugin_vm_display_init 失败（rc={rc}）：-2 = host ABI 不匹配（插件过旧/过新，\
                 双向闸拒绝）"
            ));
        }
        // ---- 双向闸 host 侧半边 ----
        if info.plugin_abi_version != HOST_VM_DISPLAY_ABI_VERSION {
            return LoadOutcome::Rejected(format!(
                "插件 ABI 版本不匹配：插件 v{}，宿主 v{HOST_VM_DISPLAY_ABI_VERSION}\
                 （升级或降级 plugin-vm 到匹配版本）",
                info.plugin_abi_version
            ));
        }
        match nemesis_web::vm_display::set_endpoint(nemesis_web::vm_display::VmDisplayEndpoint {
            listen_port: info.listen_port,
            plugin_abi_version: info.plugin_abi_version,
        }) {
            Ok(()) => LoadOutcome::Loaded {
                listen_port: info.listen_port,
            },
            Err(e) => LoadOutcome::Rejected(e),
        }
    }
}

/// estop / 停机路径：调 `plugin_vm_display_free`（若插件在位）。幂等；
/// 插件缺席 = no-op。**只断显示通道，不 unload 库**（unload 活线程是 UB）。
pub fn shutdown_plugin_vm() {
    // 查找 + dlopen 再调 free 与 init 同款路径；插件缺席静默跳过。
    let Some(path) = find_plugin_path() else {
        return;
    };
    unsafe {
        let Ok(lib) = libloading::Library::new(&path) else {
            return;
        };
        // 新句柄与 init 时句柄同指一个库（OS 引用计数）——free 幂等语义
        // 由此安全。
        if let Ok(free) =
            lib.get::<unsafe extern "C" fn()>(nemesis_plugin::vm_display::FREE_SYMBOL.as_bytes())
        {
            free();
        }
    }
}

#[cfg(all(test, feature = "vm-display"))]
mod tests;
