//! C ABI 导出（DLL / 动态库接口）。
//!
//! 产物：`nemesis_verify.dll`（Win）/ `libnemesis_verify.so`（Linux/Android）/ `libnemesis_verify.dylib`（Mac）。
//! 外部（exe/测试工具/其他语言）通过 `libloading` 或 `dlopen` 加载本库，调 `nv_*` 函数。
//!
//! # 当前状态（S4-5）
//! - `nv_verify_target` / `nv_verify_current_exe` / `nv_self_verify`：v4
//!   Authenticode 管线（S4-1）——PE 载体定位走 Certificate Table（DLL 自验
//!   同管线），ELF/raw 走 v4 footer 载体；nv_* 签名与返回码 T3 形态不变。
//! - **根锚注入**：单一真相源 = [`crate::builtin_root_anchors`]（lib.rs，S4-2）——
//!   编译期 `NEMESIS_BUILD_ROOT_ANCHOR`（根证书 SHA-256 指纹 hex）优先，
//!   fallback 运行时 `NEMESIS_ROOT_ANCHOR`；两者皆缺/非法 = 空锚集 → `NV_UNTRUSTED`。
//! - `nv_self_verify`（R7 A2）：读 DLL 字节 + `verify_bytes`（Valid→0，非
//!   Valid→-4，无锚→-5）；防 patch/防替换等 DLL 自身安全仍是 R7 后续独立命题。
//! - 查看接口（`nv_list_signatures` / `nv_get_signature`）：S4-3 起 v4（view
//!   判据同源，不下结论）。

use crate::verify;
use std::os::raw::{c_char, c_int};

// ===== 结果状态码 =====
pub const NV_VALID: u32 = 0;
pub const NV_NO_SIGNATURE: u32 = 1;
pub const NV_TAMPERED: u32 = 2;
pub const NV_SIGNATURE_INVALID: u32 = 3;
pub const NV_UNTRUSTED: u32 = 4;
pub const NV_UNSUPPORTED_VERSION: u32 = 5;
pub const NV_MALFORMED: u32 = 6;
pub const NV_REVOKED: u32 = 7;
pub const NV_EXPIRED: u32 = 8;

/// C 兼容的验证结果（out 参数）。
#[repr(C)]
#[derive(Debug)]
pub struct NvOutcome {
    /// 状态码（NV_*）。
    pub status: u32,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    /// 签名公钥（SEC1 uncompressed 65B）。
    pub pubkey: [u8; 65],
}

impl Default for NvOutcome {
    fn default() -> Self {
        Self {
            status: 0,
            signed_at: 0,
            key_fp: [0u8; 32],
            pubkey: [0u8; 65],
        }
    }
}

/// 当前时间（Unix 秒）。用 std，避免依赖 chrono。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run_verify(bytes: &[u8]) -> NvOutcome {
    let anchors = crate::builtin_root_anchors();
    let outcome = verify::verify_bytes(bytes, &anchors, now_secs());
    let status = match outcome {
        verify::VerifyOutcome::Valid { .. } => NV_VALID,
        verify::VerifyOutcome::NoSignature => NV_NO_SIGNATURE,
        verify::VerifyOutcome::Tampered(_) => NV_TAMPERED,
        verify::VerifyOutcome::SignatureInvalid => NV_SIGNATURE_INVALID,
        verify::VerifyOutcome::Untrusted => NV_UNTRUSTED,
        verify::VerifyOutcome::Revoked { .. } => NV_REVOKED,
        verify::VerifyOutcome::Expired(_) => NV_EXPIRED,
        verify::VerifyOutcome::UnsupportedVersion(_) => NV_UNSUPPORTED_VERSION,
        verify::VerifyOutcome::Malformed(_) => NV_MALFORMED,
    };
    match outcome {
        verify::VerifyOutcome::Valid {
            signed_at,
            key_fp,
            pubkey,
        } => NvOutcome {
            status,
            signed_at,
            key_fp,
            pubkey,
        },
        _ => NvOutcome {
            status,
            ..Default::default()
        },
    }
}

/// 验证目标文件。
///
/// `path`：UTF-8 路径（C 字符串）。`out`：接收结果。
/// 返回：0=成功（读文件 + 验证完成，查 out->status）；<0=参数/IO 错误。
///
/// # Safety
/// - `path` 必须指向合法 NUL 结尾的 C 字符串，或为 null（返回 -1，不解引用）。
/// - `out` 必须指向可写的 `NvOutcome` 内存，或为 null（返回 -1）。
/// - 其余情况不会解引用任一指针（读文件失败走 -3，不触 out）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nv_verify_target(path: *const c_char, out: *mut NvOutcome) -> c_int {
    if path.is_null() || out.is_null() {
        return -1;
    }
    let path_str = match unsafe { std::ffi::CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };
    let bytes = match std::fs::read(path_str) {
        Ok(b) => b,
        Err(_) => return -3,
    };
    unsafe { *out = run_verify(&bytes) };
    0
}

/// 验证调用方进程的 exe（`std::env::current_exe()`）。
///
/// # Safety
/// - `out` 必须指向可写的 `NvOutcome` 内存，或为 null（返回 -1，不解引用）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nv_verify_current_exe(out: *mut NvOutcome) -> c_int {
    if out.is_null() {
        return -1;
    }
    let exe = match std::env::current_exe() {
        Ok(p) => p,
        Err(_) => return -4,
    };
    let bytes = match std::fs::read(&exe) {
        Ok(b) => b,
        Err(_) => return -3,
    };
    unsafe { *out = run_verify(&bytes) };
    0
}

/// DLL 自验（R7 A2）：读 `dll_path` 字节 + `verify_bytes`（v4 Authenticode；
/// PE 载体 = Certificate Table 定位）+ 内置根锚判信。
///
/// 调用方传 DLL 路径（Rust cdylib 无 DllMain 存 hinstDLL，DLL 定位自身跨平台复杂——
/// 由调用方传路径绕过）。返回：0=Valid，<0=非 Valid / 错误（-1 null, -2 utf8, -3 read,
/// -4 验签失败, -5 无内置根）。
///
/// # Safety
/// - `dll_path` 必须指向合法 NUL 结尾的 C 字符串，或为 null（返回 -1，不解引用）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nv_self_verify(dll_path: *const c_char) -> c_int {
    if dll_path.is_null() {
        return -1;
    }
    let path_str = match unsafe { std::ffi::CStr::from_ptr(dll_path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };
    let bytes = match std::fs::read(path_str) {
        Ok(b) => b,
        Err(_) => return -3,
    };
    let anchors = crate::builtin_root_anchors();
    if anchors.is_empty() {
        return -5; // 未固化 + 无运行时 env
    }
    match crate::verify::verify_bytes(&bytes, &anchors, now_secs()) {
        crate::verify::VerifyOutcome::Valid { .. } => 0,
        _ => -4,
    }
}

// ===== 查看接口（离线展示签名 + 证书链，不下结论）=====

#[repr(C)]
pub struct NvSigInfo {
    pub index: u32,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    /// 签名公钥（SEC1 uncompressed 65B）。
    pub pubkey: [u8; 65],
}

impl Default for NvSigInfo {
    fn default() -> Self {
        Self {
            index: 0,
            signed_at: 0,
            key_fp: [0u8; 32],
            pubkey: [0u8; 65],
        }
    }
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NvSigCert {
    /// 主体公钥（SEC1 uncompressed 65B）。
    pub subject_pubkey: [u8; 65],
    /// 签发者 key_fp（= AKI keyIdentifier）。无 AKI 时全 0。
    pub issuer_key_fp: [u8; 32],
    pub valid_not_before: u64,
    pub valid_not_after: u64,
    pub subject_meta_len: u32,
    /// 主体 CN（UTF-8，≤64B）
    pub subject_meta: [u8; 64],
}

impl Default for NvSigCert {
    fn default() -> Self {
        Self {
            subject_pubkey: [0u8; 65],
            issuer_key_fp: [0u8; 32],
            valid_not_before: 0,
            valid_not_after: 0,
            subject_meta_len: 0,
            subject_meta: [0u8; 64],
        }
    }
}

#[repr(C)]
pub struct NvSigDetail {
    pub index: u32,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    /// 签名公钥（SEC1 uncompressed 65B）。
    pub pubkey: [u8; 65],
    pub cert_count: u32,
    /// 最多 4 级证书（leaf 在前，含根）。cert_count 为实际数（可能 < 4）。
    pub certs: [NvSigCert; 4],
    pub publisher_len: u32,
    /// publisher（签给谁/发布者，UTF-8，≤128B）
    pub publisher: [u8; 128],
}

impl Default for NvSigDetail {
    fn default() -> Self {
        Self {
            index: 0,
            signed_at: 0,
            key_fp: [0u8; 32],
            pubkey: [0u8; 65],
            cert_count: 0,
            certs: [NvSigCert::default(); 4],
            publisher_len: 0,
            publisher: [0u8; 128],
        }
    }
}

/// 列所有签名（多签名）。`count` 入参 = 缓冲容量（NvSigInfo 数组大小），出参 = 实际总数。
///
/// # Safety
/// - `path` 必须指向合法 NUL 结尾的 C 字符串；`out` 指向容量 `*count` 的数组；
///   `count` 指向可写 u32。任一为 null 返回 -1（不解引用）。
/// - 写入 `out[0..min(总数, *count)]`，只把实际总数写回 `*count`，不越界。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nv_list_signatures(
    path: *const c_char,
    out: *mut NvSigInfo,
    count: *mut u32,
) -> c_int {
    if path.is_null() || out.is_null() || count.is_null() {
        return -1;
    }
    let path_str = match unsafe { std::ffi::CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };
    let bytes = match std::fs::read(path_str) {
        Ok(b) => b,
        Err(_) => return -3,
    };
    let list = crate::view::list_signatures(&bytes);
    let total = list.len() as u32;
    let cap = unsafe { *count } as usize;
    let n = list.len().min(cap);
    for (i, info) in list.iter().take(n).enumerate() {
        unsafe {
            *out.add(i) = NvSigInfo {
                index: info.index as u32,
                signed_at: info.signed_at,
                key_fp: info.key_fp,
                pubkey: info.pubkey,
            };
        }
    }
    unsafe {
        *count = total;
    }
    0
}

/// 单签名详情（含 cert chain，最多 4 级）。`index` = list_signatures 返回的索引（0=最近）。
///
/// # Safety
/// - `path` 必须指向合法 NUL 结尾的 C 字符串；`out` 必须指向可写的 `NvSigDetail`。
///   任一为 null 返回 -1（不解引用）；签名不存在返回 -4（不触 out）。
#[unsafe(no_mangle)]
pub unsafe extern "C" fn nv_get_signature(
    path: *const c_char,
    index: u32,
    out: *mut NvSigDetail,
) -> c_int {
    if path.is_null() || out.is_null() {
        return -1;
    }
    let path_str = match unsafe { std::ffi::CStr::from_ptr(path) }.to_str() {
        Ok(s) => s,
        Err(_) => return -2,
    };
    let bytes = match std::fs::read(path_str) {
        Ok(b) => b,
        Err(_) => return -3,
    };
    let detail = match crate::view::get_signature_detail(&bytes, index as usize) {
        Some(d) => d,
        None => return -4,
    };
    let mut certs = [NvSigCert::default(); 4];
    let cert_count = detail.certs.len().min(4) as u32;
    for (i, c) in detail.certs.iter().take(4).enumerate() {
        let mut subject_meta = [0u8; 64];
        let mlen = c
            .subject_meta
            .as_deref()
            .map(|s| s.len().min(64))
            .unwrap_or(0);
        if let Some(s) = c.subject_meta.as_deref() {
            subject_meta[..mlen].copy_from_slice(&s.as_bytes()[..mlen]);
        }
        let mut issuer_key_fp = [0u8; 32];
        if let Some(fp) = c.issuer_key_fp {
            issuer_key_fp = fp;
        }
        certs[i] = NvSigCert {
            subject_pubkey: c.subject_pubkey,
            issuer_key_fp,
            valid_not_before: c.valid_not_before,
            valid_not_after: c.valid_not_after,
            subject_meta_len: mlen as u32,
            subject_meta,
        };
    }
    let mut publisher = [0u8; 128];
    let publisher_len = if let Some(p) = &detail.publisher {
        let pl = p.len().min(128);
        publisher[..pl].copy_from_slice(&p.as_bytes()[..pl]);
        pl as u32
    } else {
        0
    };
    unsafe {
        *out = NvSigDetail {
            index: detail.info.index as u32,
            signed_at: detail.info.signed_at,
            key_fp: detail.info.key_fp,
            pubkey: detail.info.pubkey,
            cert_count,
            certs,
            publisher_len,
            publisher,
        };
    }
    0
}

#[cfg(test)]
mod tests;
