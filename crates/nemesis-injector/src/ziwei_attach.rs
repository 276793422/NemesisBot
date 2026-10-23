//! ziwei_attach —— 紫微系统运行态注入（T1.1）。
//!
//! 对**已运行**进程注入 ziwei payload DLL：OpenProcess → VirtualAllocEx(参数块+stub)
//! → CreateRemoteThread(stub)。stub 在目标进程内 LoadLibraryW(dll) →
//! GetProcAddress("ZiWeiInit") → ZiWeiInit(参数块)。
//!
//! 与 EP 劫持（[`launch_and_inject`]，挂起启动场景）并列的另一条入口；
//! 两者共享本 crate 的错误风格与 windows-sys 依赖面。
//!
//! 参数块/stub 与 payload（plugins/ziwei-agent-dll）**逐字节同源**：
//! [`ZiweiParamBlock`] 是同一 repr(C) 结构体的双写拷贝，stub 的 .byte 序列
//! 与 payload 导出名 `ZiWeiInit` 双边约定——改动必须双写同步。
//!
//! 双架构：目标 wow64 → x86 DLL（x86 stub，stdcall 由被调方清栈）；
//! 否则 x64 DLL（x64 stub，单调用约定）。注入器自身编译为 x64，可注 x86。
//!
//! 错误面诚实分类（总控 §9.2）：OpenProcess 5=拒绝访问（自保护/权限不足）、
//! 87=PID 无效；PE machine 与目标 wow64 态不匹配 = 架构错；远线程超时 = 15s
//! 无退出（可能被安全软件挂起）。

use std::path::PathBuf;

use thiserror::Error;
use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Memory::{
    MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE, VirtualAllocEx, VirtualFreeEx,
};
use windows_sys::Win32::System::Threading::{
    CreateRemoteThread, GetExitCodeThread, IsWow64Process, OpenProcess, WaitForSingleObject,
};

/// 参数块魔数（与 payload 同源）。
pub const PARAM_MAGIC: u32 = 0x5A57_4931;
/// 管道协议版本（与 payload 同源）。
pub const PROTO_VERSION: u32 = 1;

/// 注入参数块（**与 plugins/ziwei-agent-dll/src/lib.rs 的 `ParamBlock` 双写同源**，
/// 改字段/顺序/容量必须两边同步并同步 offset 断言测试）。
#[repr(C)]
#[derive(Clone, Copy)]
pub struct ZiweiParamBlock {
    pub magic: u32,
    pub proto: u32,
    pub fn_load_library_w: u64,
    pub fn_get_proc_address: u64,
    pub dll_path: [u16; 260],
    pub init_name: [u8; 16],
    pub token: [u8; 64],
    pub pipe_name: [u16; 128],
    pub arch: [u8; 8],
}

// 字段内偏移（stub 里要用；测试钉死与 repr(C) 实际布局一致）。
const OFF_DLL_PATH: usize = 24;
const OFF_INIT_NAME: usize = 24 + 260 * 2;
/// 参数块总大小（888）。
pub const PARAM_BLOCK_SIZE: usize = std::mem::size_of::<ZiweiParamBlock>();

/// 注入错误（诚实分类，win32 code 原样透出）。
#[derive(Error, Debug)]
pub enum ZiweiAttachError {
    #[error("OpenProcess 失败（pid={pid} err={err}）：{hint}")]
    OpenProcess {
        pid: u32,
        err: u32,
        hint: &'static str,
    },
    #[error("目标架构与 payload 架构不匹配：目标={target} payload={payload}")]
    ArchMismatch {
        target: &'static str,
        payload: &'static str,
    },
    #[error("payload DLL PE 头解析失败（{0}）")]
    PayloadPe(String),
    #[error("VirtualAllocEx 失败: {0}")]
    Alloc(u32),
    #[error("WriteProcessMemory 失败: {0}")]
    Write(u32),
    #[error("解析 kernel32 API 失败")]
    ResolveApi,
    #[error("CreateRemoteThread 失败: {0}")]
    RemoteThread(u32),
    #[error("远线程 15s 未退出（可能被安全软件挂起；远程内存保留不释放防崩溃）")]
    RemoteThreadTimeout,
    #[error("ZiWeiInit 返回 {0}（1=魔数错 2=协议错 3=起线程失败）")]
    InitFailed(u32),
}

/// 注入请求。
pub struct ZiweiAttachParams {
    pub pid: u32,
    /// payload DLL 绝对路径（x86/x64 已按目标选定）。
    pub dll_path: PathBuf,
    /// 握手令牌（ASCII，≤63 字节）。
    pub token: String,
    /// 完整管道名 `\\.\pipe\ZiWei_<pid>_<rand>`（UTF-16 容量 ≤127）。
    pub pipe_name: String,
}

/// 注入结果。
pub struct ZiweiAttachOutcome {
    /// 实际注入架构（"x64"/"x86"）。
    pub arch: &'static str,
    pub remote_base: u64,
    pub init_result: u32,
}

// ---------------------------------------------------------------------------
// x64 stub（68 字节）：LoadLibraryW(dll_path) → GetProcAddress(hmod,"ZiWeiInit")
// → ZiWeiInit(param)；返回值=线程退出码。
// ---------------------------------------------------------------------------
const X64_STUB: [u8; 68] = [
    0x48, 0x83, 0xEC, 0x28, // 00 sub rsp,28h
    0x48, 0xB9, 0, 0, 0, 0, 0, 0, 0, 0, // 04 movabs rcx, dll_path
    0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, // 0E movabs rax, LoadLibraryW
    0xFF, 0xD0, // 18 call rax
    0x48, 0x89, 0xC1, // 1A mov rcx, rax（hmodule）
    0x48, 0xBA, 0, 0, 0, 0, 0, 0, 0, 0, // 1D movabs rdx, "ZiWeiInit"
    0x48, 0xB8, 0, 0, 0, 0, 0, 0, 0, 0, // 27 movabs rax, GetProcAddress
    0xFF, 0xD0, // 31 call rax
    0x48, 0xB9, 0, 0, 0, 0, 0, 0, 0, 0, // 33 movabs rcx, param
    0xFF, 0xD0, // 3D call rax（ZiWeiInit）
    0x48, 0x83, 0xC4, 0x28, // 3F add rsp,28h
    0xC3, // 43 ret
];
// 立即数偏移（指令内 imm 的起始位置；与上方注释的指令起始地址一一对应）。
const X64_OFF_DLLPATH: usize = 0x06;
const X64_OFF_LOADLIB: usize = 0x10;
const X64_OFF_NAME: usize = 0x1F;
const X64_OFF_GPA: usize = 0x29;
const X64_OFF_PARAM: usize = 0x35;

// ---------------------------------------------------------------------------
// x86 stub（33 字节，stdcall：被调方清栈；返回值经 eax 透传为线程退出码）。
// ⚠ GetProcAddress(HMODULE, LPCSTR) 参数右到左压栈：name 先压（深）、hmod 后压
// （浅，[esp+4]）——顺序颠倒时 GPA 拿参数块地址当模块句柄（非 MZ）恒返
// NULL+126（实证：x86 WOW64 attach 假失败 0xC0000005 的根因）。
// ---------------------------------------------------------------------------
const X86_STUB: [u8; 33] = [
    0x68, 0, 0, 0, 0, // 00 push dll_path
    0xB8, 0, 0, 0, 0, // 05 mov eax, LoadLibraryW
    0xFF, 0xD0, // 0A call eax（stdcall 清栈）
    0x68, 0, 0, 0, 0,    // 0C push "ZiWeiInit"（第二参，先压）
    0x50, // 11 push eax（hmodule，第一参后压 → [esp+4]=hmod）
    0xB8, 0, 0, 0, 0, // 12 mov eax, GetProcAddress
    0xFF, 0xD0, // 17 call eax（清 8 字节）
    0x68, 0, 0, 0, 0, // 19 push param
    0xFF, 0xD0, // 1E call eax（ZiWeiInit，清栈；eax=结果）
    0xC3, // 20 ret
];
const X86_OFF_DLLPATH: usize = 1;
const X86_OFF_LOADLIB: usize = 6;
const X86_OFF_NAME: usize = 0x0D;
const X86_OFF_GPA: usize = 0x13;
const X86_OFF_PARAM: usize = 0x1A;

fn put64(buf: &mut [u8], off: usize, v: u64) {
    buf[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

fn put32(buf: &mut [u8], off: usize, v: u32) {
    buf[off..off + 4].copy_from_slice(&v.to_le_bytes());
}

/// 读 payload DLL 的 PE machine（0x8664=x64 / 0x014C=x86）。
fn payload_machine(dll: &[u8]) -> Result<u16, ZiweiAttachError> {
    if dll.len() < 0x40 {
        return Err(ZiweiAttachError::PayloadPe("文件过短".into()));
    }
    let e_lfanew = u32::from_le_bytes(dll[0x3C..0x40].try_into().unwrap()) as usize;
    if dll.len() < e_lfanew + 6 || &dll[e_lfanew..e_lfanew + 4] != b"PE\0\0" {
        return Err(ZiweiAttachError::PayloadPe("PE 签名缺失".into()));
    }
    Ok(u16::from_le_bytes(
        dll[e_lfanew + 4..e_lfanew + 6].try_into().unwrap(),
    ))
}

/// 对已运行进程注入 payload（同步阻塞；调用方负责放 tokio spawn_blocking）。
///
/// 成功返回后 payload 工作线程已在目标内异步连管道；远程内存已回收
/// （超时挂起场景除外——保留内存防崩溃，诚实报错）。
pub fn attach_inject(params: &ZiweiAttachParams) -> Result<ZiweiAttachOutcome, ZiweiAttachError> {
    let dll_bytes = std::fs::read(&params.dll_path)
        .map_err(|e| ZiweiAttachError::PayloadPe(format!("{}: {e}", params.dll_path.display())))?;
    let dll_machine = payload_machine(&dll_bytes)?;

    // PROCESS_CREATE_THREAD | QUERY_INFORMATION | VM_OPERATION | VM_WRITE | VM_READ
    const RIGHTS: u32 = 0x0002 | 0x0400 | 0x0008 | 0x0020 | 0x0010;
    let hp = unsafe { OpenProcess(RIGHTS, 0, params.pid) };
    if hp.is_null() {
        let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
        let hint: &'static str = match err {
            5 => "拒绝访问（目标自保护/权限不足）",
            87 => "PID 无效（进程不存在）",
            _ => "打开目标进程失败",
        };
        return Err(ZiweiAttachError::OpenProcess {
            pid: params.pid,
            err,
            hint,
        });
    }
    let outcome = attach_with_handle(hp, params, dll_machine);
    unsafe { CloseHandle(hp) };
    outcome
}

/// 【export for tests】对已打开句柄注入（tests 用 own-process 句柄做注入自检）。
fn attach_with_handle(
    hp: HANDLE,
    params: &ZiweiAttachParams,
    dll_machine: u16,
) -> Result<ZiweiAttachOutcome, ZiweiAttachError> {
    // 目标架构：wow64 = x86，否则 x64。
    let mut wow64: i32 = 0;
    let target_arch: &'static str = if unsafe { IsWow64Process(hp, &mut wow64) } != 0 && wow64 != 0
    {
        "x86"
    } else {
        "x64"
    };
    let payload_arch: &'static str = match dll_machine {
        0x8664 => "x64",
        0x014C => "x86",
        other => {
            return Err(ZiweiAttachError::PayloadPe(format!(
                "未知 machine {other:#x}"
            )));
        }
    };
    if target_arch != payload_arch {
        return Err(ZiweiAttachError::ArchMismatch {
            target: target_arch,
            payload: payload_arch,
        });
    }

    // 远端 API 地址：同位数走 per-boot 同基址；WOW64 走模块快照 + SysWOW64
    // 导出表（ziwei_wow64——x64 kernel32 基址对 32 位目标是无效地址）。
    let (load_lib, gpa) = super::ziwei_wow64::resolve_remote_api(params.pid, target_arch)?;
    unsafe {
        // 组参数块
        let mut param = ZiweiParamBlock {
            magic: PARAM_MAGIC,
            proto: PROTO_VERSION,
            fn_load_library_w: load_lib,
            fn_get_proc_address: gpa,
            dll_path: [0; 260],
            init_name: [0; 16],
            token: [0; 64],
            pipe_name: [0; 128],
            arch: [0; 8],
        };
        let dp: Vec<u16> = params
            .dll_path
            .to_string_lossy()
            .encode_utf16()
            .take(259)
            .collect();
        param.dll_path[..dp.len()].copy_from_slice(&dp);
        let init = b"ZiWeiInit";
        param.init_name[..init.len()].copy_from_slice(init);
        let tok = params.token.as_bytes();
        let tok_n = tok.len().min(63);
        param.token[..tok_n].copy_from_slice(&tok[..tok_n]);
        let pn: Vec<u16> = params.pipe_name.encode_utf16().take(127).collect();
        param.pipe_name[..pn.len()].copy_from_slice(&pn);
        let arch_bytes = payload_arch.as_bytes();
        param.arch[..arch_bytes.len()].copy_from_slice(arch_bytes);

        // 远程分配：参数块 + stub（两页富余）
        let alloc_size = ((PARAM_BLOCK_SIZE + X64_STUB.len()) & !0xFFF) + 0x1000;
        let base = VirtualAllocEx(
            hp,
            std::ptr::null(),
            alloc_size,
            MEM_COMMIT | MEM_RESERVE,
            PAGE_EXECUTE_READWRITE,
        );
        if base.is_null() {
            return Err(ZiweiAttachError::Alloc(
                std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32,
            ));
        }
        let base_addr = base as u64;

        // 组内存镜像：[参数块][stub]
        let param_bytes: &[u8] = std::slice::from_raw_parts(
            &param as *const ZiweiParamBlock as *const u8,
            PARAM_BLOCK_SIZE,
        );
        let (stub, o_dllpath, o_loadlib, o_name, o_gpa, o_param): (
            &[u8],
            usize,
            usize,
            usize,
            usize,
            usize,
        ) = if payload_arch == "x64" {
            (
                &X64_STUB,
                X64_OFF_DLLPATH,
                X64_OFF_LOADLIB,
                X64_OFF_NAME,
                X64_OFF_GPA,
                X64_OFF_PARAM,
            )
        } else {
            (
                &X86_STUB,
                X86_OFF_DLLPATH,
                X86_OFF_LOADLIB,
                X86_OFF_NAME,
                X86_OFF_GPA,
                X86_OFF_PARAM,
            )
        };
        let mut image = vec![0u8; PARAM_BLOCK_SIZE + stub.len()];
        image[..PARAM_BLOCK_SIZE].copy_from_slice(param_bytes);
        let stub_base = PARAM_BLOCK_SIZE;
        image[stub_base..stub_base + stub.len()].copy_from_slice(stub);
        let stub_addr = base_addr + stub_base as u64;
        let addr_of = |field_off: usize| base_addr + field_off as u64;
        if payload_arch == "x64" {
            put64(&mut image, stub_base + o_dllpath, addr_of(OFF_DLL_PATH));
            put64(&mut image, stub_base + o_loadlib, load_lib);
            put64(&mut image, stub_base + o_name, addr_of(OFF_INIT_NAME));
            put64(&mut image, stub_base + o_gpa, gpa);
            put64(&mut image, stub_base + o_param, base_addr);
        } else {
            put32(
                &mut image,
                stub_base + o_dllpath,
                addr_of(OFF_DLL_PATH) as u32,
            );
            put32(&mut image, stub_base + o_loadlib, load_lib as u32);
            put32(
                &mut image,
                stub_base + o_name,
                addr_of(OFF_INIT_NAME) as u32,
            );
            put32(&mut image, stub_base + o_gpa, gpa as u32);
            put32(&mut image, stub_base + o_param, base_addr as u32);
        }

        let mut done: usize = 0;
        if WriteProcessMemory(
            hp,
            base,
            image.as_mut_ptr() as *const _,
            image.len(),
            &mut done,
        ) == 0
            || done != image.len()
        {
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            VirtualFreeEx(hp, base, 0, 0x8000); // MEM_RELEASE
            return Err(ZiweiAttachError::Write(err));
        }

        let thread = CreateRemoteThread(
            hp,
            std::ptr::null(),
            0,
            Some(std::mem::transmute::<
                u64,
                unsafe extern "system" fn(*mut std::ffi::c_void) -> u32,
            >(stub_addr)),
            std::ptr::null_mut(),
            0,
            std::ptr::null_mut(),
        );
        if thread.is_null() {
            let err = std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32;
            VirtualFreeEx(hp, base, 0, 0x8000);
            return Err(ZiweiAttachError::RemoteThread(err));
        }

        // 等 stub 退出（ZiWeiInit 返回即退；工作线程在 payload 内独立存活）
        let wait = WaitForSingleObject(thread, 15_000);
        let mut code: u32 = 259; // STILL_ACTIVE
        GetExitCodeThread(thread, &mut code);
        CloseHandle(thread);

        if wait != 0 || code == 259 {
            // 挂起：远程内存保留不释放（线程还在读参数块），诚实报错
            return Err(ZiweiAttachError::RemoteThreadTimeout);
        }
        if code != 0 {
            VirtualFreeEx(hp, base, 0, 0x8000);
            return Err(ZiweiAttachError::InitFailed(code));
        }
        VirtualFreeEx(hp, base, 0, 0x8000);
        Ok(ZiweiAttachOutcome {
            arch: payload_arch,
            remote_base: base_addr,
            init_result: code,
        })
    }
}

// 布局自检：字段内偏移与 stub 常量必须一致（repr(C) 布局漂移 = 注入野写）。
#[cfg(test)]
mod tests;
