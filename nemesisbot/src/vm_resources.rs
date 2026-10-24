//! plugin-vm 内嵌资源释放臂（`vm` feature 门控；2026-10-22 拍板：VM
//! 资源——除三方件外——全部内嵌进 plugin_vm.dll，**用的时候释放**，
//! 不再有 plugin_vm/ 资源目录）。
//!
//! 流程（[`ensure_released`]，镜像构建前调用）：
//! 1. [`crate::plugin_lib`]（VM spec）找 DLL + v4 签名闸（与显示装载臂同源
//!    口径——同一 DLL 同一信任裁决）；
//! 2. dlopen → `plugin_vm_resource_get` 按名取字节（.rodata 静态指针，
//!    进程生命周期有效）；
//! 3. 释放落盘 `<workspace>/tools/vm/released/`（布局镜像旧资源目录：
//!    `build-vm-image.sh` + `guest/<短arch>/nemesis-guest`）——**总是
//!    覆盖写**（插件升级后旧资源自愈，无版本协商）。
//!
//! 缺席/错误语义（调用方 vm_bridge 消费）：
//! - `Ok(None)`：插件未安装（常态）→ 调用方回落开发链；
//! - `Err`：插件在场但不可用（签名未过 / 缺导出 / 释放失败）→ 调用方
//!   仍回落开发链，但最终报错必须带上本错误（不许静默吞）；
//! - `RESOURCE_GET_ABSENT`（该架构 guest 未内嵌）= 跳过该资源，不算
//!   错（换全量构建的包即可）。
//!
//! 结果按 workspace 进程级缓存（OnceLock）——签名验的是整个 DLL
//! （~MB 级读 + ECDSA），bootstrap_status 轮询面不许反复付这笔。

use std::ffi::CString;
use std::os::raw::{c_char, c_int};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use crate::vm_plugin_lib::{
    SignatureVerdict, find_plugin_path, unsigned_dev_allow, verify_signature,
};

/// 释放根（`<workspace>/tools/vm/released`）——布局 = 旧 plugin_vm/ 资源
/// 目录，vm_bridge 的查找链函数直接以它为首选候选。
pub(crate) fn released_root(workspace: &Path) -> PathBuf {
    workspace.join("tools").join("vm").join("released")
}

/// `plugin_vm_resource_get` 符号名（与 plugin-vm 侧
/// `RESOURCE_GET_SYMBOL` 同字符串；两侧注释互指防漂移）。
const RESOURCE_GET_SYMBOL: &str = "plugin_vm_resource_get";

/// 资源名（与 plugin-vm resources 模块同字符串——wire 契约）。
const RES_SCRIPT: &str = "build-vm-image.sh";
const RES_GUEST: &[(&str, &str)] = &[
    ("x86_64", "guest/x86_64/nemesis-guest"),
    ("aarch64", "guest/aarch64/nemesis-guest"),
];

/// 进程级缓存（首调裁决，此后直接复用——签名验证只付一次）。
static CACHE: OnceLock<Result<Option<PathBuf>, String>> = OnceLock::new();

/// 确保内嵌资源已释放（幂等，覆盖写自愈）。返回 `Ok(Some(root))` =
/// 释放根（至少脚本在场）；`Ok(None)` = 插件未安装；`Err` = 插件在场
/// 但不可用（原因已说清，调用方报错时必须带上）。
pub(crate) fn ensure_released(workspace: &Path) -> Result<Option<PathBuf>, String> {
    CACHE
        .get_or_init(|| ensure_released_uncached(workspace))
        .clone()
}

fn ensure_released_uncached(workspace: &Path) -> Result<Option<PathBuf>, String> {
    // ---- 1. 在场探测 + 签名闸（与显示装载臂同源） ----
    let Some(lib_path) = find_plugin_path() else {
        return Ok(None);
    };
    match verify_signature(&lib_path) {
        SignatureVerdict::Valid => {}
        SignatureVerdict::Unsigned => {
            let msg = "plugin-vm 无 v4 签名（或 verify feature 关/锚缺席，无从验证）";
            if unsigned_dev_allow() {
                tracing::warn!(
                    "[plugin-vm] {msg}——开发形态 warn 放行（release 拒绝）：{}",
                    lib_path.display()
                );
            } else {
                return Err(format!(
                    "{msg}，release 形态拒绝释放资源（{}）",
                    lib_path.display()
                ));
            }
        }
        SignatureVerdict::HardReject(reason) => {
            return Err(format!(
                "plugin-vm v4 签名验证未过，拒绝释放资源（{}）：{reason}",
                lib_path.display()
            ));
        }
    }

    // ---- 2. dlopen + 取符号 ----
    // Library 本函数内 drop（unpin 引用计数即可——资源指针只在调用瞬间
    // 消费，落盘的是文件不是插件内存；显示装载臂才需要常驻句柄）。
    let lib = unsafe { libloading::Library::new(&lib_path) }
        .map_err(|e| format!("库加载失败（{}）：{e}", lib_path.display()))?;
    let get: libloading::Symbol<
        unsafe extern "C" fn(*const c_char, *mut *const u8, *mut usize) -> c_int,
    > = unsafe { lib.get(RESOURCE_GET_SYMBOL.as_bytes()) }.map_err(|_| {
        format!(
            "插件缺 {RESOURCE_GET_SYMBOL} 导出——plugin-vm 版本过旧（资源内嵌前的版本），\
             请更新插件库：{}",
            lib_path.display()
        )
    })?;

    // ---- 3. 逐资源释放（总是覆盖写 = 升级自愈） ----
    let root = released_root(workspace);
    let mut released = Vec::<String>::new();
    let mut write_res = |name: &str, dest: &Path| -> Result<bool, String> {
        let name_c = CString::new(name).map_err(|e| format!("资源名非法（{name}）：{e}"))?;
        let mut ptr: *const u8 = std::ptr::null();
        let mut len: usize = 0;
        let rc = unsafe { get(name_c.as_ptr(), &mut ptr, &mut len) };
        match rc {
            0 => {
                let bytes = unsafe { std::slice::from_raw_parts(ptr, len) };
                if let Some(parent) = dest.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|e| format!("创建目录失败（{}）：{e}", parent.display()))?;
                }
                std::fs::write(dest, bytes)
                    .map_err(|e| format!("写资源失败（{}）：{e}", dest.display()))?;
                released.push(name.to_string());
                Ok(true)
            }
            // 该资源构建时未内嵌（如 guest 缺该架构）= 跳过，不算错。
            2 => Ok(false),
            other => Err(format!("plugin_vm_resource_get({name}) 非法返回码 {other}")),
        }
    };

    let script_dest = root.join(RES_SCRIPT);
    let script_ok = write_res(RES_SCRIPT, &script_dest)?;
    // 脚本必内嵌（include_str! 编译期硬依赖）——拿不到 = ABI 异常，诚实报。
    if !script_ok {
        return Err(format!(
            "插件未返回镜像构建脚本资源（ABI 异常）：{}",
            lib_path.display()
        ));
    }
    for (_, name) in RES_GUEST {
        let dest = root.join(name);
        write_res(name, &dest)?;
    }

    tracing::info!(
        "[plugin-vm] 内嵌资源已释放到 {}（{} 项：{}）",
        root.display(),
        released.len(),
        released.join(", ")
    );
    Ok(Some(root))
}
