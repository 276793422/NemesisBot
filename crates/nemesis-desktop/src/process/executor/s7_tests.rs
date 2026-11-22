//! 冲刺覆盖测试：process/executor.rs 的 stderr 排水线程快照分支和
//! 对已退出进程发优雅终止信号失败的分支。
//!
//! 子进程全部用无害命令（cmd /C... / sh -c...），不弹窗、不碰生产数据。

use super::*;

// BUG-20261123-002 同族回归锁：spawn_child 起的 stderr 排水线程必须把
// 子进程 stderr 收进环形尾快照（piped 无排水 = 话痨子进程写满管道缓冲
// 冻死，窗口失去响应 / approval 结果永不到达）。
#[test]
fn s7_stderr_drain_thread_captures_child_stderr_tail() {
    let executor = DefaultPlatformExecutor::with_defaults();
    let (exe, args): (&str, Vec<String>) = if cfg!(windows) {
        (
            "cmd",
            vec!["/C".to_string(), "echo s7-stderr-marker 1>&2".to_string()],
        )
    } else {
        (
            "sh",
            vec!["-c".to_string(), "echo s7-stderr-marker 1>&2".to_string()],
        )
    };
    let mut child = executor.spawn_child(exe, &args).expect("spawn child");

    // 排水线程异步收数：轮询等快照出现标记（线程调度无界，10s 上限）。
    let mut text = String::new();
    for _ in 0..100 {
        text = child.stderr_snapshot();
        if text.contains("s7-stderr-marker") {
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(text.contains("s7-stderr-marker"), "stderr tail: {text}");

    executor.cleanup(&mut child).unwrap();
}

/// 对已退出（且已被 wait 收割）的进程：优雅信号必然失败（Windows 上
/// GenerateConsoleCtrlEvent 对死 pid 返回 0 -> debug 分支；Unix 上
/// kill(ESRCH) 返回 -1 -> debug 分支），随后轮询发现已退出，整体 Ok。
#[test]
fn s7_terminate_already_exited_child_reports_signal_failure_but_succeeds() {
    let executor = DefaultPlatformExecutor::with_defaults();
    let (exe, args): (&str, Vec<String>) = if cfg!(windows) {
        ("cmd", vec!["/C".to_string(), "exit 0".to_string()])
    } else {
        ("sh", vec!["-c".to_string(), "exit 0".to_string()])
    };
    let mut child = executor.spawn_child(exe, &args).expect("spawn child");

    // 收割子进程：pid 已死。
    let _ = child.wait().unwrap();
    assert!(!child.is_alive());

    // 优雅信号对死 pid 失败，但 terminate 仍应通过已退出路径返回 Ok。
    executor
        .terminate_child(&mut child)
        .expect("terminate on exited child should succeed");
    assert!(!child.is_alive());

    executor.cleanup(&mut child).unwrap();
}
