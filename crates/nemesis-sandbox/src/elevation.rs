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
            .map(|a| if a.contains(' ') { format!("\"{a}\"") } else { a.clone() })
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

    /// `net session` exits 0 only when the process has admin rights.
    pub fn is_elevated() -> bool {
        std::process::Command::new("net")
            .arg("session")
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .status()
            .map(|s| s.success())
            .unwrap_or(false)
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
