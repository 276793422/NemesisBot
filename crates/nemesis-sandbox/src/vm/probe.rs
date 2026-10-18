//! 宿主加速器 / QEMU 二进制可用性探测（诚实缺席——缺席要带原因明示）。
//!
//! 探测分层（渐进收紧，假阳性越来越少）：
//! 1. [`kvm_available`] / [`whpx_dll_present`]：纯静态必要条件（文件/模块
//!    在场）——零成本，缺席即终判不可用；
//! 2. [`qemu_version_ok`]：二进制自身实跑 trivial 探针（对齐仓库 diff
//!    纪律「依赖外部工具：二进制在场 ≠ 可用，用前以工具自身实跑」）；
//! 3. WHPX/KVM 的**真启动**验证（QEMU 以目标 accel 实际起一台机器）归
//!    第 2 步 VM 装配路径——DLL/设备在场只是必要条件，最终裁决在真启动。
//!
//! 全部探测**只在 VM 档激活路径与 Dashboard 探针横幅调用**；VM 总开关
//! 关闭态（铁律）零调用、零进程、零 IO。

use std::path::Path;

/// KVM 可用的静态必要条件：`/dev/kvm` 存在。仅 Linux 有意义；其他平台
/// 恒 false（诚实——非 Linux 不探测）。
pub fn kvm_available() -> bool {
    #[cfg(target_os = "linux")]
    {
        Path::new("/dev/kvm").exists()
    }
    #[cfg(not(target_os = "linux"))]
    {
        false
    }
}

/// WHPX 可用的静态必要条件：`WinHvPlatform.dll` 可加载（Windows Hypervisor
/// Platform API 在场）。非 Windows 恒 false。**非充分条件**——DLL 在场但
/// hypervisor 未运行（BIOS 关虚拟化 / Hyper-V 未启用）时仍不可用，真启动
/// 裁决归 QEMU 装配路径（分层探测第 3 层）。
pub fn whpx_dll_present() -> bool {
    #[cfg(target_os = "windows")]
    {
        // LoadLibraryW 而非 GetModuleHandleW：DLL 未随本进程加载过时
        // GetModuleHandle 假阴性。加载后立即释放句柄（无副作用）。
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = std::ffi::OsStr::new("WinHvPlatform.dll")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        // SAFETY: 传入 NUL 结尾的宽字符串字面量副本；加载系统 DLL 无副作用。
        let handle =
            unsafe { windows_sys::Win32::System::LibraryLoader::LoadLibraryW(wide.as_ptr()) };
        if !handle.is_null() {
            // SAFETY: handle 来自成功的 LoadLibraryW，FreeLibrary 配对释放
            // （FreeLibrary 挂在 Foundation 模块——windows-sys 0.59 布局）。
            unsafe {
                windows_sys::Win32::Foundation::FreeLibrary(handle);
            }
            true
        } else {
            false
        }
    }
    #[cfg(not(target_os = "windows"))]
    {
        false
    }
}

/// QEMU 二进制实跑 trivial 探针：`<qemu> --version` 退出码 0 = 二进制在场
/// 且可执行（返回版本首行）。错误信息带 stderr 现场（诊断自包含纪律）。
pub async fn qemu_version_ok(qemu: &Path) -> Result<String, String> {
    let out = tokio::process::Command::new(qemu)
        .arg("--version")
        .output()
        .await
        .map_err(|e| format!("QEMU 二进制不可执行 {}: {e}", qemu.display()))?;
    if !out.status.success() {
        return Err(format!(
            "QEMU --version 退出码非 0（{}）：{}",
            out.status,
            String::from_utf8_lossy(&out.stderr).trim()
        ));
    }
    let stdout = String::from_utf8_lossy(&out.stdout);
    Ok(stdout.lines().next().unwrap_or("").to_string())
}

/// accel 静态探测汇总（Dashboard 探针横幅与启动前置闸共用；`reason` 缺席
/// 带人读原因）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccelProbe {
    pub available: bool,
    /// 探测到哪一层（static-dll / static-dev / none）。
    pub layer: &'static str,
    /// 缺席原因（available=true 时空串）。
    pub reason: String,
}

/// KVM 探测汇总。
pub fn probe_kvm() -> AccelProbe {
    if kvm_available() {
        AccelProbe {
            available: true,
            layer: "static-dev",
            reason: String::new(),
        }
    } else {
        AccelProbe {
            available: false,
            layer: "none",
            reason: "/dev/kvm 不存在（非 Linux 宿主或 KVM 未启用）".to_string(),
        }
    }
}

/// WHPX 探测汇总（静态层）。
pub fn probe_whpx() -> AccelProbe {
    if whpx_dll_present() {
        AccelProbe {
            available: true,
            layer: "static-dll",
            reason: String::new(),
        }
    } else {
        AccelProbe {
            available: false,
            layer: "none",
            reason: "WinHvPlatform.dll 不可加载（Windows Hypervisor Platform 功能未启用）"
                .to_string(),
        }
    }
}

#[cfg(test)]
mod tests;
