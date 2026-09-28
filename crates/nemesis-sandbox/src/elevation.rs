//! Elevation: detect whether the process is admin, and re-launch self elevated
//! via `ShellExecuteW("runas", ...)`. Mirrors the `spawn_elevated` pattern in
//! `nemesis-web/src/handlers/cluster.rs:2053` (the project's existing elevation
//! primitive), kept here so `nemesis-sandbox` is self-contained.
//!
//! `sandbox install/uninstall` need admin (KmdUtil opens
//! SC_MANAGER_CREATE_SERVICE). The CLI flow: a non-elevated process detects
//! `!is_elevated()` and re-launches itself elevated with an internal flag
//! (`sandbox install --internal`); the elevated child runs KmdUtil
//! synchronously and exits; the parent polls `status::service_state` to confirm.

#[cfg(windows)]
mod win {
    use std::os::windows::ffi::OsStrExt;
    use std::path::Path;

    #[link(name = "shell32")]
    unsafe extern "system" {
        fn ShellExecuteW(
            hwnd: isize,
            lp_operation: *const u16,
            lp_file: *const u16,
            lp_parameters: *const u16,
            lp_directory: *const u16,
            n_show_cmd: i32,
        ) -> isize;
    }

    const SW_HIDE: i32 = 0;

    fn wide(s: &str) -> Vec<u16> {
        std::ffi::OsStr::new(s)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    /// Re-launch `exe` elevated with `args`. Fire-and-forget (returns once the
    /// elevated process is launched, not when it finishes) — callers poll for
    /// the side effect (e.g. service appears) to detect completion.
    pub fn relaunch_elevated(exe: &Path, args: &[String]) -> anyhow::Result<()> {
        let op = wide("runas");
        let file = wide(&exe.to_string_lossy());
        // Quote any arg containing spaces (e.g. `--home C:\Users\My Name\...`)
        // so the elevated child's command-line parser receives it as one arg.
        let params_str = args
            .iter()
            .map(|a| {
                if a.contains(' ') {
                    format!("\"{a}\"")
                } else {
                    a.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(" ");
        let params = wide(&params_str);
        let h = unsafe {
            ShellExecuteW(
                0,
                op.as_ptr(),
                file.as_ptr(),
                params.as_ptr(),
                std::ptr::null(),
                SW_HIDE,
            )
        };
        // ShellExecuteW returns the instance handle (>32) on success, or an
        // error code <= 32 on failure (e.g. user declined UAC = 1223).
        if h as isize <= 32 {
            anyhow::bail!(
                "ShellExecuteW('runas') declined or failed (code {h}; 1223 = user declined UAC)"
            );
        }
        Ok(())
    }

    /// `GetTokenInformation(TokenElevation)` 直查（替代旧 `net session` 探测：
    /// 不依赖 net.exe 在 PATH、不受 LanmanServer 服务停转影响、无子进程开销）。
    /// 查询失败按非管理员处理（保守——宁可多弹一次 UAC，不误跳过提权）。
    pub fn is_elevated() -> bool {
        #[link(name = "advapi32")]
        unsafe extern "system" {
            fn GetCurrentProcess() -> isize;
            fn OpenProcessToken(process: isize, desired_access: u32, token: *mut isize) -> i32;
            fn GetTokenInformation(
                token: isize,
                info_class: i32,
                info: *mut u8,
                info_len: u32,
                return_len: *mut u32,
            ) -> i32;
            fn CloseHandle(handle: isize) -> i32;
        }
        const TOKEN_QUERY: u32 = 0x0008;
        const TOKEN_ELEVATION: i32 = 20; // TOKEN_INFORMATION_CLASS::TokenElevation
        unsafe {
            let mut token: isize = 0;
            if OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token) == 0 {
                return false;
            }
            // TOKEN_ELEVATION 结构体就是单个 DWORD（TokenIsElevated）。
            let mut elevated: u32 = 0;
            let mut return_len: u32 = 0;
            let ok = GetTokenInformation(
                token,
                TOKEN_ELEVATION,
                std::ptr::addr_of_mut!(elevated).cast(),
                std::mem::size_of::<u32>() as u32,
                &mut return_len,
            );
            CloseHandle(token);
            ok != 0 && elevated != 0
        }
    }
}

#[cfg(not(windows))]
mod win {
    use std::path::Path;
    pub fn relaunch_elevated(_exe: &Path, _args: &[String]) -> anyhow::Result<()> {
        anyhow::bail!("elevation only supported on Windows")
    }
    pub fn is_elevated() -> bool {
        false
    }
}

pub use win::{is_elevated, relaunch_elevated};

#[cfg(test)]
mod tests;

#[cfg(test)]
mod cov_tests;
