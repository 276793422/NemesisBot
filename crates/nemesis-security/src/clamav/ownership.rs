//! Ownership check for a clamd bound to a TCP port: **is it OUR clamd?**
//!
//! Used before reusing/killing a clamd we didn't spawn (ping-hit reuse, or the
//! spawn-failure fallback): verify the process listening on our configured port
//! is our `clamd.exe` (path match), so we never USE or KILL a foreign clamd
//! (system ClamAV, or another bot instance on the same port).
//!
//! Windows: `netstat -ano` for port→PID, then FFI
//! `QueryFullProcessImageNameW` (kernel32) for PID→exe path. (A `GetExtendedTcpTable`
//! FFI was attempted to replace the netstat parse but its row-struct layout did
//! not match the returned buffer — reverted to netstat, which is verified by the
//! unit test below. The FFI swap is deferred until the layout is resolved.)
//! Non-Windows: stubbed (the bot is Windows-primary; on other platforms we
//! can't run this check and assume ours — isolated, no special handling).

use std::path::Path;

/// True if the clamd listening on `addr` (e.g. "127.0.0.1:3310") is OUR clamd
/// (its exe path matches `our_clamd_exe`). **Fail-closed** on Windows: if we
/// can't determine it (port free / netstat or FFI failure / PID gone) → false,
/// so callers degrade rather than touch an unverified clamd.
#[cfg(windows)]
pub fn clamd_is_ours(addr: &str, our_clamd_exe: &Path) -> bool {
    let Some(pid) = pid_listening_on(addr) else {
        return false;
    };
    let Some(exe) = process_exe_path(pid) else {
        return false;
    };
    paths_match(&exe, our_clamd_exe)
}

#[cfg(not(windows))]
pub fn clamd_is_ours(_addr: &str, _our_clamd_exe: &Path) -> bool {
    // Non-Windows: isolated stub — no port→PID→path here. Assume ours so
    // clamav isn't disabled on platforms where the check can't run.
    true
}

#[cfg(windows)]
fn paths_match(a: &Path, b: &Path) -> bool {
    // Case-insensitive (Windows FS), ignore the \\?\ verbatim prefix that
    // canonicalize/QueryFullProcessImageName may prepend.
    let norm = |p: &Path| {
        p.to_string_lossy()
            .to_lowercase()
            .trim_start_matches(r"\\?\")
            .to_string()
    };
    norm(a) == norm(b)
}

/// Find the PID of the TCP LISTENING socket on `addr` (host:port), via
/// `netstat -ano -p tcp`. None if not found or netstat fails. Verified by the
/// unit test below (binds a socket, expects its own PID).
#[cfg(windows)]
fn pid_listening_on(addr: &str) -> Option<u32> {
    let out = std::process::Command::new("netstat")
        .args(["-ano", "-p", "tcp"])
        .output()
        .ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    // Line shape: "  TCP    127.0.0.1:3310    0.0.0.0:0    LISTENING    1234"
    for line in text.lines() {
        let p: Vec<&str> = line.split_whitespace().collect();
        if p.len() >= 5 && p[0] == "TCP" && p[3] == "LISTENING" && p[1] == addr {
            return p[4].parse().ok();
        }
    }
    None
}

/// Get a process's exe path by PID via `QueryFullProcessImageNameW` (kernel32).
#[cfg(windows)]
fn process_exe_path(pid: u32) -> Option<std::path::PathBuf> {
    use std::os::windows::ffi::OsStringExt;

    #[link(name = "kernel32")]
    unsafe extern "system" {
        fn OpenProcess(access: u32, inherit: i32, pid: u32) -> isize;
        fn QueryFullProcessImageNameW(
            h: isize,
            flags: u32,
            buf: *mut u16,
            size: *mut u32,
        ) -> i32;
        fn CloseHandle(h: isize) -> i32;
    }
    const PROCESS_QUERY_LIMITED_INFORMATION: u32 = 0x1000;

    unsafe {
        let h = OpenProcess(PROCESS_QUERY_LIMITED_INFORMATION, 0, pid);
        if h == 0 {
            return None;
        }
        let mut buf = [0u16; 1024];
        let mut size = buf.len() as u32;
        let ok = QueryFullProcessImageNameW(h, 0, buf.as_mut_ptr(), &mut size);
        CloseHandle(h);
        if ok == 0 {
            return None;
        }
        std::ffi::OsString::from_wide(&buf[..size as usize])
            .into_string()
            .ok()
            .map(std::path::PathBuf::from)
    }
}

#[cfg(all(test, windows))]
mod tests {
    use super::*;

    /// Bind a TCP listener on an ephemeral port, then verify pid_listening_on
    /// finds OUR pid on it. Proves the port→PID step end-to-end (no guessing).
    #[test]
    fn pid_listening_on_finds_own_bound_socket() {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let port = listener.local_addr().unwrap().port();
        let addr = format!("127.0.0.1:{port}");
        let me = std::process::id();
        let pid = pid_listening_on(&addr);
        assert!(pid.is_some(), "pid_listening_on({addr}) returned None");
        assert_eq!(pid.unwrap(), me, "pid_listening_on({addr}) found wrong PID");
    }

    #[test]
    fn pid_listening_on_returns_none_for_free_port() {
        assert!(pid_listening_on("127.0.0.1:1").is_none());
    }
}
