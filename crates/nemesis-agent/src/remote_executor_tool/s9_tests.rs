//! S9 覆盖率批次（remote_executor_tool.rs stdio 传输错误臂）。
//! - 337-338：spawn 失败（exe 路径不存在）。
//! - 346-360：超时 / 读失败 / 立即退出无响应 / 非 JSON 首行 parse 失败
//!   （cmd 无参 + 管道 stdin：收 EOF 后退出或先吐 banner，四个错误臂中
//!   确定性落入其中之一；具体落点取决于该机器 cmd 的 banner 行为）。
//!   其余基线缺口（394-451 命名管道传输）由 bin 侧 executor L2.2 集成测试
//!   覆盖（nemesisbot，见 sandbox L2 记忆），lib 单测无法伪造连管子进程
//!   → 环境依赖组。

use super::*;
use std::sync::Arc;
use std::time::Duration;

fn req_ctx() -> RequestContext {
    RequestContext::new("web", "test-chat", "test-session", "/ws")
}

#[tokio::test]
async fn stdio_spawn_failure_surfaces_error() {
    let ch = ExecutorChannel::new(
        std::path::PathBuf::from("Z:/nemesis_s9/no_such_executor.exe"),
        "/ws".into(),
        Arc::new(|| false),
    )
    .with_timeout(Duration::from_secs(5));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("bogus exe must fail to spawn");
    assert!(
        err.contains("failed to spawn executor child"),
        "err: {err}"
    );
}

#[tokio::test]
async fn stdio_cmd_child_lands_in_an_error_arm() {
    // 平台各自的 shell：cmd.exe（无参交互态先吐 banner）或 /bin/sh（读到
    // 协议行当命令执行 → 语法错误退出）——都不会应答执行体协议，确定性
    // 落进错误臂之一（2026-09-01：原实现只取 ComSpec，Linux 上 spawn
    // "cmd.exe" ENOENT 落进 spawn 失败臂，断言不含该臂 → 假红）。
    let cmd = if cfg!(windows) {
        std::env::var("ComSpec").unwrap_or_else(|_| "cmd.exe".to_string())
    } else {
        "/bin/sh".to_string()
    };
    let ch = ExecutorChannel::new(
        std::path::PathBuf::from(&cmd),
        "/ws".into(),
        Arc::new(|| false),
    )
    .with_timeout(Duration::from_secs(8));
    let err = ch
        .spawn_and_call("exec", "{}", &req_ctx())
        .await
        .expect_err("cmd.exe cannot answer the protocol");
    // 留证：assert 消息随 panic 进 summary 的 failures 区，若整轮被其它
    // 测试挂死连坐取消就一起蒸发（2026-09-02 CI nightly 实录）——先旁路
    // 捕获把实际 err 直接写 stderr，任何情况下 CI 日志都留得住。
    crate::test_support::force_stderr(&format!("[s9-stdio] spawn_and_call error arm: {err}"));
    assert!(
        err.contains("timed out")
            || err.contains("exited without a response")
            || err.contains("parse executor response")
            || err.contains("read executor response"),
        "err: {err}"
    );
}
