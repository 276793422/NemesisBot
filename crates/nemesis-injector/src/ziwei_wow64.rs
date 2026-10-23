//! wow64 —— WOW64 目标的远端 API 地址解析（x64 bot → x86 目标）。
//!
//! 「kernel32 同 boot 全进程同基址」假设**只在同位数内成立**：x64 bot 的
//! `GetModuleHandleW("kernel32.dll")` 返回的是 64 位 kernel32 基址，对
//! WOW64（32 位）目标无意义——32 位 kernel32 是另一份映像（SysWOW64），
//! 基址独立。
//!
//! 正解（经典三步）：
//! 1. `CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid)`
//!    枚举目标 32 位模块 → kernel32.dll 的远端基址；
//! 2. 解析 `C:\Windows\SysWOW64\kernel32.dll` 文件（WOW64 目标加载的就是
//!    同一文件，RVA 精确）的 PE 导出表 → LoadLibraryW / GetProcAddress RVA；
//! 3. 远端地址 = 远端基址 + RVA。
//!
//! 同位数（x64 目标）走原路径（本进程 GetModuleHandleW + GetProcAddress）。

use super::ziwei_attach::ZiweiAttachError;
use windows_sys::Win32::Foundation::{CloseHandle, INVALID_HANDLE_VALUE};
use windows_sys::Win32::System::Diagnostics::ToolHelp::{
    CreateToolhelp32Snapshot, MODULEENTRY32W, Module32FirstW, Module32NextW, TH32CS_SNAPMODULE,
    TH32CS_SNAPMODULE32,
};

/// 解析目标进程内 LoadLibraryW / GetProcAddress 的绝对地址。
///
/// - x64 目标：本进程同基址前提（per-boot）；
/// - x86（WOW64）目标：模块快照 + SysWOW64 文件导出表 RVA。
pub fn resolve_remote_api(pid: u32, target_arch: &str) -> Result<(u64, u64), ZiweiAttachError> {
    if target_arch == "x64" {
        // 同位数：本进程基址 = 目标基址（同 boot Windows 全系统一致）
        let k32 = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetModuleHandleW(KERNEL32_W.as_ptr())
        };
        if k32.is_null() {
            return Err(ZiweiAttachError::ResolveApi);
        }
        let load_lib = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                k32,
                c"LoadLibraryW".as_ptr().cast(),
            )
        }
        .ok_or(ZiweiAttachError::ResolveApi)? as usize as u64;
        let gpa = unsafe {
            windows_sys::Win32::System::LibraryLoader::GetProcAddress(
                k32,
                c"GetProcAddress".as_ptr().cast(),
            )
        }
        .ok_or(ZiweiAttachError::ResolveApi)? as usize as u64;
        Ok((load_lib, gpa))
    } else {
        // WOW64：目标 32 位模块快照 + SysWOW64 导出表
        let base = find_wow64_kernel32_base(pid).ok_or(ZiweiAttachError::ResolveApi)?;
        let sys_dir = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
        let k32_path = std::path::Path::new(&sys_dir)
            .join("SysWOW64")
            .join("kernel32.dll");
        let rvas = pe_export_rvas(&k32_path, &["LoadLibraryW", "GetProcAddress"])
            .map_err(|_| ZiweiAttachError::ResolveApi)?;
        let load_lib = base + rvas[0];
        let gpa = base + rvas[1];
        Ok((load_lib, gpa))
    }
}

/// "kernel32.dll\0" UTF-16 常量。
const KERNEL32_W: &[u16] = &[
    b'k' as u16,
    b'e' as u16,
    b'r' as u16,
    b'n' as u16,
    b'e' as u16,
    b'l' as u16,
    b'3' as u16,
    b'2' as u16,
    b'.' as u16,
    b'd' as u16,
    b'l' as u16,
    b'l' as u16,
    0,
];

/// 枚举目标进程 32 位模块（WOW64 视图），返回 kernel32.dll 的基址。
fn find_wow64_kernel32_base(pid: u32) -> Option<u64> {
    unsafe {
        let snap = CreateToolhelp32Snapshot(TH32CS_SNAPMODULE | TH32CS_SNAPMODULE32, pid);
        if snap == INVALID_HANDLE_VALUE {
            return None;
        }
        let mut entry: MODULEENTRY32W = std::mem::zeroed();
        entry.dwSize = std::mem::size_of::<MODULEENTRY32W>() as u32;
        let mut found: Option<u64> = None;
        if Module32FirstW(snap, &mut entry) != 0 {
            loop {
                let name = utf16_lower(&entry.szModule);
                if name == "kernel32.dll" {
                    found = Some(entry.modBaseAddr as u64);
                    break;
                }
                if Module32NextW(snap, &mut entry) == 0 {
                    break;
                }
            }
        }
        let _ = CloseHandle(snap);
        found
    }
}

fn utf16_lower(buf: &[u16]) -> String {
    let end = buf.iter().position(|&c| c == 0).unwrap_or(buf.len());
    String::from_utf16_lossy(&buf[..end]).to_ascii_lowercase()
}

/// 从 PE 文件导出表解析指定函数名的 RVA 列表（按 `names` 顺序；缺失 = Err）。
///
/// 纯文件解析（不加载映像）：DOS → PE → 可选头 → 数据目录[0] 导出表 →
/// 名字表/序号表/函数表三路跳转。PE32 与 PE32+ 都支持（导出目录偏移不同）。
pub fn pe_export_rvas(path: &std::path::Path, names: &[&str]) -> Result<Vec<u64>, String> {
    let data = std::fs::read(path).map_err(|e| format!("读 {}: {e}", path.display()))?;
    let u16_at = |off: usize| -> Result<u16, String> {
        data.get(off..off + 2)
            .map(|b| u16::from_le_bytes([b[0], b[1]]))
            .ok_or_else(|| "PE 截断".to_string())
    };
    let u32_at = |off: usize| -> Result<u32, String> {
        data.get(off..off + 4)
            .map(|b| u32::from_le_bytes([b[0], b[1], b[2], b[3]]))
            .ok_or_else(|| "PE 截断".to_string())
    };

    // DOS header
    if data.len() < 0x40 || &data[0..2] != b"MZ" {
        return Err("非 MZ".into());
    }
    let lfanew = u32_at(0x3C)? as usize;
    if data.get(lfanew..lfanew + 4) != Some(b"PE\0\0") {
        return Err("非 PE".into());
    }
    let coff = lfanew + 4;
    let opt = coff + 20;
    let magic = u16_at(opt)?;
    let (data_dirs_off, _opt_hdr_size) = match magic {
        0x10B => (opt + 96, u16_at(coff + 16)? as usize), // PE32
        0x20B => (opt + 112, u16_at(coff + 16)? as usize), // PE32+
        m => return Err(format!("未知可选头 magic {m:#x}")),
    };
    let export_rva = u32_at(data_dirs_off)?;
    if export_rva == 0 {
        return Err("无导出表".into());
    }

    // RVA → 文件偏移（走节表）
    let sections_off = opt + u16_at(coff + 16)? as usize;
    let n_sections = u16_at(coff + 2)? as usize;
    let rva_to_off = |rva: u32| -> Result<usize, String> {
        for i in 0..n_sections {
            let s = sections_off + i * 40;
            let vsize = u32_at(s + 8)?;
            let vaddr = u32_at(s + 12)?;
            let raw = u32_at(s + 20)?;
            if vaddr <= rva && rva < vaddr + vsize.max(raw) {
                return Ok((rva - vaddr + raw) as usize);
            }
        }
        Err(format!("RVA {rva:#x} 不在任何节内"))
    };

    let exp = rva_to_off(export_rva)?;
    let n_funcs = u32_at(exp + 20)? as usize;
    let n_names = u32_at(exp + 24)? as usize;
    let names_rva = u32_at(exp + 32)?;
    let ords_rva = u32_at(exp + 36)?;
    let funcs_rva = u32_at(exp + 28)?;

    let names_off = rva_to_off(names_rva)?;
    let ords_off = rva_to_off(ords_rva)?;
    let funcs_off = rva_to_off(funcs_rva)?;

    let mut out = vec![0u64; names.len()];
    for i in 0..n_names {
        let name_rva = u32_at(names_off + i * 4)?;
        let noff = rva_to_off(name_rva)?;
        let end = data[noff..].iter().position(|&b| b == 0).map(|p| noff + p);
        let Some(end) = end else { continue };
        let name = &data[noff..end];
        for (want_idx, want) in names.iter().enumerate() {
            if name.eq_ignore_ascii_case(want.as_bytes()) {
                let ordinal = u16_at(ords_off + i * 2)? as usize;
                if ordinal >= n_funcs {
                    return Err(format!("序号越界 {ordinal}≥{n_funcs}"));
                }
                out[want_idx] = u32_at(funcs_off + ordinal * 4)? as u64;
            }
        }
    }
    if out.contains(&0) {
        return Err("导出名未找齐".into());
    }
    Ok(out)
}

#[cfg(test)]
mod tests;
