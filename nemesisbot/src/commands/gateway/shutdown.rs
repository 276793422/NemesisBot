// ---------------------------------------------------------------------------
// Global shutdown state
// ---------------------------------------------------------------------------

use std::future::Future;
use std::sync::OnceLock;
use std::sync::atomic::{AtomicBool, Ordering};

use tokio_util::sync::CancellationToken;

/// Global shutdown flag (replaces Go's globalShutdownChan).
/// pub(crate)：tests.rs 直控复位（隔离），经根 cfg(test) 再导出暴露。
pub(crate) static SHUTDOWN_REQUESTED: AtomicBool = AtomicBool::new(false);

/// Request global shutdown from any component.
#[cfg(not(target_os = "android"))]
#[allow(dead_code)] // only called from the tray's on_quit (desktop feature); kept as a general API.
pub(crate) fn trigger_global_shutdown() {
    SHUTDOWN_REQUESTED.store(true, Ordering::SeqCst);
}

/// Check if global shutdown has been requested.
#[allow(dead_code)]
pub(crate) fn is_shutdown_requested() -> bool {
    SHUTDOWN_REQUESTED.load(Ordering::SeqCst)
}

// ---------------------------------------------------------------------------
// 进程终态令牌（BUG-20261115-023：停机 panic 家族第四~八成员收口）
// ---------------------------------------------------------------------------

/// 进程终态令牌：gateway/run/acp 三入口的退出路径统一 cancel，供**常驻
/// tokio 循环**的周期等待点感知「进程即将终态」，不再重新 park 到 timer。
///
/// 机理（家族纪律，先例 heartbeat r9 / devices BUG-20261114-015 / outbox
/// BUG-20261114-019）：常驻循环 JoinHandle 丢弃 + 无停机臂，park 在
/// interval/sleep timer 上；裸 Drop 语义的 runtime（`#[tokio::test]`、
/// test-harness 等非 main 形态）shutdown 会对存活任务做最后 poll——poll
/// 里注册新 timer 即撞 `"A Tokio 1.x context was found, but it is being
/// shutdown"` panic。家族第四~八成员 = logs_sweeper / session_group_
/// sweeper / notify / board_bus / profile_service 五个 timer-park 循环。
///
/// 跨 crate 成员（BUG-20261119-208，023 横扫只扫 nemesisbot crate 的盲区
/// 整改）：nemesis-web 侧 uploads TTL sweeper（6h）与 sessions 到期唤醒
/// sweeper（10s）经 DI token 参数消费本令牌（gateway 装配传
/// `terminal_cancel_token().clone()`），换装形态与本模块 `terminal_loop_tick`
/// 契约一致（前置 is_cancelled + select 令牌优先）。023 家族的裸 Drop
/// panic 形态对这两个成员**实证不复现**（R1953 探针：纯 timer-park 且无
/// 外部唤醒源的循环，current_thread / multi_thread 两种 flavor 裸 drop
/// 都零 panic——最后 poll 重 poll 已注册 timer 不重新注册；r9 的 panic
/// 观察伴随 abort 停机互相作用），立案形态是 drain 窗口静默性破坏：
/// Step 24 cancel 后排空窗内这两循环继续工作（snooze 每 10s 唤醒会话
/// 注入新 turn 对抗 drain 收敛、uploads 继续做 FS 删除）。
///
/// 一次性语义：只在进程终态链 cancel，无动态 start/stop（区别于 outbox
/// push loop 的 per-component stop_token——那类动态生命周期组件各自持
/// token，不走本令牌）。
///
/// 测试注入口（BUG-20261115-046）：断言常驻循环**活性**的测试必须经
/// `GatewayCtx.terminal_cancel_override` 注入局部令牌——run_runtime 全链
/// 测试（run_runtime_full_cycle_* 家族）走生产停机路径不可逆 cancel 全局
/// 令牌，并行测试进程里全局令牌随时可能已毒化（born-dead 形态：循环首
/// poll 拍完基线即被前置 is_cancelled 短路收线，表现对并行敏感、单跑
/// 恒绿，极易误诊为时序/可见性问题）。不活性断言的测试不受影响（死循环
/// 不再有断言价值）；生产路径不受影响（ctx 恒 None）。
static TERMINAL_CANCEL: OnceLock<CancellationToken> = OnceLock::new();

/// 常驻循环等待点消费的进程终态令牌。
pub(crate) fn terminal_cancel_token() -> &'static CancellationToken {
    TERMINAL_CANCEL.get_or_init(CancellationToken::new)
}

/// 进程终态收口：退出路径（gateway Step 24 / run/acp 命令收尾）调用，幂等。
pub(crate) fn cancel_terminal_tasks() {
    terminal_cancel_token().cancel();
}

/// 常驻循环统一等待点：返回 `false` = 终态已触发（调用方 break），`true`
/// = 正常一拍到（调用方继续本拍工作量）。
///
/// 前置 `is_cancelled` 检查保证取消后**确定性退出**——即使 `wait` 已
/// ready 也不消费（收口语义优先于最后一拍工作量，不与 stop 竞速）。
pub(crate) async fn terminal_loop_tick<W: Future>(token: &CancellationToken, wait: W) -> bool {
    if token.is_cancelled() {
        return false;
    }
    tokio::select! {
        _ = token.cancelled() => false,
        _ = wait => true,
    }
}
