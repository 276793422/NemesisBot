use super::*;

/// 最小 HTTP mock（std::net，独立线程）：对所有请求回 200 + 固定 JSON。
/// 最多接 16 个连接后线程退出（够测试用，不永挂）。
fn start_mock_server() -> u16 {
    start_mock_engaged(false)
}

/// 与 start_mock_server 同构，但 engaged 值可指定——覆盖 status 输出的
/// ENGAGED 分支（此前固定回 false，「⛔ ENGAGED」打印臂从未点亮）。
fn start_mock_engaged(engaged: bool) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..16 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let body = format!(r#"{{"status":"ok","engaged":{engaged}}}"#);
            let resp = format!(
                "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\n\r\n{}",
                body.len(),
                body
            );
            let _ = stream.write_all(resp.as_bytes());
        }
    });
    port
}

fn write_home(home: &std::path::Path, auth_token: &str, web_port: u16) {
    std::fs::create_dir_all(home.join("workspace").join("state")).unwrap();
    std::fs::write(
        home.join("config.json"),
        serde_json::json!({"channels": {"web": {"auth_token": auth_token}}}).to_string(),
    )
    .unwrap();
    std::fs::write(
        home.join("workspace").join("state").join("gateway.json"),
        serde_json::json!({"pid": 123, "web_host": "127.0.0.1", "web_port": web_port}).to_string(),
    )
    .unwrap();
}

#[tokio::test]
async fn run_engage_release_status_against_mock() {
    let dir = tempfile::tempdir().unwrap();
    let port = start_mock_server();
    write_home(dir.path(), "secret", port);
    // engage / status / release 三条路径都应 Ok
    assert!(run(dir.path(), false, false).await.is_ok(), "engage");
    assert!(run(dir.path(), false, true).await.is_ok(), "status");
    assert!(run(dir.path(), true, false).await.is_ok(), "release");
}

#[tokio::test]
async fn run_errors_when_config_missing() {
    let dir = tempfile::tempdir().unwrap();
    let r = run(dir.path(), false, false).await;
    assert!(r.is_err(), "缺 config.json 应报错");
}

#[tokio::test]
async fn run_status_reports_engaged_true_branch() {
    // R7（coverage-95 goal）：status + 服务端 engaged=true → 「⛔ ENGAGED」
    // 打印臂（此前 mock 固定 engaged=false，该臂从未点亮）。
    let dir = tempfile::tempdir().unwrap();
    let port = start_mock_engaged(true);
    write_home(dir.path(), "", port);
    run(dir.path(), false, true)
        .await
        .expect("status against engaged=true mock → Ok");
}

#[tokio::test]
async fn run_errors_when_gateway_state_missing() {
    let dir = tempfile::tempdir().unwrap();
    std::fs::write(
        dir.path().join("config.json"),
        serde_json::json!({"channels": {"web": {"auth_token": "x"}}}).to_string(),
    )
    .unwrap();
    let r = run(dir.path(), false, false).await;
    assert!(r.is_err(), "缺 gateway.json 应报错");
}

#[tokio::test]
async fn run_errors_when_gateway_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    // 绑端口拿空闲号、立刻 drop → health check 连不上
    let port = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .port();
    write_home(dir.path(), "secret", port);
    let r = run(dir.path(), false, false).await;
    assert!(r.is_err(), "gateway 不可达应报错");
}

// ===========================================================================
// S11c（quality-hardening goal 冲刺 S11）：补齐 match (status, release,
// engaged) 的剩余分支——既有 mock 固定回 engaged:false，engaged:true 两臂
// （release 指令已发送 / engage ⛔ 已触发）和 engaged 缺字段臂从没到过，
// 加 web_port=0 的早退分支（estop.rs:36-38）。
// ===========================================================================

/// 可配置响应体的 mock（对 /api/health 回 200，其余回给定 body）。
fn start_mock_server_with(body: &'static str) -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    std::thread::spawn(move || {
        use std::io::{Read, Write};
        for _ in 0..16 {
            let Ok((mut stream, _)) = listener.accept() else {
                break;
            };
            let mut buf = [0u8; 4096];
            let _ = stream.read(&mut buf);
            let req = String::from_utf8_lossy(&buf).to_string();
            let (status, payload) = if req.contains("/api/health") {
                ("200 OK", "{\"status\":\"ok\"}")
            } else {
                ("200 OK", body)
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
async fn run_engage_with_engaged_true_prints_frozen_branch() {
    let dir = tempfile::tempdir().unwrap();
    let port = start_mock_server_with(r#"{"status":"ok","engaged":true}"#);
    write_home(dir.path(), "secret", port);
    run(dir.path(), false, false)
        .await
        .expect("engage + engaged:true → ⛔ 已触发 分支（estop.rs:76-79）");
}

#[tokio::test]
async fn run_release_with_engaged_true_prints_fallback_branch() {
    let dir = tempfile::tempdir().unwrap();
    let port = start_mock_server_with(r#"{"status":"ok","engaged":true}"#);
    write_home(dir.path(), "secret", port);
    run(dir.path(), true, false)
        .await
        .expect("release + engaged:true → 指令已发送 分支（estop.rs:75）");
}

#[tokio::test]
async fn run_with_missing_engaged_field_falls_to_generic_branches() {
    let dir = tempfile::tempdir().unwrap();
    let port = start_mock_server_with(r#"{"status":"ok"}"#);
    write_home(dir.path(), "secret", port);
    // engaged 缺字段 → None：release 和 engage 都落到兜底臂。
    run(dir.path(), true, false)
        .await
        .expect("release + engaged:None → 兜底臂");
    run(dir.path(), false, false)
        .await
        .expect("engage + engaged:None → 兜底臂");
    // status + None → 该组合也走 (true, _, Some(e)) 之外的… 实际上 status 臂
    // 要求 Some(e)；None 时落到 release/engage 兜底——一并钉住不 panic。
    run(dir.path(), false, true)
        .await
        .expect("status + engaged:None → 兜底臂，不 panic");
}

#[tokio::test]
async fn run_errors_when_web_port_zero() {
    let dir = tempfile::tempdir().unwrap();
    write_home(dir.path(), "secret", 0);
    let err = run(dir.path(), false, false)
        .await
        .expect_err("web_port=0 → state 无效早退（estop.rs:36-38）");
    assert!(
        err.to_string().contains("web_port"),
        "got: {err}"
    );
}
