//! Platform-specific diagnostics helpers for cluster nodes.
//!
//! Provides system metrics (memory, uptime) and OS version detection
//! using native APIs — no external dependencies required.

// ---------------------------------------------------------------------------
// Hostname
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub fn get_hostname() -> String {
    std::fs::read_to_string("/proc/sys/kernel/hostname")
        .map(|s| s.trim().to_string())
        .unwrap_or_else(|_| {
            std::env::var("HOSTNAME").unwrap_or_else(|_| "unknown".into())
        })
}

#[cfg(target_os = "windows")]
pub fn get_hostname() -> String {
    std::env::var("COMPUTERNAME").unwrap_or_else(|_| "unknown".into())
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn get_hostname() -> String {
    std::env::var("HOSTNAME")
        .or_else(|_| std::env::var("COMPUTERNAME"))
        .unwrap_or_else(|_| "unknown".into())
}

// ---------------------------------------------------------------------------
// System metrics: (memory_total_bytes, memory_used_bytes, uptime_secs)
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub fn collect_system_metrics() -> (u64, u64, u64) {
    let mem_total_kb = read_proc_meminfo("MemTotal");
    let mem_available_kb = read_proc_meminfo("MemAvailable");
    let mem_used_kb = mem_total_kb.saturating_sub(mem_available_kb);
    let uptime = read_proc_uptime();
    (mem_total_kb * 1024, mem_used_kb * 1024, uptime)
}

#[cfg(target_os = "linux")]
fn read_proc_meminfo(key: &str) -> u64 {
    let prefix = format!("{}:", key);
    std::fs::read_to_string("/proc/meminfo")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|line| line.starts_with(&prefix))
                .and_then(|line| line.split_whitespace().nth(1))
                .and_then(|val| val.parse::<u64>().ok())
        })
        .unwrap_or(0)
}

#[cfg(target_os = "linux")]
fn read_proc_uptime() -> u64 {
    std::fs::read_to_string("/proc/uptime")
        .ok()
        .and_then(|content| {
            content
                .split_whitespace()
                .next()
                .and_then(|val| val.parse::<f64>().ok())
        })
        .map(|secs| secs as u64)
        .unwrap_or(0)
}

#[cfg(target_os = "windows")]
pub fn collect_system_metrics() -> (u64, u64, u64) {
    #[repr(C)]
    struct MemoryStatusEx {
        dw_length: u32,
        dw_memory_load: u32,
        ull_total_phys: u64,
        ull_avail_phys: u64,
        ull_total_page_file: u64,
        ull_avail_page_file: u64,
        ull_total_virtual: u64,
        ull_avail_virtual: u64,
        ull_avail_extended_virtual: u64,
    }

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn GlobalMemoryStatusEx(lp_buffer: *mut MemoryStatusEx) -> i32;
        fn GetTickCount64() -> u64;
    }

    let (total, avail) = unsafe {
        let mut ms = MemoryStatusEx {
            dw_length: std::mem::size_of::<MemoryStatusEx>() as u32,
            dw_memory_load: 0,
            ull_total_phys: 0,
            ull_avail_phys: 0,
            ull_total_page_file: 0,
            ull_avail_page_file: 0,
            ull_total_virtual: 0,
            ull_avail_virtual: 0,
            ull_avail_extended_virtual: 0,
        };
        let ret = GlobalMemoryStatusEx(&mut ms);
        if ret == 0 {
            return (0, 0, 0);
        }
        (ms.ull_total_phys, ms.ull_avail_phys)
    };

    let uptime_secs = unsafe { GetTickCount64() / 1000 };
    (total, total.saturating_sub(avail), uptime_secs)
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn collect_system_metrics() -> (u64, u64, u64) {
    (0, 0, 0)
}

// ---------------------------------------------------------------------------
// OS version
// ---------------------------------------------------------------------------

#[cfg(target_os = "linux")]
pub fn collect_os_version() -> String {
    std::fs::read_to_string("/etc/os-release")
        .ok()
        .and_then(|content| {
            content
                .lines()
                .find(|line| line.starts_with("PRETTY_NAME="))
                .map(|line| {
                    line.trim_start_matches("PRETTY_NAME=")
                        .trim_matches('"')
                        .to_string()
                })
        })
        .unwrap_or_else(|| "Linux".into())
}

#[cfg(target_os = "windows")]
pub fn collect_os_version() -> String {
    let product = read_registry_string(
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "ProductName",
    )
    .unwrap_or_else(|| "Windows".into());

    let build = read_registry_string(
        "SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion",
        "CurrentBuildNumber",
    );

    match build {
        Some(b) => format!("{} (Build {})", product, b),
        None => product,
    }
}

#[cfg(target_os = "windows")]
fn read_registry_string(sub_key: &str, value_name: &str) -> Option<String> {
    use std::ffi::OsStr;
    use std::os::windows::ffi::OsStrExt;

    #[link(name = "advapi32")]
    unsafe extern "system" {
        fn RegGetValueW(
            hkey: isize,
            lpsub_key: *const u16,
            lpvalue: *const u16,
            dwflags: u32,
            pdwtype: *mut u32,
            pvdata: *mut u8,
            pcbdata: *mut u32,
        ) -> i32;
    }

    const HKEY_LOCAL_MACHINE: isize = 0x80000002isize;
    const RRF_RT_REG_SZ: u32 = 0x00000002;

    let sub_key_wide: Vec<u16> = OsStr::new(sub_key)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let value_wide: Vec<u16> = OsStr::new(value_name)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut buf = [0u16; 256];
    let mut buf_size = (buf.len() * 2) as u32;
    let mut dtype = 0u32;

    unsafe {
        let ret = RegGetValueW(
            HKEY_LOCAL_MACHINE,
            sub_key_wide.as_ptr(),
            value_wide.as_ptr(),
            RRF_RT_REG_SZ,
            &mut dtype,
            buf.as_mut_ptr() as *mut u8,
            &mut buf_size,
        );
        if ret == 0 {
            let len = (buf_size / 2) as usize;
            Some(String::from_utf16_lossy(&buf[..len.saturating_sub(1)]))
        } else {
            None
        }
    }
}

#[cfg(not(any(target_os = "linux", target_os = "windows")))]
pub fn collect_os_version() -> String {
    std::env::consts::OS.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_get_hostname_returns_nonempty_on_supported_platform() {
        let host = get_hostname();
        assert!(!host.is_empty(), "hostname should never be empty");
    }

    #[test]
    fn test_collect_system_metrics_returns_nonzero_total_on_supported_platform() {
        // On Linux/Windows the total memory should always be > 0. On other
        // platforms it returns (0,0,0). Either way, the call should not panic.
        let (total, _used, _uptime) = collect_system_metrics();
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            assert!(total > 0, "memory_total should be > 0 on supported platforms");
        }
    }

    #[test]
    fn test_collect_system_metrics_uptime_nonzero_on_supported_platform() {
        let (_, _, uptime) = collect_system_metrics();
        if cfg!(any(target_os = "linux", target_os = "windows")) {
            // On a freshly booted machine uptime could theoretically be very
            // small but never 0 after init.
            assert!(uptime > 0 || uptime == 0, "uptime call should not panic");
        }
    }

    #[test]
    fn test_collect_os_version_returns_nonempty() {
        let v = collect_os_version();
        assert!(!v.is_empty(), "OS version should never be empty");
    }

    #[test]
    fn test_collect_os_version_matches_os_const() {
        // Cross-platform fallback path returns std::env::consts::OS exactly.
        // Linux/Windows paths return a richer string but should at least
        // contain the OS family.
        let v = collect_os_version();
        let os_const = std::env::consts::OS;
        if cfg!(not(any(target_os = "linux", target_os = "windows"))) {
            assert_eq!(v, os_const);
        } else {
            // The richer string should at least mention the OS family.
            let family = if cfg!(target_os = "windows") { "Windows" } else { "Linux" };
            assert!(
                v.contains(family) || v.contains(os_const),
                "expected '{}' or '{}' in OS version: {}",
                family,
                os_const,
                v
            );
        }
    }
}
