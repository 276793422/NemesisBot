//! fake QMP 桩应答 CI 测试（G1 闸「fake QMP server 能应答 savevm/migrate
//! 桩」+ 第 3 步要用到的三分支形态预演）。

use std::io::{BufRead, BufReader, Read, Write};
use std::time::Duration;

use fake_qmp::Behavior;

/// 连接并完成 handshake（读 greeting → 发 qmp_capabilities → 读 return）。
fn connect_and_handshake(
    addr: std::net::SocketAddr,
) -> (std::net::TcpStream, BufReader<std::net::TcpStream>) {
    let mut stream = std::net::TcpStream::connect(addr).expect("connect fake-qmp");
    stream
        .set_read_timeout(Some(Duration::from_secs(10)))
        .expect("read timeout");
    let mut reader = BufReader::new(stream.try_clone().expect("clone"));
    let mut greeting = String::new();
    reader.read_line(&mut greeting).expect("read greeting");
    assert!(greeting.contains("\"QMP\""), "greeting 形态: {greeting}");
    stream
        .write_all(b"{\"execute\":\"qmp_capabilities\"}\n")
        .expect("handshake write");
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("handshake response");
    assert!(resp.contains("\"return\""), "handshake 应答: {resp}");
    (stream, reader)
}

fn exec_cmd(
    stream: &mut std::net::TcpStream,
    reader: &mut BufReader<std::net::TcpStream>,
    cmd: &str,
) -> serde_json::Value {
    let line = format!("{{\"execute\":\"{cmd}\"}}\n");
    stream.write_all(line.as_bytes()).expect("write cmd");
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("read resp");
    serde_json::from_str(resp.trim()).expect("parse resp")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn savevm_migrate_query_status_stubs() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");

    let (mut stream, mut reader) = connect_and_handshake(handle.addr());

    // savevm → return {}（G1 闸命令 1）。
    let resp = exec_cmd(&mut stream, &mut reader, "savevm");
    assert!(resp.get("return").is_some(), "{resp}");

    // migrate → return {}（G1 闸命令 2）。
    let resp = exec_cmd(&mut stream, &mut reader, "migrate");
    assert!(resp.get("return").is_some(), "{resp}");

    // query-status → running。
    let resp = exec_cmd(&mut stream, &mut reader, "query-status");
    assert_eq!(resp["return"]["status"], "running", "{resp}");

    // 已收命令记账（宿主侧代码断言「真的发过 savevm」的钩子）。
    let cmds = handle.commands();
    assert!(cmds.contains(&"savevm".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"migrate".to_string()), "{cmds:?}");
    assert!(cmds.contains(&"qmp_capabilities".to_string()), "{cmds:?}");

    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unknown_command_gets_command_not_found() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let (mut stream, mut reader) = connect_and_handshake(handle.addr());
    let resp = exec_cmd(&mut stream, &mut reader, "totally-bogus-cmd");
    assert_eq!(resp["error"]["class"], "CommandNotFound", "{resp}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn bad_json_gets_generic_error_and_connection_survives() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    let (mut stream, mut reader) = connect_and_handshake(handle.addr());

    stream.write_all(b"not-json\n").expect("write bad line");
    let mut resp = String::new();
    reader.read_line(&mut resp).expect("read err resp");
    assert!(resp.contains("GenericError"), "{resp}");

    // 之后照常服务（坏行不炸连接）。
    let parsed = exec_cmd(&mut stream, &mut reader, "query-status");
    assert_eq!(parsed["return"]["status"], "running", "{parsed}");
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn error_injection_via_set_behavior() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    // 运行时注入：savevm 报错（第 3 步 savevm 失败分支的驱动器）。
    handle.set_behavior(
        "savevm",
        Behavior::err("GenericError", "snapshot disk failure (injected)"),
    );
    let (mut stream, mut reader) = connect_and_handshake(handle.addr());
    let resp = exec_cmd(&mut stream, &mut reader, "savevm");
    assert_eq!(resp["error"]["class"], "GenericError", "{resp}");
    assert!(
        resp["error"]["desc"].as_str().unwrap().contains("injected"),
        "{resp}"
    );
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn hang_behavior_keeps_connection_open_without_response() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior("savevm", Behavior::Hang);
    let (mut stream, mut reader) = connect_and_handshake(handle.addr());

    stream
        .write_all(b"{\"execute\":\"savevm\"}\n")
        .expect("write savevm");
    // 短读超时：应确认真没回话（超时错误 = hang 生效）。
    stream
        .set_read_timeout(Some(Duration::from_millis(500)))
        .ok();
    let mut buf = [0u8; 64];
    let read_result = reader.read(&mut buf);
    match read_result {
        Err(e) => assert!(
            e.kind() == std::io::ErrorKind::WouldBlock || e.kind() == std::io::ErrorKind::TimedOut,
            "hang 分支读错误应为超时而非其他: {e}"
        ),
        Ok(0) => panic!("hang 分支连接不应被服务端关闭"),
        Ok(n) => panic!(
            "hang 分支不应有响应，收到 {n} 字节: {}",
            String::from_utf8_lossy(&buf[..n])
        ),
    }
    handle.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delay_behavior_defers_response() {
    let handle = fake_qmp::spawn().await.expect("spawn fake-qmp");
    handle.set_behavior(
        "migrate",
        Behavior::Delay(Duration::from_millis(300), Box::new(Behavior::ok())),
    );
    let (mut stream, mut reader) = connect_and_handshake(handle.addr());
    let started = std::time::Instant::now();
    let resp = exec_cmd(&mut stream, &mut reader, "migrate");
    assert!(resp.get("return").is_some(), "{resp}");
    assert!(
        started.elapsed() >= Duration::from_millis(250),
        "delay 未生效（{}ms）",
        started.elapsed().as_millis()
    );
    handle.shutdown().await;
}
