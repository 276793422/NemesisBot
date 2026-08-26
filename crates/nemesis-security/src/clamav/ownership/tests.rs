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

// ============================================================
// clamd_is_ours / paths_match / process_exe_path arms (2026-08-25)
// ============================================================

#[test]
fn clamd_is_ours_true_for_own_listener_and_exe() {
    // 自己绑端口 + our_clamd_exe=本进程 exe → PID→exe 全链路命中 → true。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let me = std::env::current_exe().unwrap();
    assert!(clamd_is_ours(&addr, &me), "own listener + own exe must be ours");
}

#[test]
fn clamd_is_ours_false_for_exe_mismatch() {
    // 端口有监听者（本进程），但 our_clamd_exe 是别的路径 → false。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    let addr = format!("127.0.0.1:{port}");
    let other = std::env::temp_dir().join("clamd.exe");
    assert!(!clamd_is_ours(&addr, &other));
}

#[test]
fn clamd_is_ours_false_for_free_port() {
    // 端口无监听 → pid_listening_on None → fail-closed false。
    assert!(!clamd_is_ours("127.0.0.1:1", std::path::Path::new(r"C:\clamav\clamd.exe")));
}

#[test]
fn paths_match_normalizes_verbatim_prefix_and_case() {
    // \\?\ 前缀剥离 + 大小写不敏感。
    assert!(paths_match(
        std::path::Path::new(r"\\?\C:\ClamAV\Clamd.EXE"),
        std::path::Path::new(r"c:\clamav\clamd.exe"),
    ));
    assert!(!paths_match(
        std::path::Path::new(r"C:\a\clamd.exe"),
        std::path::Path::new(r"C:\b\clamd.exe"),
    ));
}

#[test]
fn process_exe_path_own_pid_and_invalid_pid() {
    // 本进程 PID → Some(…exe)；超界 PID（Windows PID 空间 < 4194304）→ None。
    let own = process_exe_path(std::process::id());
    let own = own.expect("own pid exe path");
    assert!(own.to_string_lossy().to_lowercase().ends_with(".exe"));

    assert!(process_exe_path(4_000_000).is_none());
}

#[cfg(windows)]
#[test]
fn clamd_is_ours_free_port_returns_false() {
    // Bind an ephemeral port, then drop the listener → port free →
    // pid_listening_on returns None → fail-closed false.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);
    let addr = format!("127.0.0.1:{port}");
    assert!(!clamd_is_ours(
        &addr,
        std::path::Path::new(r"C:\nowhere\clamd.exe")
    ));
}
