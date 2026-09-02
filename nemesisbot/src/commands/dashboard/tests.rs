//! dashboard 命令单测：gateway 状态文件解析（Option 链各失败分支）+
//! health check 对拒绝连接端口的确定性失败。
//!
//! run() / start_and_wait 会真 spawn gateway 进程（结构性）；send_internal_*
//! 需要活网关（结构性）。

// 刻意设计：本文件测试用进程级串行锁（GLOBAL_STATE_LOCK 等 env/资源互斥锁）
// 保护环境操作，guard 必须跨 async 测试体的 await 持有；#[tokio::test] 每个
// 测试独立 current_thread runtime，持锁方在自己线程上恢复运行，不会死锁。
// 测试域统一豁免（逐处 allow ~200 个不现实）。
#![allow(clippy::await_holding_lock)]

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
    let err = check_health("http://127.0.0.1:1")
        .await
        .expect_err("refused");
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
            let _ =
                stream.write_all(b"HTTP/1.1 503 Service Unavailable\r\nContent-Length: 0\r\n\r\n");
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
    let err = send_internal_command(&format!("http://127.0.0.1:{port}"), "tok", "open_dashboard")
        .await
        .expect_err("500 → Err 带状态和响应体");
    assert!(
        err.to_string().contains("Internal command failed"),
        "got: {err}"
    );
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
    let v =
        send_internal_command_get_json(&format!("http://127.0.0.1:{port}"), "tok", "estop_status")
            .await
            .expect("200 + JSON → 解析值");
    assert_eq!(v.get("engaged").and_then(|x| x.as_bool()), Some(true));
}

#[tokio::test]
async fn get_json_non_json_body_falls_back_to_empty_object() {
    let port = start_mock_gateway("200 OK", "not-json-at-all");
    let v =
        send_internal_command_get_json(&format!("http://127.0.0.1:{port}"), "tok", "estop_status")
            .await
            .expect("200 + 非 JSON → 兜底空对象（166 行 unwrap_or_else）");
    assert!(v.as_object().map(|o| o.is_empty()).unwrap_or(false));
}

#[tokio::test]
async fn get_json_failure_is_error() {
    let port = start_mock_gateway("401 Unauthorized", "nope");
    let err =
        send_internal_command_get_json(&format!("http://127.0.0.1:{port}"), "tok", "estop_status")
            .await
            .expect_err("401 → Err");
    assert!(err.to_string().contains("401"), "got: {err}");
}

// --- run()（不 spawn 网关的两条路径）---

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
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

#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
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

// ===========================================================================
// r9_spawn_fail（R9 补测批零头组，2026-08-27）：start_and_wait 的 spawn-poll
// 循环（dashboard.rs 79-118）首次真链路覆盖。确定性设计：
//   - state 文件预置 web_port=1（必然拒绝连接）→ run() 走进 start_and_wait；
//   - config.json 写 {"model_list":"not-an-array"}：合法 JSON Value 但过不了
//     Config 反序列化 → 被 spawn 的子网关在绑定任何端口之前响亮退出；
//   - 于是父进程轮询满固定 30s 预算 → Err("Gateway did not start within 30
//     seconds") → main.rs 打 "Error: ..." + exit(1)。
// 无残留进程（子网关自毙）、不占任何生产端口。慢测（~32s），全程持锁串行。
// ===========================================================================

// 整 mod Windows 形态（1/1 测试全走 Windows CLI 进程边界）。
#[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
mod r9_spawn_fail {
    use test_harness::{TestWorkspace, resolve_nemesisbot_bin};

    #[cfg(windows)] // Windows-form CLI test (Linux nightly: excluded, 2026-09-02 sweep)
    #[tokio::test]
    async fn start_and_wait_times_out_when_spawned_gateway_dies_on_bad_config() {
        let _guard = crate::GLOBAL_STATE_LOCK.lock().unwrap();
        let ws = TestWorkspace::new().unwrap();

        std::fs::create_dir_all(ws.workspace().join("state")).unwrap();
        std::fs::write(ws.config_path(), r#"{"model_list":"not-an-array"}"#).unwrap();
        std::fs::write(
            ws.workspace().join("state").join("gateway.json"),
            serde_json::json!({
                "pid": 999999u64,
                "web_host": "127.0.0.1",
                "web_port": 1
            })
            .to_string(),
        )
        .unwrap();

        let bin = resolve_nemesisbot_bin().expect("需已构建二进制");
        let out = ws.run_cli_with_timeout(&bin, &["dashboard"], 75).await;

        assert!(
            !out.success(),
            "子网关起不来时 dashboard 必须以失败收场\nstdout={} stderr={}",
            out.stdout,
            out.stderr
        );
        let combined = format!("{} {}", out.stdout, out.stderr);
        assert!(
            combined.contains("Starting gateway..."),
            "要进入 start_and_wait：\n{combined}"
        );
        assert!(
            combined.contains("Gateway did not start within 30 seconds"),
            "30s 预算耗尽的错误文本缺失：\n{combined}"
        );
    }
}

// ===========================================================================
// r10_start_wait_direct（R10 终测补测，2026-08-27）：start_and_wait 的
// 进程内直调覆盖。r9_spawn_fail 走子进程链（CLI 子进程里跑生产代码），但
// 实测该子进程的 error-exit 镜像在插桩测量下不可靠（llvm 计数器丢失，
// merged lcov 里 79-119 仍 miss）。直调形态让这些行落进测试二进制自己的
// 镜像（Leg A 语义），完全绕开子进程 flush 不确定性：
//   - 成功轮询臂：state 文件预置指向本地 mock health 端口 → 第一轮 poll
//     就 check_health Ok → Ok 返回（107-115）；
//   - 超时臂：state 文件永不出现 → 30s 预算耗尽 Err（118-119）。
// 两臂都会 spawn current_exe（= 测试二进制本身，非网关）：BUG #48 修复后
// start_and_wait 在 cfg!(test) 下传 libtest 空过滤器（--exact 不存在名）
// → 子进程 0 测试秒退，无副作用、不占端口、无残留进程、零输出。
// （修复前传 "gateway" 会被 libtest 当过滤器，嵌套重跑 217 个网关测试。）
// ===========================================================================

mod r10_start_wait_direct {
    use super::*;

    /// 成功臂：预置 state 指向 mock（health 200 + internal 200），首轮
    /// poll 命中 → Ok((host,port)) 与 mock 一致。
    #[tokio::test]
    async fn start_and_wait_poll_success_against_local_mock_state() {
        let port = start_mock_gateway("200 OK", r#"{"ok":true}"#);
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("gateway.json");
        std::fs::write(
            &state_path,
            serde_json::json!({
                "pid": 42u64,
                "web_host": "127.0.0.1",
                "web_port": port
            })
            .to_string(),
        )
        .unwrap();

        let (host, got) = start_and_wait(true, &state_path)
            .await
            .expect("state 已就绪 + health 200 → 秒级 Ok");
        assert_eq!(host, "127.0.0.1");
        assert_eq!(got, port as i64);
    }

    /// 超时臂：state 文件所在目录不存在 → read_gateway_state 恒 None →
    /// 30s 预算耗尽返回 Err（慢测 ~31s：函数内预算写死 Duration 30s）。
    #[tokio::test]
    async fn start_and_wait_times_out_when_state_never_appears() {
        let dir = tempfile::tempdir().unwrap();
        let state_path = dir.path().join("no_such_dir").join("gateway.json");

        let err = start_and_wait(false, &state_path)
            .await
            .expect_err("state 永不出现必须超时 Err");
        assert!(
            err.to_string()
                .contains("Gateway did not start within 30 seconds"),
            "got: {err}"
        );
    }
}
