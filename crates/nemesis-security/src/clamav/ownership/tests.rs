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
