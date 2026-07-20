//! verify-loader：签名验证测试工具——加载 `nemesis_verify` DLL 验证目标文件。
//!
//! 子命令：
//! - `gen-keys <out>`：生成密钥体系
//! - `sign <keys> <target> <out>`：用发行方私钥签目标
//! - `verify [--keys] <dll> <target>`：加载 DLL 调 `nv_verify_target` 验证目标文件
//! - `verify-self [--keys] <dll>`：调 `nv_verify_current_exe` 验证**本进程 exe**（DLL 自验入口测试）
//!
//! `--keys` 自动注入根公钥（设 `NEMESIS_ROOT_PUBKEY`，DLL 内部读——见 c_abi.rs R7 过渡）。

use anyhow::Result;
use clap::{Parser, Subcommand};
use nemesis_verify::{
    hex_util::hex_encode,
    keygen::{generate_hierarchy, KeyHierarchy},
    verify,
};

#[derive(Parser)]
#[command(name = "verify-loader", about = "DLL 签名验证测试工具")]
struct Cli {
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// 生成密钥体系（root/CA/issuer 私钥 + 证书链）到 JSON
    GenKeys { out: String },
    /// 用发行方私钥签目标文件（带证书链），输出签名后的文件
    Sign { keys: String, target: String, out: String },
    /// 加载 DLL 调 nv_verify_target 验证目标文件
    Verify {
        dll: String,
        target: String,
        #[arg(long)]
        keys: Option<String>,
    },
    /// 调 nv_verify_current_exe 验证本进程 exe（DLL 自验入口）
    VerifySelf {
        dll: String,
        #[arg(long)]
        keys: Option<String>,
    },
    /// 查看：列目标文件所有签名 + 证书链详情（离线，不下结论）
    View { dll: String, target: String },
    /// R7 A2/A3：调 nv_self_verify 验 DLL 自身签名（防替换演示）
    VerifyDll { dll: String },
}

fn main() -> Result<()> {
    match Cli::parse().cmd {
        Cmd::GenKeys { out } => {
            let h = generate_hierarchy(0, u64::MAX);
            println!("root pubkey: {}", hex_encode(&h.root_vk.to_bytes()));
            println!("issuer pubkey: {}", hex_encode(&h.issuer_vk.to_bytes()));
            h.save(&out)?;
            println!("✓ keys → {}", out);
        }
        Cmd::Sign { keys, target, out } => {
            let h = KeyHierarchy::load(&keys)?;
            let content = std::fs::read(&target)?;
            let signed = verify::sign_content(
                &content,
                &h.issuer_sk,
                now_secs(),
                Some(&h.issuer_chain_bytes),
                None,
                None,
                None,
            )?;
            std::fs::write(&out, signed)?;
            println!("✓ signed → {}", out);
        }
        Cmd::Verify { dll, target, keys } => {
            inject_root(keys)?;
            verify_via_dll(&dll, &target)?;
        }
        Cmd::VerifySelf { dll, keys } => {
            inject_root(keys)?;
            verify_self_via_dll(&dll)?;
        }
        Cmd::View { dll, target } => {
            view_via_dll(&dll, &target)?;
        }
        Cmd::VerifyDll { dll } => {
            verify_dll_self(&dll)?;
        }
    }
    Ok(())
}

/// 注入根公钥到 NEMESIS_ROOT_PUBKEY（DLL builtin_roots 读，R7 过渡）。
fn inject_root(keys: Option<String>) -> Result<()> {
    if let Some(k) = keys {
        let h = KeyHierarchy::load(&k)?;
        // edition 2024: set_var 是 unsafe
        unsafe {
            std::env::set_var("NEMESIS_ROOT_PUBKEY", hex_encode(&h.root_vk.to_bytes()));
        }
    }
    Ok(())
}

fn status_name(s: u32) -> &'static str {
    match s {
        0 => "Valid",
        1 => "NoSignature",
        2 => "Tampered",
        3 => "SignatureInvalid",
        4 => "Untrusted",
        5 => "UnsupportedVersion",
        6 => "Malformed",
        7 => "Revoked",
        8 => "Expired",
        _ => "Unknown",
    }
}

#[repr(C)]
#[derive(Default)]
struct NvOutcome {
    status: u32,
    signed_at: u64,
    key_fp: [u8; 32],
    pubkey: [u8; 32],
}

/// 加载 DLL 调 nv_verify_target 验证目标文件。
fn verify_via_dll(dll_path: &str, target: &str) -> Result<()> {
    let lib = unsafe { libloading::Library::new(dll_path) }?;
    let nv: libloading::Symbol<
        unsafe extern "C" fn(*const std::os::raw::c_char, *mut NvOutcome) -> std::os::raw::c_int,
    > = unsafe { lib.get(b"nv_verify_target\0") }?;
    let c_path = std::ffi::CString::new(target)?;
    let mut out = NvOutcome::default();
    let rc = unsafe { nv(c_path.as_ptr(), &mut out) };
    println!("nv_verify_target({}): rc={}, status={} ({})", target, rc, out.status, status_name(out.status));
    if out.status == 0 {
        println!("  signed_at={} key_fp={} pubkey={}", out.signed_at, hex_encode(&out.key_fp), hex_encode(&out.pubkey));
    }
    Ok(())
}

/// 调 nv_verify_current_exe 验证本进程 exe（current_exe）。
fn verify_self_via_dll(dll_path: &str) -> Result<()> {
    let lib = unsafe { libloading::Library::new(dll_path) }?;
    let nv: libloading::Symbol<unsafe extern "C" fn(*mut NvOutcome) -> std::os::raw::c_int> =
        unsafe { lib.get(b"nv_verify_current_exe\0") }?;
    let mut out = NvOutcome::default();
    let rc = unsafe { nv(&mut out) };
    let exe = std::env::current_exe().map(|p| p.display().to_string()).unwrap_or_else(|_| "?".into());
    println!("nv_verify_current_exe({}): rc={}, status={} ({})", exe, rc, out.status, status_name(out.status));
    Ok(())
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvSigInfo {
    index: u32,
    signed_at: u64,
    key_fp: [u8; 32],
    pubkey: [u8; 32],
}

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct NvSigCert {
    subject_pubkey: [u8; 32],
    issuer_key_fp: [u8; 32],
    valid_not_before: u64,
    valid_not_after: u64,
}

#[repr(C)]
#[derive(Default)]
struct NvSigDetail {
    index: u32,
    signed_at: u64,
    key_fp: [u8; 32],
    pubkey: [u8; 32],
    cert_count: u32,
    certs: [NvSigCert; 4],
}

/// 查看：列目标文件所有签名 + 证书链详情（离线展示，不下结论）。
fn view_via_dll(dll_path: &str, target: &str) -> Result<()> {
    let lib = unsafe { libloading::Library::new(dll_path) }?;
    let nv_list: libloading::Symbol<
        unsafe extern "C" fn(*const std::os::raw::c_char, *mut NvSigInfo, *mut u32) -> std::os::raw::c_int,
    > = unsafe { lib.get(b"nv_list_signatures\0") }?;
    let nv_get: libloading::Symbol<
        unsafe extern "C" fn(*const std::os::raw::c_char, u32, *mut NvSigDetail) -> std::os::raw::c_int,
    > = unsafe { lib.get(b"nv_get_signature\0") }?;

    let c_path = std::ffi::CString::new(target)?;
    let mut count: u32 = 8;
    let mut infos: [NvSigInfo; 8] = [NvSigInfo::default(); 8];
    let rc = unsafe { nv_list(c_path.as_ptr(), infos.as_mut_ptr(), &mut count) };
    println!("nv_list_signatures({}): rc={}, total={}", target, rc, count);
    for i in 0..(count as usize).min(8) {
        println!(
            "  [{}] signed_at={} key_fp={}",
            infos[i].index, infos[i].signed_at, hex_encode(&infos[i].key_fp)
        );
    }
    for idx in 0..count.min(4) {
        let mut detail = NvSigDetail::default();
        let rc = unsafe { nv_get(c_path.as_ptr(), idx, &mut detail) };
        println!("  detail[{}] rc={} cert_count={}", idx, rc, detail.cert_count);
        for c in 0..detail.cert_count as usize {
            println!(
                "    cert[{}] subject={} issuer_fp={} valid=[{},{}]",
                c,
                hex_encode(&detail.certs[c].subject_pubkey),
                hex_encode(&detail.certs[c].issuer_key_fp),
                detail.certs[c].valid_not_before,
                detail.certs[c].valid_not_after
            );
        }
    }
    Ok(())
}

/// R7 A2/A3：加载 DLL 后调 nv_self_verify 验 DLL 自身签名（防替换）。
fn verify_dll_self(dll_path: &str) -> Result<()> {
    let lib = unsafe { libloading::Library::new(dll_path) }?;
    let nv_self: libloading::Symbol<unsafe extern "C" fn(*const std::os::raw::c_char) -> std::os::raw::c_int> =
        unsafe { lib.get(b"nv_self_verify\0") }?;
    let c_path = std::ffi::CString::new(dll_path)?;
    let rc = unsafe { nv_self(c_path.as_ptr()) };
    println!(
        "nv_self_verify({}): rc={} ({})",
        dll_path,
        rc,
        if rc == 0 { "Valid✓" } else { "FAILED" }
    );
    Ok(())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}
