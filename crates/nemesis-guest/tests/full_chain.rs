//! 协议全链集成测试（P5 第 1 步 G1 闸「协议全链 CI 测试」）。
//!
//! 测**真 agent 二进制**（env!("CARGO_BIN_EXE_nemesis-guest") 实 spawn），
//! 不是桩：
//! - stdio 双栖（宿主直跑形态）：spawn 无参数 → 写 v2 行 → 读响应；
//! - TCP 双栖（VM 内形态预演）：spawn `--listen 127.0.0.1:0` → 从 stderr
//!   读 `listening <addr>` → TcpStream 直连 → 多轮往返；
//! - 错误注入：未知工具 / 坏参数 / v1 拒绝 / 坏 JSON 行；
//! - 超时：exec sleep 超过 timeout 上限 → 诚实超时错误。
//!
//! 零 VM 依赖（GitHub runner 可跑）；QEMU 相关归第 2 步起。

use std::io::{BufRead, BufReader, Write};
use std::process::{Child, Command, Stdio};
use std::time::Duration;

use nemesis_exec_protocol::build_v2_request_line;

struct GuestChild {
    child: Child,
}

impl Drop for GuestChild {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

fn workspace_env(dir: &std::path::Path) -> std::collections::HashMap<String, String> {
    let mut env = std::collections::HashMap::new();
    env.insert(
        "NEMESISBOT_GUEST_WORKSPACE".to_string(),
        dir.display().to_string(),
    );
    env
}

fn spawn_stdio(
    dir: &std::path::Path,
) -> (
    GuestChild,
    BufReader<std::process::ChildStdout>,
    std::process::ChildStdin,
) {
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nemesis-guest"));
    cmd.envs(workspace_env(dir))
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null());
    let mut child = cmd.spawn().expect("spawn nemesis-guest (stdio)");
    let stdout = child.stdout.take().expect("piped stdout");
    let stdin = child.stdin.take().expect("piped stdin");
    (GuestChild { child }, BufReader::new(stdout), stdin)
}

fn send_and_read(
    stdin: &mut std::process::ChildStdin,
    reader: &mut BufReader<std::process::ChildStdout>,
    line: &str,
) -> serde_json::Value {
    stdin.write_all(line.as_bytes()).expect("write request");
    stdin.write_all(b"\n").expect("write newline");
    stdin.flush().expect("flush request");
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("read response line");
    serde_json::from_str(resp.trim()).expect("parse response JSON")
}

fn v2(tool: &str, args: &str) -> String {
    build_v2_request_line(
        tool,
        args,
        &serde_json::json!({"session_id": "fullchain"}),
        "c-fc-1",
    )
    .expect("build v2 line")
}

// ---------------------------------------------------------------------------
// stdio 双栖（宿主直跑）
// ---------------------------------------------------------------------------

#[test]
fn stdio_ping_pong_and_exec_roundtrip() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, mut reader, mut stdin) = spawn_stdio(dir.path());

    // ping/pong：最轻请求（list_dir 空目录）验证双栖直跑。
    let resp = send_and_read(&mut stdin, &mut reader, &v2("list_dir", "{}"));
    assert_eq!(resp["v"], 2, "{resp}");
    assert_eq!(resp["call_id"], "c-fc-1");
    assert_eq!(resp["ok"], true, "{resp}");

    // 单次 exec 往返（G1 闸的执行面）。
    let resp = send_and_read(
        &mut stdin,
        &mut reader,
        &v2("exec", r#"{"command":"echo guest-e2e"}"#),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let result = resp["result"].as_str().expect("result string");
    assert!(result.contains("guest-e2e"), "{resp}");
}

#[test]
fn stdio_error_injection_unknown_tool_and_bad_args() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, mut reader, mut stdin) = spawn_stdio(dir.path());

    // 未知工具 → 诚实错误 + call_id 回显。
    let resp = send_and_read(&mut stdin, &mut reader, &v2("git", "{}"));
    assert_eq!(resp["ok"], false, "{resp}");
    assert_eq!(resp["call_id"], "c-fc-1");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("unknown tool: git"),
        "{resp}"
    );

    // 坏参数 → 错误回灌。
    let resp = send_and_read(&mut stdin, &mut reader, &v2("read_file", "not-json"));
    assert_eq!(resp["ok"], false, "{resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("Invalid JSON arguments"),
        "{resp}"
    );

    // 坏 JSON 行 → v1 形态错误（call_id 提不出来）。
    let resp = send_and_read(&mut stdin, &mut reader, "not-a-json-line");
    assert_eq!(resp["ok"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("bad request line"),
        "{resp}"
    );

    // 之后进程仍活着、可继续服务（错误不炸循环）。
    let resp = send_and_read(&mut stdin, &mut reader, &v2("list_dir", "{}"));
    assert_eq!(resp["ok"], true, "{resp}");
}

#[test]
fn stdio_v1_request_rejected() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, mut reader, mut stdin) = spawn_stdio(dir.path());
    let resp = send_and_read(
        &mut stdin,
        &mut reader,
        r#"{"tool":"list_dir","args":"{}","context":{}}"#,
    );
    assert_eq!(resp["ok"], false, "{resp}");
    assert!(
        resp["error"].as_str().unwrap().contains("requires v2"),
        "{resp}"
    );
}

#[test]
fn stdio_exec_timeout_honest_error() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, mut reader, mut stdin) = spawn_stdio(dir.path());
    let sleep_cmd = if cfg!(windows) {
        "ping -n 6 127.0.0.1 >nul"
    } else {
        "sleep 5"
    };
    let line = build_v2_request_line(
        "exec",
        &format!(r#"{{"command":"{sleep_cmd}","timeout":1}}"#),
        &serde_json::json!({}),
        "c-fc-timeout",
    )
    .unwrap();
    let started = std::time::Instant::now();
    let resp = send_and_read(&mut stdin, &mut reader, &line);
    assert_eq!(resp["ok"], false, "{resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap()
            .contains("command timed out after 1s"),
        "{resp}"
    );
    assert!(
        started.elapsed() < Duration::from_secs(10),
        "超时没按 1s 生效"
    );
}

// ---------------------------------------------------------------------------
// TCP 双栖（VM 内形态预演）
// ---------------------------------------------------------------------------

fn spawn_listen(dir: &std::path::Path) -> (GuestChild, std::net::SocketAddr) {
    use std::io::Read;
    let mut cmd = Command::new(env!("CARGO_BIN_EXE_nemesis-guest"));
    cmd.arg("--listen")
        .arg("127.0.0.1:0")
        .envs(workspace_env(dir))
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    let mut child = cmd.spawn().expect("spawn nemesis-guest (--listen)");
    let mut stderr = child.stderr.take().expect("piped stderr");
    // 读 stderr 直到 "listening <addr>" 行（guest 就绪信号）。
    let mut buf = Vec::new();
    let mut byte = [0u8; 1];
    let deadline = std::time::Instant::now() + Duration::from_secs(15);
    let addr = loop {
        if std::time::Instant::now() > deadline {
            panic!(
                "guest --listen 未在 15s 内就绪（stderr: {})",
                String::from_utf8_lossy(&buf)
            );
        }
        match stderr.read(&mut byte) {
            Ok(1) => {
                buf.push(byte[0]);
                if byte[0] == b'\n'
                    && let Some(line) = String::from_utf8_lossy(&buf).lines().last()
                    && let Some(addr_str) = line.strip_prefix("listening ")
                {
                    break addr_str
                        .parse()
                        .unwrap_or_else(|e| panic!("parse listening addr from {line:?}: {e}"));
                }
            }
            Ok(0) => panic!("guest stderr EOF before listening line"),
            Ok(_) => {}
            Err(e) => panic!("read guest stderr: {e}"),
        }
    };
    (GuestChild { child }, addr)
}

#[test]
fn tcp_multi_round_trip_with_call_id_ordering() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, addr) = spawn_listen(dir.path());

    let mut stream = std::net::TcpStream::connect(addr).expect("connect guest TCP");
    stream
        .set_read_timeout(Some(Duration::from_secs(15)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    // 多轮往返：同连接 3 个请求，call_id 按序回显（串行应答语义）。
    let calls = [
        ("c-fc-a", "exec", r#"{"command":"echo one"}"#),
        (
            "c-fc-b",
            "write_file",
            r#"{"path":"t.txt","content":"two"}"#,
        ),
        ("c-fc-c", "read_file", r#"{"path":"t.txt"}"#),
    ];
    for (id, tool, args) in calls {
        let line = build_v2_request_line(tool, args, &serde_json::json!({}), id).unwrap();
        stream
            .write_all(line.as_bytes())
            .expect("write tcp request");
        let mut resp = String::new();
        reader.read_line(&mut resp).expect("read tcp response");
        let parsed: serde_json::Value = serde_json::from_str(resp.trim()).expect("parse resp");
        assert_eq!(parsed["call_id"], id, "{parsed}");
        assert_eq!(parsed["ok"], true, "{parsed}");
        if tool == "read_file" {
            assert_eq!(parsed["result"], "two", "{parsed}");
        }
    }
}

#[test]
fn tcp_connection_close_is_clean_and_new_connection_works() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, addr) = spawn_listen(dir.path());

    // 连接 1：一个请求后断开（EOF 收尾不炸 accept 循环）。
    {
        let mut s1 = std::net::TcpStream::connect(addr).expect("connect 1");
        s1.set_read_timeout(Some(Duration::from_secs(15))).ok();
        let line = v2("exec", r#"{"command":"echo first"}"#);
        s1.write_all(line.as_bytes()).expect("write");
        drop(s1);
    }
    // 连接 2：新连接照常服务。
    let mut s2 = std::net::TcpStream::connect(addr).expect("connect 2");
    s2.set_read_timeout(Some(Duration::from_secs(15))).ok();
    let line = build_v2_request_line(
        "exec",
        r#"{"command":"echo second"}"#,
        &serde_json::json!({}),
        "c-fc-2nd",
    )
    .unwrap();
    s2.write_all(line.as_bytes()).expect("write 2");
    let mut reader = BufReader::new(s2.try_clone().expect("clone"));
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("read 2");
    let parsed: serde_json::Value = serde_json::from_str(resp.trim()).unwrap();
    assert_eq!(parsed["call_id"], "c-fc-2nd", "{parsed}");
    assert_eq!(parsed["ok"], true, "{parsed}");
    assert!(
        parsed["result"].as_str().unwrap().contains("second"),
        "{parsed}"
    );
}
