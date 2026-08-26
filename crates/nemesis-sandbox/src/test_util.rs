//! 测试共享工具（S6 覆盖率批次引入）。
//!
//! `capture_logs`：装一个 thread-local tracing subscriber（sink 到 /dev/null），
//! 让 `tracing::info!/warn!/debug!` 的参数表达式行被真实求值——无 subscriber
//! 时宏参数行在 lcov 里恒为 0（参数只在事件启用时格式化）。`set_default`
//! 是 thread-local：`#[test]` 与 `#[tokio::test]`（默认 current_thread flavor）
//! 都在同一线程上跑，守卫生命周期覆盖整个测试体。

/// 装一个 DEBUG 级 fmt subscriber（输出丢弃）。返回的 DefaultGuard 必须
/// 在测试期间持有（drop 即卸载）。
pub(crate) fn capture_logs() -> tracing::subscriber::DefaultGuard {
    let subscriber = tracing_subscriber::fmt()
        .with_max_level(tracing::Level::DEBUG)
        .with_writer(std::io::sink)
        .finish();
    tracing::subscriber::set_default(subscriber)
}
