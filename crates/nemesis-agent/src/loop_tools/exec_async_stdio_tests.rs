//! AsyncExecTool stdio 决策回归（BUG-20261123-002 同族）。
//!
//! exec_async「本就不读输出」——stdio 必须**恒 null**。旧形态普通臂
//! piped stdout/stderr：「still running」臂随函数返回 drop Child，管道读端
//! 关闭——Unix 侧常驻命令首次写 stdout 即 SIGPIPE 静默死亡（「后台启动」
//! 契约被管道配置亲手破坏）；受限臂本就必须 null（workspace-dacl 受限
//! 令牌 piped 必死）。
//!
//! 测试分两层（对齐 exec_async_signal_tests 先例）：
//! - 真进程 e2e：unix 门控锁「返回后写 stdout 仍存活」（Windows 上读端
//!   关闭只产生写错误、进程不死，无法行为判别）；Windows 侧跑契约 smoke
//!   （带输出的长命令 still-running 路径 Ok）。

use super::*;

fn ctx() -> crate::context::RequestContext {
    crate::context::RequestContext::new("web", "chat1", "user1", "sess1")
}

// ---------------------------------------------------------------------------
// unix 门控 e2e：still-running 返回后写 stdout 不再被 SIGPIPE 杀死
// ---------------------------------------------------------------------------

#[cfg(unix)]
#[tokio::test]
async fn still_running_child_survives_post_return_stdout_write() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let marker = tmp.path().join("late_write_marker");
    // 恒 null 后：echo 写 stdout 落 NUL 设备（不炸），后续 marker 落盘。
    // 旧形态：t=1s 父侧 drop 关读端 → t=2s echo 触发 SIGPIPE 进程死亡，
    // `: > marker` 永不执行。sleep 2 与 wait_seconds=1 的错峰保证 drop
    // 先于子进程首写（无竞态窗口）。
    let cmd = format!("sleep 2; echo late-output; : > {}", marker.display());
    let args = serde_json::json!({"command": cmd, "wait_seconds": 1}).to_string();
    let result = tool.execute(&args, &ctx()).await.unwrap();
    assert!(result.contains("started successfully"), "result: {result}");

    // marker 出现 = 子进程在父侧返回后仍存活且完成了 stdout 写。
    let mut survived = false;
    for _ in 0..60 {
        if marker.exists() {
            survived = true;
            break;
        }
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
    }
    assert!(
        survived,
        "background child died before its post-return stdout write \
         (SIGPIPE from dropped piped read-end)"
    );
}

// ---------------------------------------------------------------------------
// 全平台契约 smoke：带输出的长命令走 still-running 臂（旧普通臂 piped 下
// Windows 无法行为判别，本用例锁「输出型命令照常后台启动」这一契约面）
// ---------------------------------------------------------------------------

#[tokio::test]
async fn output_writing_command_starts_and_reports_still_running() {
    let tmp = tempfile::TempDir::new().unwrap();
    let tool = AsyncExecTool::new(&tmp.path().to_string_lossy(), false);
    let cmd = if cfg!(target_os = "windows") {
        "echo started & ping -n 2 127.0.0.1 >nul".to_string()
    } else {
        "echo started; sleep 2".to_string()
    };
    let args = serde_json::json!({"command": cmd, "wait_seconds": 1}).to_string();
    let result = tool.execute(&args, &ctx()).await.unwrap();
    assert!(
        result.contains("started successfully"),
        "output-writing command must still start in background, got: {result}"
    );
}
