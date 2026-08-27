//! 测试辅助（R6 覆盖率批次，2026-08-27）。
//!
//! `ensure_global_subscriber`：给整个测试进程装一个丢弃式全局 tracing
//! subscriber。voice 的下载/引擎代码里大量 `tracing::info!` 的**参数行**
//! 只有在存在活跃 subscriber 时才求值（S6 批次钉过的 lcov 机制）；且
//! 下载跑在 `run_blocking` 另起的 std 线程上，thread-local `set_default`
//! 管不到，必须进程级 global。写向 `io::sink`——测试不关心日志内容，
//! 只关心参数求值路径被真实执行。

use std::sync::OnceLock;

/// 幂等：进程内首次调用装 global subscriber，后续调用为 no-op。
pub fn ensure_global_subscriber() {
    static INIT: OnceLock<()> = OnceLock::new();
    INIT.get_or_init(|| {
        let sub = tracing_subscriber::fmt()
            .with_max_level(tracing::Level::DEBUG)
            .with_writer(std::io::sink)
            .finish();
        // 并行测试里可能有两个同时进来——OnceLock 已保证只有一个走到这，
        // set_global_default 的 Err（理论上不可能）静默忽略即可。
        let _ = tracing::subscriber::set_global_default(sub);
    });
}

/// 捕获闭包里的 panic 并返回其消息（R6：null 引擎 FFI panic 测试用）。
///
/// 为什么不用 `#[should_panic]`：这些引擎的 **Drop 同样会在 null 指针上
/// panic**。should_panic 测试里方法 panic 展开时会顺带 drop 引擎 → 析构
/// 再 panic → 双 panic 触发 Windows fail-fast（STATUS_STACK_BUFFER_OVERRUN）
/// 直接击穿整个测试进程。用 catch_unwind 在闭包边界截住展开（引擎借用在
/// 闭包外，不会被 drop），随后调用方 `std::mem::forget(engine)` 跳过析构。
pub fn catch_panic_msg<F: FnOnce() -> R, R>(f: F) -> String {
    let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(f));
    let payload = match r {
        Ok(_) => panic!("expected a panic but none happened"),
        Err(p) => p,
    };
    if let Some(s) = payload.downcast_ref::<String>() {
        s.clone()
    } else if let Some(s) = payload.downcast_ref::<&str>() {
        s.to_string()
    } else {
        "<non-string panic payload>".to_string()
    }
}
