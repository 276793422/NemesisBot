//! 测试共享工具（S9 覆盖率批次引入，模式抄自 nemesis-sandbox/src/test_util.rs）。
//!
//! `capture_logs`：装一个 thread-local tracing subscriber（sink 丢弃），
//! 让 `tracing::info!/warn!/debug!` 的参数表达式行被真实求值——无 subscriber
//! 时宏参数行在 lcov 里恒为 0（参数只在事件启用时格式化）。`set_default`
//! 是 thread-local：`#[test]` 与 `#[tokio::test]`（默认 current_thread flavor）
//! 都在同一线程上跑，守卫生命周期覆盖整个测试体。
//!
//! 本文件只放 helper，不放 #[test]（测试只放独立 tests.rs / *_tests.rs）。

/// 装一个 DEBUG 级 fmt subscriber（输出丢弃）。返回的 DefaultGuard 必须
/// 在测试期间持有（drop 即卸载）。
pub(crate) fn capture_logs() -> tracing::subscriber::DefaultGuard {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::set_default(subscriber)
}

/// 绕过 libtest 输出捕获直接写进程 stderr。
///
/// 为什么需要：println!/eprintln! 的输出被 libtest 捕获进 per-test 缓冲，
/// panic 详情也统一在末尾 summary 的 failures 区输出——**测试挂死导致整轮
/// 被 cancel 时，这些全部丢失**（2026-09-02 extended-tests Linux nightly
/// 首跑实录：2 个 FAILED 的 panic 现场随 2h 挂死一起蒸发）。捕获是 TLS 挂钩
/// 只拦 std 的 stdout()/stderr() 句柄；Linux 上重新 open /proc/self/fd/2
/// 拿到的是裸 fd 2，写下去直达 job 日志。非 Linux 直接 eprintln!（本地
/// 调试场景，挂死时终端本身就在看）。
pub(crate) fn force_stderr(msg: &str) {
    #[cfg(target_os = "linux")]
    {
        if let Ok(mut f) = std::fs::OpenOptions::new()
            .write(true)
            .open("/proc/self/fd/2")
        {
            use std::io::Write;
            let _ = f.write_all(format!("{msg}\n").as_bytes());
            return;
        }
    }
    eprintln!("{msg}");
}
