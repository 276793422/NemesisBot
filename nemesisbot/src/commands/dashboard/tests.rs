//! dashboard 命令单测：gateway 状态文件解析（Option 链各失败分支）+
//! health check 对拒绝连接端口的确定性失败。
//!
//! run() / start_and_wait 会真 spawn gateway 进程（结构性）；send_internal_*
//! 需要活网关（结构性）。

use super::*;

fn state_file(dir: &tempfile::TempDir, body: &str) -> std::path::PathBuf {
    let p = dir.path().join("gateway_state.json");
    std::fs::write(&p, body).unwrap();
    p
}

#[test]
fn gateway_state_valid_json_parses_all_fields() {
    let dir = tempfile::tempdir().unwrap();
    let p = state_file(
        &dir,
        r#"{"pid": 4321, "web_host": "127.0.0.1", "web_port": 49000}"#,
    );
    let info = read_gateway_state(&p).expect("valid state must parse");
    assert_eq!(info.pid, 4321);
    assert_eq!(info.web_host, "127.0.0.1");
    assert_eq!(info.web_port, 49000);
}

#[test]
fn gateway_state_missing_file_is_none() {
    let dir = tempfile::tempdir().unwrap();
    assert!(read_gateway_state(&dir.path().join("nope.json")).is_none());
}

#[test]
fn gateway_state_bad_json_is_none() {
    let dir = tempfile::tempdir().unwrap();
    let p = state_file(&dir, "not json at all");
    assert!(read_gateway_state(&p).is_none());
}

#[test]
fn gateway_state_missing_any_required_field_is_none() {
    let dir = tempfile::tempdir().unwrap();
    // 缺 pid
    let p = state_file(&dir, r#"{"web_host": "127.0.0.1", "web_port": 1}"#);
    assert!(read_gateway_state(&p).is_none(), "缺 pid → None");
    // 缺 web_host
    let p = state_file(&dir, r#"{"pid": 1, "web_port": 1}"#);
    assert!(read_gateway_state(&p).is_none(), "缺 web_host → None");
    // 缺 web_port
    let p = state_file(&dir, r#"{"pid": 1, "web_host": "127.0.0.1"}"#);
    assert!(read_gateway_state(&p).is_none(), "缺 web_port → None");
}

#[test]
fn gateway_state_wrong_types_are_none() {
    let dir = tempfile::tempdir().unwrap();
    // pid 是字符串而不是数字 → as_u64() None。
    let p = state_file(&dir, r#"{"pid": "123", "web_host": "h", "web_port": 1}"#);
    assert!(read_gateway_state(&p).is_none());
    // web_port 是字符串。
    let p = state_file(&dir, r#"{"pid": 1, "web_host": "h", "web_port": "49000"}"#);
    assert!(read_gateway_state(&p).is_none());
    // pid 为负数（as_u64 失败）。
    let p = state_file(&dir, r#"{"pid": -1, "web_host": "h", "web_port": 1}"#);
    assert!(read_gateway_state(&p).is_none());
}

/// 回环地址 + 端口 1（无监听）→ 连接拒绝，确定性地失败（离线：不出本机）。
#[tokio::test]
async fn health_check_on_refused_port_fails() {
    let err = check_health("http://127.0.0.1:1").await.expect_err("refused");
    // reqwest 的连接错误信息（reqwest::Error Display）。
    assert!(!err.is_empty());
}

// ===========================================================================
// S11c（quality-hardening goal 冲刺 S11）：send_internal_command /
// send_internal_command_get_json（121-170）此前零覆盖（头注把它们当结构性
// 豁免——其实本地 mock 就能测）；check_health 成功分支（72-73）同理。
// run() 的 start_and_wait（spawn 当前 exe 起真网关）仍是结构性豁免；但
// "config 缺失"错误臂与"网关已在跑"完整成功路径（mock 扮网关）可测——
// 成功路径绝不进 start_and_wait，不会 spawn 网关/占生产端口。
// ===========================================================================

/// 可配置 mock 网关：GET /api/health → 200；POST /api/internal → 指定状态/体。
fn start_mock_gateway(internal_status: &'static str, internal_body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..8 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let req = String::from_utf8_lossy(&buf).to_string();
            let (status, payload) = if req.contains("/api/health") {
                ("200 OK", "{\"status\":\"ok\"}")
            } else {
                (internal_status, internal_body)
            };
            let resp = format!(
                "HTTP/1.1 {s}\r\nContent-Type: application/json\r\nContent-Length: {l}\r\n\r\n{b}",
                s = status,
                l = payload.len(),
                b = payload
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

#[tokio::test]
async fn check_health_success_against_mock() {
    let port = start_mock_gateway("200 OK", "{}");
    check_health(&format!("http://127.0.0.1:{port}"))
        .await
        .expect("mock /api/health 200 → Ok（成功分支 72-73）");
}

#[tokio::test]
async fn check_health_non_2xx_is_error_with_status() {
    // 独立裸 mock：对所有请求（含 /api/health）回 503——start_mock_gateway
    // 对 /api/health 恒回 200，测不了非 2xx 的 health 分支。
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..4 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let _ = stream.write_all(
                b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n",
            );
        }
    });
    let err = check_health(&format!("http://127.0.0.1:{port}"))
        .await
        .expect_err("503 → Err");
    assert!(err.contains("503"), "got: {err}");
}

#[tokio::test]
async fn send_internal_command_success_and_failure() {
    let port = start_mock_gateway("200 OK", r#"{"ok":true}"#);
    send_internal_command(&format!("http://127.0.0.1:{port}"), "tok", "open_dashboard")
        .await
        .expect("200 → Ok");

    let port = start_mock_gateway("500 Internal Server Error", r#"{"err":"x"}"#);
    let err =
        send_internal_command(&format!("http://127.0.0.1:{port}"), "tok", "open_dashboard")
            .await
            .expect_err("500 → Err 带状态和响应体");
    assert!(err.to_string().contains("Internal command failed"), "got: {err}");
    assert!(err.to_string().contains("500"), "got: {err}");
}

#[tokio::test]
async fn send_internal_command_unreachable_is_error() {
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    assert!(
        send_internal_command(&format!("http://127.0.0.1:{port}"), "tok", "c")
            .await
            .is_err(),
        "连接拒绝 → Err"
    );
}

#[tokio::test]
async fn get_json_parses_success_body() {
    let port = start_mock_gateway("200 OK", r#"{"engaged":true}"#);
    let v = send_internal_command_get_json(
        &format!("http://127.0.0.1:{port}"),
        "tok",
        "estop_status",
    )
    .await
    .expect("200 + JSON → 解析值");
    assert_eq!(v.get("engaged").and_then(|x| x.as_bool()), Some(true));
}

#[tokio::test]
async fn get_json_non_json_body_falls_back_to_empty_object() {
    let port = start_mock_gateway("200 OK", "not-json-at-all");
    let v = send_internal_command_get_json(
        &format!("http://127.0.0.1:{port}"),
        "tok",
        "estop_status",
    )
    .await
    .expect("200 + 非 JSON → 兜底空对象（166 行 unwrap_or_else）");
    assert!(v.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn get_json_failure_is_error() {
    let port = start_mock_gateway("401 Unauthorized", "nope");
    let err = send_internal_command_get_json(
        &format!("http://127.0.0.1:{port}"),
        "tok",
        "estop_status",
    )
    .await
    .expect_err("401 → Err");
    assert!(err.to_string().contains("401"), "got: {err}");
}

// --- run()（不 spawn 网关的两条路径）---

#[tokio::test]
async fn run_missing_config_errors_cleanly() {
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", tmp.path());
    }
    let err = run(false)
        .await
        .expect_err("缺 config.json → 友好错误（不 panic、不 spawn）");
    assert!(
        err.to_string().contains("Cannot read config.json"),
        "got: {err}"
    );
    assert!(
        err.to_string().contains("--local dashboard"),
        "错误信息要带 --local 提示"
    );
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
}

#[tokio::test]
async fn run_success_when_mock_gateway_already_running() {
    // 网关"已在跑"：health 200 + internal 200 → run 全链路 Ok，绝不 spawn。
    let port = start_mock_gateway("200 OK", r#"{"ok":true}"#);
    let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
    let tmp = tempfile::tempdir().unwrap();
    unsafe {
        std::env::set_var("NEMESISBOT_HOME", tmp.path());
    }
    let home = tmp.path().join(".nemesisbot");
    std::fs::create_dir_all(home.join("workspace").join("state")).unwrap();
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({"channels": {"web": {"auth_token": "tok"}}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join("workspace").join("state").join("gateway.json"),
        serde_json::json!({"pid": 123, "web_host": "127.0.0.1", "web_port": port}).to_string(),
    )
    .unwrap();

    run(false)
        .await
        .expect("state 指向 mock + health 通过 → 发 open_dashboard → Ok（无 spawn）");
    unsafe {
        std::env::remove_var("NEMESISBOT_HOME");
    }
}
