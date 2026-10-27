//! 磁盘水位（M2，2026-10-27 整改批次一）。
//!
//! workspace 所在卷剩余空间查询 + 阈值评级纯函数。API 选型零新依赖：
//! Windows = `windows-sys` `GetDiskFreeSpaceExW`（workspace 已有 0.59），
//! unix = `libc::statvfs`。
//!
//! 消费形态见 `loop/src/commands/gateway/services.rs`（gateway 装配点）：
//! 周期 tick → [`volume_free_bytes`] → [`evaluate`] → 升档时告警 +
//! `/ready` 检查项。本 crate 不带事件面（health 是低层 crate，不拉 web）。

use std::path::Path;

/// 水位评级。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WatermarkLevel {
    /// 剩余空间高于 warn 阈值。
    Ok,
    /// 剩余空间低于 warn 阈值、高于 critical 阈值。
    Warn,
    /// 剩余空间低于 critical 阈值（回退网/历史落盘即将失效）。
    Critical,
}

impl WatermarkLevel {
    /// SSE 载荷用的小写字符串（`ok` / `warn` / `critical`）。
    pub fn as_str(&self) -> &'static str {
        match self {
            WatermarkLevel::Ok => "ok",
            WatermarkLevel::Warn => "warn",
            WatermarkLevel::Critical => "critical",
        }
    }
}

/// 阈值评级纯函数。`warn_gb <= critical_gb` 时防呆：critical 阈值优先判
/// （口径=更严的一侧先命中，避免配置倒挂时 warn 吞掉 critical）。
pub fn evaluate(free_bytes: u64, warn_gb: u64, critical_gb: u64) -> WatermarkLevel {
    let gb = free_bytes / (1024 * 1024 * 1024);
    if gb < critical_gb {
        WatermarkLevel::Critical
    } else if gb < warn_gb {
        WatermarkLevel::Warn
    } else {
        WatermarkLevel::Ok
    }
}

/// 查询 `path` 所在卷的剩余字节数。路径须存在（Windows
/// `GetDiskFreeSpaceExW` 对不存在路径报 ERROR_PATH_NOT_FOUND，POSIX
/// `statvfs` 报 ENOENT——都不做「按卷答」的宽容解析）。调用方传入的
/// workspace 目录是存在路径，正常使用不受影响。
pub fn volume_free_bytes(path: &Path) -> Result<u64, String> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let wide: Vec<u16> = path
            .as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        let mut free: u64 = 0;
        let mut total: u64 = 0;
        let mut _total_free: u64 = 0;
        // SAFETY：三个出参都是本栈上的合法 u64 指针；入参是 NUL 结尾的
        // 宽字符路径副本。
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free as *mut u64,
                &mut total as *mut u64,
                &mut _total_free as *mut u64,
            )
        };
        if ok == 0 {
            return Err(format!(
                "GetDiskFreeSpaceExW failed for {} (win32 error {})",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        Ok(free)
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        let c = CString::new(path.as_os_str().as_encoded_bytes())
            .map_err(|_| format!("path contains NUL: {}", path.display()))?;
        // SAFETY：c 是合法 NUL 结尾 C 字符串；statvfs 出参是本栈合法结构体。
        let mut st: libc::statvfs = unsafe { std::mem::zeroed() };
        let rc = unsafe { libc::statvfs(c.as_ptr(), &mut st) };
        if rc != 0 {
            return Err(format!(
                "statvfs failed for {}: {}",
                path.display(),
                std::io::Error::last_os_error()
            ));
        }
        // f_bavail = 非特权进程可用块（f_bfree 含 root 保留块，偏乐观）。
        Ok(st.f_bavail as u64 * st.f_frsize as u64)
    }
}

#[cfg(test)]
mod tests;
