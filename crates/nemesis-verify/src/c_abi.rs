//! C ABI 导出（DLL / 动态库接口）。
//!
//! 产物：`nemesis_verify.dll`（Win）/ `libnemesis_verify.so`（Linux/Android）/ `libnemesis_verify.dylib`（Mac）。
//! 外部（exe/测试工具/其他语言）通过 `libloading` 或 `dlopen` 加载本库，调 `nv_*` 函数。
//!
//! # 当前状态（T3）
//! - `nv_verify_target` / `nv_verify_current_exe`：验证流程完整（v3 验签 + 链验证）。
//! - **内置根公钥占位**（全 0 → 无有效根）：待根密钥体系（R7 后续）填真值并保护。
//!   占位期间 `nv_verify_*` 会返回 `NV_UNTRUSTED`（无根可验）。
//! - **`nv_self_verify` 占位**：DLL 自身安全（DLL 定位自己 + 防替换 + 根公钥物理保护）
//!   是 R7 后续独立命题，本阶段返回 0（通过）。
//! - 查看接口（`nv_list_signatures` / `nv_get_signature`）：后续阶段。

use crate::verify;
use ed25519_dalek::VerifyingKey;
use std::os::raw::{c_char, c_int};

/// 编译期固化根公钥 hex（R7 A1）：build 时 `NEMESIS_BUILD_ROOT_PUBKEY=<hex>` 注入。
/// `None` = 未固化（占位，用运行时环境变量 fallback）。固化后不随运行时环境变量改变（防篡改）。
const BUILTIN_ROOT_PUBKEY_HEX: Option<&str> = option_env!("NEMESIS_BUILD_ROOT_PUBKEY");

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
#[derive(Debug, Default)]
pub struct NvOutcome {
    /// 状态码（NV_*）。
    pub status: u32,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    pub pubkey: [u8; 32],
}

/// 取内置根公钥列表。
///
/// **R7 A1**：编译期固化（`NEMESIS_BUILD_ROOT_PUBKEY` build 时注入）优先——防运行时篡改。
/// fallback：运行时 `NEMESIS_ROOT_PUBKEY` 环境变量（部署灵活 / 测试用）。
fn builtin_roots() -> Vec<VerifyingKey> {
    // A1 编译期固化优先
    if let Some(hex) = BUILTIN_ROOT_PUBKEY_HEX {
        if let Some(vk) = crate::hex_util::hex_decode_32(hex)
            .ok()
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
        {
            return vec![vk];
        }
    }
    // 运行时环境变量 fallback（R7 过渡）
    if let Ok(hex) = std::env::var("NEMESIS_ROOT_PUBKEY") {
        if let Some(vk) = crate::hex_util::hex_decode_32(&hex)
            .ok()
            .and_then(|b| VerifyingKey::from_bytes(&b).ok())
        {
            return vec![vk];
        }
    }
    Vec::new()
}

/// 当前时间（Unix 秒）。用 std，避免依赖 chrono。
fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

fn run_verify(bytes: &[u8]) -> NvOutcome {
    let roots = builtin_roots();
    let outcome = verify::verify_bytes(bytes, &roots, now_secs());
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
#[unsafe(no_mangle)]
pub extern "C" fn nv_verify_target(path: *const c_char, out: *mut NvOutcome) -> c_int {
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
#[unsafe(no_mangle)]
pub extern "C" fn nv_verify_current_exe(out: *mut NvOutcome) -> c_int {
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

/// DLL 自验（R7 A2）：读 `dll_path` 字节 + 用内置根公钥验签。
///
/// 调用方传 DLL 路径（Rust cdylib 无 DllMain 存 hinstDLL，DLL 定位自身跨平台复杂——
/// 由调用方传路径绕过）。返回：0=Valid，<0=非 Valid / 错误（-1 null, -2 utf8, -3 read,
/// -4 验签失败, -5 无内置根）。
#[unsafe(no_mangle)]
pub extern "C" fn nv_self_verify(dll_path: *const c_char) -> c_int {
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
    let roots = builtin_roots();
    if roots.is_empty() {
        return -5; // 未固化 + 无运行时 env
    }
    match crate::verify::verify_bytes(&bytes, &roots, now_secs()) {
        crate::verify::VerifyOutcome::Valid { .. } => 0,
        _ => -4,
    }
}

// ===== 查看接口（离线展示签名 + 证书链，不下结论）=====

#[repr(C)]
#[derive(Default)]
pub struct NvSigInfo {
    pub index: u32,
    pub signed_at: u64,
    pub key_fp: [u8; 32],
    pub pubkey: [u8; 32],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct NvSigCert {
    pub subject_pubkey: [u8; 32],
    pub issuer_key_fp: [u8; 32],
    pub valid_not_before: u64,
    pub valid_not_after: u64,
    pub subject_meta_len: u32,
    /// 主体名（UTF-8，≤64B；如发行方名 "org-a" / "CA"）
    pub subject_meta: [u8; 64],
}

impl Default for NvSigCert {
    fn default() -> Self {
        Self {
            subject_pubkey: [0u8; 32],
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
    pub pubkey: [u8; 32],
    pub cert_count: u32,
    /// 最多 4 级证书（leaf + intermediates）。cert_count 为实际数（可能 < 4）。
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
            pubkey: [0u8; 32],
            cert_count: 0,
            certs: [NvSigCert::default(); 4],
            publisher_len: 0,
            publisher: [0u8; 128],
        }
    }
}

/// 列所有签名（多签名）。`count` 入参 = 缓冲容量（NvSigInfo 数组大小），出参 = 实际总数。
#[unsafe(no_mangle)]
pub extern "C" fn nv_list_signatures(
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
#[unsafe(no_mangle)]
pub extern "C" fn nv_get_signature(
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
        let mlen = c.subject_meta.len().min(64);
        subject_meta[..mlen].copy_from_slice(&c.subject_meta[..mlen]);
        certs[i] = NvSigCert {
            subject_pubkey: c.subject_pubkey,
            issuer_key_fp: c.issuer_key_fp,
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
