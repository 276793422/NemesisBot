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

// B8d：超时树杀——此前超时臂直接返回 Err 不 kill，被超时命令继续在后台
// 跑。钉死「超时后命令链不再推进」：marker 由命令链末段落盘，孤儿若存活
// 必在自然结束点后写入 → 断言红。
#[test]
fn stdio_exec_timeout_kills_child_tree() {
    let dir = tempfile::tempdir().expect("tempdir");
    let (_g, mut reader, mut stdin) = spawn_stdio(dir.path());
    let marker = dir.path().join("orphan_marker");
    let command = if cfg!(windows) {
        format!("ping -n 4 127.0.0.1 >nul & type nul > {}", marker.display())
    } else {
        format!("sleep 3 && touch {}", marker.display())
    };
    // json! 构造：tempdir 路径含反斜杠，format! 手拼 JSON 会产出非法转义。
    let args = serde_json::json!({ "command": command, "timeout": 1 }).to_string();
    let line =
        build_v2_request_line("exec", &args, &serde_json::json!({}), "c-fc-treekill").unwrap();
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
    // 等 sleep/ping 自然结束点过去（3s+ 余量），孤儿若活着必已落 marker。
    while started.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(100));
    }
    assert!(
        !marker.exists(),
        "超时命令的子进程未被终止（孤儿存活，marker 已落盘）"
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

// ---------------------------------------------------------------------------
// B9-1 内部方法全链（TCP 双栖）：分块 bulk_write（clean 首装清场）→
// bulk_read/bulk_read_chunk 打包回传 → hash_tree 指纹。gateway 同步引擎
// 的传输面协议即按此消费。
// ---------------------------------------------------------------------------

#[test]
fn tcp_bulk_roundtrip_clean_write_read_back_and_hash_tree() {
    use base64::Engine as _;
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path();
    // 先放一个杂物文件：clean=true 的 bulk_write 必须清掉它（首装清场语义）。
    std::fs::write(ws.join("stray.txt"), b"stale").expect("stray file");
    let (_g, addr) = spawn_listen(ws);

    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let call = |stream: &mut std::net::TcpStream,
                reader: &mut BufReader<std::net::TcpStream>,
                id: &str,
                tool: &str,
                args: serde_json::Value|
     -> serde_json::Value {
        let line =
            build_v2_request_line(tool, &args.to_string(), &serde_json::json!({}), id).unwrap();
        stream.write_all(line.as_bytes()).expect("write request");
        let mut resp = String::new();
        reader.read_line(&mut resp).expect("read response");
        serde_json::from_str(resp.trim()).expect("parse response")
    };
    let result_json = |resp: &serde_json::Value| -> serde_json::Value {
        serde_json::from_str(resp["result"].as_str().expect("result is string"))
            .expect("parse result payload")
    };

    // 打包两个文件（一个嵌套目录）为内存 tar。
    let mut tar_buf = Vec::new();
    {
        let mut b = tar::Builder::new(&mut tar_buf);
        let mut h = tar::Header::new_gnu();
        h.set_size(6);
        h.set_mode(0o755);
        h.set_cksum();
        b.append_data(&mut h, "hello.txt", &b"guest!"[..]).unwrap();
        let mut h2 = tar::Header::new_gnu();
        h2.set_size(3);
        h2.set_mode(0o644);
        h2.set_cksum();
        b.append_data(&mut h2, "sub/deep.bin", &[7u8, 8, 9][..])
            .unwrap();
        b.finish().unwrap();
    }

    // 分两块 bulk_write（seq==0 带 clean=true）。
    let mid = tar_buf.len() / 2;
    let mut final_resp = serde_json::Value::Null;
    for (seq, slice) in [(0u64, &tar_buf[..mid]), (1u64, &tar_buf[mid..])] {
        final_resp = call(
            &mut stream,
            &mut reader,
            "c-bulk-w",
            "bulk_write",
            serde_json::json!({
                "transfer": "tcp-e2e-in",
                "seq": seq,
                "total": 2,
                "data": base64::engine::general_purpose::STANDARD.encode(slice),
                "clean": seq == 0,
            }),
        );
        assert_eq!(final_resp["ok"], true, "bulk_write seq={seq}: {final_resp}");
    }
    let written = result_json(&final_resp);
    assert_eq!(written["written"], 2, "{written}");
    // clean 清场生效：杂物文件消失，工作树里只有 tar 带进来的两个文件。
    assert!(
        !ws.join("stray.txt").exists(),
        "clean=true 必须清掉首装前的杂物文件"
    );

    // 落盘内容可读。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-bulk-r",
        "read_file",
        serde_json::json!({"path": "hello.txt"}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    assert_eq!(resp["result"], "guest!", "{resp}");

    // bulk_read → 分块回传重组 = 原 tar 内容。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-bulk-read",
        "bulk_read",
        serde_json::json!({"paths": ["hello.txt", "sub/deep.bin", "missing.txt"]}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let rd = result_json(&resp);
    assert_eq!(rd["missing"], serde_json::json!(["missing.txt"]), "{rd}");
    let transfer = rd["transfer"].as_str().expect("transfer id").to_string();
    let chunks = rd["chunks"].as_u64().expect("chunks");
    let mut roundtrip = Vec::new();
    for seq in 0..chunks {
        let resp = call(
            &mut stream,
            &mut reader,
            "c-bulk-chunk",
            "bulk_read_chunk",
            serde_json::json!({"transfer": transfer, "seq": seq}),
        );
        assert_eq!(resp["ok"], true, "chunk {seq}: {resp}");
        let ck = result_json(&resp);
        assert_eq!(ck["seq"], seq);
        roundtrip.extend(
            base64::engine::general_purpose::STANDARD
                .decode(ck["data"].as_str().expect("b64 data"))
                .expect("decode chunk"),
        );
    }
    {
        let mut ar = tar::Archive::new(&roundtrip[..]);
        let mut seen = std::collections::HashMap::new();
        for ent in ar.entries().unwrap() {
            let mut ent = ent.unwrap();
            let name = ent.path().unwrap().to_string_lossy().replace('\\', "/");
            let mut body = Vec::new();
            std::io::Read::read_to_end(&mut ent, &mut body).unwrap();
            seen.insert(name, body);
        }
        assert_eq!(
            seen.get("hello.txt").map(Vec::as_slice),
            Some(&b"guest!"[..])
        );
        assert_eq!(
            seen.get("sub/deep.bin").map(Vec::as_slice),
            Some(&[7u8, 8, 9][..])
        );
    }

    // hash_tree：全树指纹与盘面一致（sha256 直算对照）。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-bulk-hash",
        "hash_tree",
        serde_json::json!({"mode": "files"}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let ht = result_json(&resp);
    assert_eq!(ht["truncated"], false, "{ht}");
    let files = ht["files"].as_array().expect("files array");
    assert_eq!(files.len(), 2, "{ht}");
    let expect = [
        ("hello.txt", &b"guest!"[..]),
        ("sub/deep.bin", &[7u8, 8, 9][..]),
    ];
    use sha2::{Digest, Sha256};
    for (path, body) in expect {
        let f = files
            .iter()
            .find(|f| f["path"] == path)
            .unwrap_or_else(|| panic!("hash_tree 缺 {path}: {ht}"));
        assert_eq!(f["sha256"], format!("{:x}", Sha256::digest(body)), "{ht}");
    }
}

// ---------------------------------------------------------------------------
// B9-2 定点臂 e2e：hash_tree paths 过滤（缺失静默）+ bulk_delete（幂等删
// + 越界拒绝）。gateway 开工闸/commit 闸的定点指纹与宿主删除传播即按此消费。
// ---------------------------------------------------------------------------

#[test]
fn tcp_hash_tree_paths_filter_and_bulk_delete() {
    let dir = tempfile::tempdir().expect("tempdir");
    let ws = dir.path();
    std::fs::create_dir_all(ws.join("sub")).expect("mkdir sub");
    std::fs::write(ws.join("hello.txt"), b"guest!").expect("hello.txt");
    std::fs::write(ws.join("sub/deep.bin"), [7u8, 8, 9]).expect("deep.bin");
    std::fs::write(ws.join("extra.txt"), b"xx").expect("extra.txt");
    let (_g, addr) = spawn_listen(ws);

    let mut stream = std::net::TcpStream::connect(addr).expect("connect");
    stream
        .set_read_timeout(Some(Duration::from_secs(30)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone stream"));

    let call = |stream: &mut std::net::TcpStream,
                reader: &mut BufReader<std::net::TcpStream>,
                id: &str,
                tool: &str,
                args: serde_json::Value|
     -> serde_json::Value {
        let line =
            build_v2_request_line(tool, &args.to_string(), &serde_json::json!({}), id).unwrap();
        stream.write_all(line.as_bytes()).expect("write request");
        let mut resp = String::new();
        reader.read_line(&mut resp).expect("read response");
        serde_json::from_str(resp.trim()).expect("parse response")
    };
    let result_json = |resp: &serde_json::Value| -> serde_json::Value {
        serde_json::from_str(resp["result"].as_str().expect("result is string"))
            .expect("parse result payload")
    };

    // hash_tree 定点臂：paths 过滤 + 缺失静默不报 + 排序稳定。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-ht-paths",
        "hash_tree",
        serde_json::json!({"mode": "files", "paths": ["hello.txt", "missing.txt", "extra.txt"]}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let ht = result_json(&resp);
    assert_eq!(ht["truncated"], false, "{ht}");
    let files = ht["files"].as_array().expect("files array");
    assert_eq!(files.len(), 2, "缺失条目静默不报: {ht}");
    assert_eq!(files[0]["path"], "extra.txt");
    assert_eq!(files[1]["path"], "hello.txt");
    assert_eq!(files[0]["size"], 2, "{ht}");
    use sha2::Digest as _;
    assert_eq!(
        files[1]["sha256"],
        format!("{:x}", sha2::Sha256::digest(b"guest!")),
        "{ht}"
    );

    // bulk_delete：定点删 + 缺失计 missing；盘面真删，未点名的不动。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-del-1",
        "bulk_delete",
        serde_json::json!({"paths": ["hello.txt", "never-there.txt"]}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let del = result_json(&resp);
    assert_eq!(del["deleted"], 1, "{del}");
    assert_eq!(del["missing"], 1, "{del}");
    assert!(!ws.join("hello.txt").exists(), "hello.txt 应真删");
    assert!(ws.join("sub/deep.bin").exists(), "未点名的文件不动");

    // 重复删 = 幂等（0 deleted / 1 missing，不算错）。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-del-2",
        "bulk_delete",
        serde_json::json!({"paths": ["hello.txt"]}),
    );
    assert_eq!(resp["ok"], true, "{resp}");
    let del = result_json(&resp);
    assert_eq!(del["deleted"], 0, "{del}");
    assert_eq!(del["missing"], 1, "{del}");

    // 越界路径 = 诚实拒绝（safe_entry_rel 闸 guest 侧把关）。
    let resp = call(
        &mut stream,
        &mut reader,
        "c-del-3",
        "bulk_delete",
        serde_json::json!({"paths": ["../evil.txt"]}),
    );
    assert_eq!(resp["ok"], false, "越界路径应拒绝: {resp}");
    assert!(
        resp["error"]
            .as_str()
            .unwrap_or_default()
            .contains("../evil.txt"),
        "错误应带回路径原文: {resp}"
    );
}
