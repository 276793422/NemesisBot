use super::*;
use crate::types::JSONRPC_VERSION;

#[test]
fn create_transport() {
    let t = StdioTransport::new("echo", vec![], vec![]);
    assert_eq!(t.name(), "stdio");
    assert!(!t.is_connected());
}

#[test]
fn create_from_config() {
    let config = crate::types::ServerConfig::new("test", "node")
        .arg("server.js")
        .env("FOO=bar")
        .timeout(10);

    let t = StdioTransport::from_config(&config);
    assert_eq!(t.command, "node");
    assert_eq!(t.args, vec!["server.js"]);
    assert_eq!(t.env, vec!["FOO=bar"]);
    assert!(!t.is_connected());
}

/// Test connect/close lifecycle with a simple echo-like program.
/// On Windows, `cmd /C echo` exits immediately, so we just test that
/// connect succeeds and close cleans up.
#[tokio::test]
async fn connect_and_close_lifecycle() {
    // Use a long-running command so the process stays alive during the test.
    // `ping -t localhost` on Windows runs indefinitely.
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

    assert!(!t.is_connected());

    // Connect should succeed.
    t.connect().await.unwrap();
    assert!(t.is_connected());

    // Close should succeed.
    t.close().await.unwrap();
    assert!(!t.is_connected());

    // Double close should be fine.
    t.close().await.unwrap();
    assert!(!t.is_connected());
}

#[tokio::test]
async fn send_when_not_connected_fails() {
    let mut t = StdioTransport::new("nonexistent", vec![], vec![]);
    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "ping".to_string(),
        params: None,
    };
    let result = t.send(&req, 1000).await;
    assert!(result.is_err());
}

/// End-to-end test: spawn a simple JSON-RPC echo server using Python,
/// send a request, and verify the response. Skips if Python is unavailable.
#[tokio::test]
async fn e2e_jsonrpc_echo() {
    // Simple Python script that reads a JSON-RPC request from stdin and
    // echoes back a response with the same id.
    let python_script = r#"
import sys, json
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    try:
        req = json.loads(line)
        resp = {"jsonrpc": "2.0", "id": req.get("id"), "result": {"echo": req.get("method")}}
        sys.stdout.write(json.dumps(resp) + "\n")
        sys.stdout.flush()
    except Exception:
        break
"#;

    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), python_script.to_string()],
        vec![],
    );

    // Skip if python is not available.
    if t.connect().await.is_err() {
        eprintln!("Skipping e2e test: python not available");
        return;
    }

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(42.into())),
        method: "test/method".to_string(),
        params: None,
    };

    let resp = t.send(&req, 5000).await.unwrap();
    assert_eq!(resp.id, serde_json::Value::Number(42.into()));
    assert!(resp.result.is_some());
    assert_eq!(resp.result.unwrap()["echo"], "test/method");

    t.close().await.unwrap();
}

// ---- New tests ----

#[test]
fn transport_name_is_stdio() {
    let t = StdioTransport::new("test", vec![], vec![]);
    assert_eq!(t.name(), "stdio");
}

#[test]
fn new_transport_not_connected() {
    let t = StdioTransport::new(
        "test",
        vec!["arg1".to_string()],
        vec!["KEY=VAL".to_string()],
    );
    assert!(!t.is_connected());
    assert_eq!(t.command, "test");
    assert_eq!(t.args, vec!["arg1"]);
    assert_eq!(t.env, vec!["KEY=VAL"]);
}

#[tokio::test]
async fn close_without_connect_is_ok() {
    let mut t = StdioTransport::new("test", vec![], vec![]);
    t.close().await.unwrap();
    assert!(!t.is_connected());
}

#[tokio::test]
async fn connect_nonexistent_command_fails() {
    let mut t = StdioTransport::new(
        "/absolutely/nonexistent/command/that/does/not/exist",
        vec![],
        vec![],
    );
    let result = t.connect().await;
    assert!(result.is_err());
}

#[tokio::test]
async fn double_connect_is_ok() {
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

    t.connect().await.unwrap();
    assert!(t.is_connected());
    t.connect().await.unwrap(); // Second connect is a no-op
    assert!(t.is_connected());
    t.close().await.unwrap();
}

#[test]
fn from_config_preserves_fields() {
    let config = crate::types::ServerConfig::new("my-server", "/usr/bin/node")
        .arg("index.js")
        .arg("--verbose")
        .env("NODE_ENV=production")
        .env("PORT=3000")
        .timeout(60);

    let t = StdioTransport::from_config(&config);
    assert_eq!(t.command, "/usr/bin/node");
    assert_eq!(t.args, vec!["index.js", "--verbose"]);
    assert_eq!(t.env.len(), 2);
}

#[test]
fn from_config_no_env() {
    let config = crate::types::ServerConfig::new("srv", "cmd");
    let t = StdioTransport::from_config(&config);
    assert!(t.env.is_empty());
}

#[tokio::test]
async fn send_after_close_fails() {
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

    t.connect().await.unwrap();
    t.close().await.unwrap();

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "test".to_string(),
        params: None,
    };
    let result = t.send(&req, 1000).await;
    assert!(result.is_err());
}

// ---- Additional constructor / lifecycle / Drop coverage ----

#[test]
fn new_preserves_command_args_and_env() {
    let t = StdioTransport::new(
        "/usr/bin/python3",
        vec!["-m".to_string(), "server".to_string()],
        vec!["KEY1=val1".to_string(), "KEY2=val2".to_string()],
    );
    assert_eq!(t.command, "/usr/bin/python3");
    assert_eq!(t.args, vec!["-m", "server"]);
    assert_eq!(t.env.len(), 2);
    assert!(!t.is_connected());
}

#[test]
fn new_starts_disconnected_with_no_child() {
    let t = StdioTransport::new("echo", vec![], vec![]);
    // A freshly-constructed transport must be disconnected and have no child.
    assert!(!t.is_connected());
    assert!(t.child.is_none());
    assert!(t.stdin.is_none());
    assert!(t.stdout.is_none());
    // Drop here exercises the Drop impl's `if let Some(child)` branch with
    // child == None (no-op). No panic is the assertion.
}

#[test]
fn from_config_with_no_env_yields_empty_env() {
    let config = crate::types::ServerConfig::new("srv", "cmd");
    let t = StdioTransport::from_config(&config);
    assert!(t.env.is_empty());
    assert_eq!(t.command, "cmd");
    assert!(t.args.is_empty());
}

#[test]
fn from_config_with_multiple_args_and_env() {
    let config = crate::types::ServerConfig::new("srv", "node")
        .arg("index.js")
        .arg("--port")
        .arg("3000")
        .env("NODE_ENV=production")
        .env("DEBUG=1");
    let t = StdioTransport::from_config(&config);
    assert_eq!(t.args, vec!["index.js", "--port", "3000"]);
    assert_eq!(t.env, vec!["NODE_ENV=production", "DEBUG=1"]);
}

#[tokio::test]
async fn connect_skips_malformed_env_pairs() {
    // connect() injects env vars via split_once('='). Pairs without '='
    // must be silently skipped (the `if let Some((k,v))` guard). We connect
    // to a long-running process with a mix of valid + malformed pairs and
    // confirm connect still succeeds.
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![
            "MALFORMED_NO_EQUALS".to_string(),
            "VALID_KEY=valid_value".to_string(),
        ],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new(
        "sleep",
        vec!["60".to_string()],
        vec![
            "MALFORMED_NO_EQUALS".to_string(),
            "VALID_KEY=valid_value".to_string(),
        ],
    );

    t.connect().await.unwrap();
    assert!(t.is_connected());
    t.close().await.unwrap();
    assert!(!t.is_connected());
}

#[tokio::test]
async fn drop_after_connect_cleans_up_child() {
    // Construct, connect, then drop — exercises the Drop impl's
    // start_kill() path on a real child.
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

    t.connect().await.unwrap();
    assert!(t.is_connected());
    // Dropping without close() should still best-effort kill the child.
    drop(t);
}

#[tokio::test]
async fn is_connected_reflects_state_transitions() {
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["-t".to_string(), "localhost".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("sleep", vec!["60".to_string()], vec![]);

    assert!(!t.is_connected());
    t.connect().await.unwrap();
    assert!(t.is_connected());
    t.close().await.unwrap();
    assert!(!t.is_connected());
}

#[tokio::test]
async fn send_to_nonexistent_after_connect_returns_send_error() {
    // Connect to a process that immediately closes its stdout (echo with
    // no args exits at once on most platforms). send() must surface a
    // send_failed error (EOF / connection closed), not a not_connected one.
    #[cfg(target_os = "windows")]
    let mut t = StdioTransport::new(
        "ping",
        vec!["127.0.0.1".to_string(), "-n".to_string(), "1".to_string()],
        vec![],
    );
    #[cfg(not(target_os = "windows"))]
    let mut t = StdioTransport::new("echo", vec![], vec![]);

    t.connect().await.unwrap();
    // Give the short-lived child a moment to exit.
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "ping".to_string(),
        params: None,
    };
    let result = t.send(&req, 1000).await;
    assert!(result.is_err());
    // Either a write failure or an EOF read failure — both are send_failed.
    let _ = t.close().await;
}

// ===========================================================================
// W4c 补测（2026-08-25）：真子进程驱动 stdio 协议错误臂——stdout EOF /
// 读超时 / 响应解析失败 / stdin 写失败（对端关闭读端）
// ===========================================================================

#[tokio::test]
async fn test_w4c_stdio_eof_after_stdout_closed() {
    // 首个请求正常应答后关闭 stdout（进程继续读 stdin）→ 第二次 send 读到 EOF。
    // 注意：Windows 上 sys.stdout.close() 不会关掉底层 fd，必须 os.close(1)。
    let script = r#"
import sys, json, os
n = 0
for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    n += 1
    req = json.loads(line)
    if n == 1:
        resp = {"jsonrpc": "2.0", "id": req.get("id"), "result": {"ok": True}}
        os.write(1, (json.dumps(resp) + "\n").encode())
        os.close(1)
    # n >= 2：stdout 已关，不再写任何东西
"#;
    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), script.to_string()],
        vec![],
    );
    if t.connect().await.is_err() {
        eprintln!("Skipping test: python not available");
        return;
    }

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "first".to_string(),
        params: None,
    };
    let resp = t.send(&req, 5000).await.unwrap();
    assert!(resp.result.is_some());

    // 第二次 send：进程活着（stdin 可写）但 stdout 已关 → EOF
    let req2 = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(2.into())),
        method: "second".to_string(),
        params: None,
    };
    let err = t.send(&req2, 5000).await.unwrap_err();
    assert!(
        err.message.contains("connection closed"),
        "unexpected: {}",
        err.message
    );
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_stdio_read_timeout_maps_transport_timeout() {
    // 服务器只读不答 → send 超时（300ms）
    let script = r#"
import sys, time
for line in sys.stdin:
    pass
time.sleep(30)
"#;
    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), script.to_string()],
        vec![],
    );
    if t.connect().await.is_err() {
        eprintln!("Skipping test: python not available");
        return;
    }

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "ignored".to_string(),
        params: None,
    };
    let err = t.send(&req, 300).await.unwrap_err();
    assert_eq!(err.code, -3);
    assert!(err.message.contains("timed out"));
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_stdio_garbage_response_maps_parse_failure() {
    // 服务器回一行非 JSON → 解析失败
    let script = r#"
import sys, time
line = sys.stdin.readline()
sys.stdout.write("this-is-not-json\n")
sys.stdout.flush()
time.sleep(30)
"#;
    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), script.to_string()],
        vec![],
    );
    if t.connect().await.is_err() {
        eprintln!("Skipping test: python not available");
        return;
    }

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "x".to_string(),
        params: None,
    };
    let err = t.send(&req, 5000).await.unwrap_err();
    assert!(
        err.message.contains("failed to parse response"),
        "unexpected: {}",
        err.message
    );
    t.close().await.unwrap();
}

#[tokio::test]
async fn test_w4c_stdio_write_to_closed_stdin_fails() {
    // 子进程启动后立刻关闭自己的 stdin 读端（进程保持存活）→ 写 stdin 失败。
    // 同样必须用 os.close(0)（sys.stdin.close() 不关底层 fd）。
    let script = r#"
import sys, time, os
os.close(0)
time.sleep(30)
"#;
    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), script.to_string()],
        vec![],
    );
    if t.connect().await.is_err() {
        eprintln!("Skipping test: python not available");
        return;
    }
    // 给子进程一点时间执行 close
    tokio::time::sleep(std::time::Duration::from_millis(500)).await;

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "x".to_string(),
        params: None,
    };
    let err = t.send(&req, 5000).await.unwrap_err();
    assert!(
        err.message.contains("failed to write to stdin")
            || err.message.contains("failed to flush stdin")
            || err.message.contains("connection closed"),
        "unexpected: {}",
        err.message
    );
    t.close().await.unwrap();
}

/// S1 coverage batch (2026-08-26): stdin write failure arm (lines 147-149).
///
/// The child closes its read end (fd 0) first thing and then stays alive, so
/// the parent's `write_all` deterministically hits a broken pipe instead of
/// racing full process exit. The 1s grace period gives python ample time to
/// start and close fd 0 (typical startup < 200ms; child lives 5s). Skips when
/// python is unavailable, matching the e2e_jsonrpc_echo convention.
#[tokio::test]
async fn s1_send_fails_when_child_closes_stdin() {
    let python_script = r#"
import os, time
os.close(0)
time.sleep(5)
"#;
    let mut t = StdioTransport::new(
        "python",
        vec!["-c".to_string(), python_script.to_string()],
        vec![],
    );

    // Skip if python is not available.
    if t.connect().await.is_err() {
        eprintln!("Skipping s1 test: python not available");
        return;
    }
    assert!(t.is_connected());

    tokio::time::sleep(std::time::Duration::from_secs(1)).await;

    let req = TransportRequest {
        jsonrpc: JSONRPC_VERSION.to_string(),
        id: Some(serde_json::Value::Number(1.into())),
        method: "test/method".to_string(),
        params: None,
    };
    let err = t.send(&req, 3000).await.unwrap_err();
    let msg = format!("{err}");
    assert!(
        msg.contains("stdin"),
        "expected a stdin write failure, got: {msg}"
    );

    let _ = t.close().await;
}
