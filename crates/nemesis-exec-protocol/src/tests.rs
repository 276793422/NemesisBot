//! `nemesis-exec-protocol` 单测：wire v1/v2 字节契约、call_id 生成、v2 请求
//! 行构造、duplex 交换原语（roundtrip / 超时 / EOF / call_id 失配 / 坏
//! JSON）。真子进程 stdio 往返与 stderr 环形尾由 nemesis-agent 侧测试覆盖。

use std::time::Duration;

use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

use super::*;

const SHORT: Duration = Duration::from_secs(2);

// ---------------------------------------------------------------------------
// wire 字节契约
// ---------------------------------------------------------------------------

/// per-call 路径的 v1 线形态必须逐字节稳定（`v`/`call_id` 空值不写键）——
/// 现有 per-call 测试与外部断言零迁移的锁。
#[test]
fn v1_request_bytes_unchanged() {
    let request = ExecutorRequestRef {
        v: 0,
        call_id: "",
        tool: "exec",
        args: r#"{"command":"echo hi"}"#,
        context: serde_json::json!({"chat_id": "c1"}),
    };
    let line = serde_json::to_string(&request).unwrap();
    assert!(!line.contains("call_id"), "v1 不得携带 call_id: {line}");
    assert!(!line.contains("\"v\""), "v1 不得携带 v 键: {line}");
    // 键序即结构体序（serde 定序）——防字段增删扰动线形态。
    assert_eq!(
        line,
        r#"{"tool":"exec","args":"{\"command\":\"echo hi\"}","context":{"chat_id":"c1"}}"#
    );
}

#[test]
fn v2_request_carries_call_id() {
    let request = ExecutorRequestRef {
        v: PROTOCOL_V2,
        call_id: "c-1-1",
        tool: "write_file",
        args: "{}",
        context: serde_json::json!({}),
    };
    let line = serde_json::to_string(&request).unwrap();
    assert!(line.contains(r#""v":2"#), "v2 必须带 v=2: {line}");
    assert!(line.contains(r#""call_id":"c-1-1""#), "{line}");
}

#[test]
fn response_wire_accepts_v1_and_v2() {
    let v1: ExecutorResponseWire = serde_json::from_str(r#"{"ok":true,"result":"r"}"#).unwrap();
    assert!(v1.ok);
    assert_eq!(v1.result, "r");
    assert_eq!(v1.call_id, "");
    assert!(!v1.is_v2());

    let v2: ExecutorResponseWire =
        serde_json::from_str(r#"{"v":2,"call_id":"c-9-9","ok":false,"error":"boom"}"#).unwrap();
    assert!(!v2.ok);
    assert_eq!(v2.call_id, "c-9-9");
    assert!(v2.is_v2());
}

/// v1 错误响应序列化不带 call_id/v 键（per-call 响应字节不变的另一半锁）。
#[test]
fn v1_error_response_bytes_unchanged() {
    let resp = ExecutorResponseWire::error("nope");
    let line = serde_json::to_string(&resp).unwrap();
    assert_eq!(line, r#"{"ok":false,"result":"","error":"nope"}"#);
}

// ---------------------------------------------------------------------------
// call_id
// ---------------------------------------------------------------------------

#[test]
fn new_call_id_shape_and_uniqueness() {
    let a = new_call_id();
    let b = new_call_id();
    assert!(a.starts_with("c-"), "{a}");
    assert_ne!(a, b, "进程内必须单调唯一");
    // 解析回三段（c-millis-seq）。
    let parts: Vec<&str> = a.split('-').collect();
    assert_eq!(parts.len(), 3);
    assert!(parts[1].parse::<u128>().is_ok());
    assert!(parts[2].parse::<u64>().is_ok());
}

/// v2 请求行构造：带尾部换行、字段齐全（guest 测试 / VM 通道同源消费）。
#[test]
fn build_v2_request_line_appends_newline() {
    let line = build_v2_request_line("exec", "{}", &serde_json::json!({"k": 1}), "c-7-7").unwrap();
    assert!(line.ends_with('\n'), "{line}");
    let req: ExecutorRequestOwned = serde_json::from_str(line.trim()).unwrap();
    assert_eq!(req.v, PROTOCOL_V2);
    assert_eq!(req.call_id, "c-7-7");
    assert_eq!(req.tool, "exec");
    assert_eq!(req.context["k"], 1);
}

// ---------------------------------------------------------------------------
// 交换原语（duplex 对端当假子进程）
// ---------------------------------------------------------------------------

/// 假子进程：读一行请求，回显同 call_id 的 v2 响应。
async fn echo_server(
    mut stream: tokio::io::DuplexStream,
    delay: Duration,
    respond: bool,
) -> std::io::Result<()> {
    let mut line = String::new();
    {
        let mut reader = BufReader::new(&mut stream);
        reader.read_line(&mut line).await?;
    }
    if delay != Duration::ZERO {
        tokio::time::sleep(delay).await;
    }
    if !respond {
        return Ok(()); // 静默退出 → 对端 EOF
    }
    let req: ExecutorRequestOwned = serde_json::from_str(line.trim()).unwrap();
    let resp = ExecutorResponseWire {
        v: PROTOCOL_V2,
        call_id: req.call_id.clone(),
        ok: true,
        result: format!("echo:{}", req.tool),
        error: String::new(),
    };
    let mut out = serde_json::to_string(&resp).unwrap();
    out.push('\n');
    stream.write_all(out.as_bytes()).await?;
    stream.flush().await?;
    Ok(())
}

fn boxed_halves() -> (BoxStreamRead, BoxStreamWrite, tokio::io::DuplexStream) {
    let (client, server) = tokio::io::duplex(64 * 1024);
    let (read, write) = split_duplex(client);
    (read, write, server)
}

#[tokio::test]
async fn call_roundtrip_over_duplex() {
    let (read, mut write, server) = boxed_halves();
    tokio::spawn(echo_server(server, Duration::ZERO, true));
    let mut reader = BufReader::new(read);
    let call_id = new_call_id();
    let line = build_v2_request_line("grep", "{}", &serde_json::json!({}), &call_id).unwrap();
    let resp = call_over_halves(write.as_mut(), &mut reader, &line, Some(&call_id), SHORT)
        .await
        .unwrap();
    assert_eq!(resp.call_id, call_id);
    assert_eq!(resp.result, "echo:grep");
}

#[tokio::test]
async fn call_timeout_when_server_silent() {
    let (read, mut write, server) = boxed_halves();
    // 读走请求但 10s 才应答——2s 超时先到。
    tokio::spawn(echo_server(server, Duration::from_secs(10), true));
    let mut reader = BufReader::new(read);
    let line = build_v2_request_line("exec", "{}", &serde_json::json!({}), &new_call_id()).unwrap();
    let err = call_over_halves(
        write.as_mut(),
        &mut reader,
        &line,
        None,
        Duration::from_millis(200),
    )
    .await
    .unwrap_err();
    assert!(matches!(err, TransportError::Timeout), "{err}");
}

#[tokio::test]
async fn call_eof_when_server_quits() {
    let (read, mut write, server) = boxed_halves();
    tokio::spawn(echo_server(server, Duration::ZERO, false));
    let mut reader = BufReader::new(read);
    let line = build_v2_request_line("exec", "{}", &serde_json::json!({}), &new_call_id()).unwrap();
    let err = call_over_halves(write.as_mut(), &mut reader, &line, None, SHORT)
        .await
        .unwrap_err();
    assert!(matches!(err, TransportError::Eof), "{err}");
}

#[tokio::test]
async fn call_id_mismatch_is_protocol_error() {
    let (read, mut write, server) = boxed_halves();
    // 服务端故意回错 call_id（串话形态）。
    tokio::spawn(async move {
        let mut server = server;
        server
            .write_all(br#"{"v":2,"call_id":"c-other","ok":true,"result":""}"#)
            .await
            .unwrap();
    });
    let mut reader = BufReader::new(read);
    let call_id = new_call_id();
    let line = build_v2_request_line("exec", "{}", &serde_json::json!({}), &call_id).unwrap();
    let err = call_over_halves(write.as_mut(), &mut reader, &line, Some(&call_id), SHORT)
        .await
        .unwrap_err();
    match err {
        TransportError::Protocol(m) => assert!(m.contains("mismatch"), "{m}"),
        other => panic!("期望 Protocol 错误，得到 {other}"),
    }
}

#[tokio::test]
async fn bad_response_json_is_protocol_error() {
    let (read, mut write, server) = boxed_halves();
    tokio::spawn(async move {
        let mut server = server;
        server.write_all(b"not json\n").await.unwrap();
    });
    let mut reader = BufReader::new(read);
    let line = build_v2_request_line("exec", "{}", &serde_json::json!({}), &new_call_id()).unwrap();
    let err = call_over_halves(write.as_mut(), &mut reader, &line, None, SHORT)
        .await
        .unwrap_err();
    assert!(matches!(err, TransportError::Protocol(_)), "{err}");
}
