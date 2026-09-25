// remote_executor_tool.rs 覆盖率补充测试（stdio 传输成功回路 / stderr
// drain / userland 沙盒 env 标记 / 超时熔断 / 无响应退出）。
//
// 假子进程：.cmd 批处理（Rust Command 在 Windows 上按 cmd /c 语义spawn
// .cmd），用 `@echo off` 压掉命令回显后按协议回一行 JSON。命名管道传输
// （spawn_and_call_pipe 410-451）需要真 exec_worker 子进程应答管道协议，
// 假批处理无法连管 → 记豁免（见交付报告）。

use super::*;
use std::path::PathBuf;

fn req_ctx() -> RequestContext {
    RequestContext::new("web", "test-chat", "test-session", "/ws")
}

/// 写一个假 executor 子进程（.cmd），返回其路径。
fn fake_executor(dir: &std::path::Path, body: &str) -> PathBuf {
    let p = dir.join(format!(
        "fake_exec_{}.cmd",
        body.len().wrapping_mul(31) + std::process::id() as usize
    ));
    std::fs::write(&p, body).expect("write fake executor");
    p
}

fn channel(exe: PathBuf, timeout: Duration) -> ExecutorChannel {
    ExecutorChannel::new(exe, "/ws".into(), Arc::new(|| false)).with_timeout(timeout)
}

#[tokio::test]
async fn stdio_roundtrip_success_drains_stderr() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_executor(
        dir.path(),
        "@echo off\r\necho cov-err-line 1>&2\r\necho {\"ok\":true,\"result\":\"cov-ok\",\"error\":\"\"}\r\n",
    );
    let ch = channel(exe, Duration::from_secs(15));
    let out = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect("fake executor answers the protocol");
    assert_eq!(out, "cov-ok");
}

/// userland_sandbox 标记（U11 非 Windows 通路；Windows 上经私有 fn 直调，
/// 只证 env 标记分支不破坏 stdio 回路）。
#[tokio::test]
async fn stdio_userland_sandbox_sets_env_marker() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_executor(
        dir.path(),
        "@echo off\r\necho {\"ok\":true,\"result\":\"cov-sandboxed\",\"error\":\"\"}\r\n",
    );
    let ch = channel(exe, Duration::from_secs(15));
    let line = ch
        .build_request_line("exec", "{}", &req_ctx())
        .expect("request line serializes");
    let out = ch
        .spawn_and_call_stdio("exec", &line, true)
        .await
        .expect("stdio transport with userland marker");
    assert_eq!(out, "cov-sandboxed");
}

#[tokio::test]
async fn stdio_timeout_kills_silent_child() {
    let dir = tempfile::tempdir().unwrap();
    // ping -n 3 ≈ 2s > 200ms 预算 → 读超时臂 + start_kill。
    let exe = fake_executor(dir.path(), "@echo off\r\nping -n 3 127.0.0.1 >nul\r\n");
    let ch = channel(exe, Duration::from_millis(200));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("silent child must time out");
    assert!(err.contains("timed out after"), "err: {err}");
    assert!(err.contains("tool=exec"), "err: {err}");
}

#[tokio::test]
async fn stdio_child_exits_without_response() {
    let dir = tempfile::tempdir().unwrap();
    let exe = fake_executor(dir.path(), "@echo off\r\nexit /b 0\r\n");
    let ch = channel(exe, Duration::from_secs(15));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("child without a response line");
    assert!(err.contains("exited without a response"), "err: {err}");
}
