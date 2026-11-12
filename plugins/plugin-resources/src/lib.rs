//! plugin_resources — NemesisBot 统一附加资源插件（C-ABI 解冻第三例：
//! 纯数据资源载体样板）。
//!
//! 与 plugin-vm（老式功能插件）/ plugin-roaming（新式交互插件）的形态差异：
//! **无状态纯数据查询面**——无常驻线程、无 runtime、无回调、无全局可变状态，
//! 装载即静止；每次调用独立分配返回，宿主用完调 [`prs_free`] 归还（roaming
//! 的 per-call 静态缓冲 + 单线程串行纪律在此不需要，web handler 可并发调）。
//!
//! 面（ABI v1 冻结面，4 导出）：
//! - `prs_abi_version()` → u32：双向闸 host 半边（宿主不认识即按缺席处理）；
//! - `prs_manifest()` → `*mut c_char`：manifest 全量 JSON（含各 kind 的
//!   tree；路径清单，语义过滤在宿主消费侧）；
//! - `prs_get(kind, id)` → `*mut c_char`：JSON envelope
//!   `{"ok":true,"content":...}` / `{"ok":false,"err_code":"...","err_msg":"..."}`；
//!   id = manifest tree 内路径；
//! - `prs_free(p)`：归还插件侧分配（单次分配纪律：每个返回串一次 CString）。
//!
//! NULL 返回 = 插件内部失败（宿主按缺席降级，不崩）；全部导出
//! `catch_unwind` 包裹（panic 不跨 FFI 边界）。
//!
//! 附加资源分类判据（CLAUDE.md 拍板 2026-11-12）：非必须附加资源（缺席 =
//! 对应功能诚实降级、用户可不使用）进本插件，不占主二进制体积；必须资源
//! （缺席 = 功能不可用，如价目表）编译期内嵌主二进制，**不进本插件**。

mod catalog;
#[allow(dead_code)]
mod resource_filter; // build.rs 主消费（#[path] include）；lib 侧只用常量

#[cfg(test)]
mod tests;

use std::ffi::{CStr, CString};
use std::os::raw::c_char;
use std::panic::{AssertUnwindSafe, catch_unwind};

use catalog::catalog;
use resource_filter::KIND_PERSONA_CATALOG;

/// 插件侧 ABI 版本（宿主侧常量在 nemesis-plugin/src/resources.rs，双向闸）。
const PRS_ABI_VERSION: u32 = 1;

/// 单次分配返回：CString → raw（宿主 `prs_free` 归还）。内含 NUL（序列化器
/// 不可能产出）→ 诚实 NULL = 内部失败。
fn alloc_json(s: String) -> *mut c_char {
    match CString::new(s) {
        Ok(c) => c.into_raw(),
        Err(_) => std::ptr::null_mut(),
    }
}

/// envelope 构造（成功/失败统一形状）。
fn envelope_json(content: Option<&str>, err_code: &str, err_msg: &str) -> String {
    match content {
        Some(c) => serde_json::json!({ "ok": true, "content": c }).to_string(),
        None => serde_json::json!({
            "ok": false,
            "err_code": err_code,
            "err_msg": err_msg
        })
        .to_string(),
    }
}

/// ABI 不匹配注入钩子（host 侧真测试用）：正常构建 `option_env!` = None →
/// 恒 1；测试以 `FIXTURE_ABI_OVERRIDE=2 cargo build` 出错版 fixture，供
/// 装载器 ABI 握手拒绝臂的**真实**回归（插件版 ≠ 宿主版 → WARN + 按缺席）。
fn fixture_abi_override() -> Option<u32> {
    option_env!("FIXTURE_ABI_OVERRIDE").and_then(|v| v.parse().ok())
}

#[unsafe(no_mangle)]
pub extern "C" fn prs_abi_version() -> u32 {
    fixture_abi_override().unwrap_or(PRS_ABI_VERSION)
}

#[unsafe(no_mangle)]
pub extern "C" fn prs_manifest() -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| {
        alloc_json(catalog().manifest_json.clone())
    }))
    .unwrap_or(std::ptr::null_mut())
}

/// clippy 无法看见「NULL 先检 + catch_unwind 包裹」的防御次序——allow +
/// 注释即契约文书化（plugin_roaming 同款纪律）。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn prs_get(kind: *const c_char, id: *const c_char) -> *mut c_char {
    catch_unwind(AssertUnwindSafe(|| prs_get_inner(kind, id)))
        .unwrap_or_else(|_| alloc_json(envelope_json(None, "internal", "panic in prs_get")))
}

fn prs_get_inner(kind: *const c_char, id: *const c_char) -> *mut c_char {
    if kind.is_null() || id.is_null() {
        return alloc_json(envelope_json(None, "invalid_arg", "kind/id 为 NULL"));
    }
    let kind = unsafe { CStr::from_ptr(kind) }.to_string_lossy();
    let id = unsafe { CStr::from_ptr(id) }.to_string_lossy();
    if kind != KIND_PERSONA_CATALOG {
        return alloc_json(envelope_json(
            None,
            "unknown_kind",
            &format!("未知资源种类: {kind}"),
        ));
    }
    match catalog().files.get(id.as_ref()) {
        Some(content) => alloc_json(envelope_json(Some(content), "", "")),
        None => alloc_json(envelope_json(
            None,
            "not_found",
            &format!("资源不存在: {id}"),
        )),
    }
}

/// NULL 容忍（宿主重复 free 防御）；clippy 同款 allow（见 prs_get 注）。
#[allow(clippy::not_unsafe_ptr_arg_deref)]
#[unsafe(no_mangle)]
pub extern "C" fn prs_free(p: *mut c_char) {
    if !p.is_null() {
        drop(unsafe { CString::from_raw(p) });
    }
}
